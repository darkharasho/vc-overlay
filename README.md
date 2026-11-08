# vc-overlay

A tiny always-on-top Discord voice overlay for Linux Wayland: avatars of everyone in your
voice channel, a green ring on whoever is talking, red badges for muted/deafened. Pick a
spot on screen once and it stays there, click-through, above fullscreen games.

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

- **Left-click the tray icon** (or *Move / resize overlay* in its menu) to unlock. Drag
  anywhere to move, drag the bottom-right grip to resize, click **Done** to lock. While
  locked, clicks go straight through to the game.
- **Right-click the tray icon** for options: only show who's talking, show names,
  avatar size, which monitor, re-authorize, quit.

### Start with your session

```sh
install -Dm755 target/release/vc-overlay ~/.local/bin/vc-overlay
install -Dm644 assets/vc-overlay.desktop ~/.config/autostart/vc-overlay.desktop
```

## Notes

- Token exchange goes through `streamkit.discord.com`, same as StreamKit and the Discover
  overlay. If that ever stops working, the fix is a Discord developer app of your own
  (client id + secret) in `rpc.rs`.
- Font: Open Sans Semibold (Apache-2.0), bundled in `assets/`.
