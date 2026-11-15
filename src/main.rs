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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rune::{
    App, AppSpec, Cx, DisplayTarget, El, KeyPolicy, NativeDisplayId, OverlayHandle, OverlayInput, OverlayLevel,
    OverlayOptions, OverlayPlacement, OverlaySpec, RunOptions, Tx, WindowSpec, el,
};

use crate::shot::Overlay;

/// The chord that captures the screen under the pointer.
const CAPTURE_SHORTCUT: &str = "cmd+shift+space";

/// The windowless host app: owns the overlay handle so the global shortcut can reach it.
pub struct Host {
    pub(crate) overlay: Option<OverlayHandle<Overlay>>,
    /// A capture is running on its thread; a second press waits for it.
    capturing: Arc<AtomicBool>,
}

impl Host {
    pub fn hide_overlay(&self) {
        if let Some(overlay) = &self.overlay {
            overlay.hide();
        }
    }
}

fn host_init() -> Host {
    Host { overlay: None, capturing: Arc::default() }
}

fn host_view(_: &Host, _: &mut Cx<Host>) -> El {
    el()
}

/// Captures off the UI thread, then shows the frozen display on the display it came from.
fn on_shortcut(host: &mut Host, _: &mut Tx) {
    let Some(overlay) = host.overlay.clone() else { return };
    if host.capturing.swap(true, Ordering::AcqRel) {
        return;
    }
    let capturing = Busy(host.capturing.clone());
    let exclude = overlay.window_number();
    std::thread::spawn(move || {
        let _capturing = capturing; // cleared on drop, even if the capture panics
        match capture::display_under_pointer(exclude) {
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
        let host = app.handle();
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
            Overlay::new(host),
        );
        let app = app
            .init(move || Host { overlay: Some(overlay), capturing: Arc::default() })
            .global_shortcut(CAPTURE_SHORTCUT, on_shortcut);
        eprintln!("ailfred ready: press ⌘⇧Space to capture");
        // Off the UI thread: both talk to system services.
        std::thread::spawn(move || {
            if installed {
                login::register_if_needed();
            }
            app_env::screen_recording_allowed();
        });
        vec![WindowSpec::from_app("host", app)]
    });
}
