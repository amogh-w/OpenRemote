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
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows_sys::Win32::System::Console::{
        AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MessageBoxW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MSG, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
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

    pub fn info_box(msg: &str) {
        // SAFETY: both strings are NUL-terminated and outlive the call.
        unsafe {
            MessageBoxW(null_mut(), wide(msg).as_ptr(), wide("OpenRemote").as_ptr(), MB_OK | MB_ICONINFORMATION);
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
        let show = MenuItem::new("Pair a phone", true, None);
        let auto = CheckMenuItem::new("Start with Windows", true, crate::autostart::enabled(), None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[&show, &auto, &PredefinedMenuItem::separator(), &quit]).map_err(|e| e.to_string())?;
        let _tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("OpenRemote")
            .with_icon(icon)
            .build()
            .map_err(|e| e.to_string())?;

        let (show_id, auto_id, quit_id) = (show.id().clone(), auto.id().clone(), quit.id().clone());
        let admin_url_menu = admin_url.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::DoubleClick { .. } = e {
                open_in_browser(&admin_url);
            }
        }));

        // The tray's hidden window delivers its events through this loop. Menu clicks are read from the channel
        // here, on this thread, because the menu items cannot be touched from another one.
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
                    if e.id == show_id {
                        open_in_browser(&admin_url_menu);
                    } else if e.id == auto_id {
                        // the check mark has already flipped; make the registry match, or undo it
                        let want = auto.is_checked();
                        if let Err(err) = crate::autostart::set(want) {
                            auto.set_checked(!want);
                            error_box(&err);
                        }
                    } else if e.id == quit_id {
                        std::process::exit(0);
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
pub use imp::{attach_console, error_box, info_box, open_in_browser, run_tray};

#[cfg(not(windows))]
mod imp {
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
    pub fn run_tray(_admin_url: String) -> Result<(), String> {
        loop {
            std::thread::park();
        }
    }
}

#[cfg(not(windows))]
pub use imp::{attach_console, error_box, info_box, open_in_browser, run_tray};
