# vc-overlay

A tiny always-on-top Discord voice overlay for Linux Wayland: avatars of everyone in your
voice channel, a green ring on whoever is talking, Discord-style crossed-out mic/headphone badges for
muted/deafened. It sits
on a screen edge, click-through, above fullscreen games.

- **Left edge:** avatar, then name. **Right edge:** name, then avatar, flush right.
- **Top / bottom edge:** one horizontal bar.

- **Native Rust, no webview, no GPU.** Software-rendered into a Wayland layer-shell
  surface; about 16 MB of RAM.
- **Above fullscreen games.** It uses the layer-shell *overlay* layer, which KDE Plasma
  (and Sway, Hyprland, …) draw over fullscreen windows.
- **No Discord developer app needed.** It logs in to the Discord desktop client's local
  RPC the way Discord's StreamKit overlay does. The first time, Discord asks you to
  **Authorize**; the token is saved to `~/.config/vc-overlay/config.json` (mode 600).

## Requirements

- A Wayland compositor with `wlr-layer-shell` (KDE Plasma 6, Sway, Hyprland, …).
  GNOME doesn't support layer-shell.
- The official Discord desktop client running (the Flatpak works). Vesktop/arRPC don't
  expose voice events.

## Build and run

```sh
cargo build --release
./target/release/vc-overlay          # real Discord
./target/release/vc-overlay --demo   # fake channel, for trying it out
./target/release/vc-overlay --edit   # start in move/resize mode
```

## Using it

- **Left-click the tray icon** (or *Move overlay* in its menu) to unlock. Drag it
  anywhere; on release it snaps to the nearest screen edge and remembers how far along
  that edge it was. Click it without dragging to lock. While locked, clicks go straight
  through to the game.
- **Right-click the tray icon** for settings: only show who's talking, show names, show
  channel name, screen edge, max people (extras collapse into "+N", talkers kept
  visible), avatar size, monitor, start at login, updates, re-authorize, quit.

Settings live in `~/.config/vc-overlay/config.json`; `gap` (distance from the edge) is
only editable there.

### Start with your session

Install the binary somewhere stable, run it from there, and tick **Start at login** in the
tray menu. That writes `~/.config/autostart/vc-overlay.desktop` pointing at the running
binary.

```sh
install -Dm755 target/release/vc-overlay ~/.local/bin/vc-overlay
```

## Updates

Releases come from GitHub. Every 6 hours vc-overlay checks for a newer release,
downloads it, verifies its SHA-256 and replaces its own binary. If you're not in a voice
channel it restarts straight away; otherwise the tray shows *Restart to update*. Turn this
off with *Install updates automatically* in the tray menu. Builds run from `target/`
never update themselves.

## Notes

- Token exchange goes through `streamkit.discord.com`, same as StreamKit and the Discover
  overlay. If that ever stops working, the fix is a Discord developer app of your own
  (client id + secret) in `rpc.rs`.
- Font: Open Sans Semibold (Apache-2.0), bundled in `assets/`.
