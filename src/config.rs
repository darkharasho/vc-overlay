use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Monitor connector name (e.g. "DP-2"); `None` lets the compositor pick.
    pub output: Option<String>,
    /// Offset from the monitor's top-left corner, in logical pixels.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Only list people while they are talking.
    pub only_speaking: bool,
    pub show_names: bool,
    /// Avatar diameter in logical pixels.
    pub avatar_size: f32,
    /// StreamKit access token for the local Discord RPC. Kept out of logs.
    pub token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output: None,
            x: 20,
            y: 120,
            width: 260,
            height: 420,
            only_speaking: false,
            show_names: true,
            avatar_size: 32.0,
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
