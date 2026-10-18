//! End-to-end tests: real axum server on an ephemeral port, real WebSocket client, mock injector.

use futures_util::{SinkExt, StreamExt};
use openremote::command::{AppEntry, Button, Dispatcher, Injector, WindowAct, WindowInfo};
use openremote::web::{self, AppState};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

const ICON_KEY: &str = "0123456789abcdef";

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
    fn windows(&self) -> Vec<WindowInfo> {
        vec![WindowInfo { id: 42, title: "Notes".into(), exe: "notepad.exe".into(), icon: ICON_KEY.into(), minimized: false, maximized: false, foreground: true }]
    }
    fn window_action(&self, id: u64, a: WindowAct) {
        self.0 .0.lock().unwrap().push(format!("win {id} {a:?}"));
    }
    fn apps(&self) -> Vec<AppEntry> {
        vec![AppEntry { id: ICON_KEY.into(), name: "Calculator".into(), icon: ICON_KEY.into() }]
    }
    fn launch_app(&self, id: &str) {
        self.0 .0.lock().unwrap().push(format!("launch {id}"));
    }
    fn icon_png(&self, key: &str) -> Option<Vec<u8>> {
        (key == ICON_KEY).then(|| b"PNG-fake".to_vec())
    }
    fn clip_set(&self, t: &str) -> bool {
        self.0 .0.lock().unwrap().push(format!("clip {t}"));
        t != "fail"
    }
    fn clip_image_info(&self) -> Option<(u32, u32)> {
        Some((16, 9))
    }
    fn clip_image_png(&self) -> Option<Vec<u8>> {
        Some(b"PNG-from-pc".to_vec())
    }
    fn media(&self) -> Option<openremote::command::MediaInfo> {
        use openremote::command::{MediaInfo, MediaState};
        Some(MediaInfo { state: MediaState::Paused, title: "Song".into(), artist: "Band".into(), app: "Player".into(), pos: 5, dur: 180, art: true })
    }
    fn media_art(&self) -> Option<Vec<u8>> {
        Some(vec![0xFF, 0xD8, 1, 2, 3])
    }
    fn clip_image_set(&self, png: &[u8]) -> bool {
        self.0 .0.lock().unwrap().push(format!("clipimg {}", png.len()));
        true
    }
}

async fn start() -> (u16, Arc<Rec>) {
    let rec = Arc::new(Rec::default());
    let state = Arc::new(AppState {
        dispatcher: Dispatcher::new(Handle(rec.clone())),
        hostname: "TEST-PC".into(),
        verbose: true,
        pairing: openremote::pairing::Pairing::open(),
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

async fn http_post(port: u16, path: &str, content_type: &str, body: &[u8]) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1
Host: 127.0.0.1:{port}
Content-Type: {content_type}
Content-Length: {}
Connection: close

",
        body.len()
    );
    s.write_all(head.as_bytes()).await.unwrap();
    // the server may answer 413 and close before reading everything: read the reply while still writing, so a
    // connection reset cannot swallow it, and ignore write errors
    let (mut rd, mut wr) = s.into_split();
    let reader = tokio::spawn(async move {
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = rd.read(&mut buf).await {
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&buf[..n]);
        }
        raw
    });
    let _ = wr.write_all(body).await;
    let raw = reader.await.unwrap();
    String::from_utf8_lossy(&raw).into_owned()
}

fn tiny_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Grayscale);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&vec![7u8; (w * h) as usize]).unwrap();
    }
    out
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
async fn client_drives_input() {
    let (port, rec) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();

    let hello = ws.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(hello.contains(r#""t":"hello""#) && hello.contains("TEST-PC"));
    assert!(hello.contains(r#""helper":false"#), "hello reports whether the SYSTEM input helper is running: {hello}");

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
    assert!(admin.contains("TEST-PC") && !admin.contains("<svg"), "no QR code any more");
}

async fn raw_request(port: u16, request: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out).await;
    out
}

#[tokio::test]
async fn other_websites_cannot_open_the_control_socket() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (port, rec) = start().await;
    let url = format!("ws://127.0.0.1:{port}/ws");

    // a page on another site: browsers always send its Origin
    let mut evil = url.clone().into_client_request().unwrap();
    evil.headers_mut().insert("Origin", "http://evil.example".parse().unwrap());
    let err = tokio_tungstenite::connect_async(evil).await.unwrap_err().to_string();
    assert!(err.contains("403"), "expected 403, got {err}");

    // the page this server serves: same origin as the Host header
    let mut own = url.clone().into_client_request().unwrap();
    own.headers_mut().insert("Origin", format!("http://127.0.0.1:{port}").parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(own).await.unwrap();
    ws.next().await.unwrap().unwrap(); // hello
    ws.send(Message::Text(r#"{"t":"move","dx":1,"dy":1}"#.into())).await.unwrap();
    assert_eq!(wait_for(&rec, 1).await, ["move 1 1"]);

    // a script with no Origin at all (not a browser) still works
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    assert!(ws.next().await.unwrap().unwrap().into_text().unwrap().contains("hello"));
}

#[tokio::test]
async fn rebinding_hosts_and_cross_site_posts_are_refused() {
    let (port, rec) = start().await;
    for path in ["/", "/media/art", "/clip/image.png", "/manifest.webmanifest"] {
        let r = raw_request(port, &format!("GET {path} HTTP/1.1
Host: evil.example
Connection: close

")).await;
        assert!(r.starts_with("HTTP/1.1 403"), "{path}: {}", &r[..30.min(r.len())]);
    }
    let png = tiny_png(2, 2);
    let head = format!(
        "POST /clip/image HTTP/1.1
Host: 127.0.0.1:{port}
Origin: http://evil.example
Content-Type: image/png
Content-Length: {}
Connection: close

",
        png.len()
    );
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(head.as_bytes()).await.unwrap();
    s.write_all(&png).await.unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out).await;
    assert!(out.starts_with("HTTP/1.1 403"), "{}", &out[..30.min(out.len())]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rec.0.lock().unwrap().is_empty(), "a cross-site upload must not reach the clipboard");
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
async fn manifest_makes_the_page_installable() {
    let (port, _) = start().await;
    let m = http_get(port, "/manifest.webmanifest").await;
    assert!(m.starts_with("HTTP/1.1 200") && m.contains("application/manifest+json"));
    assert!(m.contains(r#""start_url":"/""#) && m.contains(r#""display":"standalone""#) && m.contains("/icon-512.png"), "{m}");
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
    assert!(index.contains("apple-touch-icon") && index.contains("viewport-fit=cover"));
    // the assembled page: nav, sheet, bundled icons (Lucide attribution), motion preferences
    for needle in ["id=\"nav\"", "id=\"sheet\"", "Lucide", "prefers-reduced-motion", "<symbol id=\"touchpad\"", "--ctp-mauve", "id=\"clipget\"", "id=\"imgfile\"", "m.t === 'clip.ack'", "gotClip(m)"] {
        assert!(index.contains(needle), "page is missing {needle}");
    }
    // a single document: exactly one <style>, then the kit script and the app script after the early theme script
    assert_eq!(index.matches("<style>").count(), 1);
    assert!(!index.contains("{head}") && !index.contains("{css}"), "unreplaced build placeholder");
}

#[tokio::test]
async fn connects_without_any_credentials() {
    let (port, rec) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();
    let hello = ws.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(hello.contains("hello"));
    ws.send(Message::Text(r#"{"t":"move","dx":1,"dy":2}"#.into())).await.unwrap();
    assert_eq!(wait_for(&rec, 1).await, ["move 1 2"]);

    // the admin page lists plain links only
    let admin = http_get(port, "/admin").await;
    assert!(admin.starts_with("HTTP/1.1 200") && !admin.contains("?t="));
}

#[tokio::test]
async fn window_switcher_and_launcher_over_websocket() {
    let (port, rec) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();
    ws.next().await.unwrap().unwrap(); // hello

    ws.send(Message::Text(r#"{"t":"win.list"}"#.into())).await.unwrap();
    let list: serde_json::Value = serde_json::from_str(ws.next().await.unwrap().unwrap().into_text().unwrap().as_str()).unwrap();
    assert_eq!(list["t"], "win.list");
    assert_eq!(list["items"][0]["title"], "Notes");
    assert_eq!(list["items"][0]["icon"], ICON_KEY);

    ws.send(Message::Text(r#"{"t":"app.list"}"#.into())).await.unwrap();
    let apps: serde_json::Value = serde_json::from_str(ws.next().await.unwrap().unwrap().into_text().unwrap().as_str()).unwrap();
    assert_eq!(apps["items"][0]["name"], "Calculator");

    ws.send(Message::Text(r#"{"t":"win.act","id":42,"a":"focus"}"#.into())).await.unwrap();
    ws.send(Message::Text(format!(r#"{{"t":"app.launch","id":"{ICON_KEY}"}}"#).into())).await.unwrap();
    ws.send(Message::Text(r#"{"t":"app.launch","id":"C:/Windows/notepad.exe"}"#.into())).await.unwrap(); // refused
    assert_eq!(wait_for(&rec, 2).await, ["win 42 Focus", format!("launch {ICON_KEY}").as_str()]);
}

#[tokio::test]
async fn icon_route_serves_only_issued_keys() {
    let (port, _) = start().await;
    let ok = http_get(port, &format!("/icon/{ICON_KEY}.png")).await;
    assert!(ok.starts_with("HTTP/1.1 200") && ok.to_lowercase().contains("content-type: image/png"), "{ok}");
    assert!(ok.contains("PNG-fake"));
    for path in ["/icon/fedcba9876543210.png", "/icon/..%2f..%2fCargo.toml.png", "/icon/0123456789abcdef", "/icon/0123456789ABCDEF.png"] {
        let r = http_get(port, path).await;
        assert!(r.starts_with("HTTP/1.1 404"), "{path}: {}", &r[..30.min(r.len())]);
    }
}

#[tokio::test]
async fn image_upload_reaches_the_clipboard() {
    let (port, rec) = start().await;
    let png = tiny_png(4, 3);
    let r = http_post(port, "/clip/image", "image/png", &png).await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    assert!(r.contains(r#""ok":true"#) && r.contains(r#""w":4"#) && r.contains(r#""h":3"#), "{r}");
    assert_eq!(wait_for(&rec, 1).await, [format!("clipimg {}", png.len())]);
}

#[tokio::test]
async fn image_upload_rejects_bad_input() {
    let (port, rec) = start().await;
    let png = tiny_png(2, 2);
    let wrong_type = http_post(port, "/clip/image", "text/plain", &png).await;
    assert!(wrong_type.starts_with("HTTP/1.1 415"), "{wrong_type}");
    let form = http_post(port, "/clip/image", "application/x-www-form-urlencoded", &png).await;
    assert!(form.starts_with("HTTP/1.1 415"), "cross-site form posts must not work: {form}");
    let garbage = http_post(port, "/clip/image", "image/png", b"definitely not a png").await;
    assert!(garbage.starts_with("HTTP/1.1 400"), "{garbage}");
    let too_wide = http_post(port, "/clip/image", "image/png", &tiny_png(8200, 4)).await;
    assert!(too_wide.starts_with("HTTP/1.1 413"), "{too_wide}");
    let huge_body = vec![0u8; 33 * 1024 * 1024];
    let big = http_post(port, "/clip/image", "image/png", &huge_body).await;
    // the server may reset the connection instead of replying while the body is still streaming in
    assert!(big.is_empty() || big.starts_with("HTTP/1.1 413"), "{}", &big[..big.len().min(60)]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(rec.0.lock().unwrap().is_empty(), "nothing rejected may reach the clipboard");
}

#[tokio::test]
async fn image_download_serves_the_pc_clipboard() {
    let (port, _) = start().await;
    let r = http_get(port, "/clip/image.png").await;
    assert!(r.starts_with("HTTP/1.1 200") && r.to_lowercase().contains("content-type: image/png"), "{r}");
    assert!(r.to_lowercase().contains("cache-control: no-store") && r.contains("PNG-from-pc"));
}

#[tokio::test]
async fn now_playing_and_cover_art() {
    let (port, _) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();
    ws.next().await.unwrap().unwrap(); // hello
    ws.send(Message::Text(r#"{"t":"media.get"}"#.into())).await.unwrap();
    let m: serde_json::Value = serde_json::from_str(ws.next().await.unwrap().unwrap().into_text().unwrap().as_str()).unwrap();
    assert_eq!(
        m,
        serde_json::json!({"t": "media", "state": "paused", "title": "Song", "artist": "Band", "app": "Player", "pos": 5, "dur": 180, "art": true})
    );
    let art = http_get(port, "/media/art").await;
    assert!(art.starts_with("HTTP/1.1 200") && art.to_lowercase().contains("content-type: image/jpeg"), "{}", &art[..60.min(art.len())]);
}

#[tokio::test]
async fn clipboard_text_roundtrip_reports_success_and_failure() {
    let (port, rec) = start().await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws")).await.unwrap();
    ws.next().await.unwrap().unwrap(); // hello

    ws.send(Message::Text(r#"{"t":"clip.set","text":"hello"}"#.into())).await.unwrap();
    let ack: serde_json::Value = serde_json::from_str(ws.next().await.unwrap().unwrap().into_text().unwrap().as_str()).unwrap();
    assert_eq!(ack, serde_json::json!({"t": "clip.ack", "ok": true}));

    ws.send(Message::Text(r#"{"t":"clip.set","text":"fail"}"#.into())).await.unwrap();
    let nack: serde_json::Value = serde_json::from_str(ws.next().await.unwrap().unwrap().into_text().unwrap().as_str()).unwrap();
    assert_eq!(nack["ok"], false);

    ws.send(Message::Text(r#"{"t":"clip.get"}"#.into())).await.unwrap();
    let got: serde_json::Value = serde_json::from_str(ws.next().await.unwrap().unwrap().into_text().unwrap().as_str()).unwrap();
    assert_eq!(got["t"], "clip");
    assert_eq!(got["image"], serde_json::json!({"w": 16, "h": 9}));
    assert_eq!(wait_for(&rec, 2).await, ["clip hello", "clip fail"]);
}

// ---------------------------------------------------------------------------------------------- pairing

async fn start_paired() -> (u16, Arc<AppState<Handle>>, Arc<Rec>) {
    let rec = Arc::new(Rec::default());
    let state = Arc::new(AppState {
        dispatcher: Dispatcher::new(Handle(rec.clone())),
        hostname: "TEST-PC".into(),
        verbose: true,
        // the test client connects from loopback, so switch the loopback exemption off to exercise the checks
        pairing: openremote::pairing::Pairing::new(true, None)
            .without_loopback_exemption()
            .with_limits(Duration::from_secs(120), 3, Duration::from_secs(60)),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(web::serve(listener, state.clone()));
    (port, state, rec)
}

async fn post_json(port: u16, path: &str, body: &str, extra: &str) -> String {
    raw_request(
        port,
        &format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}", body.len()),
    )
    .await
}

async fn get_with(port: u16, path: &str, extra: &str) -> String {
    raw_request(port, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra}Connection: close\r\n\r\n")).await
}

/// The `name=value` part of the key cookie in a response.
fn cookie_of(resp: &str) -> String {
    let lower = resp.to_lowercase();
    let i = lower.find("set-cookie: openremote_key=").expect("a Set-Cookie header");
    resp[i + "set-cookie: ".len()..].split(';').next().unwrap().to_string()
}

/// Try to open the socket with an optional cookie; returns the error when the server refuses it.
async fn ws_refusal(port: u16, cookie: &str) -> Option<String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    if !cookie.is_empty() {
        req.headers_mut().insert("Cookie", cookie.parse().unwrap());
    }
    tokio_tungstenite::connect_async(req).await.err().map(|e| e.to_string())
}

#[tokio::test]
async fn unpaired_phones_get_the_page_but_nothing_else() {
    let (port, _, _) = start_paired().await;
    assert!(get_with(port, "/", "").await.starts_with("HTTP/1.1 200"), "the page must load so it can show the pairing screen");
    assert!(get_with(port, "/manifest.webmanifest", "").await.starts_with("HTTP/1.1 200"));
    let status = get_with(port, "/pair/status", "").await;
    assert!(status.contains(r#""required":true"#) && status.contains(r#""paired":false"#), "{status}");
    for path in [format!("/icon/{ICON_KEY}.png"), "/media/art".into(), "/clip/image.png".into()] {
        assert!(get_with(port, &path, "").await.starts_with("HTTP/1.1 401"), "{path}");
    }
    let err = ws_refusal(port, "").await.expect("the socket must be refused");
    assert!(err.contains("401"), "{err}");
    assert!(post_json(port, "/clip/image", "x", "").await.starts_with("HTTP/1.1 401"));
}

#[tokio::test]
async fn pairing_with_the_code_gives_access_and_revoking_takes_it_away() {
    let (port, state, rec) = start_paired().await;

    // no offer yet: nothing to pair with
    let r = post_json(port, "/pair", r#"{"code":"123456","name":"iPhone"}"#, "").await;
    assert!(r.starts_with("HTTP/1.1 401"), "{r}");

    let offer = state.pairing.new_offer();
    let r = post_json(port, "/pair", &format!(r#"{{"code":"{}","name":"iPhone"}}"#, offer.code), "").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let lower = r.to_lowercase();
    assert!(lower.contains("httponly") && lower.contains("samesite=strict"), "the key cookie must be HttpOnly and SameSite=Strict: {r}");
    let cookie = cookie_of(&r);

    // the offer worked once only
    let again = post_json(port, "/pair", &format!(r#"{{"code":"{}"}}"#, offer.code), "").await;
    assert!(again.starts_with("HTTP/1.1 401"));

    // the paired phone can use the socket and the protected routes
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    req.headers_mut().insert("Cookie", cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws.next().await.unwrap().unwrap(); // hello
    ws.send(Message::Text(r#"{"t":"move","dx":2,"dy":3}"#.into())).await.unwrap();
    assert_eq!(wait_for(&rec, 1).await, ["move 2 3"]);
    let hdr = format!("Cookie: {cookie}\r\n");
    assert!(get_with(port, &format!("/icon/{ICON_KEY}.png"), &hdr).await.starts_with("HTTP/1.1 200"));
    let st = get_with(port, "/pair/status", &hdr).await;
    assert!(st.contains(r#""paired":true"#) && st.contains(r#""device":true"#), "{st}");

    // a made-up key gets nothing
    let fake = format!("openremote_key={}", "ab".repeat(32));
    assert!(ws_refusal(port, &fake).await.expect("refused").contains("401"));

    // revoke from the PC's admin page: access is gone at once
    let id = state.pairing.devices()[0].id.clone();
    let body = format!("id={id}");
    let rev = raw_request(
        port,
        &format!("POST /admin/revoke HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()),
    )
    .await;
    assert!(rev.starts_with("HTTP/1.1 303"), "{rev}");
    assert!(ws_refusal(port, &cookie).await.expect("refused").contains("401"));
}

#[tokio::test]
async fn the_qr_secret_pairs_and_a_phone_can_unpair_itself() {
    let (port, state, _) = start_paired().await;
    let offer = state.pairing.new_offer();
    let r = post_json(port, "/pair", &format!(r#"{{"secret":"{}","name":"Pixel"}}"#, offer.secret), "").await;
    assert!(r.starts_with("HTTP/1.1 200"), "{r}");
    let cookie = cookie_of(&r);
    assert_eq!(state.pairing.devices().len(), 1);
    let forget = post_json(port, "/pair/forget", "{}", &format!("Cookie: {cookie}\r\n")).await;
    assert!(forget.starts_with("HTTP/1.1 200"), "{forget}");
    assert!(state.pairing.devices().is_empty());
    assert!(ws_refusal(port, &cookie).await.expect("refused").contains("401"));
}

#[tokio::test]
async fn guessing_codes_locks_pairing() {
    let (port, state, _) = start_paired().await;
    let offer = state.pairing.new_offer();
    let wrong = if offer.code == "000000" { "111111" } else { "000000" };
    for _ in 0..2 {
        let r = post_json(port, "/pair", &format!(r#"{{"code":"{wrong}"}}"#), "").await;
        assert!(r.starts_with("HTTP/1.1 401"), "{r}");
    }
    let r = post_json(port, "/pair", &format!(r#"{{"code":"{wrong}"}}"#), "").await;
    assert!(r.starts_with("HTTP/1.1 429"), "{r}");
    // the real code no longer works either: the offer was burned and pairing is paused
    let r = post_json(port, "/pair", &format!(r#"{{"code":"{}"}}"#, offer.code), "").await;
    assert!(r.starts_with("HTTP/1.1 429"), "{r}");
    assert!(state.pairing.devices().is_empty());
}

#[tokio::test]
async fn other_websites_cannot_pair() {
    let (port, state, _) = start_paired().await;
    let offer = state.pairing.new_offer();
    let r = post_json(port, "/pair", &format!(r#"{{"code":"{}"}}"#, offer.code), "Origin: http://evil.example\r\n").await;
    assert!(r.starts_with("HTTP/1.1 403"), "{r}");
    assert!(state.pairing.devices().is_empty());
}

#[tokio::test]
async fn admin_shows_the_offer_and_escapes_device_names() {
    let (port, state, _) = start_paired().await;
    let offer = state.pairing.new_offer();
    let r = post_json(port, "/pair", &format!(r#"{{"code":"{}","name":"<script>alert(1)</script>"}}"#, offer.code), "").await;
    assert!(r.starts_with("HTTP/1.1 200"));
    let admin = http_get(port, "/admin").await;
    assert!(admin.starts_with("HTTP/1.1 200"));
    assert!(admin.contains("<svg"), "the QR code is on the page");
    assert!(admin.contains("Pair a phone") && admin.contains("Paired phones"));
    assert!(!admin.contains("<script>alert(1)</script>") && admin.contains("&lt;script&gt;"), "device names must be escaped");
}

#[tokio::test]
async fn revoking_closes_an_open_connection() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let (port, state, _) = start_paired().await;
    let offer = state.pairing.new_offer();
    let r = post_json(port, "/pair", &format!(r#"{{"code":"{}","name":"iPhone"}}"#, offer.code), "").await;
    let cookie = cookie_of(&r);
    let mut req = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    req.headers_mut().insert("Cookie", cookie.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    ws.next().await.unwrap().unwrap(); // hello

    let id = state.pairing.devices()[0].id.clone();
    assert!(state.pairing.revoke(&id));
    let closed = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                _ => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "a revoked phone must lose its open connection, not only new ones");
}
