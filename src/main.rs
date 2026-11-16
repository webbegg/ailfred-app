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
mod export;
mod login;
mod marks;
mod output;
mod settings;
mod settings_ui;
mod shot;
mod tray;

use std::sync::Arc;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};

use rune::{
    App, AppSpec, Cx, DisplayTarget, El, KeyPolicy, NativeDisplayId, OverlayHandle, OverlayInput, OverlayLevel,
    AppHandle, OverlayOptions, WindowHandle, WindowOptions, OverlayPlacement, OverlaySpec, RunOptions, Tx, WindowSpec, el,
};

use crate::shot::{Mode, Overlay};
use crate::settings::{CaptureShortcut, Settings};
use crate::settings_ui::SettingsUi;
use crate::tray::{Action, Tray};

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
    /// The capture shortcut's registration (shared with the settings window).
    capture_shortcut: CaptureShortcut,
    /// The saved shortcut, registered at the loop's first turn.
    initial_shortcut: Option<rune::Shortcut>,
    settings_window: Option<WindowHandle>,
    settings_ui: Option<AppHandle<SettingsUi>>,
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
        // The checkmark starts off; the startup thread reads the real state off the UI thread.
        match Tray::new(handle, self.installed, false) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("ailfred: menu-bar icon failed: {e}"),
        }
    }

    /// Registers the saved capture shortcut (main thread, loop running) and shows it in the menu.
    fn register_saved_shortcut(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        let shortcut = self.initial_shortcut.take();
        let label = shortcut.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "none (menu only)".into());
        match self.capture_shortcut.replace(&handle, shortcut.as_ref()) {
            Ok(()) => {
                eprintln!("ailfred ready: capture shortcut {label}");
                self.show_capture_shortcut(shortcut.as_ref());
            }
            Err(e) => {
                eprintln!("ailfred: the capture shortcut {label} couldn't be registered: {e}");
                if let Some(ui) = &self.settings_ui {
                    let message = format!("The shortcut isn't active: {e}. Record it again or pick another.");
                    ui.update(move |ui, _| ui.shortcut_failed(message));
                }
            }
        }
    }

    pub fn show_capture_shortcut(&self, shortcut: Option<&rune::Shortcut>) {
        if let Some(tray) = &self.tray {
            tray.show_capture_shortcut(shortcut);
        }
    }

    fn set_permission(&mut self, allowed: bool) {
        if let Some(tray) = &mut self.tray {
            tray.set_permission(allowed);
        }
    }

    pub fn on_menu(&mut self, action: Action, tx: &mut Tx) {
        match action {
            // Our windows (the menu, the highlight) are excluded from the capture; the pause lets
            // the menu bar settle and the app under the menu redraw.
            Action::Capture => start_capture(self, Duration::from_millis(200), Mode::Area),
            Action::CaptureWindow => start_capture(self, Duration::from_millis(200), Mode::Window),
            Action::CaptureFullScreen => start_capture(self, Duration::from_millis(200), Mode::FullScreen),
            Action::OpenFolder => open(&app_env::screenshots_dir()),
            Action::OpenPermissionSettings => open("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"),
            Action::ToggleLogin => {
                let (Some(handle), Some(tray)) = (self.handle.clone(), &self.tray) else { return };
                // muda already flipped the checkmark: its new state is the request. The service
                // manager can block, so ask off the UI thread; the item stays disabled meanwhile.
                let on = tray.login_checked();
                tray.set_login_busy(true);
                std::thread::spawn(move || {
                    let now = login::set(on);
                    handle.update(move |host, _| {
                        if let Some(tray) = &host.tray {
                            tray.set_login(now);
                            tray.set_login_busy(false);
                        }
                    });
                });
            }
            Action::Settings => {
                if let Some(window) = &self.settings_window {
                    window.open();
                }
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
    Host {
        overlay: None,
        handle: None,
        tray: None,
        installed: false,
        capturing: Arc::default(),
        capture_shortcut: CaptureShortcut::default(),
        initial_shortcut: None,
        settings_window: None,
        settings_ui: None,
    }
}

fn host_view(_: &Host, _: &mut Cx<Host>) -> El {
    el()
}

pub(crate) fn on_shortcut(host: &mut Host, _: &mut Tx) {
    start_capture(host, Duration::ZERO, Mode::Area);
}

/// Captures off the UI thread after `delay`, then shows the frozen display (starting in `mode`) on the display it came
/// from. A missing Screen Recording permission shows in the menu.
fn start_capture(host: &mut Host, delay: Duration, mode: Mode) {
    let Some(overlay) = host.overlay.clone() else { return };
    if host.capturing.swap(true, Ordering::AcqRel) {
        return;
    }
    let capturing = Busy(host.capturing.clone());
    let handle = host.handle.clone();
    std::thread::spawn(move || {
        let _capturing = capturing; // cleared on drop, even if the capture panics
        std::thread::sleep(delay);
        let result = capture::display_under_pointer();
        let allowed = result.is_ok() || app_env::screen_recording_allowed_quiet();
        if let Some(handle) = &handle {
            handle.update(move |host, _| host.set_permission(allowed));
        }
        match result {
            Ok(shot) => {
                let display = shot.display_id;
                // Queued in this order: state, placement, reveal. They may be applied over more than
                // one event-loop turn; the update is queued before `show`, and show refreshes first.
                overlay.update(move |s, _| s.freeze(shot, mode));
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
                    // Belt and braces: the capture filter also leaves out all of our windows.
                    exclude_from_capture: true,
                    prewarm: true,
                    ..Default::default()
                })
                .on_key(shot::on_key),
            Overlay::new(handle.clone()),
        );
        let settings = Settings::open();
        let initial_shortcut = settings.shortcut();
        let capture_shortcut = CaptureShortcut::default();
        let ui = SettingsUi::new(settings, handle.clone(), capture_shortcut.clone());
        let hidden = WindowOptions { visible: false, resizable: false, min_size: Some((0.0, 0.0)), ..Default::default() };
        let (settings_ui, settings_window) = app.window(
            WindowSpec::new("settings", AppSpec::new("ailfred Settings", (460.0, 260.0), SettingsUi::placeholder, settings_ui::view))
                .state(ui)
                .options(hidden),
        );
        let own = handle.clone();
        let app = app.init(move || Host {
            overlay: Some(overlay),
            handle: Some(own),
            tray: None,
            installed,
            capturing: Arc::default(),
            capture_shortcut,
            initial_shortcut,
            settings_window: Some(settings_window),
            settings_ui: Some(settings_ui),
        });
        // Runs on the main thread at the loop's first turn: the status item needs a running NSApp,
        // and runtime shortcut registration needs the host started.
        handle.update(|host, _| {
            host.add_tray();
            host.register_saved_shortcut();
        });
        // Off the UI thread: both talk to system services.
        std::thread::spawn(move || {
            if installed {
                login::register_if_needed();
            }
            let allowed = app_env::screen_recording_allowed();
            let login = installed && login::enabled();
            handle.update(move |host, _| {
                host.set_permission(allowed);
                if let Some(tray) = &host.tray {
                    tray.set_login(login);
                }
            });
        });
        vec![WindowSpec::from_app("host", app)]
    });
}
