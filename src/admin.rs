//! The admin dashboard: a page and a small JSON API for the person at this PC. Shared by OpenRemote and Kai Remote:
//! edit it in the cuppa plugin repo (shared/) and run `harness.mjs sync --write`, never in a project copy.
//!
//! Everything here answers only the PC itself: the peer address and the `Host` header must both be loopback (the
//! second check stops DNS rebinding), and every change must come from the dashboard's own page: a matching
//! `Origin` and an `X-Admin` header, which a web page on another site cannot send without a CORS preflight.
//! The page (`admin.html`, `admin.js`) is part of each app's web assets; the app hands it over through `AdminHost`.

use crate::pairing::Pairing;
use crate::pairroutes::PairHost;
use axum::{
    extract::{ConnectInfo, Path, State},
    http::{header, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

/// What the dashboard and the tray need from an app.
pub trait AdminHost: PairHost {
    /// Product name, for the page and the tray ("OpenRemote").
    fn name(&self) -> &str;
    fn hostname(&self) -> String;
    fn version(&self) -> &str;
    fn port(&self) -> u16;
    /// The folder with the log file and the paired-devices file.
    fn log_dir(&self) -> PathBuf;
    /// Addresses a phone on the same network can open, with a trailing slash (`http://192.168.1.5:1990/`).
    fn lan_urls(&self) -> Vec<String>;
    /// Embedded page files: `admin.html`, `admin.js`, `app.css`, `ui.js`, `theme.js` and `icons` (the SVG sprite).
    fn asset(&self, file: &str) -> Option<&'static str>;
    /// Start a fresh copy of this program and exit (the tray and the dashboard call this).
    fn restart(&self);

    /// `lan_urls`, the most likely home-network address first (192.168.x, then 10.x, then 172.16-31.x, then the rest such
    /// as VPN and virtual-switch addresses), so the dashboard and the tray offer the one a phone can actually reach.
    fn phone_urls(&self) -> Vec<String> {
        let mut urls = self.lan_urls();
        urls.sort_by_key(|u| url_rank(u));
        urls
    }
}

fn url_rank(url: &str) -> u8 {
    let host = url.trim_start_matches("http://").split(':').next().unwrap_or("");
    let octets: Vec<u8> = host.split('.').filter_map(|p| p.parse().ok()).collect();
    match octets.as_slice() {
        [192, 168, ..] => 0,
        [10, ..] => 1,
        [172, b, ..] if (16..=31).contains(b) => 2,
        _ => 3,
    }
}

/// Paths that answer the PC only; the app's guard lets them through and these handlers do the checking.
pub fn is_admin_path(path: &str) -> bool {
    path == "/admin" || path.starts_with("/admin/")
}

/// A `Host` header that names this PC itself. Exact names only: `127.0.0.1.evil.example` must not pass.
fn loopback_host(headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("").trim();
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    matches!(name, "127.0.0.1" | "localhost" | "::1")
}

fn allowed(addr: &SocketAddr, headers: &HeaderMap, method: &Method) -> bool {
    if !addr.ip().is_loopback() || !loopback_host(headers) {
        return false;
    }
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if let Some(origin) = headers.get(header::ORIGIN) {
        if !origin.to_str().is_ok_and(|o| crate::netguard::origin_matches(o, host)) {
            return false;
        }
    }
    *method == Method::GET || headers.get("x-admin").is_some_and(|v| v == "1")
}

pub fn routes<H: AdminHost>() -> Router<Arc<H>> {
    Router::new()
        .route("/admin", get(page::<H>))
        .route("/admin/assets/{file}", get(asset::<H>))
        .route("/admin/api/state", get(state::<H>))
        .route("/admin/api/offer", post(offer::<H>))
        .route("/admin/api/rename", post(rename::<H>))
        .route("/admin/api/revoke", post(revoke::<H>))
        .route("/admin/api/revoke-all", post(revoke_all::<H>))
        .route("/admin/api/pairing", post(set_pairing::<H>))
        .route("/admin/api/autostart", post(set_autostart::<H>))
        .route("/admin/api/open-log", post(open_log::<H>))
        .route("/admin/api/restart", post(restart::<H>))
}

fn no_store() -> [(header::HeaderName, &'static str); 1] {
    [(header::CACHE_CONTROL, "no-store")]
}

fn forbidden() -> Response {
    StatusCode::FORBIDDEN.into_response()
}

fn ok() -> Response {
    (no_store(), Json(serde_json::json!({"ok": true}))).into_response()
}

fn fail(status: StatusCode, msg: &str) -> Response {
    (status, no_store(), Json(serde_json::json!({"ok": false, "error": msg}))).into_response()
}

async fn page<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !allowed(&addr, &headers, &Method::GET) {
        return forbidden();
    }
    let Some(html) = h.asset("admin.html") else { return StatusCode::NOT_FOUND.into_response() };
    let html = html
        .replace("{{SPRITE}}", h.asset("icons").unwrap_or(""))
        .replace("{{NAME}}", h.name())
        .replace("{{VERSION}}", h.version());
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], html).into_response()
}

async fn asset<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(file): Path<String>,
) -> Response {
    if !allowed(&addr, &headers, &Method::GET) {
        return forbidden();
    }
    let kind = match file.as_str() {
        "admin.js" | "ui.js" | "theme.js" | "remote-client.js" => "text/javascript; charset=utf-8",
        "app.css" => "text/css; charset=utf-8",
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    match h.asset(&file) {
        Some(body) => ([(header::CONTENT_TYPE, kind), (header::CACHE_CONTROL, "no-store")], body).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Serialize)]
struct DeviceJson {
    id: String,
    name: String,
    created: u64,
    last_seen: u64,
    active: bool,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn devices_json(p: &Pairing) -> Vec<DeviceJson> {
    let t = now();
    p.devices()
        .into_iter()
        .map(|d| DeviceJson {
            active: t.saturating_sub(d.last_seen) < 120,
            id: d.id,
            name: d.name,
            created: d.created,
            last_seen: d.last_seen,
        })
        .collect()
}

async fn state<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !allowed(&addr, &headers, &Method::GET) {
        return forbidden();
    }
    (
        no_store(),
        Json(serde_json::json!({
            "name": h.name(),
            "version": h.version(),
            "hostname": h.hostname(),
            "port": h.port(),
            "urls": h.phone_urls(),
            "required": h.pairing().required(),
            "paused": h.pairing().paused(),
            "autostart": crate::autostart::enabled(),
            "now": now(),
            "devices": devices_json(h.pairing()),
        })),
    )
        .into_response()
}

fn qr_svg(url: &str) -> String {
    qrcode::QrCode::new(url.as_bytes())
        .map(|c| c.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).build())
        .unwrap_or_default()
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OfferReq {
    name: String,
}

/// A fresh one-time offer, optionally naming the phone that will use it.
async fn offer<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<OfferReq>,
) -> Response {
    if !allowed(&addr, &headers, &Method::POST) {
        return forbidden();
    }
    if h.pairing().paused() {
        return fail(StatusCode::CONFLICT, "paused");
    }
    let label = Some(req.name.trim()).filter(|n| !n.is_empty());
    let o = h.pairing().new_offer_for(label);
    let links: Vec<_> = h
        .phone_urls()
        .into_iter()
        .map(|u| {
            let url = format!("{u}?pair={}", o.secret);
            serde_json::json!({"url": url, "qr": qr_svg(&url), "base": u})
        })
        .collect();
    (no_store(), Json(serde_json::json!({"code": o.code, "secret": o.secret, "ttl": o.ttl_secs, "links": links})))
        .into_response()
}

#[derive(Deserialize)]
struct RenameReq {
    id: String,
    name: String,
}

async fn rename<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<RenameReq>,
) -> Response {
    if !allowed(&addr, &headers, &Method::POST) {
        return forbidden();
    }
    if h.pairing().rename(&req.id, &req.name) {
        ok()
    } else {
        fail(StatusCode::NOT_FOUND, "no such device")
    }
}

#[derive(Deserialize)]
struct IdReq {
    id: String,
}

async fn revoke<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<IdReq>,
) -> Response {
    if !allowed(&addr, &headers, &Method::POST) {
        return forbidden();
    }
    if h.pairing().revoke(&req.id) {
        ok()
    } else {
        fail(StatusCode::NOT_FOUND, "no such device")
    }
}

async fn revoke_all<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !allowed(&addr, &headers, &Method::POST) {
        return forbidden();
    }
    let n = h.pairing().revoke_all();
    crate::applog::info(&format!("removed all {n} paired devices from the dashboard"));
    ok()
}

#[derive(Deserialize)]
struct PausedReq {
    paused: bool,
}

async fn set_pairing<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<PausedReq>,
) -> Response {
    if !allowed(&addr, &headers, &Method::POST) {
        return forbidden();
    }
    h.pairing().set_paused(req.paused);
    ok()
}

#[derive(Deserialize)]
struct OnReq {
    on: bool,
}

async fn set_autostart<H: AdminHost>(
    State(_h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<OnReq>,
) -> Response {
    if !allowed(&addr, &headers, &Method::POST) {
        return forbidden();
    }
    match crate::autostart::set(req.on) {
        Ok(()) => ok(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn open_log<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !allowed(&addr, &headers, &Method::POST) {
        return forbidden();
    }
    crate::shell::open_folder(&h.log_dir());
    ok()
}

/// Answer first, then restart, so the page can tell that it worked and wait for the new copy.
async fn restart<H: AdminHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if !allowed(&addr, &headers, &Method::POST) {
        return forbidden();
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        h.restart();
    });
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tower::ServiceExt;

    struct Dummy {
        pairing: Pairing,
        restarted: AtomicBool,
    }

    impl PairHost for Dummy {
        fn pairing(&self) -> &Pairing {
            &self.pairing
        }
        fn is_authorized(&self, _: Option<&SocketAddr>, _: &HeaderMap) -> bool {
            true
        }
    }

    impl AdminHost for Dummy {
        fn name(&self) -> &str {
            "Test"
        }
        fn hostname(&self) -> String {
            "Desktop-PC".into()
        }
        fn version(&self) -> &str {
            "1.2.3"
        }
        fn port(&self) -> u16 {
            1990
        }
        fn log_dir(&self) -> PathBuf {
            std::env::temp_dir()
        }
        fn lan_urls(&self) -> Vec<String> {
            vec!["http://192.168.1.5:1990/".into()]
        }
        fn asset(&self, file: &str) -> Option<&'static str> {
            match file {
                "admin.html" => Some("<h1>{{NAME}} {{VERSION}}</h1>{{SPRITE}}"),
                "icons" => Some("<svg></svg>"),
                "admin.js" => Some("//js"),
                _ => None,
            }
        }
        fn restart(&self) {
            self.restarted.store(true, Ordering::Relaxed);
        }
    }

    fn host() -> Arc<Dummy> {
        Arc::new(Dummy { pairing: Pairing::new(true, None), restarted: AtomicBool::new(false) })
    }

    async fn call(
        h: &Arc<Dummy>,
        method: &str,
        path: &str,
        peer: &str,
        hdrs: &[(&str, &str)],
        body: &str,
    ) -> (StatusCode, String) {
        let mut req = Request::builder().method(method).uri(path);
        for (k, v) in hdrs {
            req = req.header(*k, *v);
        }
        let mut req = req.body(Body::from(body.to_string())).unwrap();
        req.extensions_mut().insert(ConnectInfo::<SocketAddr>(peer.parse().unwrap()));
        let res = routes::<Dummy>().with_state(h.clone()).oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    const LOCAL: &str = "127.0.0.1:50000";
    const GET: &[(&str, &str)] = &[("host", "127.0.0.1:1990")];
    const POST: &[(&str, &str)] = &[
        ("host", "127.0.0.1:1990"),
        ("x-admin", "1"),
        ("content-type", "application/json"),
        ("origin", "http://127.0.0.1:1990"),
    ];

    #[tokio::test]
    async fn only_the_pc_itself_gets_in() {
        let h = host();
        assert_eq!(call(&h, "GET", "/admin", LOCAL, GET, "").await.0, StatusCode::OK);
        // a phone on the network, even with a loopback-looking Host
        assert_eq!(call(&h, "GET", "/admin", "192.168.1.9:4000", GET, "").await.0, StatusCode::FORBIDDEN);
        // the PC's address, but a Host that is not loopback (DNS rebinding)
        for evil in ["evil.example:1990", "127.0.0.1.evil.example:1990", "desktop-pc:1990"] {
            let (s, _) = call(&h, "GET", "/admin/api/state", LOCAL, &[("host", evil)], "").await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{evil}");
        }
        assert_eq!(call(&h, "GET", "/admin/api/state", LOCAL, &[("host", "[::1]:1990")], "").await.0, StatusCode::OK);
        assert_eq!(
            call(&h, "GET", "/admin/api/state", LOCAL, &[("host", "localhost:1990")], "").await.0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn changes_need_the_dashboards_own_header_and_origin() {
        let h = host();
        let body = r#"{"name":"Tablet"}"#;
        let no_header = &[("host", "127.0.0.1:1990"), ("content-type", "application/json")];
        assert_eq!(call(&h, "POST", "/admin/api/offer", LOCAL, no_header, body).await.0, StatusCode::FORBIDDEN);
        let other_site = &[
            ("host", "127.0.0.1:1990"),
            ("x-admin", "1"),
            ("content-type", "application/json"),
            ("origin", "http://evil.example"),
        ];
        assert_eq!(call(&h, "POST", "/admin/api/offer", LOCAL, other_site, body).await.0, StatusCode::FORBIDDEN);
        assert_eq!(call(&h, "POST", "/admin/api/offer", LOCAL, POST, body).await.0, StatusCode::OK);
        assert_eq!(call(&h, "POST", "/admin/api/revoke-all", LOCAL, no_header, "{}").await.0, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_page_fills_in_name_version_and_icons() {
        let h = host();
        let (s, body) = call(&h, "GET", "/admin", LOCAL, GET, "").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body, "<h1>Test 1.2.3</h1><svg></svg>");
        assert_eq!(call(&h, "GET", "/admin/assets/admin.js", LOCAL, GET, "").await.1, "//js");
        assert_eq!(call(&h, "GET", "/admin/assets/secret.txt", LOCAL, GET, "").await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn add_name_rename_pause_and_remove_a_device() {
        let h = host();
        let (_, body) = call(&h, "POST", "/admin/api/offer", LOCAL, POST, r#"{"name":"Living room"}"#).await;
        let offer: serde_json::Value = serde_json::from_str(&body).unwrap();
        let code = offer["code"].as_str().unwrap();
        assert_eq!(code.len(), 6);
        assert_eq!(
            offer["links"][0]["url"].as_str().unwrap(),
            format!("http://192.168.1.5:1990/?pair={}", offer["secret"].as_str().unwrap())
        );
        assert!(offer["links"][0]["qr"].as_str().unwrap().contains("<svg"));

        h.pairing.pair(code, "Android phone").unwrap();
        let (_, body) = call(&h, "GET", "/admin/api/state", LOCAL, GET, "").await;
        let st: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(st["devices"][0]["name"], "Living room");
        assert_eq!(st["devices"][0]["active"], true);
        let id = st["devices"][0]["id"].as_str().unwrap().to_string();

        let rename = format!(r#"{{"id":"{id}","name":"Kitchen"}}"#);
        assert_eq!(call(&h, "POST", "/admin/api/rename", LOCAL, POST, &rename).await.0, StatusCode::OK);
        assert_eq!(h.pairing.devices()[0].name, "Kitchen");

        assert_eq!(call(&h, "POST", "/admin/api/pairing", LOCAL, POST, r#"{"paused":true}"#).await.0, StatusCode::OK);
        assert_eq!(call(&h, "POST", "/admin/api/offer", LOCAL, POST, "{}").await.0, StatusCode::CONFLICT);
        call(&h, "POST", "/admin/api/pairing", LOCAL, POST, r#"{"paused":false}"#).await;

        let gone = format!(r#"{{"id":"{id}"}}"#);
        assert_eq!(call(&h, "POST", "/admin/api/revoke", LOCAL, POST, &gone).await.0, StatusCode::OK);
        assert_eq!(call(&h, "POST", "/admin/api/revoke", LOCAL, POST, &gone).await.0, StatusCode::NOT_FOUND);
        assert!(h.pairing.devices().is_empty());
    }

    #[tokio::test]
    async fn restart_answers_before_it_restarts() {
        let h = host();
        assert_eq!(call(&h, "POST", "/admin/api/restart", LOCAL, POST, "{}").await.0, StatusCode::OK);
        assert!(!h.restarted.load(Ordering::Relaxed), "not yet: the page needs its answer first");
        std::thread::sleep(std::time::Duration::from_millis(700));
        assert!(h.restarted.load(Ordering::Relaxed));
    }

    #[test]
    fn the_home_network_address_comes_first() {
        let mut urls = vec![
            "http://100.127.96.91:1990/",
            "http://172.20.0.1:1990/",
            "http://10.0.0.4:1990/",
            "http://192.168.178.100:1990/",
        ];
        urls.sort_by_key(|u| url_rank(u));
        assert_eq!(
            urls,
            [
                "http://192.168.178.100:1990/",
                "http://10.0.0.4:1990/",
                "http://172.20.0.1:1990/",
                "http://100.127.96.91:1990/"
            ]
        );
    }

    #[test]
    fn admin_paths_are_recognised() {
        assert!(is_admin_path("/admin"));
        assert!(is_admin_path("/admin/api/state"));
        assert!(!is_admin_path("/administrator"));
        assert!(!is_admin_path("/"));
    }
}
