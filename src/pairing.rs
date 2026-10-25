//! Pairing: which phones may control this PC. Shared by OpenRemote and Kai Remote: edit it in
//! the cuppa plugin repo (shared/) and run `harness.mjs sync --write`, never in a project copy. Each crate
//! defines `PAIRING_COOKIE` in its `lib.rs`.
//!
//! The PC shows a one-time offer (a 6-digit code to type, a QR link and its token, valid for five minutes, usable once). A phone that
//! proves it has the offer gets a long random key, which it keeps in an HttpOnly cookie. The PC stores only a
//! hash of each key, so a copy of the devices file cannot be used to sign in. Wrong guesses lock pairing for a
//! few minutes. The PC itself (loopback) never needs to pair.

use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Name of the cookie that carries a phone's key.
pub const COOKIE: &str = crate::PAIRING_COOKIE;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Device {
    id: String,
    name: String,
    /// SHA-256 of the key, hex.
    hash: String,
    created: u64,
    last_seen: u64,
}

/// What the admin page shows for a paired phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub created: u64,
    pub last_seen: u64,
}

/// A fresh pairing offer, to be shown on the PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// 6 digits, typed on the phone.
    pub code: String,
    /// 32 hex characters, carried in the QR link.
    pub secret: String,
    pub ttl_secs: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PairError {
    /// Wrong, expired or already used.
    Invalid,
    /// Too many wrong tries; pairing is paused for this many seconds.
    Locked(u64),
    /// The owner paused pairing from the tray or the dashboard.
    Paused,
}

struct Pending {
    /// The device name chosen on the PC; it wins over the name the phone reports.
    label: Option<String>,
    code: String,
    secret: String,
    expires: Instant,
}

struct Inner {
    devices: Vec<Device>,
    pending: Option<Pending>,
    fails: u32,
    locked_until: Option<Instant>,
}

pub struct Pairing {
    required: bool,
    trust_loopback: bool,
    /// Set from the tray or the dashboard: no new phone can pair while true (paired phones keep working).
    paused: AtomicBool,
    path: Option<PathBuf>,
    ttl: Duration,
    max_fails: u32,
    lock: Duration,
    inner: Mutex<Inner>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_key(key: &str) -> String {
    hex(&Sha256::digest(key.as_bytes()))
}

/// Comparison that takes the same time wherever the first difference is.
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = (a.len() ^ b.len()) as u8;
    for (i, x) in a.iter().enumerate() {
        diff |= x ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

fn clean_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
    let n = n.trim();
    if n.is_empty() {
        "Phone".into()
    } else {
        n.into()
    }
}

/// The key from a `Cookie` request header, if it has the right shape (64 hex characters).
pub fn key_from_cookie(header: &str) -> Option<&str> {
    header
        .split(';')
        .filter_map(|p| p.trim().strip_prefix(COOKIE).and_then(|r| r.strip_prefix('=')))
        .find(|k| k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit()))
}

impl Pairing {
    /// `path` is where paired devices are kept (`None` keeps them in memory only, for tests).
    pub fn new(required: bool, path: Option<PathBuf>) -> Self {
        let devices = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Vec<Device>>(&t).ok())
            .unwrap_or_default();
        Self {
            required,
            trust_loopback: true,
            paused: AtomicBool::new(false),
            path,
            ttl: Duration::from_secs(300),
            max_fails: 5,
            lock: Duration::from_secs(300),
            inner: Mutex::new(Inner { devices, pending: None, fails: 0, locked_until: None }),
        }
    }

    /// No pairing at all (tests and `--no-pairing`).
    pub fn open() -> Self {
        Self::new(false, None)
    }

    /// Treat the PC's own address like any other client (tests use this to exercise the checks from loopback).
    pub fn without_loopback_exemption(mut self) -> Self {
        self.trust_loopback = false;
        self
    }

    /// Other limits (tests use short ones).
    pub fn with_limits(mut self, ttl: Duration, max_fails: u32, lock: Duration) -> Self {
        self.ttl = ttl;
        self.max_fails = max_fails;
        self.lock = lock;
        self
    }

    pub fn required(&self) -> bool {
        self.required
    }

    pub fn trusts_loopback(&self) -> bool {
        self.trust_loopback
    }

    fn lock_inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, inner: &Inner) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(&inner.devices) {
            Ok(text) => {
                if let Err(e) = std::fs::write(path, text) {
                    crate::applog::error(&format!("cannot save paired devices: {e}"));
                }
            }
            Err(e) => crate::applog::error(&format!("cannot save paired devices: {e}")),
        }
    }

    /// Create a new offer (replacing any earlier one).
    pub fn new_offer(&self) -> Offer {
        self.new_offer_for(None)
    }

    /// Like `new_offer`, but the phone that redeems it is named `label` (typed on the PC) instead of by its browser.
    pub fn new_offer_for(&self, label: Option<&str>) -> Offer {
        let mut rng = rand::rng();
        let code = format!("{:06}", rng.random_range(0..1_000_000u32));
        let mut s = [0u8; 16];
        rng.fill_bytes(&mut s);
        let secret = hex(&s);
        self.lock_inner().pending = Some(Pending {
            label: label.map(clean_name),
            code: code.clone(),
            secret: secret.clone(),
            expires: Instant::now() + self.ttl,
        });
        Offer { code, secret, ttl_secs: self.ttl.as_secs() }
    }

    /// Try to pair with the code or the secret from the current offer. On success returns the phone's new key.
    pub fn pair(&self, proof: &str, name: &str) -> Result<String, PairError> {
        if self.paused() {
            return Err(PairError::Paused);
        }
        let mut inner = self.lock_inner();
        let now = Instant::now();
        if let Some(until) = inner.locked_until {
            if now < until {
                return Err(PairError::Locked((until - now).as_secs() + 1));
            }
            inner.locked_until = None;
            inner.fails = 0;
        }
        let proof: String = proof.chars().filter(|c| !c.is_whitespace()).collect();
        let ok = match &inner.pending {
            Some(p) if now < p.expires && !proof.is_empty() => {
                ct_eq(&p.code, &proof) | ct_eq(&p.secret, &proof.to_ascii_lowercase())
            }
            _ => false,
        };
        if !ok {
            inner.fails += 1;
            if inner.fails >= self.max_fails {
                inner.locked_until = Some(now + self.lock);
                inner.pending = None; // whoever was guessing cannot keep using this offer
                crate::applog::warn(&format!(
                    "pairing locked for {} s after {} wrong tries",
                    self.lock.as_secs(),
                    inner.fails
                ));
                return Err(PairError::Locked(self.lock.as_secs()));
            }
            return Err(PairError::Invalid);
        }
        let label = inner.pending.take().and_then(|p| p.label); // one use
        inner.fails = 0;
        let mut k = [0u8; 32];
        rand::rng().fill_bytes(&mut k);
        let key = hex(&k);
        let mut id = [0u8; 4];
        rand::rng().fill_bytes(&mut id);
        inner.devices.push(Device {
            id: hex(&id),
            name: label.unwrap_or_else(|| clean_name(name)),
            hash: hash_key(&key),
            created: now_secs(),
            last_seen: now_secs(),
        });
        self.save(&inner);
        crate::applog::info(&format!("paired a new device ({} total)", inner.devices.len()));
        Ok(key)
    }

    /// Is this key a paired phone? Returns its device id and notes that it was just seen.
    pub fn check(&self, key: &str) -> Option<String> {
        let hash = hash_key(key);
        let mut inner = self.lock_inner();
        let d = inner.devices.iter_mut().find(|d| ct_eq(&d.hash, &hash))?;
        d.last_seen = now_secs();
        Some(d.id.clone())
    }

    pub fn devices(&self) -> Vec<DeviceView> {
        self.lock_inner()
            .devices
            .iter()
            .map(|d| DeviceView { id: d.id.clone(), name: d.name.clone(), created: d.created, last_seen: d.last_seen })
            .collect()
    }

    /// Refuse (or allow again) new pairings. Paired phones are not affected.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Give a paired phone a new name; true when it exists.
    pub fn rename(&self, id: &str, name: &str) -> bool {
        let mut inner = self.lock_inner();
        let Some(d) = inner.devices.iter_mut().find(|d| d.id == id) else { return false };
        d.name = clean_name(name);
        self.save(&inner);
        true
    }

    /// Remove every phone; returns how many there were.
    pub fn revoke_all(&self) -> usize {
        let mut inner = self.lock_inner();
        let n = inner.devices.len();
        inner.devices.clear();
        if n > 0 {
            self.save(&inner);
        }
        n
    }

    /// Remove a phone; true when it existed.
    pub fn revoke(&self, id: &str) -> bool {
        let mut inner = self.lock_inner();
        let before = inner.devices.len();
        inner.devices.retain(|d| d.id != id);
        let removed = inner.devices.len() != before;
        if removed {
            self.save(&inner);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> Pairing {
        Pairing::new(true, None)
    }

    #[test]
    fn code_pairs_once_and_the_key_works() {
        let p = p();
        let offer = p.new_offer();
        assert_eq!(offer.code.len(), 6);
        assert_eq!(offer.secret.len(), 32);
        let key = p.pair(&offer.code, "iPhone").unwrap();
        assert_eq!(key.len(), 64);
        assert!(p.check(&key).is_some());
        assert!(p.check(&"0".repeat(64)).is_none());
        // single use
        assert_eq!(p.pair(&offer.code, "iPhone"), Err(PairError::Invalid));
    }

    #[test]
    fn the_qr_secret_pairs_too_and_spaces_in_a_code_are_fine() {
        let p = p();
        let o = p.new_offer();
        assert!(p.pair(&o.secret, "Android").is_ok());
        let o = p.new_offer();
        let spaced = format!("{} {}", &o.code[..3], &o.code[3..]);
        assert!(p.pair(&spaced, "Android").is_ok());
    }

    #[test]
    fn nothing_pairs_without_an_offer_or_with_a_blank_proof() {
        let p = p();
        assert_eq!(p.pair("123456", "x"), Err(PairError::Invalid));
        p.new_offer();
        assert_eq!(p.pair("", "x"), Err(PairError::Invalid));
    }

    #[test]
    fn expired_offers_are_refused() {
        let p = Pairing::new(true, None).with_limits(Duration::from_millis(1), 5, Duration::from_secs(60));
        let o = p.new_offer();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(p.pair(&o.code, "x"), Err(PairError::Invalid));
    }

    #[test]
    fn wrong_guesses_lock_pairing_and_burn_the_offer() {
        let p = Pairing::new(true, None).with_limits(Duration::from_secs(120), 3, Duration::from_secs(60));
        let o = p.new_offer();
        let wrong = if o.code == "000000" { "111111" } else { "000000" };
        assert_eq!(p.pair(wrong, "x"), Err(PairError::Invalid));
        assert_eq!(p.pair(wrong, "x"), Err(PairError::Invalid));
        assert!(matches!(p.pair(wrong, "x"), Err(PairError::Locked(_))));
        // even the right code is refused while locked
        assert!(matches!(p.pair(&o.code, "x"), Err(PairError::Locked(_))));
    }

    #[test]
    fn revoking_removes_access() {
        let p = p();
        let o = p.new_offer();
        let key = p.pair(&o.code, "iPhone").unwrap();
        let id = p.devices()[0].id.clone();
        assert!(p.revoke(&id));
        assert!(p.check(&key).is_none());
        assert!(!p.revoke(&id));
        assert!(p.devices().is_empty());
    }

    #[test]
    fn devices_survive_a_restart_and_only_hashes_are_stored() {
        let path = std::env::temp_dir().join(format!("openremote-devices-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let p = Pairing::new(true, Some(path.clone()));
        let key = {
            let o = p.new_offer();
            p.pair(&o.code, "Pixel").unwrap()
        };
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&key), "the key itself must not be written to disk");
        let again = Pairing::new(true, Some(path.clone()));
        assert!(again.check(&key).is_some());
        assert_eq!(again.devices()[0].name, "Pixel");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_offer_always_has_a_code_and_the_pc_can_name_the_phone() {
        let p = p();
        let o = p.new_offer_for(Some("  Living room tablet "));
        assert_eq!(o.code.len(), 6);
        p.pair(&o.code, "Android phone").unwrap();
        assert_eq!(p.devices()[0].name, "Living room tablet");
        // an offer without a label uses the browser's name
        let o = p.new_offer();
        p.pair(&o.code, "iPhone").unwrap();
        assert_eq!(p.devices()[1].name, "iPhone");
    }

    #[test]
    fn pausing_refuses_new_phones_but_not_paired_ones() {
        let p = p();
        let o = p.new_offer();
        let key = p.pair(&o.code, "iPhone").unwrap();
        p.set_paused(true);
        let o = p.new_offer();
        assert_eq!(p.pair(&o.code, "x"), Err(PairError::Paused));
        assert!(p.check(&key).is_some());
        p.set_paused(false);
        assert!(p.pair(&o.code, "x").is_ok(), "the offer was not used up by the refused try");
    }

    #[test]
    fn renaming_and_removing_everything() {
        let p = p();
        for n in ["a", "b", "c"] {
            let o = p.new_offer();
            p.pair(&o.code, n).unwrap();
        }
        let id = p.devices()[1].id.clone();
        assert!(p.rename(&id, " Kitchen "));
        assert_eq!(p.devices()[1].name, "Kitchen");
        assert!(!p.rename("nope", "x"));
        assert_eq!(p.revoke_all(), 3);
        assert!(p.devices().is_empty());
        assert_eq!(p.revoke_all(), 0);
    }

    #[test]
    fn renames_are_saved() {
        let path = std::env::temp_dir().join(format!("pairing-rename-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let p = Pairing::new(true, Some(path.clone()));
        let o = p.new_offer();
        let key = p.pair(&o.code, "Pixel").unwrap();
        p.rename(&p.devices()[0].id, "Desk");
        let again = Pairing::new(true, Some(path.clone()));
        assert!(again.check(&key).is_some());
        assert_eq!(again.devices()[0].name, "Desk");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  \u{0}iPhone\n "), "iPhone");
        assert_eq!(clean_name(""), "Phone");
        assert_eq!(clean_name(&"x".repeat(100)).len(), 40);
    }

    #[test]
    fn keys_are_read_from_the_cookie_header() {
        let key = "ab".repeat(32);
        assert_eq!(key_from_cookie(&format!("theme=dark; {COOKIE}={key}; other=1")), Some(key.as_str()));
        assert_eq!(key_from_cookie(&format!("{COOKIE}=short")), None);
        assert_eq!(key_from_cookie("nothing=here"), None);
    }
}
