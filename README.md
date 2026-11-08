# vc-overlay

A tiny always-on-top Discord voice overlay for Linux Wayland: avatars of everyone in your
voice channel, a green ring on whoever is talking, and Discord-style crossed-out
mic/headphone badges for muted/deafened. It sits on a screen edge, click-through, above
fullscreen games.

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

- Linux x86_64 with a Wayland compositor that has `wlr-layer-shell` (KDE Plasma 6, Sway,
  Hyprland, …). GNOME doesn't support layer-shell.
- The official Discord desktop client running (the Flatpak works). Vesktop/arRPC don't
  expose voice events.

## Install

```sh
mkdir -p ~/.local/bin
curl -L -o ~/.local/bin/vc-overlay \
  https://github.com/darkharasho/vc-overlay/releases/latest/download/vc-overlay-x86_64-linux
chmod +x ~/.local/bin/vc-overlay
~/.local/bin/vc-overlay
```

1. With Discord open, click **Authorize** when Discord asks.
2. Right-click the tray icon and tick **Start at login**.

That's it: it starts with your session and updates itself (see [Updates](#updates)). The
first run also adds **VC Overlay** to your app launcher, so after that you can start it
from the menu instead of a terminal.

Each release also has a tarball with the binary, README and license, plus SHA-256
checksums, on the [releases page](https://github.com/darkharasho/vc-overlay/releases).

To uninstall, quit from the tray and delete:

```sh
rm -rf ~/.local/bin/vc-overlay ~/.config/vc-overlay \
  ~/.config/autostart/vc-overlay.desktop \
  ~/.local/share/applications/vc-overlay.desktop \
  ~/.local/share/icons/hicolor/64x64/apps/vc-overlay.png
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

## Updates

Releases come from GitHub. Every 6 hours vc-overlay checks for a newer release,
downloads it, verifies its SHA-256 and replaces its own binary. If you're not in a voice
channel it restarts straight away; otherwise the tray shows *Restart to update*. Turn this
off with *Install updates automatically* in the tray menu. Builds run from `target/`
never update themselves.

## Build from source

```sh
cargo build --release
./target/release/vc-overlay          # real Discord
./target/release/vc-overlay --demo   # fake channel, for trying it out
./target/release/vc-overlay --edit   # start in move mode
install -Dm755 target/release/vc-overlay ~/.local/bin/vc-overlay
```

Releases are built by `.github/workflows/release.yml` when a `v*` tag matching the
`Cargo.toml` version is pushed.

## Notes

- Token exchange goes through `streamkit.discord.com`, same as StreamKit and the Discover
  overlay. If that ever stops working, the fix is a Discord developer app of your own
  (client id + secret) in `rpc.rs`.
- Font: Open Sans Semibold (Apache-2.0), bundled in `assets/`.
