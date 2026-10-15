//! Command layer: typed client messages and the dispatcher that turns them into input events.

use serde::{Deserialize, Serialize};

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
    Move { dx: i32, dy: i32 },
    Btn { b: Button, s: BtnAct },
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
    Text { v: String },
    Power { a: PowerAct },
    /// Open an http(s) URL in the PC's default browser.
    Open { url: String },
    /// Replace the PC clipboard text.
    #[serde(rename = "clip.set")]
    ClipSet { text: String },
    /// Ask for the PC clipboard text; answered with `{"t":"clip","text":...}`.
    #[serde(rename = "clip.get")]
    ClipGet,
    /// List open windows; answered with `{"t":"win.list","items":[...]}`.
    #[serde(rename = "win.list")]
    WinList,
    #[serde(rename = "win.act")]
    WinAct { id: u64, a: WindowAct },
    /// List launchable apps; answered with `{"t":"app.list","items":[...]}`.
    #[serde(rename = "app.list")]
    AppList,
    #[serde(rename = "app.launch")]
    AppLaunch { id: String },
    Ping,
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
    fn power(&self, _act: PowerAct) {}
    fn open_url(&self, _url: &str) {}
    fn clip_get(&self) -> Option<String> {
        None
    }
    fn clip_set(&self, _text: &str) {}
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

pub struct Dispatcher<I: Injector> {
    inj: I,
}

impl<I: Injector> Dispatcher<I> {
    pub fn new(inj: I) -> Self {
        Self { inj }
    }

    /// Apply a message; returns a reply for the client when the message asks for one.
    pub fn handle(&self, msg: &Msg) -> Option<serde_json::Value> {
        match msg {
            Msg::ClipGet => {
                return Some(serde_json::json!({"t": "clip", "text": self.inj.clip_get().unwrap_or_default()}));
            }
            Msg::WinList => return Some(serde_json::json!({"t": "win.list", "items": self.inj.windows()})),
            Msg::AppList => return Some(serde_json::json!({"t": "app.list", "items": self.inj.apps()})),
            _ => {}
        }
        match msg {
            &Msg::Move { dx, dy } => {
                self.inj.move_rel(dx.clamp(-MAX_MOVE, MAX_MOVE), dy.clamp(-MAX_MOVE, MAX_MOVE));
            }
            Msg::Btn { b, s } => match s {
                BtnAct::Down => self.inj.button(*b, true),
                BtnAct::Up => self.inj.button(*b, false),
                BtnAct::Click => self.click(*b),
                BtnAct::Dbl => {
                    self.click(*b);
                    self.click(*b);
                }
            },
            &Msg::Scroll { dx, dy } => {
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
            Msg::ClipSet { text } => {
                if text.len() <= MAX_CLIP_BYTES {
                    self.inj.clip_set(text);
                }
            }
            Msg::WinAct { id, a } => self.inj.window_action(*id, *a),
            Msg::AppLaunch { id } => {
                if is_icon_key(id) {
                    self.inj.launch_app(id);
                }
            }
            Msg::ClipGet | Msg::WinList | Msg::AppList | Msg::Ping => {}
        }
        None
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
        let vks: Vec<u8> = parse_combo(combo)
            .into_iter()
            .map(|n| vk_from_name(n, |c| self.inj.vk_for_char(c)))
            .collect();
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
        fn clip_set(&self, t: &str) {
            self.0.lock().unwrap().push(format!("clip {t}"));
        }
        fn windows(&self) -> Vec<WindowInfo> {
            vec![WindowInfo { id: 42, title: "Notes".into(), exe: "notepad.exe".into(), icon: "0123456789abcdef".into(), minimized: false, foreground: true }]
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
    fn scroll_direction_follows_dom_convention() {
        assert_eq!(run(r#"{"t":"scroll","dy":120}"#), ["wheel false -120"]);
        assert_eq!(run(r#"{"t":"scroll","dx":-60}"#), ["wheel true -60"]);
        assert!(run(r#"{"t":"scroll"}"#).is_empty());
    }

    #[test]
    fn keys() {
        assert_eq!(
            run(r#"{"t":"key","k":"ctrl+c"}"#),
            ["key 17 true", "key 67 true", "key 67 false", "key 17 false"]
        );
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
        assert_eq!(run(r#"{"t":"open","url":"https://example.com/a?b=1&c=2"}"#), ["open https://example.com/a?b=1&c=2"]);
        for bad in ["file:///c:/windows/system32/calc.exe", "javascript:alert(1)", "https://a.com/ x", "ftp://x", "https://a.com/\u{0}"] {
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
        assert_eq!(r, serde_json::json!({"t": "clip", "text": "from pc"}));
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

    #[test]
    fn unknown_message_fails_to_parse() {
        assert!(serde_json::from_str::<Msg>(r#"{"t":"nuke"}"#).is_err());
        assert!(serde_json::from_str::<Msg>(r#"{"t":"move","dx":"a","dy":1}"#).is_err());
    }
}
