//! Where Ailfred runs (installed bundle or a terminal) and what it does about it at startup:
//! a log file instead of the lost stderr, and the Screen Recording permission check.

use std::os::fd::AsRawFd;

use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

/// The bundle's name, `<BUNDLE_NAME>.app`; `scripts/install-mac` builds it with the same `NAME=`.
pub const BUNDLE_NAME: &str = "Ailfred";

/// Whether this process runs from its installed bundle, `…/Ailfred.app/Contents/MacOS/…`.
pub fn in_bundle() -> bool {
    std::env::current_exe().is_ok_and(|exe| is_bundle_exe(&exe.to_string_lossy()))
}

fn is_bundle_exe(path: &str) -> bool {
    path.contains(&format!("/{BUNDLE_NAME}.app/Contents/MacOS/"))
}

/// Launched from Finder, `open` or at login, stderr goes nowhere: append it to
/// `~/Library/Logs/Ailfred.log` instead. One open handle for the process's life.
pub fn log_to_file() {
    let Some(home) = std::env::var_os("HOME") else { return };
    let path = std::path::Path::new(&home).join("Library/Logs/Ailfred.log");
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
    // SAFETY: dup2 onto stderr's descriptor; `file` stays valid for the call.
    unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
}

/// Logs what macOS takes the app for ("Foreground": Dock icon and ⌘Tab; "UIElement": menu bar
/// only) a second after `what`, off the UI thread: the Dock icon's switch shows in the log.
pub fn log_app_type_soon(what: &'static str) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let pid = std::process::id().to_string();
        let out = std::process::Command::new("/usr/bin/lsappinfo").args(["info", "-only", "ApplicationType", &pid]).output();
        let kind = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_else(|e| e.to_string());
        eprintln!("Ailfred: {what}; macOS says {kind}");
    });
}

/// Where screenshots are saved: the folder chosen in Settings (default ~/Desktop).
pub fn screenshots_dir() -> String {
    crate::settings::folder_now().to_string_lossy().into_owned()
}

/// System Settings' Screen Recording list (Privacy & Security).
pub const SCREEN_RECORDING_SETTINGS: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

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
        "Ailfred: Screen Recording is off. Turn on Ailfred in System Settings > Privacy & Security > \
         Screen & System Audio Recording, then reopen it."
    );
    CGRequestScreenCaptureAccess()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_only_the_installed_bundle() {
        assert!(is_bundle_exe("/Users/x/Applications/Ailfred.app/Contents/MacOS/ailfred"));
        assert!(!is_bundle_exe("/Users/x/project/target/release/ailfred"));
        assert!(!is_bundle_exe("/Users/x/Applications/Other.app/Contents/MacOS/ailfred"));
        assert!(!is_bundle_exe("/Users/x/Applications/XAilfred.app/Contents/MacOS/ailfred"));
        assert!(!is_bundle_exe("/Users/x/Applications/Ailfred.app/Contents/Resources/ailfred"));
        assert!(!is_bundle_exe("/Users/x/Applications/Capibarapp.app/Contents/MacOS/capibarapp"), "the old name");
    }

    #[test]
    fn install_script_builds_the_same_bundle_name() {
        let script = include_str!("../scripts/install-mac");
        let name = format!("NAME={BUNDLE_NAME}");
        assert!(script.lines().any(|l| l == name), "install-mac has no line {name}");
    }
}
