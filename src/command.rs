//! Command layer: typed client messages and the dispatcher that turns them into input events.

use crate::sleeptimer::{self, Media, SleepTimer};
use crate::smooth::Smoother;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BtnAct {
    Down,
    Up,
    Click,
    Dbl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyAct {
    #[default]
    Press,
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerAct {
    Lock,
    Sleep,
    Hibernate,
    Logoff,
    Restart,
    Shutdown,
}

/// What a client may do to an open window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowAct {
    Focus,
    Min,
    Max,
    Restore,
    Close,
}

/// An open top-level window, as shown in the app switcher.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowInfo {
    /// Opaque handle; only valid for `win.act` while the window exists.
    pub id: u64,
    pub title: String,
    /// Executable file name, e.g. `chrome.exe` (empty if unknown).
    pub exe: String,
    /// Icon key for `GET /icon/<key>.png`; empty when there is no icon.
    pub icon: String,
    pub minimized: bool,
    /// Maximized (so the button can toggle back to the normal size).
    pub maximized: bool,
    pub foreground: bool,
}

/// A launchable application (Start Menu shortcut).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppEntry {
    /// Opaque key handed out by the server; the client never supplies a path.
    pub id: String,
    pub name: String,
    pub icon: String,
}

/// Messages a client may send (JSON, tagged by `t`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Msg {
    /// Relative pointer movement in pixels.
    Move {
        dx: i32,
        dy: i32,
    },
    Btn {
        b: Button,
        s: BtnAct,
    },
    /// Wheel units, 120 per notch. Positive `dy` scrolls down, positive `dx` scrolls right.
    Scroll {
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    /// `k` is a key name or `mod+mod+key` combo, e.g. `ctrl+shift+t`.
    Key {
        k: String,
        #[serde(default)]
        s: KeyAct,
    },
    Text {
        v: String,
    },
    Power {
        a: PowerAct,
    },
    /// Open an http(s) URL in the PC's default browser.
    Open {
        url: String,
    },
    /// Replace the PC clipboard text.
    #[serde(rename = "clip.set")]
    ClipSet {
        text: String,
    },
    /// Ask for the PC clipboard text; answered with `{"t":"clip","text":...}`.
    #[serde(rename = "clip.get")]
    ClipGet,
    /// List open windows; answered with `{"t":"win.list","items":[...]}`.
    #[serde(rename = "win.list")]
    WinList,
    #[serde(rename = "win.act")]
    WinAct {
        id: u64,
        a: WindowAct,
    },
    /// List launchable apps; answered with `{"t":"app.list","items":[...]}`.
    #[serde(rename = "app.list")]
    AppList,
    #[serde(rename = "app.launch")]
    AppLaunch {
        id: String,
    },
    /// Ask what is playing; answered with `{"t":"media",...}` (see [`MediaInfo`]).
    #[serde(rename = "media.get")]
    MediaGet,
    /// Put the PC to sleep after `secs` seconds, or when the current episode ends (`episode`). Replaces a running
    /// timer; answered with `{"t":"sleep",...}`.
    #[serde(rename = "sleep.set")]
    SleepSet {
        #[serde(default)]
        secs: u64,
        #[serde(default)]
        episode: bool,
    },
    /// Cancel the sleep timer; answered with `{"t":"sleep","on":false}`.
    #[serde(rename = "sleep.cancel")]
    SleepCancel,
    /// Ask for the sleep timer; answered with `{"t":"sleep",...}`.
    #[serde(rename = "sleep.get")]
    SleepGet,
    Ping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaState {
    Playing,
    Paused,
    Stopped,
}

/// What the system media session reports (title, artist, position in seconds, ...).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaInfo {
    pub state: MediaState,
    pub title: String,
    pub artist: String,
    pub app: String,
    pub pos: u64,
    pub dur: u64,
    /// True when `GET /media/art` can return a cover image.
    pub art: bool,
}

/// Platform input backend. The Windows implementation lives in `input.rs`.
pub trait Injector: Send + Sync {
    fn move_rel(&self, dx: i32, dy: i32);
    fn button(&self, b: Button, down: bool);
    /// Raw wheel units; positive is up/right like the Windows API.
    fn wheel(&self, horizontal: bool, delta: i32);
    fn key(&self, vk: u8, down: bool);
    fn type_unit(&self, unit: u16);
    /// Virtual-key for a single typed character (VkKeyScan on Windows).
    fn vk_for_char(&self, c: char) -> u8;
    /// True when the SYSTEM input helper is running (input then also reaches UAC prompts and admin windows).
    fn helper_active(&self) -> bool {
        false
    }
    fn power(&self, _act: PowerAct) {}
    fn open_url(&self, _url: &str) {}
    fn clip_get(&self) -> Option<String> {
        None
    }
    /// Replace the clipboard text; false if the clipboard could not be written.
    fn clip_set(&self, _text: &str) -> bool {
        false
    }
    /// Width and height of an image on the clipboard, if there is one.
    fn clip_image_info(&self) -> Option<(u32, u32)> {
        None
    }
    /// The clipboard image encoded as PNG.
    fn clip_image_png(&self) -> Option<Vec<u8>> {
        None
    }
    /// Put a (validated) PNG on the clipboard as an image.
    fn clip_image_set(&self, _png: &[u8]) -> bool {
        false
    }
    /// The current media session, or `None` when nothing is playing.
    fn media(&self) -> Option<MediaInfo> {
        None
    }
    /// Cover image (PNG or JPEG bytes) of the current track.
    fn media_art(&self) -> Option<Vec<u8>> {
        None
    }
    fn windows(&self) -> Vec<WindowInfo> {
        Vec::new()
    }
    fn window_action(&self, _id: u64, _act: WindowAct) {}
    fn apps(&self) -> Vec<AppEntry> {
        Vec::new()
    }
    fn launch_app(&self, _id: &str) {}
    /// PNG bytes for an icon key previously returned in a window or app entry.
    fn icon_png(&self, _key: &str) -> Option<Vec<u8>> {
        None
    }
}

const MAX_CLIP_BYTES: usize = 256 * 1024;
const MAX_URL_LEN: usize = 2048;
const MAX_MOVE: i32 = 5000;
const MAX_TEXT_UNITS: usize = 2000;
/// The sleep timer's view of the PC: what is playing, and the real sleep.
struct PcHost<I> {
    inj: Arc<I>,
    events: broadcast::Sender<String>,
}

impl<I: Injector + 'static> sleeptimer::Host for PcHost<I> {
    fn media(&self) -> Option<Media> {
        let m = self.inj.media().filter(|m| m.dur > 0 && m.state != MediaState::Stopped)?;
        Some(Media { pos: m.pos as f64, dur: m.dur as f64, title: m.title })
    }
    fn suspend(&self) {
        self.inj.power(PowerAct::Sleep);
    }
    fn notify(&self, status: serde_json::Value) {
        let _ = self.events.send(status.to_string());
    }
}

pub struct Dispatcher<I: Injector> {
    inj: Arc<I>,
    /// Paces pointer movement (see `smooth.rs`); `None` injects every move the moment it arrives.
    smooth: Option<Arc<Smoother>>,
    sleep: SleepTimer,
    /// Messages for every connected phone (the sleep timer ending, or changed from another phone).
    events: broadcast::Sender<String>,
}

impl<I: Injector + 'static> Dispatcher<I> {
    /// Pointer moves are paced by a `Smoother` with time constant `tau_ms` (0 turns smoothing off).
    pub fn with_smoothing(inj: I, tau_ms: u32) -> Self {
        let inj = Arc::new(inj);
        let smooth = (tau_ms > 0).then(|| {
            let s = Arc::new(Smoother::new(f64::from(tau_ms)));
            let target = Arc::clone(&inj);
            s.spawn(move |dx, dy| target.move_rel(dx, dy));
            s
        });
        let events = broadcast::channel(16).0;
        let sleep = SleepTimer::new(PcHost { inj: Arc::clone(&inj), events: events.clone() });
        Self { inj, smooth, sleep, events }
    }
}

impl<I: Injector + 'static> Dispatcher<I> {
    /// Moves are injected as they arrive.
    pub fn new(inj: I) -> Self {
        let inj = Arc::new(inj);
        let events = broadcast::channel(16).0;
        let sleep = SleepTimer::new(PcHost { inj: Arc::clone(&inj), events: events.clone() });
        Self { inj, smooth: None, sleep, events }
    }

    /// Tell every connected phone, so they all show the same timer; the caller also answers its own phone.
    fn share(&self, v: serde_json::Value) -> serde_json::Value {
        let _ = self.events.send(v.to_string());
        v
    }

    /// Messages meant for all phones; each connection subscribes for its lifetime.
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    /// Release everything the smoother still holds, so a click or scroll happens where the finger stopped.
    fn settle(&self) {
        if let Some(s) = &self.smooth {
            s.flush(&mut |dx, dy| self.inj.move_rel(dx, dy));
        }
    }

    /// Apply a message; returns a reply for the client when the message asks for one.
    pub fn handle(&self, msg: &Msg) -> Option<serde_json::Value> {
        match msg {
            Msg::ClipGet => {
                let text = self.inj.clip_get().unwrap_or_default();
                let image = self.inj.clip_image_info().map(|(w, h)| serde_json::json!({"w": w, "h": h}));
                return Some(serde_json::json!({"t": "clip", "text": text, "image": image}));
            }
            Msg::ClipSet { text } => {
                let ok = text.len() <= MAX_CLIP_BYTES && self.inj.clip_set(text);
                return Some(serde_json::json!({"t": "clip.ack", "ok": ok}));
            }
            Msg::MediaGet => {
                return Some(match self.inj.media() {
                    Some(m) => {
                        let mut v = serde_json::to_value(m).unwrap_or_default();
                        v["t"] = "media".into();
                        v
                    }
                    None => serde_json::json!({"t": "media", "state": "none"}),
                });
            }
            &Msg::SleepSet { secs, episode } => {
                return Some(self.share(self.sleep.set((secs > 0).then_some(secs), episode)));
            }
            Msg::SleepCancel => return Some(self.share(self.sleep.cancel())),
            Msg::SleepGet => return Some(self.sleep.get()),
            Msg::WinList => return Some(serde_json::json!({"t": "win.list", "items": self.inj.windows()})),
            Msg::AppList => return Some(serde_json::json!({"t": "app.list", "items": self.inj.apps()})),
            _ => {}
        }
        match msg {
            &Msg::Move { dx, dy } => {
                let (dx, dy) = (dx.clamp(-MAX_MOVE, MAX_MOVE), dy.clamp(-MAX_MOVE, MAX_MOVE));
                match &self.smooth {
                    Some(s) => s.push(dx, dy),
                    None => self.inj.move_rel(dx, dy),
                }
            }
            Msg::Btn { b, s } => {
                self.settle();
                match s {
                    BtnAct::Down => self.inj.button(*b, true),
                    BtnAct::Up => self.inj.button(*b, false),
                    BtnAct::Click => self.click(*b),
                    BtnAct::Dbl => {
                        self.click(*b);
                        self.click(*b);
                    }
                }
            }
            &Msg::Scroll { dx, dy } => {
                self.settle();
                if dy != 0 {
                    self.inj.wheel(false, -dy.clamp(-MAX_MOVE, MAX_MOVE));
                }
                if dx != 0 {
                    self.inj.wheel(true, dx.clamp(-MAX_MOVE, MAX_MOVE));
                }
            }
            Msg::Key { k, s } => self.key(k, *s),
            Msg::Text { v } => v.encode_utf16().take(MAX_TEXT_UNITS).for_each(|u| self.inj.type_unit(u)),
            Msg::Power { a } => self.inj.power(*a),
            Msg::Open { url } => {
                if is_safe_url(url) {
                    self.inj.open_url(url);
                }
            }
            Msg::WinAct { id, a } => self.inj.window_action(*id, *a),
            Msg::AppLaunch { id } => {
                if is_icon_key(id) {
                    self.inj.launch_app(id);
                }
            }
            Msg::ClipGet
            | Msg::ClipSet { .. }
            | Msg::MediaGet
            | Msg::SleepSet { .. }
            | Msg::SleepCancel
            | Msg::SleepGet
            | Msg::WinList
            | Msg::AppList
            | Msg::Ping => {}
        }
        None
    }

    pub fn helper_active(&self) -> bool {
        self.inj.helper_active()
    }

    /// The current track's cover (served at `GET /media/art`).
    pub fn media_art(&self) -> Option<Vec<u8>> {
        self.inj.media_art()
    }

    /// The clipboard image as PNG (served at `GET /clip/image.png`).
    pub fn clip_image_png(&self) -> Option<Vec<u8>> {
        self.inj.clip_image_png()
    }

    /// Validate an uploaded PNG and put it on the clipboard. Errors are short, client-safe messages.
    pub fn clip_image_set(&self, png: &[u8]) -> Result<(u32, u32), &'static str> {
        let dims = crate::image::png_dimensions(png)?;
        if self.inj.clip_image_set(png) {
            Ok(dims)
        } else {
            Err("could not write the clipboard")
        }
    }

    /// PNG for an icon key. Rejects anything that is not a key this server could have issued.
    pub fn icon_png(&self, key: &str) -> Option<Vec<u8>> {
        if is_icon_key(key) {
            self.inj.icon_png(key)
        } else {
            None
        }
    }

    fn click(&self, b: Button) {
        self.inj.button(b, true);
        self.inj.button(b, false);
    }

    fn key(&self, combo: &str, act: KeyAct) {
        let vks: Vec<u8> =
            parse_combo(combo).into_iter().map(|n| vk_from_name(n, |c| self.inj.vk_for_char(c))).collect();
        // Any unknown name makes the whole combo a no-op rather than a partial press.
        if vks.is_empty() || vks.contains(&0) {
            return;
        }
        if act != KeyAct::Up {
            vks.iter().for_each(|&v| self.inj.key(v, true));
        }
        if act != KeyAct::Down {
            vks.iter().rev().for_each(|&v| self.inj.key(v, false));
        }
    }
}

/// Icon and app ids are 16 lowercase hex digits (a hash of the source path).
pub fn is_icon_key(key: &str) -> bool {
    key.len() == 16 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Only plain http(s) URLs without whitespace/control characters may be opened.
pub fn is_safe_url(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://"))
        && url.len() <= MAX_URL_LEN
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Split `ctrl+shift+t` into parts; a trailing `+` means the plus key (`ctrl++`).
fn parse_combo(k: &str) -> Vec<&str> {
    if k == "+" {
        return vec!["+"];
    }
    let (head, plus) = match k.strip_suffix("++") {
        Some(h) => (h, true),
        None => (k, false),
    };
    let mut parts: Vec<&str> = head.split('+').filter(|p| !p.is_empty()).collect();
    if plus {
        parts.push("+");
    }
    parts
}

/// Key-name table (common names and aliases). Unknown names map to 0.
pub fn vk_from_name(name: &str, single_char: impl Fn(char) -> u8) -> u8 {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return single_char(c);
    }
    match name.to_lowercase().as_str() {
        "enter" | "rtn" | "return" => 13,
        "backspace" | "bas" | "back" => 8,
        "alt" => 18,
        "ctrl" | "control" => 17,
        "shift" => 16,
        "cmd" | "win" | "lwin" | "meta" => 91,
        "rwin" => 92,
        "menu" | "apps" => 93,
        "tab" => 9,
        "esc" | "escape" => 27,
        "home" => 36,
        "insert" => 45,
        "delete" | "del" => 46,
        "end" => 35,
        "pageup" => 33,
        "pagedown" => 34,
        "pause" => 19,
        "capslock" => 20,
        "printscreen" => 44,
        "numlock" => 144,
        "scrolllock" => 145,
        "space" => 32,
        "up" => 38,
        "down" => 40,
        "left" => 37,
        "right" => 39,
        "volume_up" => 175,
        "volume_down" => 174,
        "volume_mute" => 173,
        "media_play_pause" => 179,
        "media_next_track" => 176,
        "media_prev_track" => 177,
        "media_stop" => 178,
        f if f.starts_with('f') => match f[1..].parse::<u8>() {
            Ok(n @ 1..=24) => 111 + n,
            _ => 0,
        },
        _ => 0,
    }
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Records every injected event as a string, for assertions.
    #[derive(Default)]
    pub struct Rec(pub Mutex<Vec<String>>);

    impl Rec {
        pub fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl Injector for Arc<Rec> {
        fn move_rel(&self, dx: i32, dy: i32) {
            self.0.lock().unwrap().push(format!("move {dx} {dy}"));
        }
        fn button(&self, b: Button, down: bool) {
            self.0.lock().unwrap().push(format!("btn {b:?} {down}"));
        }
        fn wheel(&self, h: bool, d: i32) {
            self.0.lock().unwrap().push(format!("wheel {h} {d}"));
        }
        fn key(&self, vk: u8, down: bool) {
            self.0.lock().unwrap().push(format!("key {vk} {down}"));
        }
        fn type_unit(&self, u: u16) {
            self.0.lock().unwrap().push(format!("type {u}"));
        }
        fn vk_for_char(&self, c: char) -> u8 {
            c.to_ascii_uppercase() as u8
        }
        fn power(&self, a: PowerAct) {
            self.0.lock().unwrap().push(format!("power {a:?}"));
        }
        fn open_url(&self, u: &str) {
            self.0.lock().unwrap().push(format!("open {u}"));
        }
        fn clip_get(&self) -> Option<String> {
            Some("from pc".into())
        }
        fn clip_set(&self, t: &str) -> bool {
            self.0.lock().unwrap().push(format!("clip {t}"));
            t != "fail"
        }
        fn media(&self) -> Option<MediaInfo> {
            Some(MediaInfo {
                state: MediaState::Playing,
                title: "Song".into(),
                artist: "Band".into(),
                app: "Player".into(),
                pos: 12,
                dur: 200,
                art: true,
            })
        }
        fn media_art(&self) -> Option<Vec<u8>> {
            Some(vec![0x89, b'P', b'N', b'G'])
        }
        fn clip_image_info(&self) -> Option<(u32, u32)> {
            Some((3, 2))
        }
        fn clip_image_png(&self) -> Option<Vec<u8>> {
            Some(vec![1, 2, 3])
        }
        fn clip_image_set(&self, png: &[u8]) -> bool {
            self.0.lock().unwrap().push(format!("clipimg {}", png.len()));
            true
        }
        fn windows(&self) -> Vec<WindowInfo> {
            vec![WindowInfo {
                id: 42,
                title: "Notes".into(),
                exe: "notepad.exe".into(),
                icon: "0123456789abcdef".into(),
                minimized: false,
                maximized: false,
                foreground: true,
            }]
        }
        fn window_action(&self, id: u64, a: WindowAct) {
            self.0.lock().unwrap().push(format!("win {id} {a:?}"));
        }
        fn apps(&self) -> Vec<AppEntry> {
            vec![AppEntry { id: "fedcba9876543210".into(), name: "Calculator".into(), icon: "fedcba9876543210".into() }]
        }
        fn launch_app(&self, id: &str) {
            self.0.lock().unwrap().push(format!("launch {id}"));
        }
        fn icon_png(&self, key: &str) -> Option<Vec<u8>> {
            (key == "0123456789abcdef").then(|| vec![1, 2, 3])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Rec;
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn run(json: &str) -> Vec<String> {
        let rec = Arc::new(Rec::default());
        let d = Dispatcher::new(rec.clone());
        d.handle(&serde_json::from_str(json).expect("valid message"));
        rec.take()
    }

    #[test]
    fn mouse_messages() {
        assert_eq!(run(r#"{"t":"move","dx":3,"dy":-2}"#), ["move 3 -2"]);
        assert_eq!(run(r#"{"t":"move","dx":99999,"dy":-99999}"#), ["move 5000 -5000"]);
        assert_eq!(run(r#"{"t":"btn","b":"left","s":"click"}"#), ["btn Left true", "btn Left false"]);
        assert_eq!(run(r#"{"t":"btn","b":"right","s":"down"}"#), ["btn Right true"]);
        assert_eq!(run(r#"{"t":"btn","b":"middle","s":"up"}"#), ["btn Middle false"]);
        assert_eq!(run(r#"{"t":"btn","b":"left","s":"dbl"}"#).len(), 4);
    }

    #[test]
    fn smoothed_moves_arrive_in_pieces_and_a_click_never_overtakes_them() {
        let rec = Arc::new(Rec::default());
        let d = Dispatcher::with_smoothing(rec.clone(), 15);
        let total = |log: &[String]| -> (i32, i32) {
            log.iter().filter_map(|l| l.strip_prefix("move ")).fold((0, 0), |a, l| {
                let mut p = l.split(' ').map(|n| n.parse::<i32>().unwrap());
                (a.0 + p.next().unwrap(), a.1 + p.next().unwrap())
            })
        };
        d.handle(&serde_json::from_str(r#"{"t":"move","dx":60,"dy":-30}"#).unwrap());
        std::thread::sleep(std::time::Duration::from_millis(250));
        let log = rec.take();
        assert!(log.len() > 2, "the move should be spread over several steps: {log:?}");
        assert_eq!(total(&log), (60, -30));

        // a click right behind a move: the whole move lands first, then the click
        d.handle(&serde_json::from_str(r#"{"t":"move","dx":40,"dy":0}"#).unwrap());
        d.handle(&serde_json::from_str(r#"{"t":"btn","b":"left","s":"click"}"#).unwrap());
        let log = rec.take();
        let click = log.iter().position(|l| l == "btn Left true").expect("click present");
        assert_eq!(total(&log[..click]), (40, 0), "{log:?}");
        std::thread::sleep(std::time::Duration::from_millis(60));
        assert_eq!(total(&rec.take()), (0, 0), "nothing should trail the click");
    }

    #[test]
    fn scroll_direction_follows_dom_convention() {
        assert_eq!(run(r#"{"t":"scroll","dy":120}"#), ["wheel false -120"]);
        assert_eq!(run(r#"{"t":"scroll","dx":-60}"#), ["wheel true -60"]);
        assert!(run(r#"{"t":"scroll"}"#).is_empty());
    }

    #[test]
    fn keys() {
        assert_eq!(run(r#"{"t":"key","k":"ctrl+c"}"#), ["key 17 true", "key 67 true", "key 67 false", "key 17 false"]);
        assert_eq!(run(r#"{"t":"key","k":"shift","s":"down"}"#), ["key 16 true"]);
        assert_eq!(run(r#"{"t":"key","k":"f5","s":"up"}"#), ["key 116 false"]);
        assert_eq!(run(r#"{"t":"key","k":"ctrl++"}"#).len(), 4);
        assert_eq!(run(r#"{"t":"key","k":"enter"}"#), ["key 13 true", "key 13 false"]);
        assert!(run(r#"{"t":"key","k":"ctrl+bogus"}"#).is_empty());
    }

    #[test]
    fn text_is_typed_as_utf16_units() {
        assert_eq!(run(r#"{"t":"text","v":"hi"}"#), ["type 104", "type 105"]);
        assert_eq!(run(r#"{"t":"text","v":"😀"}"#).len(), 2); // surrogate pair
    }

    #[test]
    fn system_messages() {
        assert_eq!(run(r#"{"t":"power","a":"restart"}"#), ["power Restart"]);
        assert_eq!(
            run(r#"{"t":"open","url":"https://example.com/a?b=1&c=2"}"#),
            ["open https://example.com/a?b=1&c=2"]
        );
        for bad in [
            "file:///c:/windows/system32/calc.exe",
            "javascript:alert(1)",
            "https://a.com/ x",
            "ftp://x",
            "https://a.com/\u{0}",
        ] {
            let json = serde_json::json!({"t": "open", "url": bad}).to_string();
            assert!(run(&json).is_empty(), "{bad} must be refused");
        }
        assert_eq!(run(r#"{"t":"clip.set","text":"hello"}"#), ["clip hello"]);
    }

    #[test]
    fn clipboard_get_returns_reply() {
        let rec = Arc::new(Rec::default());
        let d = Dispatcher::new(rec);
        let r = d.handle(&serde_json::from_str(r#"{"t":"clip.get"}"#).unwrap()).unwrap();
        assert_eq!(r, serde_json::json!({"t": "clip", "text": "from pc", "image": {"w": 3, "h": 2}}));
    }

    #[test]
    fn clip_set_is_acknowledged() {
        let d = Dispatcher::new(Arc::new(Rec::default()));
        let ok = d.handle(&serde_json::from_str(r#"{"t":"clip.set","text":"hi"}"#).unwrap()).unwrap();
        assert_eq!(ok, serde_json::json!({"t": "clip.ack", "ok": true}));
        let failed = d.handle(&serde_json::from_str(r#"{"t":"clip.set","text":"fail"}"#).unwrap()).unwrap();
        assert_eq!(failed["ok"], false, "a backend failure must reach the phone");
        let huge = serde_json::json!({"t": "clip.set", "text": "x".repeat(300_000)}).to_string();
        let big = d.handle(&serde_json::from_str(&huge).unwrap()).unwrap();
        assert_eq!(big["ok"], false, "over the size cap is refused");
    }

    #[test]
    fn clip_image_upload_is_validated_before_the_backend_sees_it() {
        let rec = Arc::new(Rec::default());
        let d = Dispatcher::new(rec.clone());
        let png = crate::image::encode_rgba(2, 2, &[255u8; 16]).unwrap();
        assert_eq!(d.clip_image_set(&png), Ok((2, 2)));
        assert_eq!(rec.take(), [format!("clipimg {}", png.len())]);
        assert_eq!(d.clip_image_set(b"not an image at all"), Err("not a PNG image"));
        assert!(rec.take().is_empty(), "garbage must never reach the backend");
        assert_eq!(d.clip_image_png(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn window_switcher_messages() {
        let rec = Arc::new(Rec::default());
        let d = Dispatcher::new(rec.clone());
        let list = d.handle(&serde_json::from_str(r#"{"t":"win.list"}"#).unwrap()).unwrap();
        assert_eq!(list["t"], "win.list");
        assert_eq!(list["items"][0]["title"], "Notes");
        assert_eq!(list["items"][0]["id"], 42);
        assert_eq!(list["items"][0]["foreground"], true);

        assert_eq!(run(r#"{"t":"win.act","id":42,"a":"close"}"#), ["win 42 Close"]);
        assert_eq!(run(r#"{"t":"win.act","id":7,"a":"focus"}"#), ["win 7 Focus"]);
        assert!(serde_json::from_str::<Msg>(r#"{"t":"win.act","id":7,"a":"explode"}"#).is_err());
    }

    #[test]
    fn app_launcher_messages() {
        let rec = Arc::new(Rec::default());
        let d = Dispatcher::new(rec);
        let list = d.handle(&serde_json::from_str(r#"{"t":"app.list"}"#).unwrap()).unwrap();
        assert_eq!(list["items"][0]["name"], "Calculator");

        assert_eq!(run(r#"{"t":"app.launch","id":"fedcba9876543210"}"#), ["launch fedcba9876543210"]);
        // the client can only launch ids the server issued: anything shaped like a path is refused
        for bad in ["C:/Windows/System32/cmd.exe", "../../x", "FEDCBA9876543210", "fedcba98765432", ""] {
            let json = serde_json::json!({"t": "app.launch", "id": bad}).to_string();
            assert!(run(&json).is_empty(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn icon_keys_are_validated() {
        let d = Dispatcher::new(Arc::new(Rec::default()));
        assert_eq!(d.icon_png("0123456789abcdef"), Some(vec![1, 2, 3]));
        assert_eq!(d.icon_png("0123456789abcde"), None);
        assert_eq!(d.icon_png("../0123456789abcdef"), None);
        assert_eq!(d.icon_png("0123456789ABCDEF"), None);
    }

    fn ask(d: &Dispatcher<Arc<Rec>>, json: &str) -> serde_json::Value {
        d.handle(&serde_json::from_str(json).unwrap()).expect("a sleep reply")
    }

    #[test]
    fn sleep_timer_replies_with_the_t_key_and_puts_the_pc_to_sleep() {
        let rec = Arc::new(Rec::default());
        let d = Dispatcher::new(rec.clone());
        let r = ask(&d, r#"{"t":"sleep.set","secs":1}"#);
        assert_eq!(
            (r["t"].clone(), r["on"].clone(), r["episode"].clone()),
            ("sleep".into(), true.into(), false.into())
        );
        assert!(rec.take().is_empty(), "nothing happens before the time is up");
        std::thread::sleep(Duration::from_millis(1600));
        assert_eq!(rec.take(), ["power Sleep"]);
        assert_eq!(ask(&d, r#"{"t":"sleep.get"}"#)["on"], false);
    }

    #[test]
    fn sleep_timer_follows_the_end_of_the_episode_and_keeps_the_timer_on_a_bad_request() {
        let d = Dispatcher::new(Arc::new(Rec::default()));
        // the mock plays a 200 s item at 12 s: 188 s left plus the 2 s grace
        let r = ask(&d, r#"{"t":"sleep.set","episode":true}"#);
        assert_eq!((r["on"].clone(), r["episode"].clone()), (true.into(), true.into()));
        assert!((r["total"].as_f64().unwrap() - 190.0).abs() < 1.0);
        let r = ask(&d, r#"{"t":"sleep.set"}"#);
        assert_eq!((r["err"].clone(), r["on"].clone()), ("bad-time".into(), true.into()));
        assert_eq!(ask(&d, r#"{"t":"sleep.set","secs":99999999}"#)["total"], (12 * 3600) as f64);
        assert_eq!(ask(&d, r#"{"t":"sleep.cancel"}"#)["on"], false);
    }

    #[test]
    fn sleep_changes_and_the_end_reach_every_phone() {
        let d = Dispatcher::new(Arc::new(Rec::default()));
        let mut other = d.subscribe();
        ask(&d, r#"{"t":"sleep.set","secs":1}"#);
        let on: serde_json::Value = serde_json::from_str(&other.try_recv().unwrap()).unwrap();
        assert_eq!((on["t"].clone(), on["on"].clone()), ("sleep".into(), true.into()));
        std::thread::sleep(Duration::from_millis(1600));
        let off: serde_json::Value = serde_json::from_str(&other.try_recv().unwrap()).unwrap();
        assert_eq!(off["on"], false, "the phone is told when the timer fires");
    }

    #[test]
    fn unknown_message_fails_to_parse() {
        assert!(serde_json::from_str::<Msg>(r#"{"t":"nuke"}"#).is_err());
        assert!(serde_json::from_str::<Msg>(r#"{"t":"move","dx":"a","dy":1}"#).is_err());
    }
}
