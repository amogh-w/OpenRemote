//! "Now playing" for the Media page: reads the Windows system media session (the same source as the
//! volume flyout), so it works for any player that reports to Windows (browsers, Spotify, VLC, ...).

use crate::command::{MediaInfo, MediaState};
use std::sync::Once;
use windows::core::{factory, Interface, RuntimeName};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    IGlobalSystemMediaTransportControlsSessionManagerStatics as ManagerStatics,
};
use windows::Storage::Streams::{DataReader, IDataReaderFactory};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

const MAX_ART_BYTES: u64 = 4 * 1024 * 1024;

/// Keep the DLL behind a WinRT class loaded for the life of the process.
///
/// windows-rs caches each class factory in a static for good. COM unloads in-process DLLs it thinks are idle, and
/// once `windows.media.mediacontrol.dll` was gone the next "now playing" poll called through the cached factory
/// into freed memory and the whole program died with an access violation. A pinned DLL cannot be unloaded.
fn pin_dll_of<C: RuntimeName, I: Interface>() {
    extern "system" {
        fn GetModuleHandleExW(flags: u32, addr: *const core::ffi::c_void, module: *mut isize) -> i32;
    }
    const FROM_ADDRESS: u32 = 0x4;
    const PIN: u32 = 0x1;
    if let Ok(factory) = factory::<C, I>() {
        // SAFETY: a COM object starts with its vtable pointer. `FROM_ADDRESS` only looks up which loaded module
        // contains that address (and `PIN` keeps it loaded); nothing is dereferenced beyond that first word.
        unsafe {
            let vtable = *(factory.as_raw() as *const *const core::ffi::c_void);
            let mut module = 0isize;
            GetModuleHandleExW(FROM_ADDRESS | PIN, vtable, &mut module);
        }
    }
}

/// WinRT needs the calling thread initialized. Repeating this on an initialized thread is harmless, and a
/// thread already set up another way just keeps its mode.
fn init() {
    // SAFETY: plain COM/WinRT initialization of the current thread, no pointers involved.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    static PIN: Once = Once::new();
    PIN.call_once(|| {
        pin_dll_of::<Manager, ManagerStatics>();
        pin_dll_of::<DataReader, IDataReaderFactory>();
    });
}

/// Ticks of 100 ns (a `TimeSpan`) to whole seconds.
fn secs(ticks: i64) -> u64 {
    (ticks.max(0) / 10_000_000) as u64
}

/// "Spotify.exe" -> "Spotify", "Microsoft.ZuneMusic_8wekyb3d8bbwe!App" -> "ZuneMusic".
fn app_name(id: &str) -> String {
    let id = id.rsplit('!').next().unwrap_or(id);
    let id = id.strip_suffix(".exe").unwrap_or(id);
    let id = id.split('_').next().unwrap_or(id);
    id.rsplit('.').next().unwrap_or(id).to_string()
}

pub fn info() -> Option<MediaInfo> {
    init();
    let session = Manager::RequestAsync().ok()?.get().ok()?.GetCurrentSession().ok()?;
    let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let state = match session.GetPlaybackInfo().ok()?.PlaybackStatus().ok()? {
        Status::Playing => MediaState::Playing,
        Status::Paused => MediaState::Paused,
        _ => MediaState::Stopped,
    };
    let (mut pos, mut dur) = (0, 0);
    if let Ok(t) = session.GetTimelineProperties() {
        if let (Ok(start), Ok(end), Ok(p)) = (t.StartTime(), t.EndTime(), t.Position()) {
            dur = secs(end.Duration - start.Duration);
            pos = secs(p.Duration - start.Duration).min(dur);
        }
    }
    Some(MediaInfo {
        state,
        title: props.Title().map(|s| s.to_string()).unwrap_or_default(),
        artist: props.Artist().map(|s| s.to_string()).unwrap_or_default(),
        app: session.SourceAppUserModelId().map(|s| app_name(&s.to_string())).unwrap_or_default(),
        pos,
        dur,
        art: props.Thumbnail().is_ok(),
    })
}

/// The current track's cover image (PNG or JPEG bytes), if the player supplies one.
pub fn art() -> Option<Vec<u8>> {
    init();
    let session = Manager::RequestAsync().ok()?.get().ok()?.GetCurrentSession().ok()?;
    let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_ART_BYTES {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.get().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_are_tidied() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("chrome.exe"), "chrome");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
        assert_eq!(app_name(""), "");
    }

    #[test]
    fn timespans_become_seconds() {
        assert_eq!(secs(30_000_000), 3);
        assert_eq!(secs(-5), 0);
    }

    /// The server answers each media request on a thread-pool thread that may be gone by the next one.
    #[test]
    fn survives_the_calling_thread_ending() {
        for _ in 0..4 {
            std::thread::spawn(|| {
                let _ = info();
            })
            .join()
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// The crash seen in the field: `windows.media.mediacontrol.dll` was gone from the process while windows-rs still
    /// held its cached factory, and the next poll called through a dangling vtable (access violation). Without the
    /// pin this test dies with STATUS_ACCESS_VIOLATION; with it, unloading is refused and the DLL stays put.
    #[test]
    fn media_dll_cannot_be_unloaded_under_the_cached_factory() {
        extern "system" {
            fn GetModuleHandleW(name: *const u16) -> isize;
            fn FreeLibrary(m: isize) -> i32;
        }
        let name: Vec<u16> = "windows.media.mediacontrol.dll".encode_utf16().chain([0]).collect();
        let _ = std::thread::spawn(info).join().unwrap();
        for _ in 0..64 {
            // SAFETY: `name` is a NUL-terminated wide string; FreeLibrary on a handle GetModuleHandleW just returned.
            let m = unsafe { GetModuleHandleW(name.as_ptr()) };
            if m == 0 {
                break;
            }
            unsafe { FreeLibrary(m) };
        }
        assert_ne!(unsafe { GetModuleHandleW(name.as_ptr()) }, 0, "the DLL must stay loaded");
        let _ = std::thread::spawn(info).join().unwrap();
        let _ = std::thread::spawn(info).join().unwrap();
    }
}
