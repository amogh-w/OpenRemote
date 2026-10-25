//! HTTP + WebSocket front end: serves the web app and the control socket. Pairing routes and the loopback-only
//! dashboard come from `pairroutes.rs` and `admin.rs` (shared with Kai Remote).

use crate::command::{Dispatcher, Injector, Msg};
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::serve::ListenerExt;
use axum::Json;
use axum::Router;
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
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
    /// The port this server listens on (shown on the dashboard and in phone links).
    pub port: u16,
}

pub fn router<I: Injector + 'static>(state: Arc<AppState<I>>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler::<I>))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon/{file}", get(app_icon::<I>))
        .route("/media/art", get(media_art::<I>))
        .route("/clip/image.png", get(clip_image_get::<I>))
        .route("/clip/image", post(clip_image_post::<I>).layer(DefaultBodyLimit::max(crate::image::MAX_PNG_BYTES)))
        .route("/sw.js", get(service_worker))
        .route("/icon-180.png", get(|| async { png(ICON_180) }))
        .route("/icon-192.png", get(|| async { png(ICON_192) }))
        .route("/icon-512.png", get(|| async { png(ICON_512) }))
        .merge(crate::pairroutes::routes::<AppState<I>>())
        .merge(crate::admin::routes::<AppState<I>>())
        .layer(middleware::from_fn_with_state(state.clone(), guard::<I>))
        .with_state(state)
}

pub use crate::netguard::{host_allowed, origin_matches};

/// Is the sender allowed to control this PC? The PC itself always is, and so is everybody when pairing is off.
fn authorized<I: Injector>(st: &AppState<I>, addr: Option<&SocketAddr>, headers: &HeaderMap) -> bool {
    if !st.pairing.required() || (st.pairing.trusts_loopback() && addr.is_some_and(|a| a.ip().is_loopback())) {
        return true;
    }
    crate::pairroutes::device_id(&st.pairing, headers).is_some()
}

impl<I: Injector + 'static> crate::pairroutes::PairHost for AppState<I> {
    fn pairing(&self) -> &crate::pairing::Pairing {
        &self.pairing
    }
    fn is_authorized(&self, addr: Option<&SocketAddr>, headers: &HeaderMap) -> bool {
        authorized(self, addr, headers)
    }
}

impl<I: Injector + 'static> crate::admin::AdminHost for AppState<I> {
    fn name(&self) -> &str {
        crate::APP_NAME
    }
    fn hostname(&self) -> String {
        self.hostname.clone()
    }
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
    fn port(&self) -> u16 {
        self.port
    }
    fn log_dir(&self) -> std::path::PathBuf {
        crate::settings::path().parent().map(|p| p.to_path_buf()).unwrap_or_default()
    }
    fn lan_urls(&self) -> Vec<String> {
        lan_urls(self.port)
    }
    fn asset(&self, file: &str) -> Option<&'static str> {
        Some(match file {
            "admin.html" => include_str!("../assets/ui/admin.html"),
            "admin.js" => include_str!("../assets/ui/admin.js"),
            "app.css" => include_str!("../assets/ui/app.css"),
            "ui.js" => include_str!("../assets/ui/kit/ui.js"),
            "theme.js" => include_str!("../assets/ui/kit/theme.js"),
            "icons" => include_str!("../assets/ui/icons.html"),
            _ => return None,
        })
    }
    fn restart(&self) {
        crate::shell::restart()
    }
}

/// Pages and routes a phone must be able to reach before it is paired (the page shows the pairing screen).
fn is_public(path: &str) -> bool {
    matches!(path, "/" | "/manifest.webmanifest" | "/sw.js" | "/icon-180.png" | "/icon-192.png" | "/icon-512.png")
        || crate::pairroutes::PUBLIC_PATHS.contains(&path)
        || crate::admin::is_admin_path(path) // the dashboard checks for loopback itself
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
        Ok(Some(bytes)) => {
            ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], bytes).into_response()
        }
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
        return (StatusCode::UNSUPPORTED_MEDIA_TYPE, Json(serde_json::json!({"ok": false, "error": "send image/png"})))
            .into_response();
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
async fn app_icon<I: Injector + 'static>(State(st): State<Arc<AppState<I>>>, Path(file): Path<String>) -> Response {
    let Some(key) = file.strip_suffix(".png").map(str::to_owned) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let st2 = st.clone();
    // Icon extraction is blocking GDI work; keep it off the async workers.
    match tokio::task::spawn_blocking(move || st2.dispatcher.icon_png(&key)).await {
        Ok(Some(bytes)) => {
            ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=3600")], bytes)
                .into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

fn png(bytes: &'static [u8]) -> Response {
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], bytes).into_response()
}

/// The service worker with its cache name tied to the page it caches, so a UI change retires the old cache by itself.
fn service_worker_source() -> &'static str {
    static SRC: OnceLock<String> = OnceLock::new();
    SRC.get_or_init(|| {
        let build = format!("{:x}", Sha256::digest(INDEX_HTML.as_bytes()));
        SW_JS.replace("__BUILD__", &build[..12])
    })
}

async fn service_worker() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::HeaderName::from_static("service-worker-allowed"), "/"),
        ],
        service_worker_source(),
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
    ([(header::CONTENT_TYPE, "application/manifest+json"), (header::CACHE_CONTROL, "no-store")], body.to_string())
        .into_response()
}

async fn ws_handler<I: Injector + 'static>(
    State(st): State<Arc<AppState<I>>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
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

/// How often the server pings an idle phone, and how long without any frame (a pong counts) ends the connection.
const PING_EVERY: Duration = Duration::from_secs(15);
const IDLE_LIMIT: Duration = Duration::from_secs(45);

async fn socket_loop<I: Injector + 'static>(mut socket: WebSocket, st: Arc<AppState<I>>, key: Option<String>) {
    let helper = st.dispatcher.helper_active();
    let hello =
        serde_json::json!({"t": "hello", "host": st.hostname, "v": env!("CARGO_PKG_VERSION"), "helper": helper});
    if socket.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    let mut events = st.dispatcher.subscribe();
    let mut recheck = tokio::time::interval(std::time::Duration::from_secs(2));
    let (mut last_rx, mut last_ping) = (Instant::now(), Instant::now());
    loop {
        let frame = tokio::select! {
            _ = recheck.tick() => {
                // a revoked phone loses its open connection too, not just new ones
                if st.pairing.required() && key.as_deref().is_some_and(|k| st.pairing.check(k).is_none()) {
                    break;
                }
                // A phone that slept or left the Wi-Fi never says goodbye. Browsers answer protocol pings by
                // themselves, so silence for IDLE_LIMIT means the link is dead and its slot can be freed.
                if last_rx.elapsed() > IDLE_LIMIT {
                    break;
                }
                if last_ping.elapsed() >= PING_EVERY {
                    last_ping = Instant::now();
                    if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                        break;
                    }
                }
                continue;
            }
            ev = events.recv() => {
                // A lagging phone just misses some updates; it asks again when it needs the state.
                if let Ok(text) = ev {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                continue;
            }
            f = socket.recv() => match f {
                Some(Ok(frame)) => frame,
                _ => break,
            },
        };
        last_rx = Instant::now();
        // Anything but text (Close included) is skipped: recv() finishes the handshake and then yields None.
        if let Message::Text(text) = frame {
            match serde_json::from_str::<Msg>(&text) {
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
                        crate::applog::warn(&format!("ignoring bad ws message {text:?}: {e}"));
                    }
                }
            }
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
