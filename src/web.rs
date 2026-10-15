//! HTTP + WebSocket front end: serves the web app, the control socket and the loopback-only admin page.

use crate::command::{Dispatcher, Injector, Msg};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::net::TcpListener;

const INDEX_HTML: &str = include_str!("../assets/index.html");
const SW_JS: &str = include_str!("../assets/sw.js");
const ICON_180: &[u8] = include_bytes!("../assets/icon-180.png");
const ICON_192: &[u8] = include_bytes!("../assets/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../assets/icon-512.png");
const MAX_WS_MESSAGE: usize = 512 * 1024;

pub struct AppState<I: Injector> {
    pub dispatcher: Dispatcher<I>,
    pub token: String,
    /// When false the WebSocket accepts connections without a token.
    pub require_token: bool,
    pub hostname: String,
    pub verbose: bool,
}

pub fn router<I: Injector + 'static>(state: Arc<AppState<I>>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler::<I>))
        .route("/admin", get(admin::<I>))
        .route("/manifest.webmanifest", get(manifest::<I>))
        .route("/icon/{file}", get(app_icon::<I>))
        .route("/sw.js", get(service_worker))
        .route("/icon-180.png", get(|| async { png(ICON_180) }))
        .route("/icon-192.png", get(|| async { png(ICON_192) }))
        .route("/icon-512.png", get(|| async { png(ICON_512) }))
        .with_state(state)
}

pub async fn serve<I: Injector + 'static>(listener: TcpListener, state: Arc<AppState<I>>) -> std::io::Result<()> {
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>()).await
}

/// Constant-time comparison so token checks don't leak a matching prefix.
pub fn token_ok(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

/// App/window icon by key (`/icon/<16 hex>.png`). Keys are issued by the server in window and app
/// lists, so this can only return icons of things the server itself found.
async fn app_icon<I: Injector + 'static>(
    State(st): State<Arc<AppState<I>>>,
    Path(file): Path<String>,
    Query(q): Query<WsQuery>,
) -> Response {
    if st.require_token && !token_ok(&st.token, q.t.as_deref().unwrap_or("")) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
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

/// Web app manifest. `start_url` carries the pairing token only when the request proves it already
/// knows the token, so an unauthenticated LAN client cannot read it from here. iOS home-screen apps
/// have their own storage, so they need the token in `start_url` to pair on first launch.
async fn manifest<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>, Query(q): Query<WsQuery>) -> Response {
    let start_url = match q.t.as_deref() {
        Some(t) if token_ok(&st.token, t) => format!("/?t={}", st.token),
        _ => "/".to_string(),
    };
    let body = serde_json::json!({
        "id": "/",
        "name": "OpenRemote",
        "short_name": "OpenRemote",
        "description": "Use your phone as a touchpad, keyboard and remote for your PC.",
        "start_url": start_url,
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

#[derive(Deserialize)]
struct WsQuery {
    t: Option<String>,
}

async fn ws_handler<I: Injector + 'static>(
    State(st): State<Arc<AppState<I>>>,
    Query(q): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if st.require_token && !token_ok(&st.token, q.t.as_deref().unwrap_or("")) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
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

/// Reachable URLs for phones on the LAN, each embedding the pairing token.
/// The token is appended only when pairing is required (`token` is `Some`).
pub fn lan_urls(port: u16, token: Option<&str>) -> Vec<String> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.addr.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => Some(match token {
                Some(t) => format!("http://{ip}:{port}/?t={t}"),
                None => format!("http://{ip}:{port}/"),
            }),
            _ => None,
        })
        .collect()
}

fn qr_svg(url: &str) -> String {
    qrcode::QrCode::new(url.as_bytes())
        .map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).build())
        .unwrap_or_default()
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
    let urls = lan_urls(port, st.require_token.then_some(st.token.as_str()));
    let mut cards = String::new();
    for u in &urls {
        cards.push_str(&format!("<div class=card>{}<p><a href=\"{u}\">{u}</a></p></div>", qr_svg(u)));
    }
    if urls.is_empty() {
        cards.push_str("<p>No LAN address found. Connect this PC to a network.</p>");
    }
    Html(format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>\
<title>OpenRemote admin</title>\
<style>body{{font:16px system-ui;background:#111;color:#eee;max-width:720px;margin:2rem auto;padding:0 1rem}}\
.card{{background:#1c1c1e;border-radius:12px;padding:1rem;margin:1rem 0;text-align:center}}\
svg{{background:#fff;padding:8px;border-radius:8px}}a{{color:#6cf;word-break:break-all}}</style>\
<h1>OpenRemote</h1><p>Host: <b>{}</b>. Scan a code with your phone (same Wi-Fi). {}</p>{cards}",
        st.hostname,
        if st.require_token {
            "Anyone with the link can control this PC; plain HTTP is not encrypted."
        } else {
            "Pairing is OFF: anyone on your network can control this PC (start with --require-token to change that)."
        }
    ))
    .into_response()
}

fn addr_port(headers: &axum::http::HeaderMap) -> Option<u16> {
    headers.get("host")?.to_str().ok()?.rsplit(':').next()?.parse().ok()
}
