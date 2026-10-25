//! The phone's side of pairing: type the code (or open the QR link) and get a key. Shared by OpenRemote and Kai Remote:
//! edit it in the cuppa plugin repo (shared/) and run `harness.mjs sync --write`, never in a project copy.
//!
//! Three routes, all reachable before a phone is paired (the page shows the pairing screen): `POST /pair` (code or
//! link secret, sets the key cookie), `GET /pair/status` (does this phone still belong?), `POST /pair/forget`
//! (a phone removes itself). The rules live in `pairing.rs`; the PC side is `admin.rs`.

use crate::pairing::{self, PairError, Pairing};
use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::{net::SocketAddr, sync::Arc};

/// What the pairing routes need from an app's state.
pub trait PairHost: Send + Sync + 'static {
    fn pairing(&self) -> &Pairing;
    /// May this request use the app (a paired phone, or the PC itself when the app trusts loopback)?
    fn is_authorized(&self, addr: Option<&SocketAddr>, headers: &HeaderMap) -> bool;
}

/// Paths a phone must reach before it is paired.
pub const PUBLIC_PATHS: [&str; 3] = ["/pair", "/pair/status", "/pair/forget"];

/// The device id of the phone whose key is in the request's cookies, if it is still paired.
pub fn device_id(pairing: &Pairing, headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(pairing::key_from_cookie)
        .find_map(|k| pairing.check(k))
}

pub fn key_cookie(key: &str, max_age: u64) -> String {
    format!("{}={key}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Strict", pairing::COOKIE)
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

pub fn routes<H: PairHost>() -> Router<Arc<H>> {
    Router::new()
        .route("/pair", post(pair_post::<H>))
        .route("/pair/status", get(pair_status::<H>))
        .route("/pair/forget", post(pair_forget::<H>))
}

/// A phone proves it has the offer shown on the PC and gets its key as a cookie.
async fn pair_post<H: PairHost>(State(h): State<Arc<H>>, Json(req): Json<PairReq>) -> Response {
    let proof = if req.secret.is_empty() { req.code } else { req.secret };
    match h.pairing().pair(&proof, &req.name) {
        Ok(key) => (
            [
                (header::SET_COOKIE, key_cookie(&key, 10 * 365 * 24 * 3600)),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            Json(serde_json::json!({"ok": true})),
        )
            .into_response(),
        Err(PairError::Locked(secs)) => {
            (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({"ok": false, "error": "locked", "retry": secs})))
                .into_response()
        }
        Err(PairError::Paused) => {
            (StatusCode::FORBIDDEN, Json(serde_json::json!({"ok": false, "error": "paused"}))).into_response()
        }
        Err(PairError::Invalid) => {
            (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"ok": false, "error": "invalid"}))).into_response()
        }
    }
}

/// What the page needs to decide between the pairing screen and the app.
async fn pair_status<H: PairHost>(
    State(h): State<Arc<H>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let device = device_id(h.pairing(), &headers).is_some();
    let paired = h.is_authorized(Some(&addr), &headers);
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({
            "required": h.pairing().required(),
            "paired": paired,
            "device": device,
            "paused": h.pairing().paused(),
        })),
    )
        .into_response()
}

/// A phone removes itself (Settings > Unpair this phone).
async fn pair_forget<H: PairHost>(State(h): State<Arc<H>>, headers: HeaderMap) -> Response {
    if let Some(id) = device_id(h.pairing(), &headers) {
        h.pairing().revoke(&id);
    }
    ([(header::SET_COOKIE, key_cookie("", 0))], Json(serde_json::json!({"ok": true}))).into_response()
}
