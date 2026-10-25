pub mod admin;
pub mod applog;
pub mod autostart;
pub mod command;
pub mod image;
pub mod input;
#[cfg(windows)]
pub mod media;
pub mod netguard;
pub mod pairing;
pub mod pairroutes;
pub mod settings;
pub mod shell;
pub mod sleeptimer;
pub mod smooth;
pub mod uac;
pub mod web;
#[cfg(windows)]
pub mod winapps;

/// Shown in the tray, the dashboard, message boxes and the Start with Windows entry.
pub const APP_NAME: &str = "OpenRemote";

/// Name of the cookie that carries a phone's pairing key.
pub const PAIRING_COOKIE: &str = "openremote_key";
