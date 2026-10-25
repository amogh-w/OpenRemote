//! "Now playing" for the Media page: reads the Windows system media session (the same source as the
//! volume flyout), so it works for any player that reports to Windows (browsers, Spotify, VLC, ...).

use crate::command::{MediaInfo, MediaState};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

const MAX_ART_BYTES: u64 = 4 * 1024 * 1024;

/// WinRT needs the calling thread initialized. Repeating this on an initialized thread is harmless, and a
/// thread already set up another way just keeps its mode.
fn init() {
    // SAFETY: plain COM/WinRT initialization of the current thread, no pointers involved.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
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
}
