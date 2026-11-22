//! Body files an earlier Ailfred left in the temp folder (it quit mid-call, so its `BodyFile`
//! was never dropped): removed once, before the first call of this run.

use std::path::{Path, PathBuf};

/// The pid in a body file's name, `ailfred-<pid>-<n>.json`; None for any other name.
pub fn pid_of(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("ailfred-")?.strip_suffix(".json")?;
    let (pid, n) = rest.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(pid) && digits(n)).then(|| pid.parse().ok()).flatten()
}

/// Removes the body files in `dir` of a process that isn't this one (`own`) and isn't running
/// (`alive`): another Ailfred going (the dev build beside the installed one) keeps its own.
/// What it removed.
pub fn sweep(dir: &Path, own: u32, alive: impl Fn(u32) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(pid_of) else { continue };
        if pid == own || alive(pid) || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// Whether process `pid` is running (one of another user's counts too).
pub fn alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return false };
    // SAFETY: signal 0 sends nothing; it only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ailfred-leftovers-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_body_files_name_gives_its_pid_and_nothing_else_does() {
        assert_eq!(pid_of("ailfred-123-0.json"), Some(123));
        assert_eq!(pid_of("ailfred-9-41.json"), Some(9));
        for other in ["ailfred-123.json", "ailfred-x-0.json", "ailfred-1-a.json", "ailfred--0.json", "ailfred-1-0.json.tmp", "other-1-0.json", "ailfred-1-0-2.json"] {
            assert_eq!(pid_of(other), None, "{other}");
        }
    }

    #[test]
    fn only_the_body_files_of_processes_gone_are_removed() {
        let dir = scratch("sweep");
        for name in ["ailfred-10-0.json", "ailfred-10-3.json", "ailfred-20-0.json", "ailfred-30-1.json", "ailfred-10.json", "notes.txt"] {
            std::fs::write(dir.join(name), "{}").unwrap();
        }
        std::fs::create_dir(dir.join("ailfred-40-0.json")).unwrap();
        // 10 is gone, 20 is another Ailfred running, 30 is this one, 40 is a folder.
        let removed = sweep(&dir, 30, |pid| pid == 20);
        let mut removed: Vec<String> = removed.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        removed.sort();
        assert_eq!(removed, ["ailfred-10-0.json", "ailfred-10-3.json"]);
        assert_eq!(names(&dir), ["ailfred-10.json", "ailfred-20-0.json", "ailfred-30-1.json", "ailfred-40-0.json", "notes.txt"]);
    }

    #[test]
    fn a_folder_that_cant_be_read_removes_nothing() {
        assert!(sweep(Path::new("/nonexistent/ailfred"), 1, |_| false).is_empty());
    }

    #[test]
    fn this_process_is_alive_and_a_pid_past_the_limit_isnt() {
        assert!(alive(std::process::id()));
        assert!(alive(1), "launchd: another user's process still counts");
        assert!(!alive(u32::MAX));
    }
}
