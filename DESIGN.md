# Design

Goal: control a PC from a phone with the least moving parts. One server process on the PC, any
frontend on the phone, a small JSON protocol in between.

## Features and the simplest way to build each

| # | Feature | Approach |
|---|---|---|
| 1 | Mouse: move, click, double, right, middle, button hold (drag), vertical and horizontal scroll | `SendInput`. Pointer acceleration and sensitivity live in the frontend; the server applies raw deltas. |
| 2 | Keyboard: named keys, combos, held combos, typed text, key down/up | Key-name table to virtual-key codes. Text goes in via `KEYEVENTF_UNICODE`. A later "compatible keyboard" setting can switch to scan codes for games and older apps. |
| 3 | Media and volume keys | Just key names (`volume_up`, `media_play_pause`, ...). |
| 4 | Power: restart, shutdown, logoff, hibernate, sleep, lock | Spawn `shutdown.exe` / `rundll32` with no window; sleep calls `SetSuspendState(0,1,0)` itself on a thread (via `rundll32` it hibernates). Behind a two-tap confirm in the UI. |
| 5 | Open a URL, browser shortcuts | URL: http(s) only, handed to `url.dll,FileProtocolHandler` as one argument (no shell). Browser actions are plain key combos sent by the frontend. |
| 6 | App switcher and launcher (**done**) | `EnumWindows` filtered like the taskbar (visible, unowned, not cloaked, not a tool window). Actions via `ShowWindow`, `SetForegroundWindow` (after an F24 key tap so Windows allows it) and `PostMessage(WM_CLOSE)`. Launcher = Start Menu `.lnk` files via `ShellExecute` on a COM thread. Icons: `PrivateExtractIconsW` (64 px) for executables, the shell image list (48 px, no shortcut arrow) for shortcuts, encoded to PNG with the `png` crate and cached. The server issues opaque keys; clients never send paths. Pinned and recent apps are not covered. |
| 7 | Window thumbnails | Later: `IShellItemImageFactory` to PNG. |
| 8 | Foreground-app push | `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` and broadcast `{t:"foreground", title, exe}`. |
| 9 | Clipboard sync: text and images (**done**) | `arboard` with retries while another program holds the clipboard. Images travel as PNG over HTTP (`POST /clip/image`, `GET /clip/image.png`), validated by dimensions before decoding; the phone converts photos with a canvas. Copies are acknowledged (`clip.ack`). |
| 10 | Connected-device list | In-memory list (name, IP, last seen) shown in the admin page. |
| 11 | Pairing (**done**) | One-time offer on the PC (QR link with a 128-bit secret, plus a 6-digit code), 2 minute expiry, single use, global lockout after 5 wrong guesses. A phone that proves the offer gets a 256-bit random key in an HttpOnly SameSite=Strict cookie (no secrets in URLs, so screenshots and history are safe; it also covers the socket and image routes automatically). Only SHA-256 hashes are stored (`devices.json`). Open sockets re-check their key every 2 seconds so a revoked phone is cut off at once. Loopback is trusted. The first token-in-URL version was removed because it was not good enough. |
| 12 | Discovery | mDNS (`mdns-sd`, service `_openremote._tcp`), the QR code and manual IP entry. |
| 13 | QR code | The admin page renders it (`qrcode` crate, SVG) for each LAN address. |
| 14 | Tray icon and menu (**done**: Show phone link, Quit) | `tray-icon` crate. The server runs on a worker thread while the main thread runs the Win32 message loop; the release build uses the GUI subsystem so there is no console window. |
| 15 | Settings UI | No native GUI. The admin page on `127.0.0.1` plus a settings file under `%APPDATA%`. |
| 16 | Launch on startup (**done**) | `HKCU\...\Run` value (no admin rights needed), toggled from the tray menu or with `--autostart` / `--no-autostart`. |
| 17 | Works on UAC prompts and the lock screen (**done, opt-in**) | The same exe has three extra modes: `--service` (LocalSystem service that keeps one helper alive in the active console session), `--input-helper` (SYSTEM token copied from that session's winlogon, attaches to the input desktop, injects events read from a named pipe) and `--install-helper` / `--uninstall-helper`. The pipe is limited to SYSTEM and the signed-in user and refuses remote clients. The app sends input to the helper when connected and injects locally otherwise. Installed copy lives in Program Files so a normal program cannot replace the service binary. Off by default; see the README for the risks (a paired phone can click through a UAC prompt). |
| 18 | Keep the cursor visible with no mouse attached | A few lines of `SystemParametersInfo` (MouseKeys). |
| 19 | Sleep timer on the Media page (**done**) | The PC keeps the countdown and sleeps itself, so it works if the phone drops. See "Sleep timer" below. |

| 20 | Monitor modes under More (**done**) | `{"t":"display","m":"dual\|main"}` runs `DisplaySwitch.exe /extend` or `/external`, the same two commands as the desktop shortcuts. The phone names a mode, never a command or a file. |
| 21 | Launch screen (**done**) | The page ships with an in-markup splash (app glyph, name, loader) that shows from the first paint and fades once the PC answers, or after 5 s. Android's own splash comes from the manifest icon and `background_color`; iPhones get `apple-touch-startup-image` launch images (every portrait size, dark and light), made by `tools/gen_splash.py` and served from `/splash/`. |

Out of scope for now: Bluetooth transport, auto-update, screen mirroring, file transfer, telemetry.

## Architecture

One Rust binary:

```
frontend (any)  <--- WebSocket JSON + HTTP --->  server (axum + tokio)
                                                   |- dispatcher -> platform layer (Windows)
                                                   |     input (SendInput) | windows (EnumWindows, icons)
                                                   |     clipboard | power | foreground hook | autostart
                                                   |- admin page (127.0.0.1 only): phone links, settings, devices
                                                   |- embedded web frontend (PWA)
                                                   `- mDNS announcer, tray icon (planned)
```

One port (default 1990): `GET /ws` for control, `GET /` serves the web app, `/admin` is
reachable only from loopback.

### Message protocol

JSON text frames, see the table in `README.md`. Pointer latency over a LAN WebSocket with
`TCP_NODELAY` is plenty, so there is no UDP path. The frontend coalesces `move` events to one per
animation frame.

### Sleep timer

Lives in `src/sleeptimer.rs` (shared with Kai Remote; the master is in the cuppa repo), so it is platform independent
and unit-tested with a mock host. `Dispatcher` (`src/command.rs`) supplies the host: what is playing, the real sleep, and
the broadcast to every phone. The phone only starts, cancels and displays the timer; it never decides when the PC sleeps.

Messages (see `README.md` for the wire format): `sleep.set` (`secs`), `sleep.cancel`, `sleep.get`.
All three answer with `{"t":"sleep", "on", "left", "total"}` (or `"on": false`, plus `"err"` when refused).
The shared module also has an end-of-episode mode (it asks the host what is playing). Kai Remote uses it; OpenRemote
does not offer it, so its host reports nothing playing and the phone never asks.

State: one `State` behind a mutex, holding a generation counter and the current timer (`started`, `deadline`, and
the playing title for an episode timer). There is at most one timer.

- **Start:** `secs` is clamped to 12 hours and `0` is refused (`bad-time`). With `episode` the server reads the media
  session: it needs a known length and a state other than stopped (else `no-episode`), and the deadline is
  `now + (dur - pos) + 2 s` (a grace period for credits and the player to settle). Starting bumps the generation,
  stores the timer and spawns a watcher thread that carries that generation.
- **Replace and cancel:** both just bump the generation (cancel also clears the timer). A watcher whose generation is no
  longer current exits without doing anything, so an old timer can never fire.
- **Watcher:** sleeps on a `Condvar` until the deadline (no polling); a cancel or a replacement wakes it so it exits at
  once. When due, a plain timer takes itself out of the state and calls `Injector::power(Sleep)`. An episode timer first asks the media session again: if
  the same title still has more than 3 s left (it was paused, or the player was seeked back), the deadline moves to
  `now + left + 2 s` and it keeps waiting; otherwise it sleeps. The media call runs without the lock held.
- **Progress:** `left` and `total` are measured on the PC (`total` is `deadline - started`, so a moved episode deadline
  grows `total`, so the bar refills a little instead of reading wrong). The phone converts `left` to a local end
  time, repaints every 250 ms, and re-syncs from a `sleep.get` sent with each 2 s media poll. A cancel from another
  phone shows up at the next poll.
- **The sleep itself:** `PowerAct::Sleep`, which calls `SetSuspendState(0, 1, 0)` in-process on its own thread (the call
  returns only after the PC wakes). The older `rundll32 powrprof.dll,SetSuspendState 0,1,0` hibernated on a PC with
  hibernation, because rundll32 does not parse that argument string.
- **Not persisted:** a timer does not survive a restart of the server (and a PC that went to sleep has used it up).

### Helper link

Input goes to the optional SYSTEM helper through a named pipe (`uac.rs`). The app's end is opened in no-wait mode: if the
helper stops reading and the pipe fills, a write fails at once, the link is dropped and reconnected, and the event is
injected locally. Without this, one hung helper would block every writer, and with them all phones' input.

### Held input

`Dispatcher` remembers which mouse buttons and keys a phone has pressed with `down` and not yet released with `up`
(a drag, a held modifier). When a connection ends, the server calls `Dispatcher::release_all`, so a Wi-Fi drop in the middle
of a drag cannot leave a button or key stuck down on the PC. With several phones connected, one leaving also releases
what another was holding; that is rare and harmless.

### Security model

- Pairing (see decision 11), on by default. Every route checks the `Host` header (IP, `localhost`, the PC's name or `.local`, against DNS rebinding), any request with an `Origin` must match the served page (against other websites), and everything except the page, manifest, icons and pairing routes needs a paired phone. The admin page is loopback-only and checks the `Host` header.
- `open` accepts only `http` and `https`; text, scroll and move sizes are clamped.
- Plain HTTP on the LAN exposes input to anyone on the network and to sniffers. Pairing keeps strangers out but does not hide traffic. Later: a self-signed TLS
  certificate (`rcgen` + `rustls`) whose fingerprint is shown during pairing.

### Crates

`axum`, `tokio`, `serde`/`serde_json`, `rand`, `sha2`, `qrcode`, `if-addrs`, `arboard`, `png`, `tray-icon`, `windows-sys`; planned:
`mdns-sd`, `rust-embed`, `rcgen`/`rustls`.

## Milestones

1. **Core** (done): WebSocket server, mouse, keyboard, text, admin page, web app, PWA basics.
2. **Desktop shell**: tray icon (done), settings page in the admin UI, autostart, mDNS, device list.
3. **Extras**: app switcher and launcher (done), foreground push, image clipboard.
4. **Hardening**: TLS, thumbnails, installer, update check.
