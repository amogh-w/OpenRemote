# OpenRemote

Use your phone as a touchpad, keyboard and media remote for your Windows PC, with no app to install on the phone.
One small Rust program on the PC serves a touch-friendly web app over your Wi-Fi.

<p align="center"><img src="docs/screenshots/overview.png" alt="OpenRemote on a phone: touchpad, keyboard, app switcher, media remote, light theme" width="100%"></p>

## Features

**Control**
- **Touchpad** with tap to click, two-finger tap to right-click, two-finger scroll, tap-then-hold to drag, a scroll strip, and labeled Left / Mid / Right buttons that you can hold.
- **Keyboard**: on-screen QWERTY with number row, symbols, shift and caps, key pop-ups and hold-to-repeat; sticky Ctrl/Alt/Shift/Win for shortcuts; or your phone's own keyboard (emoji, other languages, swipe typing).
- **Special keys**: Fn row, numpad, navigation block, arrows.
- **Media remote**: play/pause, next/previous, volume (hold to repeat), mute, fullscreen, and a presentation card for slides.

**Windows**
- **App switcher**: live list of open windows with real icons; focus, minimize, maximize, restore or close each one. The active window is highlighted.
- **App launcher**: every Start Menu app with its icon, A-Z sections, search, and a Recent row.
- **Browser controls**: back, forward, tabs, zoom, find, and open a link on the PC.
- **Clipboard** for text and images, both ways: send text, a photo or a pasted image to the PC; get the PC's text or image on the phone (long-press an image to Copy or Save). Copies are confirmed, and a busy Windows clipboard is retried. Sending images works when the page is opened from the PC's own address.
- **Power**: lock, sleep, hibernate, sign out, restart, shut down (two taps to confirm, except Lock).

**App and desktop**
- **Tray icon** with Show QR code and Quit; the release build has no console window.
- **Installable web app (PWA)** with a manifest, icons and an offline shell.
- **Catppuccin theming**: Latte, Frappe, Macchiato, Mocha or Auto, with six accent colors.
- **Polished motion**: short, subtle animations, switched off by the phone's "reduce motion" setting.
- **Several computers**: save more than one PC and switch between them.
- **Landscape layout** with a side navigation rail.
- Optional pairing token and QR code (off by default).

## Screenshots

<table>
  <tr>
    <td align="center"><img src="docs/screenshots/pad.png" width="190"><br><sub>Pad</sub></td>
    <td align="center"><img src="docs/screenshots/gestures.png" width="190"><br><sub>First-run gestures</sub></td>
    <td align="center"><img src="docs/screenshots/type.png" width="190"><br><sub>Type</sub></td>
    <td align="center"><img src="docs/screenshots/media.png" width="190"><br><sub>Media</sub></td>
  </tr>
  <tr>
    <td align="center"><img src="docs/screenshots/windows.png" width="190"><br><sub>Open windows</sub></td>
    <td align="center"><img src="docs/screenshots/launcher.png" width="190"><br><sub>App launcher</sub></td>
    <td align="center"><img src="docs/screenshots/more.png" width="190"><br><sub>More sheet</sub></td>
    <td align="center"><img src="docs/screenshots/settings.png" width="190"><br><sub>Settings</sub></td>
  </tr>
  <tr>
    <td align="center"><img src="docs/screenshots/light-type.png" width="190"><br><sub>Latte + mauve</sub></td>
    <td align="center"><img src="docs/screenshots/light-windows.png" width="190"><br><sub>Latte windows</sub></td>
    <td></td>
    <td></td>
  </tr>
</table>

Screenshots use a mock connection with fictional window titles. The icons are real extractions from a Windows machine.

## Quick start

Requirements: Windows 10/11 and a [Rust](https://rustup.rs) toolchain.

```powershell
cargo run --release
```

1. A tray icon appears. On the PC, open `http://127.0.0.1:1990/admin` (tray: *Show QR code*) to see the phone link and QR code.
2. On the phone, on the same Wi-Fi, open `http://<pc-ip>:1990/` (or scan the QR code).
3. Allow the program through Windows Firewall (Private networks) if prompted.
4. Optional: *Add to Home Screen* for a full-screen app (see below).

Options:

| Flag | Meaning |
|---|---|
| `--port N` | listen port (default 1990, saved) |
| `--hostname NAME` | name shown on the phone (default: computer name) |
| `--require-token` / `--no-require-token` | turn pairing on or off (saved; off by default) |
| `--regen-token` | create a new pairing token (invalidates old QR codes) |
| `--no-tray` | no tray icon; run in the foreground |
| `-v` | log ignored messages |

Settings live in `%APPDATA%\openremote\settings.json`.

### Install as an app

- **iPhone / iPad**: Safari, Share, *Add to Home Screen*. Works over plain HTTP.
- **Android**: Chrome only does a full install over HTTPS or localhost; over plain HTTP you get a shortcut. For the full install, open `chrome://flags`, enable *Insecure origins treated as secure*, add `http://<pc-ip>:1990`, relaunch Chrome, then *Install app*.

## Architecture

```
 Phone (any modern browser)                       Windows PC (one process: openremote.exe)
+---------------------------+                    +------------------------------------------------+
|  Web app (PWA)            |   HTTP  GET /      |  web.rs        axum + tokio                    |
|  - pages: Pad/Type/Apps/  | <----------------> |    /            single-page app (embedded)     |
|    Media + More sheet     |   WebSocket /ws    |    /ws          JSON control channel           |
|  - gesture + key logic    | <----------------> |    /icon/<key>  PNG icons                      |
|  - Catppuccin + motion    |   HTTP  /icon/..   |    /admin       QR + phone links (loopback)    |
+---------------------------+                    |        |                                       |
                                                 |        v                                       |
                                                 |  command.rs   Msg -> Dispatcher -> Injector    |
                                                 |        |        (validation, clamping,         |
                                                 |        |         key names, URL allow-list)    |
                                                 |        v                                       |
                                                 |  input.rs / winapps.rs   Windows backend       |
                                                 |    SendInput, EnumWindows, ShowWindow,         |
                                                 |    ShellExecute, icon extraction, clipboard,   |
                                                 |    power commands                              |
                                                 |                                                |
                                                 |  main thread: shell.rs  tray icon + Win32 loop |
                                                 +------------------------------------------------+
```

**Data flow.** The phone sends small JSON messages (`move`, `btn`, `key`, `text`, `win.list`, ...) over one WebSocket. `web.rs` parses them into a typed `Msg`, and the `Dispatcher` in `command.rs` validates and clamps them, then calls the platform `Injector`. Replies (window lists, clipboard text, pong) go back on the same socket. Window and app icons are fetched over plain HTTP by key.

| Part | Role |
|---|---|
| `src/main.rs` | CLI, settings, builds the runtime; server on a worker thread, tray on the main thread |
| `src/web.rs` | HTTP routes, WebSocket loop, token check, manifest, icon route, loopback-only admin page |
| `src/command.rs` | `Msg` types, `Injector` trait, `Dispatcher`, key-name table, URL and id validation |
| `src/input.rs` | Windows `Injector`: `SendInput`, power, clipboard, open URL (stub on other OSes) |
| `src/winapps.rs` | Window enumeration and actions, Start Menu scan, icon extraction to PNG |
| `src/shell.rs` | Tray icon and message loop, console attach, error dialog |
| `src/image.rs` | PNG validation (dimensions before decoding), RGBA conversion for clipboard images |
| `src/settings.rs` | Persistent port, token and pairing flag |
| `build.rs` + `assets/ui/` | Assembles the web app into one self-contained page at compile time |

**Design decisions**
- **One port, one binary.** No installer, runtime or phone app. The page is embedded in the executable and works offline on the LAN.
- **Frontend is replaceable.** The WebSocket protocol is the contract, so a native app could be built on the same server.
- **Clients never name files or handles.** The server issues opaque 16-hex keys for apps and icons; window actions only apply to windows the server would list.
- **Testable core.** All Win32 calls sit behind the `Injector` trait, so protocol and validation are tested with a mock and a real WebSocket.
- **No UDP, no custom crypto.** A LAN WebSocket with `TCP_NODELAY` is fast enough; the frontend batches pointer moves to one per frame.

## Protocol

JSON text frames on `GET /ws` (add `?t=<token>` when pairing is on).

```
{"t":"move","dx":3,"dy":-2}                 pixels, relative
{"t":"btn","b":"left|right|middle","s":"down|up|click|dbl"}
{"t":"scroll","dx":0,"dy":120}              120 = one notch; positive dy scrolls down
{"t":"key","k":"ctrl+shift+t","s":"down|up"}  s omitted = press; names in command.rs
{"t":"text","v":"hello"}                    typed as unicode
{"t":"power","a":"lock|sleep|hibernate|logoff|restart|shutdown"}
{"t":"open","url":"https://..."}            http(s) only, opened in the default browser
{"t":"clip.set","text":"..."}               replace PC clipboard text (max 256 KB) -> {"t":"clip.ack","ok":true|false}
{"t":"clip.get"}                            -> {"t":"clip","text":"...","image":{"w":..,"h":..}|null}
{"t":"win.list"}                            -> {"t":"win.list","items":[{id,title,exe,icon,minimized,foreground}]}
{"t":"win.act","id":123,"a":"focus|min|max|restore|close"}
{"t":"app.list"}                            -> {"t":"app.list","items":[{id,name,icon}]}
{"t":"app.launch","id":"<id from app.list>"}
{"t":"ping"}                                -> {"t":"pong"}
```

The server greets with `{"t":"hello","host":...,"v":...}`. Unknown or malformed messages are ignored.

Plain HTTP routes next to the socket:
- `GET /icon/<key>.png`: window and app icons. Keys are issued by the server in `win.list` / `app.list`, so clients can only fetch or launch things the server itself found.
- `GET /clip/image.png`: the PC clipboard image as a PNG.
- `POST /clip/image`: put an uploaded PNG on the PC clipboard (`Content-Type: image/png`, up to 32 MB, at most 8192 px a side and 36 megapixels). The page converts photos to PNG on the phone first.

The clipboard and icon routes need the token (`?t=`) when pairing is on.

## Security

- **Plain HTTP, no pairing by default.** Anyone who can reach port 1990 can control the PC (type keys, shut down, read or write the clipboard). Use it on a network you trust, or turn on `--require-token` (the token still travels unencrypted, so it can be sniffed).
- `/admin` answers only loopback requests with a loopback `Host` header.
- Input is clamped (move, scroll, text length); `open` accepts only `http`/`https` URLs without whitespace; app launches and icons only by server-issued keys.
- Image uploads are validated before use: the PNG signature and dimensions are checked before any pixel is decoded, and only `image/png` bodies are accepted. A website cannot send that to the PC without a CORS preflight, which the server never grants.
- Window titles are sent to connected phones; the web app renders them as text, never HTML.

## Project layout

```
src/            Rust server (see Architecture)
assets/ui/      Web app sources: head.html, palette.css, app.css, icons.html, body.html, app.js
assets/         sw.js (service worker), PWA and tray icons
build.rs        Joins assets/ui/* into one page
tests/ws.rs     End-to-end tests: real server + WebSocket client + mock injector
tools/          gen_icons.py (app icons), fetch_lucide.py (icon sprite)
docs/           theme-config.md (Catppuccin role mapping), screenshots/
DESIGN.md       Feature inventory and design notes
```

## Development

```powershell
cargo test                          # unit + end-to-end tests
node --check assets/ui/app.js       # syntax-check the web app script
cargo build --release               # stop a running openremote.exe first (it locks the file)
python tools/fetch_lucide.py        # regenerate the icon sprite (needs network)
python tools/gen_icons.py           # regenerate PWA and tray icons (needs Pillow)
```

See `CLAUDE.md` for contributor conventions and gotchas.

## Roadmap

**Next**
- [ ] Autostart at login (per-user registry entry, no admin rights)
- [ ] Settings page on the admin site: pairing toggle, hostname, token regeneration, connected devices
- [ ] mDNS discovery (`_openremote._tcp`) so native clients can find the PC
- [ ] Foreground-app push, so the phone can adapt to the active program

**Planned**
- [ ] HTTPS with a self-signed certificate (fingerprint in the QR link); unlocks the full Android install and tilt-to-move
- [ ] Window thumbnails
- [ ] Pinned and recent apps from the taskbar; Microsoft Store app icons
- [ ] Input on UAC prompts and the lock screen (small service plus per-session helper)
- [ ] Installer, update check, and per-device tokens with rate limiting
- [ ] Keyboard shortcuts macros (key sequences with delays)
- [ ] macOS and Linux backends behind the existing `Injector` trait

**Not planned**: Bluetooth transport, screen mirroring, file transfer, telemetry.

## Third-party

Icons: [Lucide](https://lucide.dev) (ISC). Palette: [Catppuccin](https://catppuccin.com) (MIT). Details in `THIRD_PARTY.md`.
