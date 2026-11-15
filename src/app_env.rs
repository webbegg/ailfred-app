//! Where ailfred runs (installed bundle or a terminal) and what it does about it at startup:
//! a log file instead of the lost stderr, and the Screen Recording permission check.

use std::os::fd::AsRawFd;

use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

/// Whether this process runs from its installed bundle, `…/ailfred.app/Contents/MacOS/…`.
pub fn in_bundle() -> bool {
    std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().contains("/ailfred.app/Contents/MacOS/"))
}

/// Launched from Finder, `open` or at login, stderr goes nowhere: append it to
/// `~/Library/Logs/ailfred.log` instead. One open handle for the process's life.
pub fn log_to_file() {
    let Some(home) = std::env::var_os("HOME") else { return };
    let path = std::path::Path::new(&home).join("Library/Logs/ailfred.log");
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
    // SAFETY: dup2 onto stderr's descriptor; `file` stays valid for the call.
    unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
}

/// Where screenshots are saved: ~/Desktop.
pub fn screenshots_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/Desktop")
}

/// Whether Screen Recording is allowed, without prompting or logging.
pub fn screen_recording_allowed_quiet() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// Checks Screen Recording; without it, asks macOS to prompt (once per app; afterwards the
/// user goes to System Settings). Returns whether capture is allowed now.
pub fn screen_recording_allowed() -> bool {
    if CGPreflightScreenCaptureAccess() {
        return true;
    }
    eprintln!(
        "ailfred: Screen Recording is off. Turn on ailfred in System Settings > Privacy & Security > \
         Screen & System Audio Recording, then reopen it."
    );
    CGRequestScreenCaptureAccess()
}
