use crate::config::Config;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// How long a join/leave notice stays up.
pub const NOTICE_TTL: Duration = Duration::from_secs(4);
/// Most notices shown at once; older ones are dropped first.
const MAX_NOTICES: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// No Discord desktop client answered on the RPC ports.
    NoDiscord,
    /// Waiting for the user to click "Authorize" in Discord.
    Authorizing,
    Connected,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Member {
    pub id: String,
    pub name: String,
    pub avatar_url: String,
    pub muted: bool,
    pub deafened: bool,
    pub speaking: bool,
}

/// A brief "X joined" / "X left" line.
#[derive(Clone, Debug)]
pub struct Notice {
    pub name: String,
    pub joined: bool,
    pub until: Instant,
}

/// Progress of the self-updater, shown in the tray.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    /// This version is installed and runs after a restart.
    Ready(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Voice {
    pub status: Status,
    pub channel_name: Option<String>,
    /// Members of the current voice channel, in join order.
    pub members: Vec<Member>,
    /// Recent joins/leaves, oldest first.
    pub notices: Vec<Notice>,
}

impl Default for Voice {
    fn default() -> Self {
        Self { status: Status::NoDiscord, channel_name: None, members: Vec::new(), notices: Vec::new() }
    }
}

impl Voice {
    /// Notices that haven't expired yet.
    pub fn live_notices(&self) -> impl Iterator<Item = &Notice> {
        let now = Instant::now();
        self.notices.iter().filter(move |n| n.until > now)
    }
}

/// State shared by the RPC thread, the tray thread and the UI.
pub struct Shared {
    voice: Mutex<Voice>,
    config: Mutex<Config>,
    pub editing: AtomicBool,
    /// Set by the tray to drop the saved token and log in again.
    pub reauth: AtomicBool,
    pub quit: AtomicBool,
    /// Re-exec the (updated) binary after quitting.
    pub restart: AtomicBool,
    /// Set by the tray's "Check for updates".
    pub check_update: AtomicBool,
    update: Mutex<UpdateState>,
    waker: OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// Monitors the overlay can be placed on: (connector name, description).
    pub outputs: Mutex<Vec<(String, String)>>,
}

impl Shared {
    pub fn new(config: Config) -> Self {
        Self {
            voice: Mutex::new(Voice::default()),
            config: Mutex::new(config),
            editing: AtomicBool::new(false),
            reauth: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            check_update: AtomicBool::new(false),
            update: Mutex::new(UpdateState::Idle),
            waker: OnceLock::new(),
            outputs: Mutex::new(Vec::new()),
        }
    }

    /// Registers the function that wakes the overlay's event loop.
    pub fn set_waker(&self, f: impl Fn() + Send + Sync + 'static) {
        let _ = self.waker.set(Box::new(f));
    }

    /// Ask the overlay to redraw (callable from any thread).
    pub fn repaint(&self) {
        if let Some(wake) = self.waker.get() {
            wake();
        }
    }

    pub fn voice(&self) -> MutexGuard<'_, Voice> {
        self.voice.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn config(&self) -> MutexGuard<'_, Config> {
        self.config.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Mutate voice state and schedule a repaint.
    pub fn update_voice(&self, f: impl FnOnce(&mut Voice)) {
        f(&mut self.voice());
        self.repaint();
    }

    /// Mutate config, persist it and schedule a repaint.
    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let snapshot = {
            let mut c = self.config();
            f(&mut c);
            c.clone()
        };
        snapshot.save();
        self.repaint();
    }

    /// Show "`name` joined/left" for a few seconds, if the user has notices on.
    pub fn push_notice(self: &std::sync::Arc<Self>, name: String, joined: bool) {
        if !self.config().join_notices {
            return;
        }
        self.update_voice(|v| {
            let now = Instant::now();
            v.notices.retain(|n| n.until > now);
            v.notices.push(Notice { name, joined, until: now + NOTICE_TTL });
            let extra = v.notices.len().saturating_sub(MAX_NOTICES);
            v.notices.drain(..extra);
        });
        // Redraw once it has expired so it disappears.
        let shared = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(NOTICE_TTL + Duration::from_millis(50));
            shared.update_voice(|v| {
                let now = Instant::now();
                v.notices.retain(|n| n.until > now);
            });
        });
    }

    pub fn update_state(&self) -> UpdateState {
        self.update.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_update_state(&self, s: UpdateState) {
        *self.update.lock().unwrap_or_else(|e| e.into_inner()) = s;
        self.repaint();
    }

    pub fn is_editing(&self) -> bool {
        self.editing.load(Ordering::Relaxed)
    }
}
