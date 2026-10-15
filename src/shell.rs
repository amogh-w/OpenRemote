//! Process shell: console attach, error dialogs and the system-tray icon.
//!
//! The release build is a GUI-subsystem program (no console window). `attach_console` reconnects
//! stdout/stderr when it is started from a terminal so `--help` and logs still show up there.

#[cfg(windows)]
mod imp {
    use std::mem::zeroed;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::ptr::{null, null_mut};
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{
        AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MessageBoxW, TranslateMessage, MB_ICONERROR, MB_OK, MSG,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const TRAY_ICON_RGBA: &[u8] = include_bytes!("../assets/tray-32.rgba");

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn attach_console() {
        // SAFETY: plain console/handle calls; failure just means there is no parent console.
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                return;
            }
            let h = CreateFileW(wide("CONOUT$").as_ptr(), GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut());
            if h != INVALID_HANDLE_VALUE {
                SetStdHandle(STD_OUTPUT_HANDLE, h);
                SetStdHandle(STD_ERROR_HANDLE, h);
            }
        }
    }

    pub fn error_box(msg: &str) {
        // SAFETY: both strings are NUL-terminated and outlive the call.
        unsafe {
            MessageBoxW(null_mut(), wide(msg).as_ptr(), wide("OpenRemote").as_ptr(), MB_OK | MB_ICONERROR);
        }
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

    /// Show the tray icon and run the Win32 message loop on this thread until the process exits.
    pub fn run_tray(admin_url: String) -> Result<(), String> {
        let icon = Icon::from_rgba(TRAY_ICON_RGBA.to_vec(), 32, 32).map_err(|e| e.to_string())?;
        let show = MenuItem::new("Show phone link", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit]).map_err(|e| e.to_string())?;
        let _tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("OpenRemote")
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;

        let (show_id, quit_id) = (show.id().clone(), quit.id().clone());
        let url = admin_url.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == show_id {
                open_in_browser(&url);
            } else if e.id == quit_id {
                std::process::exit(0);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::DoubleClick { .. } = e {
                open_in_browser(&admin_url);
            }
        }));

        // The tray's hidden window delivers its events through this loop.
        // SAFETY: standard message pump on the thread that created the tray icon.
        unsafe {
            let mut msg: MSG = zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use imp::{attach_console, error_box, open_in_browser, run_tray};

#[cfg(not(windows))]
mod imp {
    pub fn attach_console() {}
    pub fn error_box(msg: &str) {
        eprintln!("{msg}");
    }
    pub fn open_in_browser(url: &str) {
        eprintln!("open {url} in a browser");
    }
    pub fn run_tray(_admin_url: String) -> Result<(), String> {
        loop {
            std::thread::park();
        }
    }
}

#[cfg(not(windows))]
pub use imp::{attach_console, error_box, open_in_browser, run_tray};
