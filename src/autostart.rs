//! "Start at login" via an XDG autostart entry (~/.config/autostart/vc-overlay.desktop).

use std::path::PathBuf;

fn entry_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("autostart").join("vc-overlay.desktop"))
}

pub fn is_enabled() -> bool {
    entry_path().is_some_and(|p| p.exists())
}

/// Turns autostart on or off. The entry launches whichever binary is running now.
pub fn set(enabled: bool) -> std::io::Result<()> {
    let Some(path) = entry_path() else { return Ok(()) };
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let entry = desktop_entry(&std::env::current_exe()?.to_string_lossy());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, entry)
}

fn desktop_entry(exe: &str) -> String {
    // Quote per the desktop entry spec: backslash-escape ", `, $ and \ inside double quotes.
    let mut quoted = String::from("\"");
    for c in exe.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    format!(
        "[Desktop Entry]\nType=Application\nName=VC Overlay\nComment=Discord voice overlay\n\
         Exec={quoted}\nIcon=audio-input-microphone\nTerminal=false\nX-KDE-autostart-after=panel\n"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn exec_is_quoted() {
        let e = super::desktop_entry("/home/a b/$x\\vc-overlay");
        assert!(e.contains("\nExec=\"/home/a b/\\$x\\\\vc-overlay\"\n"), "{e}");
    }
}
