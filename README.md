# OpenRemote

Use your phone as a touchpad, keyboard and remote for your PC. One Rust binary serves a touchpad
web app, a WebSocket control channel and a loopback-only admin page with a QR code. See `DESIGN.md`
for the plan.

```
cargo run --release
# on the PC: http://127.0.0.1:1990/admin shows the phone link and a QR code
# on the phone (same Wi-Fi): open http://<pc-ip>:1990/
```

Options: `--port N`, `--hostname NAME`, `--require-token` / `--no-require-token`, `--regen-token`, `--no-tray`, `-v`.

The release build runs as a **tray app** (no console window). Right-click the tray icon for Show QR code / Quit;
double-click opens the admin page. From a terminal, output and `--help` still print there.
Settings (port, pairing token, pairing on/off) live in `%APPDATA%\openremote\settings.json`.

**Pairing is OFF by default**: open `http://<pc-ip>:1990/` on the phone and you land on the remote screen.
Run once with `--require-token` to turn the token/QR pairing back on (saved in settings).

## Status

Done: token-optional WebSocket; mouse (move, buttons, drag, scroll); keyboard (names, combos,
sticky modifiers, unicode text); web app:
gradient touchpad with scroll strip, 3-button row, tab bar with on-screen QWERTY (abc/123/symbols,
hold-to-repeat), media remote with volume dial, browser controls + open link on PC, special keys +
numpad + F-keys, **app switcher and launcher** (open windows with focus/minimize/maximize/close, Start Menu
apps with real icons and search), clipboard text sync, power (tap twice to confirm), settings (Catppuccin theme: Auto =
Latte/Mocha by phone setting, or pick Latte, Frappé, Macchiato, Mocha; 6 accent colors; pointer/scroll
speed; natural scrolling; vibration), saved-computers picker, landscape layout.

Not yet (see DESIGN.md): autostart, mDNS, settings page, foreground push,
image clipboard, service for UAC/lock screen, TLS. Tilt-to-move needs HTTPS on phones.

## Theme

Catppuccin v1.8.0, inlined in `assets/index.html` and mapped by role (surfaces `crust`/`mantle`/`base`/`surface*`,
muted chrome `overlay*`, links and primary action `blue`, danger `red`, connected `green`). The mapping
reference is `docs/theme-config.md`. Icons use Mocha blue to mauve (`tools/gen_icons.py`).

## Install as an app (PWA)

The page ships a manifest, icons (`tools/gen_icons.py` regenerates them), and a service worker.
- **iPhone/iPad**: Safari, Share, Add to Home Screen. Full screen, works over plain HTTP.
- **Android**: Chrome only does a full install on HTTPS or localhost. Over plain HTTP you get a shortcut.
  To get the real install anyway: `chrome://flags`, enable "Insecure origins treated as secure",
  add `http://<pc-ip>:1990`, relaunch Chrome, then menu, Install app.
- The manifest only contains the pairing token when requested with the valid token, so strangers on
  the LAN cannot read it from there. iOS home-screen apps keep separate storage, which is why the
  token travels in `start_url` when pairing is on.

## Web app gestures

1 finger = move, tap = click, 2-finger tap = right click, 2 fingers = scroll, tap then tap-and-hold =
drag and drop. Right-edge strip scrolls like a scrollbar.

## Protocol (JSON text frames on `GET /ws`, plus `?t=<token>` when pairing is on)

```
{"t":"move","dx":3,"dy":-2}                 pixels, relative
{"t":"btn","b":"left|right|middle","s":"down|up|click|dbl"}
{"t":"scroll","dx":0,"dy":120}              120 = one notch; positive dy scrolls down
{"t":"key","k":"ctrl+shift+t","s":"down|up"}  s omitted = press; names in command.rs
{"t":"text","v":"hello"}                    typed as unicode
{"t":"power","a":"lock|sleep|hibernate|logoff|restart|shutdown"}
{"t":"open","url":"https://..."}            http(s) only, opened in the default browser
{"t":"clip.set","text":"..."}               replace PC clipboard (max 256 KB)
{"t":"clip.get"}                            -> {"t":"clip","text":"..."}
{"t":"win.list"}                            -> {"t":"win.list","items":[{id,title,exe,icon,minimized,foreground}]}
{"t":"win.act","id":123,"a":"focus|min|max|restore|close"}
{"t":"app.list"}                            -> {"t":"app.list","items":[{id,name,icon}]}
{"t":"app.launch","id":"<id from app.list>"}
{"t":"ping"}                                -> {"t":"pong"}
```
`GET /icon/<key>.png` serves window/app icons (48-64 px). Keys are issued by the server in the lists above, so
clients can only launch or fetch things the server itself found; paths and arbitrary window handles are refused.
Server sends `{"t":"hello","host":...,"v":...}` on connect. Unknown or malformed messages are ignored.

## Security

Plain HTTP and, by default, no pairing: anyone who can reach port 1990 can control the PC (type keys,
shut down, read/write the clipboard). Only run it on a network you trust, or use `--require-token`
(still plain HTTP, so the token can be sniffed). `/admin` only answers loopback requests with a
loopback `Host` header. Input is clamped (move, scroll, text length).

## Tests

`cargo test`: unit tests plus end-to-end tests (real server and WebSocket client, mock injector).
Windows-only injection was also verified by moving the real cursor through the socket.
