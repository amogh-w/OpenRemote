# Design

Goal: control a PC from a phone with the least moving parts. One server process on the PC, any
frontend on the phone, a small JSON protocol in between.

## Features and the simplest way to build each

| # | Feature | Approach |
|---|---|---|
| 1 | Mouse: move, click, double, right, middle, button hold (drag), vertical and horizontal scroll | `SendInput`. Pointer acceleration and sensitivity live in the frontend; the server applies raw deltas. |
| 2 | Keyboard: named keys, combos, held combos, typed text, key down/up | Key-name table to virtual-key codes. Text goes in via `KEYEVENTF_UNICODE`. A later "compatible keyboard" setting can switch to scan codes for games and older apps. |
| 3 | Media and volume keys | Just key names (`volume_up`, `media_play_pause`, ...). |
| 4 | Power: restart, shutdown, logoff, hibernate, sleep, lock | Spawn `shutdown.exe` / `rundll32` with no window. Behind a two-tap confirm in the UI. |
| 5 | Open a URL, browser shortcuts | URL: http(s) only, handed to `url.dll,FileProtocolHandler` as one argument (no shell). Browser actions are plain key combos sent by the frontend. |
| 6 | App switcher and launcher (**done**) | `EnumWindows` filtered like the taskbar (visible, unowned, not cloaked, not a tool window). Actions via `ShowWindow`, `SetForegroundWindow` (after an F24 key tap so Windows allows it) and `PostMessage(WM_CLOSE)`. Launcher = Start Menu `.lnk` files via `ShellExecute` on a COM thread. Icons: `PrivateExtractIconsW` (64 px) for executables, the shell image list (48 px, no shortcut arrow) for shortcuts, encoded to PNG with the `png` crate and cached. The server issues opaque keys; clients never send paths. Pinned and recent apps are not covered. |
| 7 | Window thumbnails | Later: `IShellItemImageFactory` to PNG. |
| 8 | Foreground-app push | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` and broadcast `{t:"foreground", title, exe}`. |
| 9 | Clipboard sync: text and images (**done**) | `arboard` with retries while another program holds the clipboard. Images travel as PNG over HTTP (`POST /clip/image`, `GET /clip/image.png`), validated by dimensions before decoding; the phone converts photos with a canvas. Copies are acknowledged (`clip.ack`). |
| 10 | Connected-device list | In-memory list (name, IP, last seen) shown in the admin page. |
| 11 | Pairing | Optional random 128-bit token carried in the QR link. Off by default for now. |
| 12 | Discovery | mDNS (`mdns-sd`, service `_openremote._tcp`), the QR code and manual IP entry. |
| 13 | QR code | The admin page renders it (`qrcode` crate, SVG). |
| 14 | Tray icon and menu (**done**: Show QR code, Quit) | `tray-icon` crate. The server runs on a worker thread while the main thread runs the Win32 message loop; the release build uses the GUI subsystem so there is no console window. |
| 15 | Settings UI | No native GUI. The admin page on `127.0.0.1` plus a settings file under `%APPDATA%`. |
| 16 | Launch on startup | `HKCU\...\Run` value (no admin rights needed). |
| 17 | Works on UAC prompts and the lock screen | Later: the same exe in a `--service` mode that spawns a `--helper` in the active session. |
| 18 | Keep the cursor visible with no mouse attached | A few lines of `SystemParametersInfo` (MouseKeys). |

Out of scope for now: Bluetooth transport, auto-update, screen mirroring, file transfer, telemetry.

## Architecture

One Rust binary:

```
frontend (any)  <--- WebSocket JSON + HTTP --->  server (axum + tokio)
                                                   |- optional token check
                                                   |- dispatcher -> platform layer (Windows)
                                                   |     input (SendInput) | windows (EnumWindows, icons)
                                                   |     clipboard | power | foreground hook | autostart
                                                   |- admin page (127.0.0.1 only): QR, settings, devices
                                                   |- embedded web frontend (PWA)
                                                   `- mDNS announcer, tray icon (planned)
```

One port (default 1990): `GET /ws` for control, `GET /` serves the web app, `/admin` and the QR are
reachable only from loopback.

### Message protocol

JSON text frames, see the table in `README.md`. Pointer latency over a LAN WebSocket with
`TCP_NODELAY` is plenty, so there is no UDP path. The frontend coalesces `move` events to one per
animation frame.

### Security model

- Optional token on the WebSocket; the admin page is loopback-only and checks the `Host` header.
- `open` accepts only `http` and `https`; text, scroll and move sizes are clamped.
- Plain HTTP on the LAN exposes input and any token to sniffers. Later: a self-signed TLS certificate
  (`rcgen` + `rustls`) with its fingerprint embedded in the QR link.

### Crates

`axum`, `tokio`, `serde`/`serde_json`, `qrcode`, `if-addrs`, `arboard`, `png`, `tray-icon`, `windows-sys`; planned:
`mdns-sd`, `rust-embed`, `rcgen`/`rustls`.

## Milestones

1. **Core** (done): WebSocket server, mouse, keyboard, text, QR admin page, web app, PWA basics.
2. **Desktop shell**: tray icon (done), settings page in the admin UI, autostart, mDNS, device list.
3. **Extras**: app switcher and launcher (done), foreground push, image clipboard.
4. **Hardening**: service and helper for UAC/lock screen, TLS, thumbnails, installer, update check.
