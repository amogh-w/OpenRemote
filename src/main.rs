// Release builds are GUI-subsystem (no console window); `shell::attach_console` restores output
// when started from a terminal. Debug builds keep the normal console.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use openremote::command::Dispatcher;
use openremote::input::PlatformInjector;
use openremote::settings::{self, Settings};
use openremote::shell;
use openremote::web::{self, AppState};
use std::sync::Arc;
use tokio::net::TcpListener;

const USAGE: &str = "\
openremote - control this PC from a phone browser

USAGE: openremote [OPTIONS]
  --port <PORT>     listen port (default from settings, 1990)
  --hostname <NAME> name shown in the admin page and app (default: computer name)
  --install-helper --yes   install the SYSTEM input helper (UAC prompts, admin windows, lock screen)
  --uninstall-helper       remove it again
  --helper-status          is the helper running?
  --pairing / --no-pairing   require phones to pair (the default) or not; saved
  --autostart       start with Windows (this user only); --no-autostart turns it off
  --smoothing <MS>  pointer smoothing time constant in ms (default 15, 0 = off); higher is smoother but adds delay
  --no-tray         no tray icon; run the server in the foreground
  --startup         set by Start with Windows: do not open the dashboard
  --restarted       set by Restart: wait for the old copy to exit
  -v, --verbose     log ignored messages
  -h, --help";

/// Default pointer smoothing time constant: a burst of finger movement is spread over a few of these.
const DEFAULT_SMOOTHING_MS: u32 = 15;

const TRAY_ICON_RGBA: &[u8] = include_bytes!("../assets/tray-32.rgba");

fn main() {
    shell::attach_console();
    // service, helper and install modes run before anything else starts
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = openremote::uac::cli(&argv) {
        std::process::exit(code);
    }
    openremote::applog::init(settings::path().with_file_name("openremote.log"));
    let mut st = Settings::load_or_create().unwrap_or_else(|e| die(&format!("cannot read settings: {e}")));
    let mut hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "My PC".into());
    let mut verbose = false;
    let mut smoothing = DEFAULT_SMOOTHING_MS;
    let mut tray = true;
    let mut save = false;
    let (mut startup, mut restarted) = (false, false);

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => {
                st.port = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| die("--port needs a number"));
                save = true;
            }
            "--hostname" => hostname = args.next().unwrap_or_else(|| die("--hostname needs a value")),
            "--pairing" | "--no-pairing" => {
                st.require_pairing = a == "--pairing";
                save = true;
            }
            "--autostart" | "--no-autostart" => {
                let on = a == "--autostart";
                match openremote::autostart::set(on) {
                    Ok(()) => println!("Start with Windows is now {}.", if on { "on" } else { "off" }),
                    Err(e) => die(&e),
                }
                return;
            }
            "--smoothing" => {
                smoothing = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--smoothing needs a number of milliseconds"))
            }
            "--no-tray" => tray = false,
            "--startup" => startup = true,
            "--restarted" => restarted = true,
            "-v" | "--verbose" => verbose = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => die(&format!("unknown argument {other}\n\n{USAGE}")),
        }
    }
    if save {
        st.save().unwrap_or_else(|e| die(&format!("cannot save settings: {e}")));
    }

    let admin_url = format!("http://127.0.0.1:{}/admin", st.port);
    // One copy per user. A second launch just shows the dashboard of the first. After a restart the old copy is
    // still on its way out, so wait for it.
    if !shell::single_instance(st.port, if restarted { 10 } else { 0 }) {
        if !startup {
            shell::open_in_browser(&admin_url);
        }
        return;
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| die(&format!("cannot start runtime: {e}")));
    let listener = bind(&rt, st.port, restarted).unwrap_or_else(|e| {
        die(&format!(
            "Cannot listen on port {}: {e}

Is OpenRemote already running?",
            st.port
        ))
    });

    println!("Dashboard:   {admin_url}");
    for url in web::lan_urls(st.port) {
        println!("Phone link:  {url}");
    }
    if st.require_pairing {
        println!("Pairing is ON: open the tray menu > Pair a phone (or the dashboard) to add a phone.");
    } else {
        println!("Pairing is OFF: anyone on your network can control this PC.");
    }
    println!("Settings file: {}", settings::path().display());

    openremote::applog::info(&format!(
        "OpenRemote {} listening on port {}, pairing {}",
        env!("CARGO_PKG_VERSION"),
        st.port,
        if st.require_pairing { "on" } else { "off" }
    ));
    let state = Arc::new(AppState {
        dispatcher: Dispatcher::with_smoothing(PlatformInjector::default(), smoothing),
        hostname,
        verbose,
        pairing: openremote::pairing::Pairing::new(st.require_pairing, Some(settings::devices_path())),
        port: st.port,
    });
    // Started by hand (not at login, not by Restart): show the dashboard, so it is clear the app is running. With
    // nothing paired yet it goes straight to the "Add a device" section.
    if !startup && !restarted {
        let add = if st.require_pairing && state.pairing.devices().is_empty() { "#add" } else { "" };
        shell::open_in_browser(&format!("{admin_url}{add}"));
    }

    // The server runs on its own thread; this thread owns the tray icon and its message loop.
    let server = std::thread::spawn({
        let state = state.clone();
        move || {
            if let Err(e) = rt.block_on(web::serve(listener, state)) {
                die(&format!("server error: {e}"));
            }
        }
    });
    if tray {
        if let Err(e) = shell::run_tray(state, TRAY_ICON_RGBA.to_vec(), admin_url) {
            openremote::applog::warn(&format!("tray icon unavailable ({e}); running without it"));
        }
    }
    let _ = server.join();
}

/// Bind the listen port. After a restart the old copy may still hold it for a moment, so keep trying for a while.
fn bind(rt: &tokio::runtime::Runtime, port: u16, patient: bool) -> std::io::Result<TcpListener> {
    let started = std::time::Instant::now();
    loop {
        match rt.block_on(TcpListener::bind(("0.0.0.0", port))) {
            Ok(l) => return Ok(l),
            Err(e) if patient && started.elapsed() < std::time::Duration::from_secs(10) => {
                let _ = e;
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => return Err(e),
        }
    }
}

fn die(msg: &str) -> ! {
    openremote::applog::error(msg);
    shell::error_box(msg);
    std::process::exit(2);
}
