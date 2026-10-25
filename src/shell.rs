//! Process shell: console attach, message boxes, single instance, restart, small desktop helpers and the system-tray
//! icon. Shared by OpenRemote and Kai Remote: edit it in the cuppa plugin repo (shared/) and run
//! `harness.mjs sync --write`, never in a project copy. Each crate defines `APP_NAME` in its `lib.rs`.
//!
//! The release builds are GUI-subsystem programs (no console window). `attach_console` reconnects stdout/stderr when
//! the program is started from a terminal so `--help` and logs still show up there.

use std::path::Path;

#[cfg(windows)]
mod imp {
    use super::Path;
    use crate::admin::AdminHost;
    use std::mem::zeroed;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::ptr::{null, null_mut};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{
        AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MessageBoxW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MB_ICONERROR,
        MB_ICONINFORMATION, MB_OK, MSG, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn attach_console() {
        // SAFETY: plain console/handle calls; failure just means there is no parent console.
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            let h = CreateFileW(
                wide("CONOUT$").as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            );
            if h != INVALID_HANDLE_VALUE {
                SetStdHandle(STD_OUTPUT_HANDLE, h);
                SetStdHandle(STD_ERROR_HANDLE, h);
            }
        }
    }

    fn message_box(msg: &str, icon: u32) {
        // SAFETY: both strings are NUL-terminated and outlive the call.
        unsafe {
            MessageBoxW(null_mut(), wide(msg).as_ptr(), wide(crate::APP_NAME).as_ptr(), MB_OK | icon);
        }
    }

    pub fn error_box(msg: &str) {
        message_box(msg, MB_ICONERROR);
    }

    pub fn info_box(msg: &str) {
        message_box(msg, MB_ICONINFORMATION);
    }

    /// Open a URL in the default browser (single argument to url.dll, never through a shell).
    pub fn open_in_browser(url: &str) {
        let _ = Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    /// Show a folder in Explorer.
    pub fn open_folder(path: &Path) {
        let _ = Command::new("explorer.exe").arg(path).spawn();
    }

    /// Put text on the clipboard through `clip.exe` (no extra dependency; the text is a URL, so ASCII).
    pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
        use std::io::Write;
        let mut child = Command::new("clip.exe")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        child.wait().map(|_| ()).map_err(|e| e.to_string())
    }

    /// True when this is the only copy on this `port` for this user. With `wait_secs` > 0 (a restart) it waits for the old
    /// copy to go away first. The lock lives as long as the process.
    pub fn single_instance(port: u16, wait_secs: u64) -> bool {
        let name = wide(&format!("Local\\{}-{port}", crate::APP_NAME.replace(' ', "")));
        let start = Instant::now();
        loop {
            // SAFETY: the name is NUL-terminated; the handle is closed here or kept for the life of the process.
            let (handle, exists) = unsafe {
                let h = CreateMutexW(null(), 0, name.as_ptr());
                (h, GetLastError() == ERROR_ALREADY_EXISTS)
            };
            if !handle.is_null() && !exists {
                return true; // deliberately not closed: closing it would release the lock
            }
            if !handle.is_null() {
                // SAFETY: `handle` was just returned by CreateMutexW.
                unsafe {
                    CloseHandle(handle);
                }
            }
            if start.elapsed() >= Duration::from_secs(wait_secs) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Start a fresh copy of this program (the new one waits for this one to go away) and exit.
    pub fn restart() -> ! {
        if let Ok(exe) = std::env::current_exe() {
            let args = std::env::args().skip(1).filter(|a| a != "--restarted").chain(["--restarted".to_string()]);
            let _ = Command::new(exe).args(args).creation_flags(CREATE_NO_WINDOW).spawn();
        }
        std::process::exit(0);
    }

    fn tooltip<H: AdminHost>(host: &H) -> String {
        let n = host.pairing().devices().len();
        let paused = if host.pairing().paused() { ", pairing paused" } else { "" };
        format!(
            "{}: {} paired{paused}",
            host.name(),
            if n == 1 { "1 phone".to_string() } else { format!("{n} phones") }
        )
    }

    /// Show the tray icon and run the Win32 message loop on this thread until the process exits.
    pub fn run_tray<H: AdminHost>(host: Arc<H>, icon_rgba: Vec<u8>, dashboard_url: String) -> Result<(), String> {
        let icon = Icon::from_rgba(icon_rgba, 32, 32).map_err(|e| e.to_string())?;
        let open = MenuItem::new("Open dashboard", true, None);
        let pair = MenuItem::new("Pair a phone", true, None);
        let copy = MenuItem::new("Copy phone link", true, None);
        let pause = CheckMenuItem::new("Pause pairing", true, host.pairing().paused(), None);
        let auto = CheckMenuItem::new("Start with Windows", true, crate::autostart::enabled(), None);
        let logs = MenuItem::new("Open log folder", true, None);
        let again = MenuItem::new("Restart", true, None);
        let quit = MenuItem::new("Exit", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &open,
            &pair,
            &copy,
            &PredefinedMenuItem::separator(),
            &pause,
            &auto,
            &PredefinedMenuItem::separator(),
            &logs,
            &again,
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(tooltip(&*host))
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;

        let (open_id, pair_id, copy_id, pause_id) =
            (open.id().clone(), pair.id().clone(), copy.id().clone(), pause.id().clone());
        let (auto_id, logs_id, again_id, quit_id) =
            (auto.id().clone(), logs.id().clone(), again.id().clone(), quit.id().clone());
        let double_click_url = dashboard_url.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::DoubleClick { .. } = e {
                open_in_browser(&double_click_url);
            }
        }));

        // The tray's hidden window delivers its events through this loop. Menu clicks are read from the channel
        // here, on this thread, because the menu items cannot be touched from another one.
        let mut last_tip = Instant::now();
        // SAFETY: standard message pump on the thread that created the tray icon.
        unsafe {
            let mut msg: MSG = zeroed();
            loop {
                MsgWaitForMultipleObjects(0, null(), 0, 250, QS_ALLINPUT);
                while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                    if msg.message == WM_QUIT {
                        return Ok(());
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                while let Ok(e) = MenuEvent::receiver().try_recv() {
                    if e.id == open_id {
                        open_in_browser(&dashboard_url);
                    } else if e.id == pair_id {
                        open_in_browser(&format!("{dashboard_url}#add"));
                    } else if e.id == copy_id {
                        match host.phone_urls().first() {
                            Some(url) => {
                                if let Err(err) = copy_to_clipboard(url) {
                                    error_box(&format!("Could not copy the link: {err}"));
                                }
                            }
                            None => info_box("No network address found. Connect this PC to a network."),
                        }
                    } else if e.id == pause_id {
                        host.pairing().set_paused(pause.is_checked());
                    } else if e.id == auto_id {
                        // the check mark has already flipped; make the registry match, or undo it
                        let want = auto.is_checked();
                        if let Err(err) = crate::autostart::set(want) {
                            auto.set_checked(!want);
                            error_box(&err);
                        }
                    } else if e.id == logs_id {
                        open_folder(&host.log_dir());
                    } else if e.id == again_id {
                        host.restart();
                    } else if e.id == quit_id {
                        std::process::exit(0);
                    }
                }
                // the dashboard can change these too: keep the menu and the tooltip in step
                if pause.is_checked() != host.pairing().paused() {
                    pause.set_checked(host.pairing().paused());
                }
                if last_tip.elapsed() >= Duration::from_secs(4) {
                    let _ = tray.set_tooltip(Some(tooltip(&*host)));
                    last_tip = Instant::now();
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Path;
    use crate::admin::AdminHost;
    use std::sync::Arc;

    pub fn attach_console() {}
    pub fn error_box(msg: &str) {
        eprintln!("{msg}");
    }
    pub fn info_box(msg: &str) {
        eprintln!("{msg}");
    }
    pub fn open_in_browser(url: &str) {
        eprintln!("open {url} in a browser");
    }
    pub fn open_folder(path: &Path) {
        eprintln!("open {}", path.display());
    }
    pub fn copy_to_clipboard(_text: &str) -> Result<(), String> {
        Err("the clipboard helper is only available on Windows".into())
    }
    pub fn single_instance(_port: u16, _wait_secs: u64) -> bool {
        true
    }
    pub fn restart() -> ! {
        std::process::exit(0)
    }
    pub fn run_tray<H: AdminHost>(_host: Arc<H>, _icon_rgba: Vec<u8>, _dashboard_url: String) -> Result<(), String> {
        loop {
            std::thread::park();
        }
    }
}

pub use imp::{
    attach_console, copy_to_clipboard, error_box, info_box, open_folder, open_in_browser, restart, run_tray,
    single_instance,
};
