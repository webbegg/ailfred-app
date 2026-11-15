//! ailfred: one frameless, transparent overlay covering the whole screen under the pointer,
//! above every other window. On macOS Rune hosts it in a non-activating `NSPanel` at
//! AppKit level 26 (above the menu bar and Dock), on every Space and over full-screen apps.
//!
//! Cmd+Shift+Space shows it on the current screen. Escape hides it. Cmd+Q quits.

use rune::{
    App, AppHandle, AppSpec, Color, Cx, DisplayTarget, El, Key, KeyEvent, KeyPolicy, OverlayHandle, OverlayInput,
    OverlayLevel, OverlayOptions, OverlayPlacement, OverlaySpec, RunOptions, Tx, WindowSpec, col, el, text,
};

/// The chord that brings the overlay up on the screen under the pointer.
const SHOW_SHORTCUT: &str = "cmd+shift+space";

/// The overlay's own state; screenshot tools will live here. An overlay drops `Tx::close_window`
/// and `Tx::quit` from its handlers (it has no window to ask), so it acts through the host.
struct Overlay {
    host: AppHandle<Host>,
}

fn overlay_view(_: &Overlay, _: &mut Cx<Overlay>) -> El {
    let hint = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.85 };
    col().w_full().h_full().center().gap(8.0)
        // A light dim so the overlay is visible; the window itself stays transparent.
        .fill(Color { r: 0.0, g: 0.0, b: 0.0, a: 0.25 })
        .child(text("ailfred").font(rune::tok::type_::TITLE).color(hint))
        .child(text("Esc to hide · ⌘Q to quit").color(hint))
}

fn overlay_key(s: &mut Overlay, event: &KeyEvent, _: &mut Tx) -> bool {
    if event.key == Key::Escape {
        // Hide, keeping the panel and its GPU surface warm for the next show.
        s.host.update(|host, _| if let Some(overlay) = &host.overlay { overlay.hide() });
        return true;
    }
    if event.is_command('q') {
        s.host.quit();
        return true;
    }
    false
}

/// The windowless host app: owns the overlay handle so the global shortcut can reach it.
struct Host {
    overlay: Option<OverlayHandle<Overlay>>,
}

fn host_init() -> Host {
    Host { overlay: None }
}

fn host_view(_: &Host, _: &mut Cx<Host>) -> El {
    el()
}

/// Re-resolves the screen under the pointer, then shows the overlay there and takes keys.
/// All three requests land in the same event-loop turn, so placement happens before reveal.
fn show_on_current_screen(overlay: &OverlayHandle<Overlay>) {
    overlay.set_display(DisplayTarget::UnderPointer);
    overlay.show();
    overlay.request_keys();
}

fn on_shortcut(host: &mut Host, _: &mut Tx) {
    if let Some(overlay) = &host.overlay {
        show_on_current_screen(overlay);
    }
}

fn main() {
    // accessory: no Dock icon and no activation at launch.
    rune::run_windows_with(RunOptions { accessory: true }, |_displays| {
        let mut app = App::new(AppSpec::new("ailfred", (1.0, 1.0), host_init, host_view)).windowless();
        let host = app.handle();
        let overlay = app.overlay(
            // The size is replaced by the display's size: FullScreen placement.
            OverlaySpec::new("ailfred overlay", (0.0, 0.0), overlay_view)
                .options(OverlayOptions {
                    display: DisplayTarget::UnderPointer,
                    placement: Some(OverlayPlacement::FullScreen),
                    level: OverlayLevel::AboveStatus,
                    input: OverlayInput::All,
                    keys: KeyPolicy::OnDemand,
                    // Later screenshots must not capture the overlay itself.
                    exclude_from_capture: true,
                    prewarm: true,
                    ..Default::default()
                })
                .on_key(overlay_key)
                .on_created(|created| eprintln!("ailfred overlay created {created:?}")),
            Overlay { host },
        );
        show_on_current_screen(&overlay);
        let app = app
            .init(move || Host { overlay: Some(overlay) })
            .global_shortcut(SHOW_SHORTCUT, on_shortcut);
        vec![WindowSpec::from_app("host", app)]
    });
}
