//! Persistent settings: pairing token and port, stored as JSON under the user's config dir.

use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::{fs, io};

pub const DEFAULT_PORT: u16 = 1990;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub token: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Pairing is OFF by default: anyone who can reach the port may connect. Turn on with
    /// `--require-token` (the token and QR flow stay intact).
    #[serde(default)]
    pub require_token: bool,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

pub fn new_token() -> String {
    let mut b = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("openremote")
}

pub fn path() -> PathBuf {
    config_dir().join("settings.json")
}

impl Settings {
    /// Load settings, creating the file with a fresh token on first run.
    pub fn load_or_create() -> io::Result<Self> {
        let p = path();
        if let Ok(text) = fs::read_to_string(&p) {
            if let Ok(s) = serde_json::from_str::<Settings>(&text) {
                if s.token.len() >= 16 {
                    return Ok(s);
                }
            }
        }
        let s = Settings { token: new_token(), port: DEFAULT_PORT, require_token: false };
        s.save()?;
        Ok(s)
    }

    pub fn save(&self) -> io::Result<()> {
        let p = path();
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(p, serde_json::to_string_pretty(self).map_err(io::Error::other)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_hex() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
