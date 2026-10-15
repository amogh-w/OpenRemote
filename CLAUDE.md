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
python tools/fetch_lucide.py            # regenerate assets/ui/icons.html (network)
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
- `src/web.rs`: axum routes, WebSocket loop, token check, manifest, `/icon/{file}`, loopback-only `/admin`.
- `src/input.rs`, `src/winapps.rs`: Windows `Injector` (SendInput, power, clipboard, windows, apps, icons).
  Non-Windows builds get a logging stub.
- `src/shell.rs`: tray icon + Win32 message loop (main thread), console attach, error box.
- `src/main.rs`: CLI, settings, tokio runtime on a worker thread.
- `build.rs`: assembles `assets/ui/*` into `$OUT_DIR/index.html`; `web.rs` embeds it with `include_str!`.
- `tests/ws.rs`: spins up the real server on an ephemeral port with a mock injector.

## Adding a feature (protocol message)

1. Add a variant to `Msg` in `command.rs`; handle it in `Dispatcher::handle` (return `Some(json)` for replies).
2. Add a default method to `Injector`; implement it in `input.rs` (Windows) and keep the default for others.
3. Extend the recording mock in `command.rs` (`testing::Rec`) and in `tests/ws.rs` (`Handle`), then add tests.
4. Add the UI in `assets/ui/` and update the protocol table in `README.md`.

## Rules that matter

- **Never trust client input.** Clients refer to apps, icons and windows only by server-issued ids; no paths.
  Validate and clamp everything (`is_safe_url`, `is_icon_key`, move/scroll/text limits).
- Window titles and app names are untrusted: render with `textContent`, never `innerHTML`.
- Every `unsafe` block gets a `// SAFETY:` comment. Win32 shell/icon calls run serialized and COM-initialized
  (see `Registry::icon_png` and `launch`); do not cache failures.
- The admin page is loopback-only and checks the `Host` header. Do not widen it.
- Pairing is optional and off by default; token logic must keep working (`--require-token`) and stay tested.

## Frontend conventions (`assets/ui/`)

- One self-contained page: no CDN, no external requests. Edit the parts, not a generated file.
- Colors are Catppuccin roles (`--ctp-*` from `palette.css`, mapped in `app.css`); see `docs/theme-config.md`.
  Text on solid accent uses `base`. Do not invent hex values.
- Icons are Lucide symbols (`<use href="#i-name">`), listed in `tools/fetch_lucide.py`. Add one there and regenerate.
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
