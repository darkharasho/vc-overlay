use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom];

    /// Top and bottom edges lay people out in a row.
    pub fn is_horizontal(self) -> bool {
        matches!(self, Edge::Top | Edge::Bottom)
    }

    pub fn label(self) -> &'static str {
        match self {
            Edge::Left => "Left",
            Edge::Right => "Right",
            Edge::Top => "Top",
            Edge::Bottom => "Bottom",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Monitor connector name (e.g. "DP-2"); `None` lets the compositor pick.
    pub output: Option<String>,
    /// Screen edge the overlay is attached to.
    pub edge: Edge,
    /// Distance along the edge from its start (top for left/right, left for top/bottom).
    pub offset: i32,
    /// Distance from the edge itself.
    pub gap: i32,
    /// Only list people while they are talking.
    pub only_speaking: bool,
    pub show_names: bool,
    /// Show the voice channel name above/before the list.
    pub show_channel: bool,
    /// Most people listed at once; the rest collapse into "+N". 0 = no limit.
    pub max_entries: u32,
    /// Avatar diameter in logical pixels.
    pub avatar_size: f32,
    /// Download and install new releases from GitHub.
    pub auto_update: bool,
    /// StreamKit access token for the local Discord RPC. Kept out of logs.
    pub token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output: None,
            edge: Edge::Left,
            offset: 120,
            gap: 8,
            only_speaking: false,
            show_names: true,
            show_channel: false,
            max_entries: 8,
            avatar_size: 32.0,
            auto_update: true,
            token: None,
        }
    }
}

fn path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("vc-overlay")
        .join("config.json")
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(json) = serde_json::to_string_pretty(self) else { return };
        let tmp = p.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
            }
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}
