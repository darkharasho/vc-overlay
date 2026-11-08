use crate::config::Config;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

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

#[derive(Clone, Debug)]
pub struct Voice {
    pub status: Status,
    pub channel_name: Option<String>,
    /// Members of the current voice channel, in join order.
    pub members: Vec<Member>,
}

impl Default for Voice {
    fn default() -> Self {
        Self { status: Status::NoDiscord, channel_name: None, members: Vec::new() }
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

    pub fn is_editing(&self) -> bool {
        self.editing.load(Ordering::Relaxed)
    }
}
