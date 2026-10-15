//! Platform input injection. Windows uses `SendInput`; other platforms get a logging stub.

#[cfg(windows)]
pub use win::WinInjector as PlatformInjector;

#[cfg(not(windows))]
pub use stub::LogInjector as PlatformInjector;

#[cfg(windows)]
mod win {
    use crate::command::{AppEntry, Button, Injector, PowerAct, WindowAct, WindowInfo};
    use std::mem::size_of;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

    #[derive(Default)]
    pub struct WinInjector {
        reg: crate::winapps::Registry,
    }

    fn send(input: INPUT) {
        // SAFETY: `input` is a fully initialised INPUT and the size matches the struct.
        unsafe {
            SendInput(1, &input, size_of::<INPUT>() as i32);
        }
    }

    fn mouse(flags: u32, dx: i32, dy: i32, data: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        }
    }

    fn keybd(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        }
    }

    /// Keys that must carry KEYEVENTF_EXTENDEDKEY to act as the navigation/Win keys.
    fn is_extended(vk: u8) -> bool {
        matches!(vk, 33..=40 | 45 | 46 | 91 | 92 | 163 | 165)
    }

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// Run a clipboard operation, retrying while another program holds the clipboard open (clipboard
    /// managers and remote tools do this for a few milliseconds). Any other error gives up at once.
    fn clipboard<T>(op: impl Fn(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Option<T> {
        for _ in 0..8 {
            match arboard::Clipboard::new() {
                Ok(mut c) => match op(&mut c) {
                    Ok(v) => return Some(v),
                    Err(arboard::Error::ClipboardOccupied) => {}
                    Err(_) => return None,
                },
                Err(_) => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        None
    }

    /// Fire-and-forget child process with no console window.
    fn spawn_hidden(program: &str, args: &[&str]) {
        let _ = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    impl Injector for WinInjector {
        fn power(&self, act: PowerAct) {
            match act {
                PowerAct::Lock => spawn_hidden("rundll32.exe", &["user32.dll,LockWorkStation"]),
                PowerAct::Sleep => spawn_hidden("rundll32.exe", &["powrprof.dll,SetSuspendState", "0,1,0"]),
                PowerAct::Hibernate => spawn_hidden("shutdown.exe", &["/h"]),
                PowerAct::Logoff => spawn_hidden("shutdown.exe", &["/l"]),
                PowerAct::Restart => spawn_hidden("shutdown.exe", &["/r", "/t", "0"]),
                PowerAct::Shutdown => spawn_hidden("shutdown.exe", &["/s", "/t", "0"]),
            }
        }

        /// The URL is passed as a single argv entry to url.dll, never through a shell.
        fn open_url(&self, url: &str) {
            spawn_hidden("rundll32.exe", &["url.dll,FileProtocolHandler", url]);
        }

        fn windows(&self) -> Vec<WindowInfo> {
            self.reg.windows()
        }

        fn window_action(&self, id: u64, act: WindowAct) {
            crate::winapps::window_action(id, act);
        }

        fn apps(&self) -> Vec<AppEntry> {
            self.reg.apps()
        }

        fn launch_app(&self, id: &str) {
            self.reg.launch(id);
        }

        fn icon_png(&self, key: &str) -> Option<Vec<u8>> {
            self.reg.icon_png(key)
        }

        fn clip_get(&self) -> Option<String> {
            clipboard(|c| c.get_text())
        }

        fn clip_set(&self, text: &str) -> bool {
            clipboard(|c| c.set_text(text.to_owned())).is_some()
        }

        fn media(&self) -> Option<crate::command::MediaInfo> {
            crate::media::info()
        }

        fn media_art(&self) -> Option<Vec<u8>> {
            crate::media::art()
        }

        fn clip_image_info(&self) -> Option<(u32, u32)> {
            clipboard(|c| c.get_image()).map(|i| (i.width as u32, i.height as u32))
        }

        fn clip_image_png(&self) -> Option<Vec<u8>> {
            let img = clipboard(|c| c.get_image())?;
            crate::image::encode_rgba(img.width as u32, img.height as u32, &img.bytes)
        }

        fn clip_image_set(&self, png: &[u8]) -> bool {
            let Ok((w, h, rgba)) = crate::image::decode_rgba(png) else { return false };
            clipboard(|c| {
                c.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: std::borrow::Cow::Borrowed(&rgba) })
            })
            .is_some()
        }

        fn move_rel(&self, dx: i32, dy: i32) {
            send(mouse(MOUSEEVENTF_MOVE, dx, dy, 0));
        }

        fn button(&self, b: Button, down: bool) {
            let flags = match (b, down) {
                (Button::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (Button::Left, false) => MOUSEEVENTF_LEFTUP,
                (Button::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (Button::Right, false) => MOUSEEVENTF_RIGHTUP,
                (Button::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                (Button::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            };
            send(mouse(flags, 0, 0, 0));
        }

        fn wheel(&self, horizontal: bool, delta: i32) {
            let flag = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
            send(mouse(flag, 0, 0, delta));
        }

        fn key(&self, vk: u8, down: bool) {
            if vk == 0 {
                return;
            }
            let mut flags = if down { 0 } else { KEYEVENTF_KEYUP };
            if is_extended(vk) {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            send(keybd(vk as u16, 0, flags));
        }

        fn type_unit(&self, unit: u16) {
            send(keybd(0, unit, KEYEVENTF_UNICODE));
            send(keybd(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
        }

        fn vk_for_char(&self, c: char) -> u8 {
            let mut buf = [0u16; 2];
            let unit = c.encode_utf16(&mut buf)[0];
            // SAFETY: plain FFI call with a value argument.
            let r = unsafe { VkKeyScanW(unit) };
            if r == -1 {
                0
            } else {
                (r & 0xff) as u8
            }
        }
    }
}

#[cfg(not(windows))]
mod stub {
    use crate::command::{Button, Injector};

    #[derive(Default)]
    pub struct LogInjector;

    impl Injector for LogInjector {
        fn move_rel(&self, dx: i32, dy: i32) {
            eprintln!("[stub] move {dx} {dy}");
        }
        fn button(&self, b: Button, down: bool) {
            eprintln!("[stub] button {b:?} down={down}");
        }
        fn wheel(&self, h: bool, d: i32) {
            eprintln!("[stub] wheel horizontal={h} {d}");
        }
        fn key(&self, vk: u8, down: bool) {
            eprintln!("[stub] key {vk} down={down}");
        }
        fn type_unit(&self, u: u16) {
            eprintln!("[stub] type {u}");
        }
        fn vk_for_char(&self, c: char) -> u8 {
            c.to_ascii_uppercase() as u8
        }
    }
}
