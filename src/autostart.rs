//! Start with Windows. Shared by every project that uses it: edit it in the cuppa plugin repo (shared/) and run
//! `harness.mjs sync --write`, never in a project copy. Each crate defines `APP_NAME` in its `lib.rs`.
//!
//! A per-user entry under `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run`.
//! No administrator rights are needed and nothing outside the current user is touched.

use std::path::Path;

#[cfg(windows)]
pub use imp::{enabled, refresh, set};

/// Split a stored Run command into the exe path and its arguments.
#[cfg_attr(not(windows), allow(dead_code))]
fn split_command(stored: &str) -> (String, String) {
    let stored = stored.trim();
    if let Some(rest) = stored.strip_prefix('"') {
        return match rest.split_once('"') {
            Some((exe, args)) => (exe.to_string(), args.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
    }
    // An unquoted command may have spaces in its path: the exe ends at the first ".exe".
    match stored.to_ascii_lowercase().find(".exe") {
        Some(i) => (stored[..i + 4].to_string(), stored[i + 4..].trim().to_string()),
        None => (stored.to_string(), String::new()),
    }
}

/// An entry is stale when its exe is gone, or when it points at this exe without the `--startup` flag.
#[cfg_attr(not(windows), allow(dead_code))]
fn is_stale(stored: &str, current: &Path) -> bool {
    let (exe, args) = split_command(stored);
    let exe = Path::new(&exe);
    !exe.exists() || (exe == current && args != "--startup")
}

#[cfg(not(windows))]
pub fn enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set(_on: bool) -> Result<(), String> {
    Err("start at login is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn refresh() {}

#[cfg(windows)]
mod imp {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = crate::APP_NAME;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Open the Run key with `access`, run `f`, close it.
    fn with_key<T>(access: u32, f: impl FnOnce(HKEY) -> T) -> Result<T, String> {
        let mut key: HKEY = null_mut();
        // SAFETY: `key` receives an owned handle on success and is closed below; the path is NUL-terminated.
        let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, wide(KEY).as_ptr(), 0, access, &mut key) };
        if rc != ERROR_SUCCESS {
            return Err(format!("cannot open the Run key (error {rc})"));
        }
        let out = f(key);
        // SAFETY: `key` was opened above.
        unsafe {
            RegCloseKey(key);
        }
        Ok(out)
    }

    pub fn enabled() -> bool {
        with_key(KEY_QUERY_VALUE, |key| {
            // SAFETY: only asks whether the value exists; no buffers are passed.
            unsafe {
                RegQueryValueExW(key, wide(NAME).as_ptr(), null(), null_mut(), null_mut(), null_mut()) == ERROR_SUCCESS
            }
        })
        .unwrap_or(false)
    }

    /// The stored command, if the entry exists.
    fn value() -> Option<String> {
        with_key(KEY_QUERY_VALUE, |key| {
            let name = wide(NAME);
            let mut bytes: u32 = 0;
            // SAFETY: the first call only asks for the size of the value.
            let rc = unsafe { RegQueryValueExW(key, name.as_ptr(), null(), null_mut(), null_mut(), &mut bytes) };
            if rc != ERROR_SUCCESS || bytes == 0 {
                return None;
            }
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
            let mut len = (buf.len() * 2) as u32;
            // SAFETY: `buf` holds `len` bytes, which is what the call may write.
            let rc = unsafe {
                RegQueryValueExW(key, name.as_ptr(), null(), null_mut(), buf.as_mut_ptr() as *mut u8, &mut len)
            };
            if rc != ERROR_SUCCESS {
                return None;
            }
            let text = String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]);
            Some(text.trim_end_matches('\0').to_string())
        })
        .ok()
        .flatten()
    }

    /// Repair an entry that Windows could not use any more (its exe was moved or deleted, or it lacks `--startup`).
    /// An entry that points at some other, existing copy is left alone: that may be a deliberate choice.
    pub fn refresh() {
        let (Some(stored), Ok(exe)) = (value(), std::env::current_exe()) else { return };
        if super::is_stale(&stored, &exe) {
            match set(true) {
                Ok(()) => crate::applog::info("start with Windows: repaired a stale startup entry"),
                Err(e) => crate::applog::warn(&format!("start with Windows: could not repair the entry: {e}")),
            }
        }
    }

    pub fn set(on: bool) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        with_key(KEY_SET_VALUE, |key| {
            let name = wide(NAME);
            // SAFETY: the value buffer is a NUL-terminated UTF-16 string and its byte length is passed exactly.
            let rc = unsafe {
                if on {
                    let value = wide(&format!("\"{}\" --startup", exe.display()));
                    RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr() as *const u8, (value.len() * 2) as u32)
                } else {
                    RegDeleteValueW(key, name.as_ptr())
                }
            };
            if rc == ERROR_SUCCESS || (!on && rc == ERROR_FILE_NOT_FOUND) {
                Ok(())
            } else {
                Err(format!("cannot change the startup entry (error {rc})"))
            }
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_split_into_exe_and_arguments() {
        assert_eq!(split_command(r#""C:\a b\app.exe" --startup"#), (r"C:\a b\app.exe".into(), "--startup".into()));
        assert_eq!(split_command(r"C:\Remote Mouse\app.EXE"), (r"C:\Remote Mouse\app.EXE".into(), String::new()));
        assert_eq!(split_command(r"C:\x\app.exe --a b"), (r"C:\x\app.exe".into(), "--a b".into()));
    }

    #[test]
    fn only_a_dead_or_flagless_entry_is_stale() {
        let me = std::env::current_exe().unwrap();
        let q = |args: &str| format!("\"{}\" {args}", me.display());
        assert!(!is_stale(&q("--startup"), &me));
        assert!(is_stale(&q(""), &me), "this exe without --startup");
        assert!(is_stale(r"C:\no\such folder\app.exe", &me), "the exe is gone");
        // another copy that still exists is left alone
        assert!(!is_stale(&q("--startup"), Path::new("elsewhere.exe")));
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platforms_say_so() {
        assert!(!super::enabled());
        assert!(super::set(true).is_err());
    }
}
