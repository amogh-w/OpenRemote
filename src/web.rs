//! HTTP + WebSocket front end: serves the web app, the control socket and the loopback-only admin page.

use crate::command::{Dispatcher, Injector, Msg};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
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
}

pub fn router<I: Injector + 'static>(state: Arc<AppState<I>>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler::<I>))
        .route("/admin", get(admin::<I>))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon/{file}", get(app_icon::<I>))
        .route("/clip/image.png", get(clip_image_get::<I>))
        .route(
            "/clip/image",
            post(clip_image_post::<I>).layer(DefaultBodyLimit::max(crate::image::MAX_PNG_BYTES)),
        )
        .route("/sw.js", get(service_worker))
        .route("/icon-180.png", get(|| async { png(ICON_180) }))
        .route("/icon-192.png", get(|| async { png(ICON_192) }))
        .route("/icon-512.png", get(|| async { png(ICON_512) }))
        .with_state(state)
}

pub async fn serve<I: Injector + 'static>(listener: TcpListener, state: Arc<AppState<I>>) -> std::io::Result<()> {
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>()).await
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
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

async fn ws_handler<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_WS_MESSAGE).on_upgrade(move |s| socket_loop(s, st))
}

async fn socket_loop<I: Injector>(mut socket: WebSocket, st: Arc<AppState<I>>) {
    let hello = serde_json::json!({"t": "hello", "host": st.hostname, "v": env!("CARGO_PKG_VERSION")});
    if socket.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    while let Some(Ok(frame)) = socket.recv().await {
        match frame {
            Message::Text(text) => match serde_json::from_str::<Msg>(&text) {
                Ok(Msg::Ping) => {
                    if socket.send(Message::Text(r#"{"t":"pong"}"#.into())).await.is_err() {
                        break;
                    }
                }
                Ok(msg) => {
                    if let Some(reply) = st.dispatcher.handle(&msg) {
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

async fn admin<I: Injector + 'static>(
    State(st): State<Arc<AppState<I>>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Response {
    // Loopback only, and refuse requests whose Host isn't loopback (DNS rebinding).
    let host_ok = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.starts_with("127.0.0.1") || h.starts_with("localhost") || h.starts_with("[::1]"));
    if !addr.ip().is_loopback() || !host_ok {
        return StatusCode::FORBIDDEN.into_response();
    }
    let port = addr_port(&headers).unwrap_or(0);
    let urls = lan_urls(port);
    let mut cards = String::new();
    for u in &urls {
        cards.push_str(&format!("<div class=card><a href=\"{u}\">{u}</a></div>"));
    }
    if urls.is_empty() {
        cards.push_str("<p>No LAN address found. Connect this PC to a network.</p>");
    }
    Html(format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>\
<title>OpenRemote admin</title>\
<style>body{{font:16px system-ui;background:#111;color:#eee;max-width:720px;margin:2rem auto;padding:0 1rem}}\
.card{{background:#1c1c1e;border-radius:12px;padding:1rem;margin:1rem 0;text-align:center;font-size:1.2rem}}\
a{{color:#6cf;word-break:break-all}}</style>\
<h1>OpenRemote</h1><p>Host: <b>{}</b>. Open one of these addresses in your phone's browser (same Wi-Fi).</p>{cards}\
<p>There is no pairing yet: anyone on your network can control this PC, and plain HTTP is not encrypted.</p>",
        st.hostname
    ))
    .into_response()
}

fn addr_port(headers: &axum::http::HeaderMap) -> Option<u16> {
    headers.get("host")?.to_str().ok()?.rsplit(':').next()?.parse().ok()
}
