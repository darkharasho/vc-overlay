//! Self-update from GitHub releases.
//!
//! Every few hours, asks GitHub for the latest release. If it's newer than this build, it
//! downloads the `vc-overlay-x86_64-linux` asset, checks it against the published
//! SHA-256, and renames it over the running binary (Linux keeps the old inode alive for
//! the running process). The new version takes over on the next start; if you're not in
//! a voice channel at that moment, it restarts right away.

use crate::state::{Shared, UpdateState};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

const LATEST: &str = "https://api.github.com/repos/darkharasho/vc-overlay/releases/latest";
const ASSET: &str = "vc-overlay-x86_64-linux";
const FIRST_CHECK: Duration = Duration::from_secs(20);
const INTERVAL: Duration = Duration::from_secs(6 * 3600);

pub fn spawn(shared: Arc<Shared>, exe: PathBuf) {
    // Dev builds under target/ never replace themselves.
    if exe.components().any(|c| c.as_os_str() == "target") {
        return;
    }
    std::thread::spawn(move || run(shared, exe));
}

fn run(shared: Arc<Shared>, exe: PathBuf) {
    let mut next = Instant::now() + FIRST_CHECK;
    loop {
        if shared.quit.load(Ordering::Relaxed) {
            return;
        }
        let forced = shared.check_update.swap(false, Ordering::Relaxed);
        if forced || Instant::now() >= next {
            next = Instant::now() + INTERVAL;
            if forced || shared.config().auto_update {
                if matches!(shared.update_state(), UpdateState::Ready(_)) {
                    // Already installed; just waiting for a restart.
                } else {
                    shared.set_update_state(UpdateState::Checking);
                    let state = match check_and_install(&exe) {
                        Ok(Some(v)) => UpdateState::Ready(v),
                        Ok(None) => UpdateState::UpToDate,
                        Err(e) => {
                            eprintln!("vc-overlay: update failed: {e}");
                            UpdateState::Failed(e)
                        }
                    };
                    shared.set_update_state(state);
                }
            }
        }
        // Restart into the new version when it won't interrupt anything.
        if matches!(shared.update_state(), UpdateState::Ready(_)) {
            let idle = shared.voice().channel_name.is_none() && !shared.is_editing();
            if idle {
                shared.restart.store(true, Ordering::Relaxed);
                shared.quit.store(true, Ordering::Relaxed);
                shared.repaint();
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

/// Returns the version installed, or `None` if already up to date.
fn check_and_install(exe: &Path) -> Result<Option<String>, String> {
    let body = get(LATEST)?.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let release: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let tag = release["tag_name"].as_str().ok_or("release has no tag")?;
    if !is_newer(tag, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let asset_url = |name: &str| {
        release["assets"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["name"] == name))
            .and_then(|x| x["browser_download_url"].as_str())
            .map(str::to_owned)
            .ok_or(format!("release {tag} has no {name}"))
    };
    let bin_url = asset_url(ASSET)?;
    let sum_url = asset_url(&format!("{ASSET}.sha256"))?;

    let expected = get(&sum_url)?.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let expected = expected.split_whitespace().next().ok_or("empty checksum")?.to_lowercase();
    let bin = get(&bin_url)?
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let actual = hex(&Sha256::digest(&bin));
    if actual != expected {
        return Err(format!("checksum mismatch for {tag}"));
    }
    install(exe, &bin).map_err(|e| format!("can't replace {}: {e}", exe.display()))?;
    Ok(Some(tag.trim_start_matches('v').to_string()))
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    ureq::get(url)
        .header("User-Agent", concat!("vc-overlay/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())
}

/// Writes `bin` next to `exe` and atomically renames it into place.
fn install(exe: &Path, bin: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = exe.with_file_name(".vc-overlay.update");
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o755).open(&tmp)?;
        f.write_all(bin)?;
        f.sync_all()?;
        std::fs::rename(&tmp, exe)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares dotted numeric versions, ignoring a leading "v".
fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v').split(['.', '-']).map_while(|p| p.parse().ok()).collect()
    };
    parse(candidate) > parse(current)
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn version_order() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.10", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }
}
