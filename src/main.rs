//! ailfred: an area screenshot tool. The hotkey captures the display under the pointer, then
//! shows it frozen in a frameless, transparent overlay above every other window. Drag to select;
//! on release the area goes to the clipboard and to ~/Desktop as a PNG.
//!
//! On macOS Rune hosts the overlay in a non-activating `NSPanel` at AppKit level 26 (above the
//! menu bar and Dock), on every Space and over full-screen apps.
//!
//! Cmd+Shift+Space captures. Escape cancels. Cmd+Q quits.

mod app_env;
mod capture;
mod login;
mod shot;
mod tray;

use std::sync::Arc;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};

use rune::{
    App, AppSpec, Cx, DisplayTarget, El, KeyPolicy, NativeDisplayId, OverlayHandle, OverlayInput, OverlayLevel,
    AppHandle, OverlayOptions, OverlayPlacement, OverlaySpec, RunOptions, Tx, WindowSpec, el,
};

use crate::shot::Overlay;
use crate::tray::{Action, Tray};

/// The chord that captures the screen under the pointer.
const CAPTURE_SHORTCUT: &str = "cmd+shift+space";

/// The windowless host app: owns the overlay and the menu-bar icon; the global shortcut and the
/// menu act on it.
pub struct Host {
    pub(crate) overlay: Option<OverlayHandle<Overlay>>,
    /// For work that finishes on another thread and reports back.
    handle: Option<AppHandle<Host>>,
    tray: Option<Tray>,
    /// Running from ~/Applications/ailfred.app (login item and log file only there).
    installed: bool,
    /// A capture is running on its thread; a second press waits for it.
    capturing: Arc<AtomicBool>,
}

impl Host {
    pub fn hide_overlay(&self) {
        if let Some(overlay) = &self.overlay {
            overlay.hide();
        }
    }

    /// Creates the menu-bar icon. Main thread, event loop running: called from a queued update.
    fn add_tray(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        let login = self.installed && login::enabled();
        match Tray::new(handle, self.installed, login) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("ailfred: menu-bar icon failed: {e}"),
        }
    }

    fn set_permission(&mut self, allowed: bool) {
        if let Some(tray) = &mut self.tray {
            tray.set_permission(allowed);
        }
    }

    pub fn on_menu(&mut self, action: Action, tx: &mut Tx) {
        match action {
            // Let the menu finish closing so it isn't in the capture.
            Action::Capture => start_capture(self, Duration::from_millis(250)),
            Action::OpenFolder => open(&app_env::screenshots_dir()),
            Action::OpenPermissionSettings => open("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"),
            Action::ToggleLogin => {
                let Some(handle) = self.handle.clone() else { return };
                // The service manager can block; ask off the UI thread, then show the result.
                let on = !login::enabled();
                std::thread::spawn(move || {
                    let now = login::set(on);
                    handle.update(move |host, _| {
                        if let Some(tray) = &host.tray {
                            tray.set_login(now);
                        }
                    });
                });
            }
            Action::Quit => tx.quit(),
        }
    }
}

/// Opens a path or URL with its default app (`open`), without waiting for it.
fn open(target: &str) {
    if let Err(e) = std::process::Command::new("open").arg(target).spawn() {
        eprintln!("ailfred: open {target} failed: {e}");
    }
}

fn host_init() -> Host {
    Host { overlay: None, handle: None, tray: None, installed: false, capturing: Arc::default() }
}

fn host_view(_: &Host, _: &mut Cx<Host>) -> El {
    el()
}

fn on_shortcut(host: &mut Host, _: &mut Tx) {
    start_capture(host, Duration::ZERO);
}

/// Captures off the UI thread after `delay`, then shows the frozen display on the display it came
/// from. A missing Screen Recording permission shows in the menu.
fn start_capture(host: &mut Host, delay: Duration) {
    let Some(overlay) = host.overlay.clone() else { return };
    if host.capturing.swap(true, Ordering::AcqRel) {
        return;
    }
    let capturing = Busy(host.capturing.clone());
    let handle = host.handle.clone();
    let exclude = overlay.window_number();
    std::thread::spawn(move || {
        let _capturing = capturing; // cleared on drop, even if the capture panics
        std::thread::sleep(delay);
        let result = capture::display_under_pointer(exclude);
        let allowed = result.is_ok() || app_env::screen_recording_allowed_quiet();
        if let Some(handle) = &handle {
            handle.update(move |host, _| host.set_permission(allowed));
        }
        match result {
            Ok(shot) => {
                let display = shot.display_id;
                // Queued in this order: state, placement, reveal. They may be applied over more than
                // one event-loop turn; the update is queued before `show`, and show refreshes first.
                overlay.update(move |s, _| s.freeze(shot));
                overlay.set_display(DisplayTarget::Native(NativeDisplayId::MacOS(display)));
                overlay.show();
                overlay.request_keys();
            }
            Err(e) => eprintln!("ailfred: {e}"),
        }
    });
}

/// Clears the "capture running" flag when dropped.
struct Busy(Arc<AtomicBool>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn main() {
    let installed = app_env::in_bundle();
    if installed {
        app_env::log_to_file();
    }
    // accessory: no Dock icon and no activation at launch.
    rune::run_windows_with(RunOptions { accessory: true }, move |_displays| {
        let mut app = App::new(AppSpec::new("ailfred", (1.0, 1.0), host_init, host_view)).windowless();
        let handle = app.handle();
        let overlay = app.overlay(
            // The size is replaced by the display's size: FullScreen placement.
            OverlaySpec::new("ailfred overlay", (0.0, 0.0), shot::view)
                .options(OverlayOptions {
                    display: DisplayTarget::UnderPointer,
                    placement: Some(OverlayPlacement::FullScreen),
                    level: OverlayLevel::AboveStatus,
                    input: OverlayInput::All,
                    keys: KeyPolicy::OnDemand,
                    // Belt and braces: the capture filter also excludes this window by number.
                    exclude_from_capture: true,
                    prewarm: true,
                    ..Default::default()
                })
                .on_key(shot::on_key),
            Overlay::new(handle.clone()),
        );
        let own = handle.clone();
        let app = app
            .init(move || Host {
                overlay: Some(overlay),
                handle: Some(own),
                tray: None,
                installed,
                capturing: Arc::default(),
            })
            .global_shortcut(CAPTURE_SHORTCUT, on_shortcut);
        // Runs on the main thread at the loop's first turn: the status item needs a running NSApp.
        handle.update(|host, _| host.add_tray());
        eprintln!("ailfred ready: press ⌘⇧Space to capture");
        // Off the UI thread: both talk to system services.
        std::thread::spawn(move || {
            if installed {
                login::register_if_needed();
            }
            let allowed = app_env::screen_recording_allowed();
            handle.update(move |host, _| {
                host.set_permission(allowed);
                if let Some(tray) = &host.tray {
                    tray.set_login(host.installed && login::enabled());
                }
            });
        });
        vec![WindowSpec::from_app("host", app)]
    });
}
