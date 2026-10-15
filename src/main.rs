use openremote::command::Dispatcher;
use openremote::input::PlatformInjector;
use openremote::settings::{self, Settings};
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
  -v, --verbose     log ignored messages
  -h, --help";

#[tokio::main]
async fn main() {
    let mut st = Settings::load_or_create().unwrap_or_else(|e| die(&format!("cannot read settings: {e}")));
    let mut hostname = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "My PC".into());
    let mut verbose = false;
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

    let listener = TcpListener::bind(("0.0.0.0", st.port))
        .await
        .unwrap_or_else(|e| die(&format!("cannot listen on port {}: {e}", st.port)));

    println!("Admin page (QR code): http://127.0.0.1:{}/admin", st.port);
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
    if let Err(e) = web::serve(listener, state).await {
        die(&format!("server error: {e}"));
    }
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2);
}
