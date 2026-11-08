//! Talks to the Discord desktop client's local RPC websocket.
//!
//! Login uses StreamKit's client id, the same way Discord's StreamKit OBS overlay
//! and the Discover overlay do. That means no Discord developer app or approval is
//! needed: the first time, Discord shows an "Authorize" prompt, and the token we get
//! back is saved so later starts are silent.

use crate::state::{Member, Shared, Status, Voice};
use serde_json::{Value, json};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::{Message, WebSocket};

const CLIENT_ID: &str = "207646673902501888";
const ORIGIN: &str = "https://streamkit.discord.com";
const TOKEN_URL: &str = "https://streamkit.discord.com/overlay/token";
const SCOPES: [&str; 3] = ["rpc", "messages.read", "rpc.notifications.read"];
const CHANNEL_EVENTS: [&str; 5] = [
    "VOICE_STATE_CREATE",
    "VOICE_STATE_UPDATE",
    "VOICE_STATE_DELETE",
    "SPEAKING_START",
    "SPEAKING_STOP",
];
const RETRY: Duration = Duration::from_secs(3);

type Ws = WebSocket<TcpStream>;

#[derive(Debug)]
enum RpcErr {
    /// The connection is unusable; reconnect.
    Fatal(String),
    /// Discord rejected one command; the connection is still fine.
    Command(String),
    /// User denied the Authorize prompt; wait for them to retry from the tray.
    Denied,
    Reauth,
    Quit,
}

impl From<tungstenite::Error> for RpcErr {
    fn from(e: tungstenite::Error) -> Self {
        RpcErr::Fatal(e.to_string())
    }
}

/// Runs forever on its own thread, reconnecting as Discord comes and goes.
pub fn run(shared: Arc<Shared>) {
    loop {
        if shared.quit.load(Ordering::Relaxed) {
            return;
        }
        let result = match connect() {
            Some(ws) => Session::new(ws, shared.clone()).run(),
            None => Err(RpcErr::Fatal("Discord not running".into())),
        };
        let status = match &result {
            Err(RpcErr::Denied) => Status::Error("Authorization denied".into()),
            Err(RpcErr::Fatal(e)) if e != "Discord not running" => {
                eprintln!("vc-overlay: discord rpc: {e}");
                Status::NoDiscord
            }
            _ => Status::NoDiscord,
        };
        shared.update_voice(|v| *v = Voice { status: status.clone(), ..Voice::default() });

        match result {
            Err(RpcErr::Quit) => return,
            Err(RpcErr::Reauth) => continue,
            Err(RpcErr::Denied) => {
                // Don't re-prompt in a loop; wait until the user asks from the tray.
                while !shared.reauth.swap(false, Ordering::Relaxed) {
                    if shared.quit.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            _ => sleep_unless_flagged(&shared, RETRY),
        }
    }
}

fn sleep_unless_flagged(shared: &Shared, d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if shared.quit.load(Ordering::Relaxed) || shared.reauth.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn connect() -> Option<Ws> {
    for port in 6463..=6472u16 {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let Ok(stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
        let url = format!("ws://127.0.0.1:{port}/?v=1&client_id={CLIENT_ID}&encoding=json");
        let Ok(mut req) = url.into_client_request() else { continue };
        req.headers_mut().insert("Origin", HeaderValue::from_static(ORIGIN));
        if let Ok((ws, _)) = tungstenite::client(req, stream) {
            return Some(ws);
        }
    }
    None
}

struct Session {
    ws: Ws,
    shared: Arc<Shared>,
    nonce: u64,
    channel_id: Option<String>,
    /// Channel switch seen while busy; `Some(None)` means "left voice".
    pending_switch: Option<Option<String>>,
}

impl Session {
    fn new(ws: Ws, shared: Arc<Shared>) -> Self {
        Self { ws, shared, nonce: 0, channel_id: None, pending_switch: None }
    }

    fn run(&mut self) -> Result<(), RpcErr> {
        self.wait_ready()?;
        self.login()?;
        self.call("SUBSCRIBE", json!({}), Some("VOICE_CHANNEL_SELECT"))?;
        let channel = self.call("GET_SELECTED_VOICE_CHANNEL", json!({}), None)?;
        self.shared.update_voice(|v| v.status = Status::Connected);
        self.join(channel)?;

        loop {
            if let Some(next) = self.pending_switch.take() {
                let data = match next {
                    Some(id) => match self.call("GET_CHANNEL", json!({ "channel_id": id }), None) {
                        Ok(d) => d,
                        Err(RpcErr::Command(_)) => Value::Null,
                        Err(e) => return Err(e),
                    },
                    None => Value::Null,
                };
                self.join(data)?;
                continue;
            }
            self.check_flags()?;
            if let Some(msg) = self.read()? {
                self.handle(msg);
            }
        }
    }

    fn wait_ready(&mut self) -> Result<(), RpcErr> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.check_flags()?;
            if let Some(msg) = self.read()? {
                if msg["evt"] == "READY" {
                    return Ok(());
                }
            }
        }
        Err(RpcErr::Fatal("no READY from Discord".into()))
    }

    fn login(&mut self) -> Result<(), RpcErr> {
        let saved = self.shared.config().token.clone();
        if let Some(token) = saved {
            match self.call("AUTHENTICATE", json!({ "access_token": token }), None) {
                Ok(_) => return Ok(()),
                Err(RpcErr::Command(_)) => self.shared.update_config(|c| c.token = None),
                Err(e) => return Err(e),
            }
        }

        self.shared.update_voice(|v| v.status = Status::Authorizing);
        let data = match self.call(
            "AUTHORIZE",
            json!({ "client_id": CLIENT_ID, "scopes": SCOPES }),
            None,
        ) {
            Ok(d) => d,
            Err(RpcErr::Command(_)) => return Err(RpcErr::Denied),
            Err(e) => return Err(e),
        };
        let code = data["code"]
            .as_str()
            .ok_or_else(|| RpcErr::Fatal("AUTHORIZE returned no code".into()))?;
        let token = exchange_code(code).map_err(RpcErr::Fatal)?;
        match self.call("AUTHENTICATE", json!({ "access_token": token }), None) {
            Ok(_) => {}
            Err(RpcErr::Command(e)) => return Err(RpcErr::Fatal(e)),
            Err(e) => return Err(e),
        }
        self.shared.update_config(|c| c.token = Some(token));
        Ok(())
    }

    fn join(&mut self, channel: Value) -> Result<(), RpcErr> {
        if let Some(old) = self.channel_id.take() {
            for evt in CHANNEL_EVENTS {
                ignore_command_err(self.call("UNSUBSCRIBE", json!({ "channel_id": old }), Some(evt)))?;
            }
        }

        let members: Vec<Member> = channel["voice_states"]
            .as_array()
            .map(|a| a.iter().filter_map(parse_member).collect())
            .unwrap_or_default();
        let name = channel["name"].as_str().map(String::from);
        self.shared.update_voice(|v| {
            v.channel_name = name;
            v.members = members;
        });

        if let Some(id) = channel["id"].as_str().map(String::from) {
            self.channel_id = Some(id.clone());
            for evt in CHANNEL_EVENTS {
                ignore_command_err(self.call("SUBSCRIBE", json!({ "channel_id": id }), Some(evt)))?;
            }
        }
        Ok(())
    }

    /// Send a command and wait for its reply, handling events that arrive meanwhile.
    fn call(&mut self, cmd: &str, args: Value, evt: Option<&str>) -> Result<Value, RpcErr> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        let mut msg = json!({ "cmd": cmd, "args": args, "nonce": nonce });
        if let Some(evt) = evt {
            msg["evt"] = json!(evt);
        }
        self.ws.send(Message::text(msg.to_string()))?;

        loop {
            self.check_flags()?;
            let Some(reply) = self.read()? else { continue };
            if reply["nonce"].as_str() == Some(nonce.as_str()) {
                if reply["evt"] == "ERROR" {
                    let why = reply["data"]["message"].as_str().unwrap_or("unknown error");
                    return Err(RpcErr::Command(format!("{cmd}: {why}")));
                }
                return Ok(reply["data"].clone());
            }
            self.handle(reply);
        }
    }

    /// One message, or `None` if nothing arrived within the socket read timeout.
    fn read(&mut self) -> Result<Option<Value>, RpcErr> {
        match self.ws.read() {
            Ok(Message::Text(text)) => Ok(serde_json::from_str(text.as_str()).ok()),
            Ok(Message::Close(frame)) => Err(RpcErr::Fatal(format!("closed by Discord: {frame:?}"))),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn check_flags(&self) -> Result<(), RpcErr> {
        if self.shared.quit.load(Ordering::Relaxed) {
            return Err(RpcErr::Quit);
        }
        if self.shared.reauth.swap(false, Ordering::Relaxed) {
            self.shared.update_config(|c| c.token = None);
            return Err(RpcErr::Reauth);
        }
        Ok(())
    }

    fn handle(&mut self, msg: Value) {
        if msg["cmd"] != "DISPATCH" {
            return;
        }
        let data = &msg["data"];
        let evt = msg["evt"].as_str().unwrap_or_default();

        // Ignore stragglers from a channel we already left.
        if let Some(ch) = data["channel_id"].as_str() {
            if evt != "VOICE_CHANNEL_SELECT" && self.channel_id.as_deref() != Some(ch) {
                return;
            }
        }

        match evt {
            "VOICE_CHANNEL_SELECT" => {
                self.pending_switch = Some(data["channel_id"].as_str().map(String::from));
            }
            "VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE" => {
                if let Some(m) = parse_member(data) {
                    self.shared.update_voice(|v| match v.members.iter_mut().find(|x| x.id == m.id) {
                        Some(existing) => *existing = Member { speaking: existing.speaking, ..m },
                        None => v.members.push(m),
                    });
                }
            }
            "VOICE_STATE_DELETE" => {
                if let Some(id) = data["user"]["id"].as_str() {
                    self.shared.update_voice(|v| v.members.retain(|m| m.id != id));
                }
            }
            "SPEAKING_START" | "SPEAKING_STOP" => {
                let speaking = evt == "SPEAKING_START";
                if let Some(id) = data["user_id"].as_str() {
                    self.shared.update_voice(|v| {
                        if let Some(m) = v.members.iter_mut().find(|m| m.id == id) {
                            m.speaking = speaking;
                        }
                    });
                }
            }
            _ => {}
        }
    }
}

fn ignore_command_err(r: Result<Value, RpcErr>) -> Result<(), RpcErr> {
    match r {
        Ok(_) | Err(RpcErr::Command(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

fn exchange_code(code: &str) -> Result<String, String> {
    let body = json!({ "code": code }).to_string();
    let mut resp = ureq::post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .send(body.as_str())
        .map_err(|e| format!("token exchange: {e}"))?;
    let text = resp.body_mut().read_to_string().map_err(|e| format!("token exchange: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("token exchange: {e}"))?;
    v["access_token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "token exchange: no access_token in response".into())
}

/// Turn an RPC voice state object into a `Member`.
pub fn parse_member(d: &Value) -> Option<Member> {
    let user = &d["user"];
    let id = user["id"].as_str()?.to_string();
    let non_empty = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(String::from);
    let name = non_empty(&d["nick"])
        .or_else(|| non_empty(&user["global_name"]))
        .or_else(|| non_empty(&user["username"]))
        .unwrap_or_else(|| "?".into());
    let vs = &d["voice_state"];
    let flag = |v: &Value| v.as_bool().unwrap_or(false);
    Some(Member {
        avatar_url: avatar_url(&id, user["avatar"].as_str()),
        name,
        muted: flag(&vs["self_mute"]) || flag(&vs["mute"]) || flag(&vs["suppress"]),
        deafened: flag(&vs["self_deaf"]) || flag(&vs["deaf"]),
        speaking: false,
        id,
    })
}

pub fn avatar_url(user_id: &str, hash: Option<&str>) -> String {
    match hash {
        Some(h) => format!("https://cdn.discordapp.com/avatars/{user_id}/{h}.png?size=128"),
        None => {
            let index = user_id.parse::<u64>().map(|id| (id >> 22) % 6).unwrap_or(0);
            format!("https://cdn.discordapp.com/embed/avatars/{index}.png")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_voice_state() {
        let d = json!({
            "nick": "",
            "mute": false,
            "voice_state": { "self_mute": true, "self_deaf": false, "mute": false, "deaf": false },
            "user": { "id": "80351110224678912", "username": "nelly", "global_name": "Nelly", "avatar": "abc" }
        });
        let m = parse_member(&d).unwrap();
        assert_eq!(m.name, "Nelly");
        assert!(m.muted && !m.deafened && !m.speaking);
        assert_eq!(m.avatar_url, "https://cdn.discordapp.com/avatars/80351110224678912/abc.png?size=128");
    }

    #[test]
    fn default_avatar_when_no_hash() {
        assert_eq!(
            avatar_url("80351110224678912", None),
            format!("https://cdn.discordapp.com/embed/avatars/{}.png", (80351110224678912u64 >> 22) % 6)
        );
    }
}
