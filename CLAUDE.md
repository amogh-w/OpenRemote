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
.\scripts\check.ps1                     # fmt + clippy (-D warnings) + tests + node --check, same as CI
node C:\Users\Amogh\Projects\cuppa\scripts\harness.mjs status   # report drift of the kit and shared code from the masters (sync --write repairs)
node C:\Users\Amogh\Projects\cuppa\scripts\harness.mjs lint openremote   # UI rules: tokens, motion scale, radii, type scale, theme, icons (also a step of check.ps1)
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
- `src/web.rs`: axum routes, WebSocket loop, manifest, `/icon/{file}`, `guard` (Host, Origin, pairing). The pairing routes and the dashboard are mounted from `pairroutes.rs` and `admin.rs`.
- `src/smooth.rs`: pointer smoother. Pointer moves are queued and released in small steps by a pacer thread (exponential drain, time constant `--smoothing`, default 15 ms); clicks and scrolls flush it first so they land where the finger stopped. Platform independent and unit-tested.
- `src/pairing.rs`, `src/pairroutes.rs`, `src/admin.rs`, `src/shell.rs`, `src/autostart.rs`, `src/netguard.rs`, `src/sleeptimer.rs`, `src/applog.rs` are shared with Kai Remote: change the master in the cuppa plugin repo (`C:\Users\Amogh\Projects\cuppa\shared`), then run `node C:\Users\Amogh\Projects\cuppa\scripts\harness.mjs sync --write`. Same for the web side: `assets/ui/admin.html`, `admin.js`, `pair-client.js` and the pair screen between the `cuppa:pair-screen` markers in `body.html`. Pairing: one offer carries a 6-digit code to type, a QR and a link token (valid 5 minutes, once); only key hashes are stored. `PAIRING_COOKIE` and `APP_NAME` are in `lib.rs`.
- `src/media.rs`: now-playing info and cover art via the WinRT media session API (`windows` crate; Windows only).
- `src/uac.rs`: optional SYSTEM input helper (event wire format, `HelperLink` client, helper process, service, install/uninstall). Input events always go through `WinInjector::emit`, which tries the helper first.
- `src/input.rs`, `src/winapps.rs`: Windows `Injector` (SendInput, power, clipboard, windows, apps, icons).
  Non-Windows builds get a logging stub.
- `src/image.rs`: PNG validation and RGBA conversion for clipboard images (limits live here).
- Tray (shared `shell.rs`): Open dashboard, Pair a phone, Copy phone link, Pause pairing, Start with Windows, Open log folder, Restart, Exit. One copy per port (a second launch opens the dashboard); `--restarted` waits for the old copy; `--startup` (written by Start with Windows) skips the first-run dashboard.
- `src/main.rs`: CLI, settings, tokio runtime on a worker thread.
- `build.rs`: assembles `assets/ui/*` (and `kit/ui.js`) into `$OUT_DIR/index.html`; `web.rs` embeds it with `include_str!`. It does not run Node.
- `tests/ws.rs`: spins up the real server on an ephemeral port with a mock injector.

## Adding a feature (protocol message)

1. Add a variant to `Msg` in `command.rs`; handle it in `Dispatcher::handle` (return `Some(json)` for replies).
2. Add a default method to `Injector`; implement it in `input.rs` (Windows) and keep the default for others.
3. Extend the recording mock in `command.rs` (`testing::Rec`) and in `tests/ws.rs` (`Handle`), then add tests.
4. Add the UI in `assets/ui/` and update the protocol table in `README.md`.
5. Add valid and invalid examples to `tests/fixtures/protocol.json` and the new name to `name()`/`ALL` in `tests/protocol.rs` (it will not compile until you do). `scripts\check.ps1` also fails if `app.js` sends a message the fixture lacks.

## Rules that matter

- **Never trust client input.** Clients refer to apps, icons and windows only by server-issued ids; no paths.
  Validate and clamp everything (`is_safe_url`, `is_icon_key`, move/scroll/text limits).
- Clipboard images: validate PNG dimensions (`image::png_dimensions`) before decoding anything, and accept only `image/png` bodies.
  Backend clipboard calls go through the retrying `clipboard()` helper.
- Window titles and app names are untrusted: render with `textContent`, never `innerHTML`.
- Every `unsafe` block gets a `// SAFETY:` comment. Win32 shell/icon calls run serialized and COM-initialized
  (see `Registry::icon_png` and `launch`); do not cache failures.
- Log: `openremote.log` beside `settings.json` in `%APPDATA%\openremote` (rotates at 512 KB, one `.1` copy); it is the only record in the GUI release build, which has no console. Events worth finding later go through `applog::info/warn/error` (`src/applog.rs`, shared with the other project: edit it in the cuppa plugin repo (`C:\Users\Amogh\Projects\cuppa\shared`) and run `node C:\Users\Amogh\Projects\cuppa\scripts\harness.mjs sync --write`). Never log keys, pairing secrets, stream links or other private data.
- Never install, start or remove the input helper service on the user's machine while testing (it needs administrator approval and changes the system). Test the pipe by running `--input-helper` by hand and sending only ping or zero-movement frames.
- A middleware in `web.rs` (`guard`) checks `Host` and `Origin` on every route. Keep it in front of any new route.
- The dashboard (`/admin`, `admin.rs`) answers only loopback peers with a loopback `Host`, and every change needs the `X-Admin: 1` header and a matching `Origin`. Do not widen it. It lists, adds (named offers), renames and removes devices and pauses pairing.
- Pairing is on by default (`pairing.rs`, cookie `openremote_key`, only key hashes on disk, loopback trusted). New routes that act on the PC or read its data must stay behind the `guard` in `web.rs` (it requires a paired phone unless the path is in `is_public`). Never put a key or code in a URL. Test the checks with `Pairing::without_loopback_exemption()`, because test clients connect from loopback.

## Frontend conventions (`assets/ui/`)

- One self-contained page: no CDN, no external requests. Edit the parts, not a generated file.
- The look comes from the cuppa-ui kit (`assets/ui/kit/`: kit.css, palette.css, ui.js, icons). Build markup from kit classes (`btn`, `row`, `chip`, `seg`, `sheet`, ...); `app.src.css` holds only what the kit lacks (touchpad, keyboard, page shell).
- Edit `body.html`, `app.js`, `app.src.css`, then run `npm run build`; never edit the generated `app.css` or `icons.html`. Class names must appear whole in `body.html` or `app.js` for Tailwind to find them.
- `assets/ui/remote-client.js` (the WebSocket link: backoff, reconnect on return, silent-link detection) is shared between OpenRemote and Kai Remote. Edit it in the cuppa plugin repo (`C:\Users\Amogh\Projects\cuppa\shared`), run `node C:\Users\Amogh\Projects\cuppa\scripts\harness.mjs sync --write`, and test with `node --test "shared/*.test.mjs" (run in the cuppa plugin repo)`. `app.js` only uses `link.send`, `link.isOpen` and the callbacks.
- Colors are Catppuccin roles (`bg-base`, `text-subtext0`, `bg-accent`, `--ctp-*`); no hex values (the `theme-color` meta is the one exception).
  Text on a solid accent or semantic fill is `text-crust`.
- Icons are Lucide symbols (`<use href="#name">` with the Lucide name), listed in `assets/ui/kit/icons.txt`. Add one there and run `npm run build`.
- Motion, radii and type sizes follow the cuppa-ui skill (`references/motion.md` is the only place the numbers live). `harness.mjs lint` fails the check on a violation. Keep the `prefers-reduced-motion` block working.
- Theme (flavor, accent, corner radius) is the kit's `theme.js`, inlined in the head by `build.rs`; the settings sheet uses its `data-flavor` / `data-accent` / `data-radius` hooks. Never write a second switcher. Default accent is lavender.
- Storage keys are prefixed `or_` (`ui-*` belongs to `theme.js`; the lint checks this). The service worker cache name is stamped from a hash of the page at runtime (`__BUILD__` in `assets/sw.js`); do not hard-code a version.
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
- `README.md` follows the cuppa README convention (`C:\Users\Amogh\Projects\cuppa\skills\harness\references\readme.md`): same sections in the same order. The Tray and dashboard, Pairing and Sleep timer sections use the shared wording, so change them in both projects' READMEs together.

## Editing gotchas

- Some files use CRLF (for example `Cargo.toml`). Match the file's newline style when editing programmatically.
- Backslash escapes in inline shell/Python snippets can get mangled; prefer small script files or the edit tools.
- A stray NUL byte once slipped into a Rust test string; keep escapes explicit (`\u{0}`).
