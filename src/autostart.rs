//! Start with Windows. Shared by OpenRemote and Kai Remote: edit it in the cuppa plugin repo (shared/) and run
//! `harness.mjs sync --write`, never in a project copy. Each crate defines `APP_NAME` in its `lib.rs`.
//!
//! A per-user entry under `HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run`.
//! No administrator rights are needed and nothing outside the current user is touched.

#[cfg(windows)]
pub use imp::{enabled, set};

#[cfg(not(windows))]
pub fn enabled() -> bool {
    false
}

#[cfg(not(windows))]
pub fn set(_on: bool) -> Result<(), String> {
    Err("start at login is only available on Windows".into())
}

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
    #[cfg(not(windows))]
    #[test]
    fn unsupported_platforms_say_so() {
        assert!(!super::enabled());
        assert!(super::set(true).is_err());
    }
}
