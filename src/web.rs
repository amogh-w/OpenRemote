//! HTTP + WebSocket front end: serves the web app, the control socket and the loopback-only admin page.

use crate::command::{Dispatcher, Injector, Msg};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Form, Path, Request, State};
use axum::middleware::{self, Next};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::serve::ListenerExt;
use axum::Json;
use axum::Router;
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;

const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/index.html"));
const SW_JS: &str = include_str!("../assets/sw.js");
const ICON_180: &[u8] = include_bytes!("../assets/icon-180.png");
const ICON_192: &[u8] = include_bytes!("../assets/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../assets/icon-512.png");
const MAX_WS_MESSAGE: usize = 512 * 1024;

pub struct AppState<I: Injector> {
    pub dispatcher: Dispatcher<I>,
    pub hostname: String,
    pub verbose: bool,
    pub pairing: crate::pairing::Pairing,
}

pub fn router<I: Injector + 'static>(state: Arc<AppState<I>>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler::<I>))
        .route("/admin", get(admin::<I>))
        .route("/admin/revoke", post(admin_revoke::<I>))
        .route("/pair", post(pair_post::<I>))
        .route("/pair/status", get(pair_status::<I>))
        .route("/pair/forget", post(pair_forget::<I>))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon/{file}", get(app_icon::<I>))
        .route("/media/art", get(media_art::<I>))
        .route("/clip/image.png", get(clip_image_get::<I>))
        .route(
            "/clip/image",
            post(clip_image_post::<I>).layer(DefaultBodyLimit::max(crate::image::MAX_PNG_BYTES)),
        )
        .route("/sw.js", get(service_worker))
        .route("/icon-180.png", get(|| async { png(ICON_180) }))
        .route("/icon-192.png", get(|| async { png(ICON_192) }))
        .route("/icon-512.png", get(|| async { png(ICON_512) }))
        .layer(middleware::from_fn_with_state(state.clone(), guard::<I>))
        .with_state(state)
}

/// The address the client used must be one a person would type: an IP address, `localhost`, this PC's name
/// or a `.local` name. That stops DNS rebinding (a web page re-pointing its own name at this PC).
pub fn host_allowed(host: &str, machine: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => match host.rsplit_once(':') {
            Some((n, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => n,
            _ => host,
        },
    };
    if name.is_empty() {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    name.parse::<IpAddr>().is_ok()
        || lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || (!machine.is_empty() && lower == machine.to_ascii_lowercase())
}

/// A request that carries an `Origin` (every browser WebSocket and every cross-site fetch does) must come from
/// the page this server itself serves: the origin's host and port must equal the `Host` header.
pub fn origin_matches(origin: &str, host: &str) -> bool {
    let rest = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://"));
    rest.is_some_and(|authority| authority.eq_ignore_ascii_case(host))
}

/// Is the sender allowed to control this PC? The PC itself always is, and so is everybody when pairing is off.
fn authorized<I: Injector>(st: &AppState<I>, addr: Option<&SocketAddr>, headers: &HeaderMap) -> bool {
    if !st.pairing.required() || (st.pairing.trusts_loopback() && addr.is_some_and(|a| a.ip().is_loopback())) {
        return true;
    }
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::pairing::key_from_cookie)
        .is_some_and(|k| st.pairing.check(k).is_some())
}

/// Pages and routes a phone must be able to reach before it is paired (the page shows the pairing screen).
fn is_public(path: &str) -> bool {
    matches!(
        path,
        "/" | "/pair" | "/pair/status" | "/pair/forget" | "/manifest.webmanifest" | "/sw.js" | "/icon-180.png" | "/icon-192.png" | "/icon-512.png"
    ) || path == "/admin"
        || path == "/admin/revoke" // these two check for loopback themselves
}

/// Applied to every route. Without it, any web page open in a browser on the same network could open the
/// control socket (or read the clipboard) just by guessing this PC's address. After the address and origin
/// checks, everything except the public pages needs a paired phone (when pairing is on).
async fn guard<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    if !host_allowed(host, &st.hostname) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        if !origin.to_str().is_ok_and(|o| origin_matches(o, host)) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    if !is_public(req.uri().path()) {
        let addr = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
        if !authorized(&st, addr.as_ref(), req.headers()) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    next.run(req).await
}

#[derive(Deserialize)]
struct PairReq {
    #[serde(default)]
    code: String,
    #[serde(default)]
    secret: String,
    #[serde(default)]
    name: String,
}

fn key_cookie(key: &str, max_age: u64) -> String {
    format!("{}={key}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Strict", crate::pairing::COOKIE)
}

/// A phone proves it has the offer shown on the PC and gets its key as a cookie.
async fn pair_post<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>, Json(req): Json<PairReq>) -> Response {
    use crate::pairing::PairError;
    let proof = if req.secret.is_empty() { req.code } else { req.secret };
    match st.pairing.pair(&proof, &req.name) {
        Ok(key) => (
            [(header::SET_COOKIE, key_cookie(&key, 10 * 365 * 24 * 3600)), (header::CACHE_CONTROL, "no-store".to_string())],
            Json(serde_json::json!({"ok": true})),
        )
            .into_response(),
        Err(PairError::Locked(secs)) => (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({"ok": false, "error": "locked", "retry": secs}))).into_response(),
        Err(PairError::Invalid) => (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"ok": false, "error": "invalid"}))).into_response(),
    }
}

/// What the page needs to decide between the pairing screen and the app.
async fn pair_status<I: Injector + 'static>(
    State(st): State<Arc<AppState<I>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let device = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::pairing::key_from_cookie)
        .is_some_and(|k| st.pairing.check(k).is_some());
    let paired = authorized(&st, Some(&addr), &headers);
    ([(header::CACHE_CONTROL, "no-store")], Json(serde_json::json!({"required": st.pairing.required(), "paired": paired, "device": device}))).into_response()
}

/// A phone removes itself (Settings > Unpair this phone).
async fn pair_forget<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>, headers: HeaderMap) -> Response {
    let key = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()).and_then(crate::pairing::key_from_cookie);
    if let Some(id) = key.and_then(|k| st.pairing.check(k)) {
        st.pairing.revoke(&id);
    }
    ([(header::SET_COOKIE, key_cookie("", 0))], Json(serde_json::json!({"ok": true}))).into_response()
}

pub async fn serve<I: Injector + 'static>(listener: TcpListener, state: Arc<AppState<I>>) -> std::io::Result<()> {
    // Small frames (pongs, replies) must leave at once: Nagle plus a phone's delayed ACK can hold them for ~40 ms.
    let listener = listener.tap_io(|tcp| {
        let _ = tcp.set_nodelay(true);
    });
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>()).await
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// Cover image of the track that is playing (PNG or JPEG, whatever the player supplied).
async fn media_art<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>) -> Response {
    let st2 = st.clone();
    match tokio::task::spawn_blocking(move || st2.dispatcher.media_art()).await {
        Ok(Some(bytes)) => {
            let ctype = if bytes.starts_with(&[0xFF, 0xD8]) { "image/jpeg" } else { "image/png" };
            ([(header::CONTENT_TYPE, ctype), (header::CACHE_CONTROL, "no-store")], bytes).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The image currently on the PC clipboard, as PNG. Read live, so it is always current.
async fn clip_image_get<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>) -> Response {
    let st2 = st.clone();
    match tokio::task::spawn_blocking(move || st2.dispatcher.clip_image_png()).await {
        Ok(Some(bytes)) => ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], bytes).into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Put an uploaded PNG on the PC clipboard. Only `image/png` bodies are accepted: that content type
/// cannot be sent cross-site without a CORS preflight, which this server never allows.
async fn clip_image_post<I: Injector + 'static>(
    State(st): State<Arc<AppState<I>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let is_png = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("image/png"));
    if !is_png {
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, Json(serde_json::json!({"ok": false, "error": "send image/png"}))).into_response();
    }
    let st2 = st.clone();
    match tokio::task::spawn_blocking(move || st2.dispatcher.clip_image_set(&body)).await {
        Ok(Ok((w, h))) => Json(serde_json::json!({"ok": true, "w": w, "h": h})).into_response(),
        Ok(Err(msg)) => {
            let status = match msg {
                "image too large" => StatusCode::PAYLOAD_TOO_LARGE,
                "could not write the clipboard" => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_REQUEST,
            };
            (status, Json(serde_json::json!({"ok": false, "error": msg}))).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// App/window icon by key (`/icon/<16 hex>.png`). Keys are issued by the server in window and app
/// lists, so this can only return icons of things the server itself found.
async fn app_icon<I: Injector + 'static>(
    State(st): State<Arc<AppState<I>>>,
    Path(file): Path<String>,
) -> Response {
    let Some(key) = file.strip_suffix(".png").map(str::to_owned) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let st2 = st.clone();
    // Icon extraction is blocking GDI work; keep it off the async workers.
    match tokio::task::spawn_blocking(move || st2.dispatcher.icon_png(&key)).await {
        Ok(Some(bytes)) => (
            [(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=3600")],
            bytes,
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn png(bytes: &'static [u8]) -> Response {
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], bytes).into_response()
}

async fn service_worker() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::HeaderName::from_static("service-worker-allowed"), "/"),
        ],
        SW_JS,
    )
        .into_response()
}

/// Web app manifest, so the page can be installed to the home screen.
async fn manifest() -> Response {
    let body = serde_json::json!({
        "id": "/",
        "name": "OpenRemote",
        "short_name": "OpenRemote",
        "description": "Use your phone as a touchpad, keyboard and remote for your PC.",
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "orientation": "any",
        "background_color": "#1e1e2e", // Catppuccin Mocha base
        "theme_color": "#1e1e2e",
        "icons": [
            {"src": "/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any maskable"},
            {"src": "/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any maskable"}
        ]
    });
    (
        [(header::CONTENT_TYPE, "application/manifest+json"), (header::CACHE_CONTROL, "no-store")],
        body.to_string(),
    )
        .into_response()
}

async fn ws_handler<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    // remembered so the socket can be closed the moment this phone is revoked
    let key = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(crate::pairing::key_from_cookie)
        .map(str::to_owned);
    ws.max_message_size(MAX_WS_MESSAGE).on_upgrade(move |s| socket_loop(s, st, key))
}

/// Handle one message. The media query talks to a Windows service and can take a moment, so it runs on a
/// blocking thread instead of an async worker.
async fn reply_for<I: Injector + 'static>(st: &Arc<AppState<I>>, msg: Msg) -> Option<serde_json::Value> {
    if matches!(msg, Msg::MediaGet) {
        let st2 = st.clone();
        return tokio::task::spawn_blocking(move || st2.dispatcher.handle(&msg)).await.ok().flatten();
    }
    st.dispatcher.handle(&msg)
}

async fn socket_loop<I: Injector + 'static>(mut socket: WebSocket, st: Arc<AppState<I>>, key: Option<String>) {
    let helper = st.dispatcher.helper_active();
    let hello = serde_json::json!({"t": "hello", "host": st.hostname, "v": env!("CARGO_PKG_VERSION"), "helper": helper});
    if socket.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    let mut recheck = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        let frame = tokio::select! {
            _ = recheck.tick() => {
                // a revoked phone loses its open connection too, not just new ones
                if st.pairing.required() && key.as_deref().is_some_and(|k| st.pairing.check(k).is_none()) {
                    break;
                }
                continue;
            }
            f = socket.recv() => match f {
                Some(Ok(frame)) => frame,
                _ => break,
            },
        };
        match frame {
            Message::Text(text) => match serde_json::from_str::<Msg>(&text) {
                Ok(Msg::Ping) => {
                    if socket.send(Message::Text(r#"{"t":"pong"}"#.into())).await.is_err() {
                        break;
                    }
                }
                Ok(msg) => {
                    if let Some(reply) = reply_for(&st, msg).await {
                        if socket.send(Message::Text(reply.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    if st.verbose {
                        eprintln!("[ws] ignoring bad message {text:?}: {e}");
                    }
                }
            },
            _ => {} // Close: recv() finishes the handshake and then yields None
        }
    }
}

/// Reachable URLs for phones on the LAN.
pub fn lan_urls(port: u16) -> Vec<String> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.addr.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => Some(format!("http://{ip}:{port}/")),
            _ => None,
        })
        .collect()
}

fn qr_svg(url: &str) -> String {
    qrcode::QrCode::new(url.as_bytes())
        .map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).build())
        .unwrap_or_default()
}

fn esc(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// "5 min ago" style text for the device list.
fn ago(secs: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let d = now.saturating_sub(secs);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86400),
    }
}

/// Loopback only, and only with a loopback `Host` (DNS rebinding).
fn is_admin_request(addr: &SocketAddr, headers: &HeaderMap) -> bool {
    let host_ok = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.starts_with("127.0.0.1") || h.starts_with("localhost") || h.starts_with("[::1]"));
    addr.ip().is_loopback() && host_ok
}

async fn admin<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>, ConnectInfo(addr): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    if !is_admin_request(&addr, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let port = addr_port(&headers).unwrap_or(0);
    let urls = lan_urls(port);
    let mut body = String::new();
    if st.pairing.required() {
        let offer = st.pairing.new_offer();
        body.push_str(&format!(
            "<h2>Pair a phone</h2><p>On the phone (same Wi-Fi) scan the QR code, or open the address below and type the code. \
This offer works once and expires in {} minutes; this page refreshes it.</p><div class=code>{} {}</div>",
            offer.ttl_secs / 60,
            &offer.code[..3],
            &offer.code[3..]
        ));
        for u in &urls {
            let link = format!("{u}?pair={}", offer.secret);
            body.push_str(&format!("<div class=card>{}<p>Or open <b>{}</b> and type the code</p></div>", qr_svg(&link), esc(u)));
        }
        if urls.is_empty() {
            body.push_str("<p>No LAN address found. Connect this PC to a network.</p>");
        }
        body.push_str("<h2>Paired phones</h2>");
        let devices = st.pairing.devices();
        if devices.is_empty() {
            body.push_str("<p>None yet.</p>");
        }
        for d in devices {
            body.push_str(&format!(
                "<form class=dev method=post action=/admin/revoke><span><b>{}</b><br><small>last seen {}</small></span>\
<input type=hidden name=id value=\"{}\"><button>Revoke</button></form>",
                esc(&d.name),
                ago(d.last_seen),
                esc(&d.id)
            ));
        }
    } else {
        body.push_str("<h2>Pairing is off</h2><p>Anyone on your network can control this PC. Start with <code>--pairing</code> to require phones to pair.</p>");
        for u in &urls {
            body.push_str(&format!("<div class=card><a href=\"{u}\">{u}</a></div>"));
        }
    }
    (
        [(header::CACHE_CONTROL, "no-store")],
        Html(format!(
            "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>\
<meta http-equiv=refresh content=100><title>OpenRemote admin</title>\
<style>body{{font:16px system-ui;background:#111;color:#eee;max-width:720px;margin:2rem auto;padding:0 1rem}}\
.card{{background:#1c1c1e;border-radius:12px;padding:1rem;margin:1rem 0;text-align:center}}\
svg{{background:#fff;padding:8px;border-radius:8px}}a{{color:#6cf;word-break:break-all}}\
.code{{font:700 2.6rem ui-monospace,Consolas,monospace;letter-spacing:.15em;text-align:center;background:#1c1c1e;border-radius:12px;padding:.8rem;margin:1rem 0}}\
.dev{{display:flex;justify-content:space-between;align-items:center;background:#1c1c1e;border-radius:12px;padding:.7rem 1rem;margin:.6rem 0}}\
small{{color:#999}}button{{font:inherit;background:#b3261e;color:#fff;border:0;border-radius:8px;padding:.4rem .9rem}}code{{background:#222;padding:0 .3rem;border-radius:4px}}</style>\
<h1>OpenRemote</h1><p>Host: <b>{}</b></p>{body}\
<p style=color:#999>Plain HTTP is not encrypted: pair only on a network you trust.</p>",
            esc(&st.hostname)
        )),
    )
        .into_response()
}

#[derive(Deserialize)]
struct RevokeForm {
    id: String,
}

async fn admin_revoke<I: Injector + 'static>(
    State(st): State<Arc<AppState<I>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(f): Form<RevokeForm>,
) -> Response {
    if !is_admin_request(&addr, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    st.pairing.revoke(&f.id);
    Redirect::to("/admin").into_response()
}

fn addr_port(headers: &axum::http::HeaderMap) -> Option<u16> {
    headers.get("host")?.to_str().ok()?.rsplit(':').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_people_type_are_allowed() {
        for h in ["192.168.1.20:1990", "127.0.0.1:1990", "[::1]:1990", "localhost:1990", "localhost", "my-pc:1990", "MY-PC:1990", "laptop.local:1990", "10.0.0.5"] {
            assert!(host_allowed(h, "My-PC"), "{h}");
        }
    }

    #[test]
    fn rebinding_style_hosts_are_refused() {
        for h in ["", "evil.example:1990", "evil.example", "my-pc.evil.example:1990", ":1990", "attacker.com:80"] {
            assert!(!host_allowed(h, "My-PC"), "{h}");
        }
    }

    #[test]
    fn origin_must_equal_the_host() {
        assert!(origin_matches("http://192.168.1.20:1990", "192.168.1.20:1990"));
        assert!(origin_matches("http://MY-PC:1990", "my-pc:1990"));
        assert!(!origin_matches("http://evil.example", "192.168.1.20:1990"));
        assert!(!origin_matches("http://192.168.1.20:8080", "192.168.1.20:1990"));
        assert!(!origin_matches("null", "192.168.1.20:1990"));
        assert!(!origin_matches("", "192.168.1.20:1990"));
        assert!(!origin_matches("chrome-extension://abc", "192.168.1.20:1990"));
    }
}
