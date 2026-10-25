//! Input on the UAC prompt, in administrator windows and on the lock screen.
//!
//! Windows blocks `SendInput` from a normal program in two cases: the target window runs elevated, or the
//! target is on the secure desktop (the UAC prompt, the lock screen). Only a SYSTEM process running in the
//! user's session can reach those. So there are three small pieces, all inside this one binary:
//!
//! - **the service** (`--service`, LocalSystem): keeps one helper process alive in the active console session;
//! - **the helper** (`--input-helper`, SYSTEM token in the user's session): reads input events from a named
//!   pipe and injects them on whatever desktop currently receives input;
//! - **the link** ([`HelperLink`], inside the normal app): forwards input events to the helper when it is
//!   there, and the app falls back to plain `SendInput` when it is not.
//!
//! The helper is optional and off until the user installs it (`--install-helper --yes`, needs administrator
//! approval on the PC). The pipe only accepts the signed-in user and SYSTEM, and never remote clients.

/// Name of the pipe the helper listens on.
pub const PIPE_NAME: &str = r"\\.\pipe\openremote-input";
/// Name of the Windows service.
pub const SERVICE_NAME: &str = "OpenRemoteHelper";
/// Size of one event on the wire.
pub const FRAME: usize = 9;

/// One input event, as sent from the app to the helper: a tag byte and two little-endian `i32` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Move {
        dx: i32,
        dy: i32,
    },
    /// `b`: 0 left, 1 right, 2 middle.
    Button {
        b: u8,
        down: bool,
    },
    Wheel {
        horizontal: bool,
        delta: i32,
    },
    Key {
        vk: u8,
        down: bool,
    },
    /// A UTF-16 unit typed as text.
    Unit(u16),
    /// Sent to check that the helper is there; does nothing.
    Ping,
}

impl Event {
    pub fn encode(self) -> [u8; FRAME] {
        let (tag, a, b): (u8, i32, i32) = match self {
            Event::Move { dx, dy } => (1, dx, dy),
            Event::Button { b, down } => (2, b as i32, down as i32),
            Event::Wheel { horizontal, delta } => (3, horizontal as i32, delta),
            Event::Key { vk, down } => (4, vk as i32, down as i32),
            Event::Unit(u) => (5, u as i32, 0),
            Event::Ping => (6, 0, 0),
        };
        let mut f = [0u8; FRAME];
        f[0] = tag;
        f[1..5].copy_from_slice(&a.to_le_bytes());
        f[5..9].copy_from_slice(&b.to_le_bytes());
        f
    }

    /// Parse one frame; anything out of range is rejected, so a garbled pipe cannot inject odd input.
    pub fn decode(f: &[u8; FRAME]) -> Option<Event> {
        let a = i32::from_le_bytes([f[1], f[2], f[3], f[4]]);
        let b = i32::from_le_bytes([f[5], f[6], f[7], f[8]]);
        let flag = |v: i32| match v {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        Some(match f[0] {
            1 => Event::Move { dx: a, dy: b },
            2 if (0..=2).contains(&a) => Event::Button { b: a as u8, down: flag(b)? },
            3 => Event::Wheel { horizontal: flag(a)?, delta: b },
            4 if (0..=255).contains(&a) => Event::Key { vk: a as u8, down: flag(b)? },
            5 if (0..=65535).contains(&a) => Event::Unit(a as u16),
            6 => Event::Ping,
            _ => return None,
        })
    }
}

/// Only a plain SID string may go into the pipe's access rule: digits, dashes and the leading `S`.
pub fn is_sid_string(s: &str) -> bool {
    s.starts_with("S-1-") && s.len() <= 184 && s.chars().all(|c| c.is_ascii_digit() || c == '-' || c == 'S')
}

#[cfg(windows)]
pub use imp::{cli, HelperLink};

#[cfg(not(windows))]
pub use stub::{cli, HelperLink};

#[cfg(not(windows))]
mod stub {
    use super::Event;

    #[derive(Default)]
    pub struct HelperLink;

    impl HelperLink {
        pub fn send(&self, _ev: Event) -> bool {
            false
        }
        pub fn active(&self) -> bool {
            false
        }
    }

    pub fn cli(_args: &[String]) -> Option<i32> {
        None
    }
}

#[cfg(windows)]
mod imp {
    use super::{is_sid_string, Event, FRAME, PIPE_NAME, SERVICE_NAME};
    use std::ffi::c_void;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::mem::{size_of, zeroed};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_PIPE_CONNECTED, GENERIC_ALL, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        DuplicateTokenEx, GetTokenInformation, SecurityImpersonation, TokenPrimary, TokenUser, SECURITY_ATTRIBUTES,
        TOKEN_DUPLICATE, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::RemoteDesktop::{
        ProcessIdToSessionId, WTSGetActiveConsoleSessionId, WTSQueryUserToken,
    };
    use windows_sys::Win32::System::Services::{
        RegisterServiceCtrlHandlerW, SetServiceStatus, StartServiceCtrlDispatcherW, SERVICE_ACCEPT_SHUTDOWN,
        SERVICE_ACCEPT_STOP, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
        SERVICE_STATUS, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
    };
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, SetThreadDesktop};
    use windows_sys::Win32::System::Threading::{
        CreateProcessAsUserW, GetExitCodeProcess, OpenProcess, OpenProcessToken, TerminateProcess, WaitForSingleObject,
        CREATE_NO_WINDOW, INFINITE, PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION, STARTUPINFOW,
    };
    use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // ------------------------------------------------------------------ the link, inside the normal app

    struct LinkState {
        pipe: Option<std::fs::File>,
        next_try: Instant,
    }

    /// Forwards input events to the helper. Reconnects at most once a second, so having no helper costs
    /// one failed `open` per second and nothing per event.
    pub struct HelperLink(Mutex<LinkState>);

    impl Default for HelperLink {
        fn default() -> Self {
            Self(Mutex::new(LinkState { pipe: None, next_try: Instant::now() }))
        }
    }

    impl HelperLink {
        /// True when the event reached the helper, false when the caller should inject it itself.
        pub fn send(&self, ev: Event) -> bool {
            let mut st = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if st.pipe.is_none() {
                if Instant::now() < st.next_try {
                    return false;
                }
                st.next_try = Instant::now() + Duration::from_secs(1);
                st.pipe = OpenOptions::new().write(true).open(PIPE_NAME).ok();
            }
            let Some(f) = st.pipe.as_mut() else { return false };
            if f.write_all(&ev.encode()).is_ok() {
                true
            } else {
                st.pipe = None;
                false
            }
        }

        /// Is the helper running and connected?
        pub fn active(&self) -> bool {
            self.send(Event::Ping)
        }
    }

    // ------------------------------------------------------------------ the helper process

    /// Make this thread use the desktop that currently gets input (the secure desktop while a UAC prompt is up).
    fn attach_input_desktop() {
        // SAFETY: OpenInputDesktop returns an owned handle or null; the thread keeps the desktop alive after
        // SetThreadDesktop, so the handle can be closed straight away.
        unsafe {
            let d = OpenInputDesktop(0, 0, GENERIC_ALL);
            if !d.is_null() {
                SetThreadDesktop(d);
                CloseDesktop(d);
            }
        }
    }

    fn make_pipe(allow_sid: Option<&str>) -> Result<HANDLE, String> {
        // SYSTEM and the signed-in user only. "OW" (owner) stands in when the helper is started by hand.
        let who = match allow_sid {
            Some(s) if is_sid_string(s) => s,
            Some(_) => return Err("bad SID".into()),
            None => "OW",
        };
        let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{who})"));
        let mut sd: *mut c_void = null_mut();
        // SAFETY: `sddl` is NUL-terminated; on success `sd` is a LocalAlloc'd descriptor we free below.
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut())
                == 0
            {
                return Err(format!("cannot build the pipe access rule (error {})", GetLastError()));
            }
            let sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd,
                bInheritHandle: 0,
            };
            let pipe = CreateNamedPipeW(
                wide(PIPE_NAME).as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                512,
                512,
                0,
                &sa,
            );
            LocalFree(sd);
            if pipe == INVALID_HANDLE_VALUE {
                return Err(format!("cannot create the pipe (error {}); is another helper running?", GetLastError()));
            }
            Ok(pipe)
        }
    }

    /// Read exactly one frame; false when the client went away.
    fn read_frame(pipe: HANDLE, frame: &mut [u8; FRAME]) -> bool {
        let mut got = 0usize;
        while got < FRAME {
            let mut n = 0u32;
            // SAFETY: the buffer slice is valid for `FRAME - got` bytes; no OVERLAPPED is used (blocking pipe).
            let ok = unsafe { ReadFile(pipe, frame[got..].as_mut_ptr(), (FRAME - got) as u32, &mut n, null_mut()) };
            if ok == 0 || n == 0 {
                return false;
            }
            got += n as usize;
        }
        true
    }

    pub fn run_helper(allow_sid: Option<&str>) -> Result<(), String> {
        let pipe = make_pipe(allow_sid)?;
        loop {
            // SAFETY: `pipe` is a valid server handle for the whole loop.
            let connected =
                unsafe { ConnectNamedPipe(pipe, null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED };
            if !connected {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            let mut last_attach = Instant::now() - Duration::from_secs(1);
            let mut frame = [0u8; FRAME];
            while read_frame(pipe, &mut frame) {
                let Some(ev) = Event::decode(&frame) else { continue };
                if ev == Event::Ping {
                    continue;
                }
                if last_attach.elapsed() > Duration::from_millis(200) {
                    attach_input_desktop();
                    last_attach = Instant::now();
                }
                crate::input::inject(&ev);
            }
            // SAFETY: valid handle; frees the instance for the next client.
            unsafe {
                DisconnectNamedPipe(pipe);
            }
        }
    }

    // ------------------------------------------------------------------ the service

    static STOP: AtomicBool = AtomicBool::new(false);
    static STATUS_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

    fn set_status(state: u32) {
        let s = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: if state == SERVICE_RUNNING {
                SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
            } else {
                0
            },
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: if state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING { 5000 } else { 0 },
        };
        // SAFETY: the handle comes from RegisterServiceCtrlHandlerW; `s` is fully initialised.
        unsafe {
            SetServiceStatus(STATUS_HANDLE.load(Ordering::SeqCst), &s);
        }
    }

    unsafe extern "system" fn service_handler(control: u32) {
        if control == SERVICE_CONTROL_STOP || control == SERVICE_CONTROL_SHUTDOWN {
            STOP.store(true, Ordering::SeqCst);
            set_status(SERVICE_STOP_PENDING);
        }
    }

    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
        let h = RegisterServiceCtrlHandlerW(wide(SERVICE_NAME).as_ptr(), Some(service_handler));
        if h.is_null() {
            return;
        }
        STATUS_HANDLE.store(h, Ordering::SeqCst);
        set_status(SERVICE_RUNNING);
        supervise();
        set_status(SERVICE_STOPPED);
    }

    /// The SID string of the user signed in to `session`, or `None` when nobody is.
    fn session_user_sid(session: u32) -> Option<String> {
        // SAFETY: standard token queries; every handle and buffer is released before returning.
        unsafe {
            let mut token: HANDLE = null_mut();
            if WTSQueryUserToken(session, &mut token) == 0 {
                return None;
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as *mut c_void, len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return None;
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut p: *mut u16 = null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut p) == 0 || p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            LocalFree(p as *mut c_void);
            Some(s)
        }
    }

    /// A SYSTEM primary token placed in `session`, copied from that session's winlogon.exe.
    fn system_token_for(session: u32) -> Option<HANDLE> {
        // SAFETY: Toolhelp snapshot walk plus token duplication; handles are closed on every path.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut pe: PROCESSENTRY32W = zeroed();
            pe.dwSize = size_of::<PROCESSENTRY32W>() as u32;
            let mut pid = 0u32;
            let mut more = Process32FirstW(snap, &mut pe) != 0;
            while more {
                let end = pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(pe.szExeFile.len());
                let name = String::from_utf16_lossy(&pe.szExeFile[..end]);
                if name.eq_ignore_ascii_case("winlogon.exe") {
                    let mut s = 0u32;
                    if ProcessIdToSessionId(pe.th32ProcessID, &mut s) != 0 && s == session {
                        pid = pe.th32ProcessID;
                        break;
                    }
                }
                more = Process32NextW(snap, &mut pe) != 0;
            }
            CloseHandle(snap);
            if pid == 0 {
                return None;
            }
            let proc = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
            if proc.is_null() {
                return None;
            }
            let mut token: HANDLE = null_mut();
            let ok = OpenProcessToken(proc, TOKEN_DUPLICATE | TOKEN_QUERY, &mut token);
            CloseHandle(proc);
            if ok == 0 {
                return None;
            }
            let mut dup: HANDLE = null_mut();
            let ok = DuplicateTokenEx(
                token,
                0x0200_0000, /* MAXIMUM_ALLOWED */
                null(),
                SecurityImpersonation,
                TokenPrimary,
                &mut dup,
            );
            CloseHandle(token);
            if ok == 0 {
                None
            } else {
                Some(dup)
            }
        }
    }

    /// Start the helper in `session` with a SYSTEM token. Returns the process handle.
    fn spawn_helper(exe: &str, session: u32) -> Option<HANDLE> {
        let sid = session_user_sid(session)?;
        let token = system_token_for(session)?;
        let mut cmd = wide(&format!("\"{exe}\" --input-helper --allow-sid {sid}"));
        let mut desktop = wide("winsta0\\default");
        // SAFETY: `cmd` and `desktop` are writable NUL-terminated buffers that outlive the call; the token is
        // closed afterwards and only the process handle is kept.
        unsafe {
            let mut si: STARTUPINFOW = zeroed();
            si.cb = size_of::<STARTUPINFOW>() as u32;
            si.lpDesktop = desktop.as_mut_ptr();
            let mut pi: PROCESS_INFORMATION = zeroed();
            let ok = CreateProcessAsUserW(
                token,
                null(),
                cmd.as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_NO_WINDOW,
                null(),
                null(),
                &si,
                &mut pi,
            );
            CloseHandle(token);
            if ok == 0 {
                return None;
            }
            CloseHandle(pi.hThread);
            Some(pi.hProcess)
        }
    }

    /// Keep exactly one helper alive in the active console session until the service is told to stop.
    fn supervise() {
        let Ok(exe) = std::env::current_exe() else { return };
        let exe = exe.display().to_string();
        let mut child: Option<(HANDLE, u32)> = None;
        while !STOP.load(Ordering::SeqCst) {
            // SAFETY: plain calls on handles this function owns.
            unsafe {
                let session = WTSGetActiveConsoleSessionId();
                if let Some((h, s)) = child {
                    let dead = WaitForSingleObject(h, 0) == WAIT_OBJECT_0;
                    if dead || s != session {
                        if !dead {
                            TerminateProcess(h, 0);
                        }
                        CloseHandle(h);
                        child = None;
                    }
                }
                if child.is_none() && session != 0xFFFF_FFFF {
                    child = spawn_helper(&exe, session).map(|h| (h, session));
                }
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        if let Some((h, _)) = child {
            // SAFETY: the handle is still owned here.
            unsafe {
                TerminateProcess(h, 0);
                CloseHandle(h);
            }
        }
    }

    fn run_service() -> Result<(), String> {
        let mut name = wide(SERVICE_NAME);
        let table = [
            SERVICE_TABLE_ENTRYW { lpServiceName: name.as_mut_ptr(), lpServiceProc: Some(service_main) },
            SERVICE_TABLE_ENTRYW { lpServiceName: null_mut(), lpServiceProc: None },
        ];
        // SAFETY: the table is NUL-terminated by its last entry and outlives the call (which blocks until the service ends).
        if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
            return Err("this mode is for the Windows service manager; use --install-helper".into());
        }
        Ok(())
    }

    // ------------------------------------------------------------------ install, uninstall, status

    fn is_admin() -> bool {
        // SAFETY: no arguments.
        unsafe { IsUserAnAdmin() != 0 }
    }

    /// Run this program again with administrator rights (the UAC prompt appears on the PC) and wait for it.
    fn rerun_elevated(args: &str) -> Result<u32, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let file = wide(&exe.display().to_string());
        let params = wide(args);
        let verb = wide("runas");
        // SAFETY: the struct is fully initialised, the strings are NUL-terminated and outlive the call.
        unsafe {
            let mut info: SHELLEXECUTEINFOW = zeroed();
            info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = params.as_ptr();
            if ShellExecuteExW(&mut info) == 0 || info.hProcess.is_null() {
                return Err("administrator approval was not given".into());
            }
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 1u32;
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
            Ok(code)
        }
    }

    fn sc(args: &[&str]) -> Result<(), String> {
        let out = Command::new("sc.exe").args(args).stdin(Stdio::null()).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("sc {} failed: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stdout).trim()))
        }
    }

    fn install_dir() -> PathBuf {
        PathBuf::from(std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into())).join("OpenRemote")
    }

    /// Copy the program into Program Files (only administrators can write there, so a normal program cannot
    /// swap the file the service runs) and register the service.
    fn install() -> Result<String, String> {
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = install_dir();
        let dst = dir.join("openremote.exe");
        let _ = sc(&["stop", SERVICE_NAME]);
        std::thread::sleep(Duration::from_millis(1500));
        std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        if me != dst {
            std::fs::copy(&me, &dst).map_err(|e| format!("cannot copy the program to {}: {e}", dst.display()))?;
        }
        let _ = sc(&["delete", SERVICE_NAME]);
        std::thread::sleep(Duration::from_millis(500));
        let bin = format!("\"{}\" --service", dst.display());
        sc(&[
            "create",
            SERVICE_NAME,
            "binPath=",
            &bin,
            "start=",
            "auto",
            "obj=",
            "LocalSystem",
            "DisplayName=",
            "OpenRemote input helper",
        ])?;
        let _ = sc(&[
            "description",
            SERVICE_NAME,
            "Lets OpenRemote send mouse and keyboard input to UAC prompts, administrator windows and the lock screen.",
        ]);
        let _ = sc(&["failure", SERVICE_NAME, "reset=", "60", "actions=", "restart/5000"]);
        sc(&["start", SERVICE_NAME])?;
        Ok(format!("Installed. The helper runs from {} and starts with Windows.", dst.display()))
    }

    fn uninstall() -> Result<String, String> {
        let _ = sc(&["stop", SERVICE_NAME]);
        std::thread::sleep(Duration::from_millis(1500));
        let _ = sc(&["delete", SERVICE_NAME]);
        let _ = std::fs::remove_dir_all(install_dir());
        Ok("Removed. Input on UAC prompts is off again.".into())
    }

    const WARNING: &str = "\
This installs the OpenRemote input helper: a Windows service that runs as SYSTEM so the phone's mouse
and keyboard also work on UAC prompts, administrator windows and the lock screen.

Read this first:
  - Any program that runs as you can use the helper to send input, including to UAC prompts.
  - Every phone you pair can click through a UAC prompt on this PC (and with --no-pairing, anyone on your
    network can). Pair only phones you trust and revoke lost ones on the admin page.
  - Windows will ask you to approve the installation (a UAC prompt on this PC).

To continue, run it again with --yes. To remove it later: openremote --uninstall-helper";

    fn report(result: Result<String, String>, quiet: bool) -> i32 {
        let (msg, code) = match result {
            Ok(m) => (m, 0),
            Err(e) => (e, 1),
        };
        if quiet {
            // the elevated copy has no console: say it in a box
            if code == 0 {
                crate::shell::info_box(&msg);
            } else {
                crate::shell::error_box(&msg);
            }
        } else {
            println!("{msg}");
        }
        code
    }

    /// Handle the helper-related command lines. `Some(exit code)` means the arguments were handled here.
    pub fn cli(args: &[String]) -> Option<i32> {
        let has = |f: &str| args.iter().any(|a| a == f);
        if has("--input-helper") {
            let sid = args.iter().position(|a| a == "--allow-sid").and_then(|i| args.get(i + 1)).map(String::as_str);
            return Some(match run_helper(sid) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            });
        }
        if has("--service") {
            return Some(match run_service() {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            });
        }
        if has("--helper-status") {
            let on = HelperLink::default().active();
            println!("{}", if on { "The input helper is running." } else { "The input helper is not running." });
            return Some(0);
        }
        let install_cmd = has("--install-helper");
        if install_cmd || has("--uninstall-helper") {
            if install_cmd && !has("--yes") {
                println!("{WARNING}");
                return Some(1);
            }
            let elevated = is_admin();
            if !elevated {
                println!("Asking Windows for administrator approval...");
                let flag = if install_cmd { "--install-helper --yes" } else { "--uninstall-helper" };
                return Some(match rerun_elevated(flag) {
                    Ok(0) => {
                        println!(
                            "{}",
                            if install_cmd { "Done. Check with: openremote --helper-status" } else { "Done." }
                        );
                        0
                    }
                    Ok(c) => c as i32,
                    Err(e) => {
                        println!("{e}");
                        1
                    }
                });
            }
            let result = if install_cmd { install() } else { uninstall() };
            return Some(report(result, true));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_survives_the_wire() {
        let all = [
            Event::Move { dx: -7, dy: 300 },
            Event::Button { b: 2, down: true },
            Event::Wheel { horizontal: true, delta: -120 },
            Event::Key { vk: 0x5B, down: false },
            Event::Unit(0xD83D),
            Event::Ping,
        ];
        for e in all {
            assert_eq!(Event::decode(&e.encode()), Some(e), "{e:?}");
        }
    }

    #[test]
    fn garbled_frames_are_rejected() {
        let mut f = Event::Button { b: 1, down: true }.encode();
        f[1] = 9; // no such button
        assert_eq!(Event::decode(&f), None);
        let mut f = Event::Key { vk: 1, down: true }.encode();
        f[5] = 2; // down is not a flag
        assert_eq!(Event::decode(&f), None);
        let mut f = Event::Unit(1).encode();
        f[3] = 1; // unit above 65535
        assert_eq!(Event::decode(&f), None);
        assert_eq!(Event::decode(&[0u8; FRAME]), None);
        assert_eq!(Event::decode(&[200u8; FRAME]), None);
    }

    #[test]
    fn only_plain_sids_go_into_the_access_rule() {
        assert!(is_sid_string("S-1-5-21-1004336348-1177238915-682003330-1001"));
        assert!(!is_sid_string("S-1-5-21)(A;;GA;;;WD"));
        assert!(!is_sid_string("WD"));
        assert!(!is_sid_string(""));
    }
}
