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
  --require-token   turn pairing ON (clients need the token / QR link); saved
  --no-require-token  turn pairing OFF again (the default); saved
  --regen-token     create a new pairing token (invalidates old QR codes)
  --no-tray         no tray icon; run the server in the foreground
  -v, --verbose     log ignored messages
  -h, --help";

fn main() {
    shell::attach_console();
    let mut st = Settings::load_or_create().unwrap_or_else(|e| die(&format!("cannot read settings: {e}")));
    let mut hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "My PC".into());
    let mut verbose = false;
    let mut tray = true;
    let mut save = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => {
                st.port = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| die("--port needs a number"));
                save = true;
            }
            "--hostname" => hostname = args.next().unwrap_or_else(|| die("--hostname needs a value")),
            "--require-token" | "--no-require-token" => {
                st.require_token = a == "--require-token";
                save = true;
            }
            "--regen-token" => {
                st.token = settings::new_token();
                save = true;
            }
            "--no-tray" => tray = false,
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

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| die(&format!("cannot start runtime: {e}")));
    let listener = rt.block_on(TcpListener::bind(("0.0.0.0", st.port))).unwrap_or_else(|e| {
        die(&format!("Cannot listen on port {}: {e}\n\nIs OpenRemote already running?", st.port))
    });

    let admin_url = format!("http://127.0.0.1:{}/admin", st.port);
    println!("Admin page (QR code): {admin_url}");
    for url in web::lan_urls(st.port, st.require_token.then_some(st.token.as_str())) {
        println!("Phone link:           {url}");
    }
    if !st.require_token {
        println!("Pairing is OFF: anyone on your network can control this PC (--require-token to enable).");
    }
    println!("Settings file: {}", settings::path().display());

    let state = Arc::new(AppState {
        dispatcher: Dispatcher::new(PlatformInjector::default()),
        token: st.token,
        require_token: st.require_token,
        hostname,
        verbose,
    });

    // The server runs on its own thread; this thread owns the tray icon and its message loop.
    let server = std::thread::spawn(move || {
        if let Err(e) = rt.block_on(web::serve(listener, state)) {
            die(&format!("server error: {e}"));
        }
    });
    if tray {
        if let Err(e) = shell::run_tray(admin_url) {
            eprintln!("tray icon unavailable ({e}); running without it");
        }
    }
    let _ = server.join();
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    shell::error_box(msg);
    std::process::exit(2);
}
