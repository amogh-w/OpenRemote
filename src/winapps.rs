//! Windows only: open-window enumeration, window actions, Start Menu apps and icon extraction.
//!
//! The server hands out opaque icon/app keys (a hash of a path it found itself). Clients can only
//! refer to things by those keys, never by path, so they cannot launch or read arbitrary files.

use crate::command::{AppEntry, WindowAct, WindowInfo};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::ffi::{c_void, OsString};
use std::hash::{Hash, Hasher};
use std::iter::once;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, TRUE};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows_sys::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::core::GUID;
use windows_sys::Win32::UI::Controls::{ImageList_GetIcon, HIMAGELIST, ILD_NORMAL};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP, VK_F24};
use windows_sys::Win32::UI::Shell::{
    SHGetFileInfoW, SHGetImageList, ShellExecuteW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SYSICONINDEX,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, DestroyIcon, EnumWindows, GetForegroundWindow, GetIconInfo, GetWindow, GetWindowLongW,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
    PostMessageW, PrivateExtractIconsW, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GW_OWNER, HICON, ICONINFO,
    SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOWNORMAL, WM_CLOSE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

const MAX_WINDOWS: usize = 100;
const MAX_APPS: usize = 500;
const APP_LIST_TTL: Duration = Duration::from_secs(60);
const ICON_PX: i32 = 64;

struct Entry {
    path: PathBuf,
    /// True only for Start Menu shortcuts; running-window executables are never launchable.
    launchable: bool,
}

#[derive(Default)]
pub struct Registry {
    paths: Mutex<HashMap<String, Entry>>,
    /// Successful extractions only: a transient failure must not become a permanent 404.
    icons: Mutex<HashMap<String, Vec<u8>>>,
    /// Shell icon APIs are not safe to hammer from many threads at once (first use races), so
    /// extractions run one at a time.
    extract: Mutex<()>,
    apps: Mutex<Option<(Instant, Vec<AppEntry>)>>,
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(once(0)).collect()
}

impl Registry {
    /// Stable 16-hex-digit key for a path, remembered so the key can be resolved later.
    fn key_for(&self, path: &Path, launchable: bool) -> String {
        let mut h = DefaultHasher::new();
        path.to_string_lossy().to_lowercase().hash(&mut h);
        let key = format!("{:016x}", h.finish());
        let mut map = self.paths.lock().unwrap();
        let e = map.entry(key.clone()).or_insert_with(|| Entry { path: path.to_path_buf(), launchable });
        e.launchable |= launchable;
        key
    }

    pub fn windows(&self) -> Vec<WindowInfo> {
        let mut found: Vec<HWND> = Vec::new();
        // SAFETY: the callback only runs during this call and `found` outlives it.
        unsafe {
            EnumWindows(Some(collect), &mut found as *mut Vec<HWND> as LPARAM);
        }
        let fg = unsafe { GetForegroundWindow() };
        found
            .into_iter()
            .filter_map(|h| self.window_info(h, h == fg))
            .take(MAX_WINDOWS)
            .collect()
    }

    fn window_info(&self, hwnd: HWND, foreground: bool) -> Option<WindowInfo> {
        // SAFETY: plain Win32 queries on a handle that may vanish at any time; failures are handled.
        unsafe {
            let n = GetWindowTextLengthW(hwnd);
            if n <= 0 {
                return None;
            }
            let mut buf = vec![0u16; n as usize + 1];
            let got = GetWindowTextW(hwnd, buf.as_mut_ptr(), n + 1);
            let title = String::from_utf16_lossy(&buf[..got.max(0) as usize]);
            if title == "Program Manager" {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            let path = exe_path(pid);
            let (exe, icon) = match &path {
                Some(p) => (
                    p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                    self.key_for(p, false),
                ),
                None => (String::new(), String::new()),
            };
            Some(WindowInfo { id: hwnd as usize as u64, title, exe, icon, minimized: IsIconic(hwnd) != 0, foreground })
        }
    }

    /// Start Menu shortcuts, cached for a minute.
    pub fn apps(&self) -> Vec<AppEntry> {
        if let Some((at, list)) = &*self.apps.lock().unwrap() {
            if at.elapsed() < APP_LIST_TTL {
                return list.clone();
            }
        }
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        for base in ["PROGRAMDATA", "APPDATA"] {
            if let Some(root) = std::env::var_os(base) {
                scan_shortcuts(&PathBuf::from(root).join(r"Microsoft\Windows\Start Menu\Programs"), &mut found, 0);
            }
        }
        found.sort_by_key(|(name, _)| name.to_lowercase());
        let mut seen = std::collections::HashSet::new();
        let list: Vec<AppEntry> = found
            .into_iter()
            .filter(|(name, _)| seen.insert(name.to_lowercase()))
            .take(MAX_APPS)
            .map(|(name, path)| {
                let key = self.key_for(&path, true);
                AppEntry { id: key.clone(), name, icon: key }
            })
            .collect();
        *self.apps.lock().unwrap() = Some((Instant::now(), list.clone()));
        list
    }

    pub fn launch(&self, id: &str) {
        let path = match self.paths.lock().unwrap().get(id) {
            Some(e) if e.launchable => e.path.clone(),
            _ => return,
        };
        // ShellExecute may call into COM shell extensions, so give it its own COM-initialised thread.
        // That also keeps a slow-starting app from blocking the caller.
        std::thread::spawn(move || {
            let file = wide(&path);
            let verb: Vec<u16> = "open".encode_utf16().chain(once(0)).collect();
            // SAFETY: both strings are NUL-terminated and live across the call; COM is balanced below.
            unsafe {
                let com = CoInitializeEx(null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) >= 0;
                ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL);
                if com {
                    CoUninitialize();
                }
            }
        });
    }

    pub fn icon_png(&self, key: &str) -> Option<Vec<u8>> {
        if let Some(hit) = self.icons.lock().unwrap().get(key) {
            return Some(hit.clone());
        }
        let path = self.paths.lock().unwrap().get(key)?.path.clone();
        let _one_at_a_time = self.extract.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = self.icons.lock().unwrap().get(key) {
            return Some(hit.clone()); // another request finished it while we waited
        }
        // SAFETY: COM is initialised and released on this same thread around the extraction.
        let png = unsafe {
            let com = CoInitializeEx(null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32) >= 0;
            let png = icon_for_path(&path);
            if com {
                CoUninitialize();
            }
            png
        }?;
        self.icons.lock().unwrap().insert(key.to_string(), png.clone());
        Some(png)
    }
}

/// Apply an action to a window, but only if it is still a window we would have listed.
pub fn window_action(id: u64, act: WindowAct) {
    let hwnd = id as usize as HWND;
    // SAFETY: Win32 calls on a handle validated by `is_listable`.
    unsafe {
        if !is_listable(hwnd) {
            return;
        }
        match act {
            WindowAct::Focus => focus(hwnd),
            WindowAct::Min => {
                ShowWindow(hwnd, SW_MINIMIZE);
            }
            WindowAct::Max => {
                ShowWindow(hwnd, SW_MAXIMIZE);
            }
            WindowAct::Restore => {
                ShowWindow(hwnd, SW_RESTORE);
            }
            WindowAct::Close => {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
}

unsafe fn focus(hwnd: HWND) {
    if IsIconic(hwnd) != 0 {
        ShowWindow(hwnd, SW_RESTORE);
    }
    // Windows only lets the process that received the last input event take the foreground.
    // A harmless key (F24) press counts as that input.
    keybd_event(VK_F24 as u8, 0, 0, 0);
    keybd_event(VK_F24 as u8, 0, KEYEVENTF_KEYUP, 0);
    SetForegroundWindow(hwnd);
    BringWindowToTop(hwnd);
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam as *mut Vec<HWND>);
    if is_listable(hwnd) {
        list.push(hwnd);
    }
    TRUE
}

/// Visible, un-owned, non-cloaked, titled top-level windows that are not ours (what the taskbar shows).
unsafe fn is_listable(hwnd: HWND) -> bool {
    if IsWindow(hwnd) == 0 || IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
        return false;
    }
    let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if ex & WS_EX_TOOLWINDOW != 0 && ex & WS_EX_APPWINDOW == 0 {
        return false;
    }
    let mut cloaked = 0u32;
    DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as _, &mut cloaked as *mut u32 as *mut c_void, size_of::<u32>() as u32);
    if cloaked != 0 || GetWindowTextLengthW(hwnd) == 0 {
        return false;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    pid != GetCurrentProcessId()
}

fn exe_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: the process handle is closed before returning; the buffer length is passed in/out.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| PathBuf::from(OsString::from_wide(&buf[..len as usize])))
    }
}

fn scan_shortcuts(dir: &Path, out: &mut Vec<(String, PathBuf)>, depth: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if depth < 4 {
                scan_shortcuts(&path, out, depth + 1);
            }
        } else if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
            if let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) {
                if !is_junk_shortcut(&name) {
                    out.push((name, path));
                }
            }
        }
    }
}

/// Uninstallers, help and documentation links clutter the app list.
fn is_junk_shortcut(name: &str) -> bool {
    let n = name.to_lowercase();
    ["uninstall", "readme", "documentation", "release notes", "license", "manual", "changelog", "website", "help"]
        .iter()
        .any(|w| n.contains(w))
}

fn icon_for_path(path: &Path) -> Option<Vec<u8>> {
    let w = wide(path);
    let is_shortcut = path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("lnk") || x.eq_ignore_ascii_case("url"));
    // SAFETY: `w` is NUL-terminated; any icon handle obtained is destroyed exactly once.
    unsafe {
        let mut hicon: HICON = null_mut();
        if !is_shortcut {
            let mut id = 0u32;
            let n = PrivateExtractIconsW(w.as_ptr(), 0, ICON_PX, ICON_PX, &mut hicon, &mut id, 1, 0);
            if n == 0 {
                hicon = null_mut();
            }
        }
        if hicon.is_null() && is_shortcut {
            hicon = shortcut_icon(&w);
        }
        if hicon.is_null() {
            let mut sfi: SHFILEINFOW = zeroed();
            if SHGetFileInfoW(w.as_ptr(), 0, &mut sfi, size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON) == 0 {
                return None;
            }
            hicon = sfi.hIcon;
        }
        if hicon.is_null() {
            return None;
        }
        let png = hicon_to_png(hicon);
        DestroyIcon(hicon);
        png
    }
}

/// 48 px icon of a shortcut from the system image list. Unlike `SHGFI_ICON`, this has no shortcut
/// arrow baked in (overlays are only drawn when asked for).
unsafe fn shortcut_icon(path: &[u16]) -> HICON {
    const SHIL_EXTRALARGE: i32 = 2;
    // IID_IImageList {46EB5926-582E-4017-9FDF-E8998DAA0950}
    const IID_IIMAGELIST: GUID = GUID {
        data1: 0x46EB5926,
        data2: 0x582E,
        data3: 0x4017,
        data4: [0x9F, 0xDF, 0xE8, 0x99, 0x8D, 0xAA, 0x09, 0x50],
    };
    let mut sfi: SHFILEINFOW = zeroed();
    if SHGetFileInfoW(path.as_ptr(), 0, &mut sfi, size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX) == 0 {
        return null_mut();
    }
    let mut list: *mut c_void = null_mut();
    if SHGetImageList(SHIL_EXTRALARGE, &IID_IIMAGELIST, &mut list) != 0 || list.is_null() {
        return null_mut();
    }
    // An IImageList pointer is a valid HIMAGELIST. The list belongs to the shell, so it is not released.
    ImageList_GetIcon(list as HIMAGELIST, sfi.iIcon, ILD_NORMAL)
}

/// Render an icon to a PNG, using the AND mask when the colour bitmap carries no alpha.
unsafe fn hicon_to_png(hicon: HICON) -> Option<Vec<u8>> {
    let mut info: ICONINFO = zeroed();
    if GetIconInfo(hicon, &mut info) == 0 {
        return None;
    }
    let result = (|| {
        if info.hbmColor.is_null() {
            return None; // monochrome icons: not worth supporting
        }
        let mut bm: BITMAP = zeroed();
        GetObjectW(info.hbmColor, size_of::<BITMAP>() as i32, &mut bm as *mut BITMAP as *mut c_void);
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        if w <= 0 || h <= 0 || w > 512 || h > 512 {
            return None;
        }
        let mut color = read_pixels(info.hbmColor, w, h)?;
        if color.chunks_exact(4).all(|p| p[3] == 0) {
            if !info.hbmMask.is_null() {
                let mask = read_pixels(info.hbmMask, w, h)?;
                for (c, m) in color.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                    c[3] = if m[0] == 0 { 255 } else { 0 };
                }
            } else {
                color.chunks_exact_mut(4).for_each(|c| c[3] = 255);
            }
        }
        color.chunks_exact_mut(4).for_each(|p| p.swap(0, 2)); // BGRA -> RGBA
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().ok()?.write_image_data(&color).ok()?;
        }
        Some(out)
    })();
    DeleteObject(info.hbmColor);
    DeleteObject(info.hbmMask);
    result
}

/// Top-down 32-bit BGRA pixels of a bitmap.
unsafe fn read_pixels(bmp: windows_sys::Win32::Graphics::Gdi::HBITMAP, w: i32, h: i32) -> Option<Vec<u8>> {
    let mut bi: BITMAPINFO = zeroed();
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB as u32,
        ..zeroed()
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let hdc = GetDC(null_mut());
    let lines = GetDIBits(hdc, bmp, 0, h as u32, buf.as_mut_ptr() as *mut c_void, &mut bi, DIB_RGB_COLORS);
    ReleaseDC(null_mut(), hdc);
    (lines != 0).then_some(buf)
}
