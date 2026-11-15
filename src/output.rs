//! Where a capture goes once encoded: a file named like macOS screenshots, and the capture sound.
//! Called on the export thread, never the UI thread.

use std::path::{Path, PathBuf};

/// The system's screenshot sound (macOS 13+ location).
const SOUND: &str = "/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/system/Screen Capture.aif";

/// `ailfred 2026-11-15 at 18.05.23.png` in local time, like macOS's own screenshots.
fn file_name(unix_secs: i64) -> String {
    let t: libc::time_t = unix_secs as libc::time_t;
    // SAFETY: localtime_r writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return format!("ailfred {unix_secs}");
    }
    format!(
        "ailfred {:04}-{:02}-{:02} at {:02}.{:02}.{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Candidate paths in `dir`: `name.png`, then `name (2).png`, …
fn candidates<'a>(dir: &'a Path, name: &'a str) -> impl Iterator<Item = PathBuf> + 'a {
    std::iter::once(dir.join(format!("{name}.png"))).chain((2..).map(move |n| dir.join(format!("{name} ({n}).png"))))
}

/// Writes the PNG into `dir` under a new name (never over another file, even when two exports
/// land in the same second: `create_new` is atomic) and plays the capture sound.
pub fn save(dir: &Path, png: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let name = file_name(secs);
    for path in candidates(dir, &name).take(1000) {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(png).map_err(|e| format!("saving {} failed: {e}", path.display()))?;
                play_sound();
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("saving {} failed: {e}", path.display())),
        }
    }
    Err(format!("no free name for {name} in {}", dir.display()))
}

/// Plays the capture sound. Runs on the export thread, so waiting for `afplay` (about a second)
/// costs nothing and reaps it: no zombie per capture.
fn play_sound() {
    if Path::new(SOUND).exists() {
        let _ = std::process::Command::new("afplay")
            .arg(SOUND)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_has_the_macos_shape() {
        let name = file_name(1_794_762_000);
        // "ailfred YYYY-MM-DD at HH.MM.SS"
        assert!(name.starts_with("ailfred 20"), "{name}");
        assert_eq!(name.len(), "ailfred 2026-11-15 at 18.05.23".len(), "{name}");
        assert_eq!(&name[18..22], " at ", "{name}");
    }

    #[test]
    fn candidates_number_duplicates() {
        let dir = Path::new("/d");
        let names: Vec<_> = candidates(dir, "shot").take(3).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["shot.png", "shot (2).png", "shot (3).png"]);
    }
}
