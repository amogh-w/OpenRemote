//! Persistent settings (the listen port), stored as JSON under the user's config dir.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::{fs, io};

pub const DEFAULT_PORT: u16 = 1990;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_port")]
    pub port: u16,
    /// New phones must pair before they can control anything (on by default).
    #[serde(default = "yes")]
    pub require_pairing: bool,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn yes() -> bool {
    true
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

/// Where paired phones are kept (hashes of their keys, never the keys).
pub fn devices_path() -> PathBuf {
    config_dir().join("devices.json")
}

impl Settings {
    /// Load settings, creating the file with defaults on first run. Fields from older versions
    /// (for example a pairing token) are ignored and dropped on the next save.
    pub fn load_or_create() -> io::Result<Self> {
        if let Ok(text) = fs::read_to_string(path()) {
            if let Ok(s) = serde_json::from_str::<Settings>(&text) {
                return Ok(s);
            }
        }
        let s = Settings { port: DEFAULT_PORT, require_pairing: true };
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
    fn old_files_with_a_token_still_load() {
        let s: Settings =
            serde_json::from_str(r#"{"token":"0123456789abcdef","port":2000,"require_token":true}"#).unwrap();
        assert_eq!(s.port, 2000);
        assert!(s.require_pairing);
        let d: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(d.port, DEFAULT_PORT);
        assert!(d.require_pairing, "pairing is on unless it was turned off");
    }
}
