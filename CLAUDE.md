# CLAUDE.md

Guidance for working on this repository.

## What this is

OpenRemote: a Windows program (Rust, one binary) that turns a phone into a touchpad, keyboard, media remote,
app switcher and launcher. It serves a web app (PWA) over HTTP plus a JSON WebSocket. Read `README.md` for
features, architecture and protocol; `DESIGN.md` for design notes and the roadmap.

## Commands

Use PowerShell. Rust is installed under the user profile, so put it on PATH first.

```powershell
$env:PATH="$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo test                              # all unit + end-to-end tests (should stay green)
cargo build --release                   # needs a running openremote.exe to be stopped first
node --check assets/ui/app.js           # syntax check of the web app script
npm run build                           # after UI changes: regenerate assets/ui/app.css + icons.html (Node; commit the output)
python tools/gen_icons.py               # regenerate PWA + tray icons (Pillow)
Get-Process openremote | Stop-Process -Force
Start-Process .\target\release\openremote.exe   # detached; release build has no console window
```

- Toolchain is `stable-x86_64-pc-windows-gnu`. `.cargo/config.toml` sets `link-self-contained=yes` because the
  installed MinGW lacks `libgcc_eh`. Do not remove it unless the toolchain changes.
- The release binary uses the GUI subsystem: no console. Use `--no-tray` or run a debug build to see output.
- Default port 1990. Settings: `%APPDATA%\openremote\settings.json`.
- Redirecting cargo's stderr in PowerShell prints a harmless `NativeCommandError`; check the real output.

## Code map

- `src/command.rs`: `Msg` (serde, tagged by `t`), `Injector` trait (platform backend, default no-op methods),
  `Dispatcher` (validation, clamping, replies), key-name table. Platform-independent and unit-tested.
- `src/web.rs`: axum routes, WebSocket loop, manifest, `/icon/{file}`, `guard` (Host, Origin, pairing), loopback-only `/admin`.
- `src/smooth.rs`: pointer smoother. Pointer moves are queued and released in small steps by a pacer thread (exponential drain, time constant `--smoothing`, default 15 ms); clicks and scrolls flush it first so they land where the finger stopped. Platform independent and unit-tested.
- `src/pairing.rs`: pairing offers, paired phones (hashes), lockout. Platform independent and unit-tested.
- `src/media.rs`: now-playing info and cover art via the WinRT media session API (`windows` crate; Windows only).
- `src/uac.rs`: optional SYSTEM input helper (event wire format, `HelperLink` client, helper process, service, install/uninstall). Input events always go through `WinInjector::emit`, which tries the helper first.
- `src/input.rs`, `src/winapps.rs`: Windows `Injector` (SendInput, power, clipboard, windows, apps, icons).
  Non-Windows builds get a logging stub.
- `src/image.rs`: PNG validation and RGBA conversion for clipboard images (limits live here).
- `src/shell.rs`: tray icon + Win32 message loop (main thread), console attach, error box.
- `src/main.rs`: CLI, settings, tokio runtime on a worker thread.
- `build.rs`: assembles `assets/ui/*` (and `kit/ui.js`) into `$OUT_DIR/index.html`; `web.rs` embeds it with `include_str!`. It does not run Node.
- `tests/ws.rs`: spins up the real server on an ephemeral port with a mock injector.

## Adding a feature (protocol message)

1. Add a variant to `Msg` in `command.rs`; handle it in `Dispatcher::handle` (return `Some(json)` for replies).
2. Add a default method to `Injector`; implement it in `input.rs` (Windows) and keep the default for others.
3. Extend the recording mock in `command.rs` (`testing::Rec`) and in `tests/ws.rs` (`Handle`), then add tests.
4. Add the UI in `assets/ui/` and update the protocol table in `README.md`.

## Rules that matter

- **Never trust client input.** Clients refer to apps, icons and windows only by server-issued ids; no paths.
  Validate and clamp everything (`is_safe_url`, `is_icon_key`, move/scroll/text limits).
- Clipboard images: validate PNG dimensions (`image::png_dimensions`) before decoding anything, and accept only `image/png` bodies.
  Backend clipboard calls go through the retrying `clipboard()` helper.
- Window titles and app names are untrusted: render with `textContent`, never `innerHTML`.
- Every `unsafe` block gets a `// SAFETY:` comment. Win32 shell/icon calls run serialized and COM-initialized
  (see `Registry::icon_png` and `launch`); do not cache failures.
- Never install, start or remove the input helper service on the user's machine while testing (it needs administrator approval and changes the system). Test the pipe by running `--input-helper` by hand and sending only ping or zero-movement frames.
- A middleware in `web.rs` (`guard`) checks `Host` and `Origin` on every route. Keep it in front of any new route.
- The admin page is loopback-only and checks the `Host` header. Do not widen it.
- Pairing is on by default (`pairing.rs`, cookie `openremote_key`, only key hashes on disk, loopback trusted). New routes that act on the PC or read its data must stay behind the `guard` in `web.rs` (it requires a paired phone unless the path is in `is_public`). Never put a key or code in a URL. Test the checks with `Pairing::without_loopback_exemption()`, because test clients connect from loopback.

## Frontend conventions (`assets/ui/`)

- One self-contained page: no CDN, no external requests. Edit the parts, not a generated file.
- The look comes from the cuppa-ui kit (`assets/ui/kit/`: kit.css, palette.css, ui.js, icons). Build markup from kit classes (`btn`, `row`, `chip`, `seg`, `sheet`, ...); `app.src.css` holds only what the kit lacks (touchpad, keyboard, page shell).
- Edit `body.html`, `app.js`, `app.src.css`, then run `npm run build`; never edit the generated `app.css` or `icons.html`. Class names must appear whole in `body.html` or `app.js` for Tailwind to find them.
- Colors are Catppuccin roles (`bg-base`, `text-subtext0`, `bg-accent`, `--ctp-*`); no hex values.
  Text on a solid accent or semantic fill is `text-crust`.
- Icons are Lucide symbols (`<use href="#name">` with the Lucide name), listed in `assets/ui/kit/icons.txt`. Add one there and run `npm run build`.
- Motion: animate only `transform` and `opacity`, 120-320 ms, and keep the `prefers-reduced-motion` block working.
- Storage keys are prefixed `or_`. After UI changes, bump the cache name in `assets/sw.js`.
- Keep touch targets at least 44 px and respect safe-area insets.

## Testing a UI change

Run `cargo test`, `node --check assets/ui/app.js`, rebuild release and restart the server. Then check it in Chrome
at phone size (an iframe of 390x844 from a blob URL keeps the real origin) in both a dark and the light flavor.
Drive the page with synthetic touch/pointer events and a `WebSocket.prototype.send` spy instead of real actions:
never trigger power, close, launch or clipboard writes on the user's machine.

## Repo hygiene

- Do not mention, name or link any other product this resembles; describe everything as its own idea.
- Do not put real window titles, emails, hostnames or personal app lists in screenshots, docs or tests. Use the
  mock-connection approach (fictional titles, host `Desktop-PC`).
- Local-only ignore rules belong in `.git/info/exclude`, not `.gitignore`.
- Commit only when asked. Keep `README.md` in sync with behavior, especially the protocol table and roadmap.

## Editing gotchas

- Some files use CRLF (for example `Cargo.toml`). Match the file's newline style when editing programmatically.
- Backslash escapes in inline shell/Python snippets can get mangled; prefer small script files or the edit tools.
- A stray NUL byte once slipped into a Rust test string; keep escapes explicit (`\u{0}`).
