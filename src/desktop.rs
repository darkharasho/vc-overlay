//! Desktop integration via XDG files in the user's home:
//! - an app-launcher entry (~/.local/share/applications/vc-overlay.desktop) plus icon,
//!   written on every start so the app shows up in the menu without an installer
//!   (and kept pointing at the right binary if it moves);
//! - an optional autostart entry (~/.config/autostart/vc-overlay.desktop) for
//!   "Start at login".
//!
//! Both point at the binary that is running, so they follow it wherever it's installed.

use std::path::{Path, PathBuf};

const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
const ICON_NAME: &str = "vc-overlay";

fn autostart_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("autostart").join("vc-overlay.desktop"))
}

/// Adds (or refreshes) the app-launcher entry and icon for `exe`. Skipped for dev builds
/// under `target/`, which shouldn't end up in the menu.
pub fn register_launcher(exe: &Path) {
    if exe.components().any(|c| c.as_os_str() == "target") {
        return;
    }
    let Some(data) = dirs::data_dir() else { return };
    let icon = data.join("icons/hicolor/64x64/apps").join(format!("{ICON_NAME}.png"));
    let entry = data.join("applications/vc-overlay.desktop");
    let exe_s = exe.to_string_lossy();
    let mut result = write_if_changed(&icon, ICON_PNG)
        .and_then(|_| write_if_changed(&entry, desktop_entry(&exe_s, false).as_bytes()));
    // Keep an existing "Start at login" entry pointing at this binary too.
    if let Some(auto) = autostart_path().filter(|p| p.exists()) {
        result = result.and_then(|_| write_if_changed(&auto, desktop_entry(&exe_s, true).as_bytes()));
    }
    if let Err(e) = result {
        eprintln!("vc-overlay: couldn't add the app-menu entry: {e}");
    }
}

pub fn autostart_enabled() -> bool {
    autostart_path().is_some_and(|p| p.exists())
}

/// Turns "Start at login" on or off for whichever binary is running now.
pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let Some(path) = autostart_path() else { return Ok(()) };
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let entry = desktop_entry(&std::env::current_exe()?.to_string_lossy(), true);
    write_if_changed(&path, entry.as_bytes())
}

fn write_if_changed(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == contents) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)
}

fn desktop_entry(exe: &str, autostart: bool) -> String {
    // Quote per the desktop entry spec: backslash-escape ", `, $ and \ inside double quotes.
    let mut quoted = String::from("\"");
    for c in exe.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    let extra = if autostart {
        "X-KDE-autostart-after=panel\n"
    } else {
        "Categories=Utility;\nKeywords=discord;voice;overlay;\n"
    };
    format!(
        "[Desktop Entry]\nType=Application\nName=VC Overlay\nGenericName=Discord voice overlay\n\
         Comment=Shows who's talking in your Discord voice channel\nExec={quoted}\n\
         Icon={ICON_NAME}\nTerminal=false\n{extra}"
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn exec_is_quoted() {
        let e = super::desktop_entry("/home/a b/$x\\vc-overlay", false);
        assert!(e.contains("\nExec=\"/home/a b/\\$x\\\\vc-overlay\"\n"), "{e}");
        assert!(e.contains("\nCategories=Utility;\n"));
        assert!(!e.contains("autostart"));
    }

    #[test]
    fn autostart_entry() {
        let e = super::desktop_entry("/x/vc-overlay", true);
        assert!(e.contains("\nX-KDE-autostart-after=panel\n"));
        assert!(!e.contains("Categories"));
    }
}
