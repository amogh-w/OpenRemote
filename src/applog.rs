//! A tiny file log, shared by OpenRemote and Kai Remote: edit it in the cuppa plugin
//! repo (shared/) and run `harness.mjs sync --write`, never in a project copy.
//!
//! Console output is lost in release builds that have no console (OpenRemote runs as a Windows GUI program), so
//! events worth finding later go to a file as well. One line per event, UTC time, the file rotates at 512 KB and
//! keeps one older copy (`<name>.1`). Never log keys, pairing secrets, stream links or other private data.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_BYTES: u64 = 512 * 1024;
static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Start logging to `path`. Until this is called, events only go to stderr.
pub fn init(path: PathBuf) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = PATH.set(path);
}

pub fn info(msg: &str) {
    log("INFO", msg);
}

pub fn warn(msg: &str) {
    log("WARN", msg);
}

pub fn error(msg: &str) {
    log("ERROR", msg);
}

fn log(level: &str, msg: &str) {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = format_line(secs, level, msg);
    eprint!("{line}");
    if let Some(path) = PATH.get() {
        append(path, &line);
    }
}

/// `2023-11-14T22:13:20Z INFO message`, newlines in the message flattened so each event stays on one line.
fn format_line(secs: u64, level: &str, msg: &str) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let msg = msg.replace(['\r', '\n'], " ");
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z {level} {msg}\n", rem / 3_600, rem % 3_600 / 60, rem % 60)
}

fn append(path: &Path, line: &str) {
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = fs::rename(path, PathBuf::from(old));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_have_a_utc_timestamp_and_stay_on_one_line() {
        assert_eq!(format_line(1_700_000_000, "INFO", "hello"), "2023-11-14T22:13:20Z INFO hello\n");
        assert_eq!(format_line(0, "WARN", "a\nb\r\nc"), "1970-01-01T00:00:00Z WARN a b  c\n");
        assert_eq!(format_line(951_782_400, "INFO", "x"), "2000-02-29T00:00:00Z INFO x\n", "leap day");
    }

    #[test]
    fn the_file_rotates_once_it_is_big() {
        let dir = std::env::temp_dir().join(format!("applog-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.log");
        append(&path, "first\n");
        append(&path, "second\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        append(&path, "after\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "after\n");
        assert_eq!(fs::metadata(dir.join("app.log.1")).unwrap().len(), MAX_BYTES + 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
