mod autostart;
mod config;
mod overlay;
mod render;
mod rpc;
mod state;
mod tray;

use state::{Member, Shared, Status};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!(
            "vc-overlay: always-on-top Discord voice overlay\n\n\
             --edit   start in move/resize mode\n\
             --demo   fake voice channel, no Discord needed\n\n\
             Left-click the tray icon to move/resize; right-click for options."
        );
        return;
    }
    let demo = args.iter().any(|a| a == "--demo");

    let shared = Arc::new(Shared::new(config::Config::load()));
    if args.iter().any(|a| a == "--edit") {
        shared.editing.store(true, Ordering::Relaxed);
    }

    let tray = tray::spawn(shared.clone());
    let worker = shared.clone();
    if demo {
        std::thread::spawn(move || run_demo(worker));
    } else {
        std::thread::spawn(move || rpc::run(worker));
    }

    if let Err(e) = overlay::run(shared.clone(), tray) {
        eprintln!("vc-overlay: {e}");
        std::process::exit(1);
    }
    shared.quit.store(true, Ordering::Relaxed);
    let final_cfg = shared.config().clone();
    final_cfg.save();
}

/// Fake channel with people randomly talking, for trying the overlay without Discord.
fn run_demo(shared: Arc<Shared>) {
    let names = ["Haro", "Moss", "Juniper", "Kestrel", "Pixel"];
    shared.update_voice(|v| {
        v.status = Status::Connected;
        v.channel_name = Some("Demo channel".into());
        v.members = names
            .iter()
            .enumerate()
            .map(|(i, n)| Member {
                id: i.to_string(),
                name: (*n).into(),
                avatar_url: format!("https://cdn.discordapp.com/embed/avatars/{}.png", i % 6),
                muted: i >= 3, // deafened implies muted
                deafened: i == 4,
                speaking: false,
            })
            .collect();
    });
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    while !shared.quit.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(350));
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        shared.update_voice(|v| {
            for (i, m) in v.members.iter_mut().enumerate() {
                if !m.muted && (seed >> (i * 8)) & 0xff < 70 {
                    m.speaking = !m.speaking;
                }
            }
        });
    }
}
