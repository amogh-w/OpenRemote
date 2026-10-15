//! End-to-end tests: real axum server on an ephemeral port, real WebSocket client, mock injector.

use futures_util::{SinkExt, StreamExt};
use openremote::command::{Button, Dispatcher, Injector};
use openremote::web::{self, AppState};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Rec(Mutex<Vec<String>>);

struct Handle(Arc<Rec>);

impl Injector for Handle {
    fn move_rel(&self, dx: i32, dy: i32) {
        self.0 .0.lock().unwrap().push(format!("move {dx} {dy}"));
    }
    fn button(&self, b: Button, down: bool) {
        self.0 .0.lock().unwrap().push(format!("btn {b:?} {down}"));
    }
    fn wheel(&self, h: bool, d: i32) {
        self.0 .0.lock().unwrap().push(format!("wheel {h} {d}"));
    }
    fn key(&self, vk: u8, down: bool) {
        self.0 .0.lock().unwrap().push(format!("key {vk} {down}"));
    }
    fn type_unit(&self, u: u16) {
        self.0 .0.lock().unwrap().push(format!("type {u}"));
    }
    fn vk_for_char(&self, c: char) -> u8 {
        c.to_ascii_uppercase() as u8
    }
}

async fn start() -> (u16, Arc<Rec>) {
    start_with(true).await
}

async fn start_with(require_token: bool) -> (u16, Arc<Rec>) {
    let rec = Arc::new(Rec::default());
    let state = Arc::new(AppState {
        dispatcher: Dispatcher::new(Handle(rec.clone())),
        token: TOKEN.into(),
        require_token,
        hostname: "TEST-PC".into(),
        verbose: true,
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(web::serve(listener, state));
    (port, rec)
}

async fn http_get(port: u16, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    String::from_utf8_lossy(&raw).into_owned() // binary bodies (PNG) become lossy text; headers stay intact
}

async fn wait_for(rec: &Rec, n: usize) -> Vec<String> {
    for _ in 0..100 {
        {
            let v = rec.0.lock().unwrap();
            if v.len() >= n {
                return v.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    rec.0.lock().unwrap().clone()
}

#[tokio::test]
async fn rejects_missing_and_wrong_token() {
    let (port, _) = start().await;
    for url in [format!("ws://127.0.0.1:{port}/ws"), format!("ws://127.0.0.1:{port}/ws?t=nope")] {
        let err = tokio_tungstenite::connect_async(url).await.unwrap_err().to_string();
        assert!(err.contains("401"), "expected 401, got {err}");
    }
}

#[tokio::test]
async fn authenticated_client_drives_input() {
    let (port, rec) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws?t={TOKEN}")).await.unwrap();

    let hello = ws.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(hello.contains(r#""t":"hello""#) && hello.contains("TEST-PC"));

    for m in [
        r#"{"t":"move","dx":4,"dy":-1}"#,
        r#"{"t":"btn","b":"left","s":"click"}"#,
        r#"{"t":"scroll","dy":120}"#,
        r#"{"t":"key","k":"ctrl+c"}"#,
        r#"{"t":"text","v":"a"}"#,
        "not json",
        r#"{"t":"nuke"}"#,
    ] {
        ws.send(Message::Text(m.into())).await.unwrap();
    }
    ws.send(Message::Text(r#"{"t":"ping"}"#.into())).await.unwrap();
    let pong = ws.next().await.unwrap().unwrap().into_text().unwrap();
    assert_eq!(pong.as_str(), r#"{"t":"pong"}"#);

    let got = wait_for(&rec, 10).await;
    assert_eq!(
        got,
        [
            "move 4 -1",
            "btn Left true",
            "btn Left false",
            "wheel false -120",
            "key 17 true",
            "key 67 true",
            "key 67 false",
            "key 17 false",
            "type 97",
        ]
    );
}

#[tokio::test]
async fn serves_frontend_and_loopback_admin() {
    let (port, _) = start().await;
    let index = http_get(port, "/").await;
    assert!(index.starts_with("HTTP/1.1 200") && index.contains("OpenRemote"));

    let admin = http_get(port, "/admin").await;
    assert!(admin.starts_with("HTTP/1.1 200"), "{}", &admin[..60.min(admin.len())]);
    assert!(admin.contains("TEST-PC"));
}

#[tokio::test]
async fn admin_rejects_foreign_host_header() {
    let (port, _) = start().await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"GET /admin HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    assert!(out.starts_with("HTTP/1.1 403"), "{out}");
}

#[tokio::test]
async fn manifest_reveals_token_only_to_holders() {
    let (port, _) = start().await;

    let anon = http_get(port, "/manifest.webmanifest").await;
    assert!(anon.starts_with("HTTP/1.1 200") && anon.contains("application/manifest+json"));
    assert!(anon.contains(r#""start_url":"/""#), "{anon}");
    assert!(!anon.contains(TOKEN), "anonymous manifest must not leak the token");

    let wrong = http_get(port, "/manifest.webmanifest?t=nope").await;
    assert!(wrong.contains(r#""start_url":"/""#) && !wrong.contains(TOKEN));

    let ok = http_get(port, &format!("/manifest.webmanifest?t={TOKEN}")).await;
    assert!(ok.contains(&format!(r#""start_url":"/?t={TOKEN}""#)), "{ok}");
    assert!(ok.contains(r#""display":"standalone""#) && ok.contains("/icon-512.png"));
}

#[tokio::test]
async fn pwa_assets_are_served() {
    let (port, _) = start().await;
    for (path, ctype) in [("/icon-180.png", "image/png"), ("/icon-192.png", "image/png"), ("/icon-512.png", "image/png"), ("/sw.js", "application/javascript")] {
        let r = http_get(port, path).await;
        assert!(r.starts_with("HTTP/1.1 200"), "{path}: {}", &r[..40.min(r.len())]);
        assert!(r.to_lowercase().contains(&format!("content-type: {ctype}")), "{path} content type");
    }
    assert!(http_get(port, "/sw.js").await.to_lowercase().contains("service-worker-allowed: /"));
    let index = http_get(port, "/").await;
    assert!(index.contains("apple-touch-icon") && index.contains("apple-mobile-web-app-capable"));
}

#[tokio::test]
async fn open_mode_accepts_connections_without_token() {
    let (port, rec) = start_with(false).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();
    let hello = ws.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(hello.contains("hello"));
    ws.send(Message::Text(r#"{"t":"move","dx":1,"dy":2}"#.into())).await.unwrap();
    assert_eq!(wait_for(&rec, 1).await, ["move 1 2"]);

    // the admin page must not advertise a token in this mode
    let admin = http_get(port, "/admin").await;
    assert!(admin.starts_with("HTTP/1.1 200") && !admin.contains("?t=") && admin.contains("Pairing is OFF"));
}
