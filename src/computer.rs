//! Computer control (roadmap S6): the model sees the screen and works the Mac's apps through
//! tools Ailfred serves, within guards it can't change. Ported from Gentle Dot's helper
//! (`apps/desktop/src-tauri/src/computer`), macOS only.
//!
//! The rules are pure modules with their tests: the rate limit, screenshot geometry, key
//! combos, the apps never targeted, which actions ask first, and the session the user grants.
//! `Desktop` is the seam to the real Mac (`macos`): screenshots, input events, and what the
//! Accessibility API says is under a point. `mcp` serves the tools over JSON-RPC behind those
//! guards, and `control` holds the session the app shares with it. The HTTP server that wires
//! them up comes in a later slice; until then nothing calls them (remove this allow once it does).
#![allow(dead_code, unused_imports)]

pub mod alert;
pub mod blocklist;
pub mod control;
pub mod coords;
pub mod encode;
#[cfg(test)]
mod fake;
pub mod keys;
mod macos;
pub mod mcp;
pub mod rate;
pub mod risk;
pub mod server;
pub mod session;
pub mod windows;

use blocklist::AppIdentity;
use coords::{Point, ScreenshotGeometry};
use keys::KeyCombo;
use risk::{AxElement, Focus};

pub use macos::{MacDesktop, open_privacy_pane, request_permission};

#[cfg(test)]
mod tests {
    #[test]
    fn the_panic_chord_is_option_shift_escape() {
        let chord: rune::Shortcut = super::PANIC_SHORTCUT.parse().expect("a chord Rune reads");
        assert_eq!(chord.to_string(), "alt+shift+escape");
    }
}

use std::sync::{Arc, RwLock};

/// The panic chord: ends computer control's session from anywhere, while it is on.
pub const PANIC_SHORTCUT: &str = "alt+shift+escape";

/// Computer control as the app runs it: the session it shares with the tools, and where they
/// are served (for the model it starts).
pub struct Running {
    pub control: Arc<control::Control>,
    pub endpoint: server::Endpoint,
}

/// Starts serving the tools on a loopback port with a fresh token, switched off until the app
/// turns them on (`Control::set_enabled`). `notify` hears every session change. Once per launch.
pub fn start(notify: impl Fn(&session::StateEvent) + Send + Sync + 'static) -> std::io::Result<Running> {
    let token = server::new_token()?;
    let control = Arc::new(control::Control::new(notify));
    let helper = mcp::Helper::new(
        Arc::new(MacDesktop::default()),
        Arc::new(alert::MacDialogs),
        Arc::new(SystemClock),
        control.clone(),
        token.clone(),
        std::process::id() as i32,
    );
    let endpoint = server::start(Arc::new(helper), token)?;
    // A session that times out says so when it does, not at the next tool call.
    let watched = Arc::downgrade(&control);
    std::thread::Builder::new().name("computer-timeout".into()).spawn(move || {
        while let Some(control) = watched.upgrade() {
            control.expire(SystemClock.now_ms());
            drop(control);
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    })?;
    Ok(Running { control, endpoint })
}

/// Where the tools are served while computer control is on (any thread): what a model started
/// now is handed. None while it's off.
static ENDPOINT_NOW: RwLock<Option<server::Endpoint>> = RwLock::new(None);

pub fn endpoint_now() -> Option<server::Endpoint> {
    ENDPOINT_NOW.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Turns `running` on or off, and publishes where it's served while it's on.
pub fn set_enabled(running: &Running, on: bool) {
    running.control.set_enabled(on);
    *ENDPOINT_NOW.write().unwrap_or_else(|e| e.into_inner()) = on.then(|| running.endpoint.clone());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

/// One input the tools post. Points are global, in points.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Move(Point),
    Click { at: Point, button: MouseButton, count: u8 },
    Drag { from: Point, to: Point },
    /// `dy > 0` scrolls down and `dx > 0` scrolls right, in pixels.
    Scroll { at: Point, dx: i32, dy: i32 },
    Key(KeyCombo),
    /// A chunk of text without newlines (newlines are sent as Return).
    Text(String),
}

/// An encoded screenshot of the display under the pointer, Ailfred's windows left out.
#[derive(Debug, Clone)]
pub struct Capture {
    pub data: Vec<u8>,
    pub mime_type: &'static str,
    pub geometry: ScreenshotGeometry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub bundle_id: Option<String>,
    pub active: bool,
}

/// The macOS permissions computer control needs, and whether screenshots work at all
/// (ScreenCaptureKit takes them from macOS 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub accessibility: bool,
    pub screen_recording: bool,
    pub screenshots_supported: bool,
}

/// Which macOS permission the user is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    Accessibility,
    ScreenRecording,
}

/// The Mac, as the tools see it. Nothing behind it decides policy.
pub trait Desktop: Send + Sync {
    fn permissions(&self) -> Permissions;
    /// A screenshot of the display under the pointer, downscaled to `coords::MAX_LONG_EDGE`.
    fn capture(&self) -> Result<Capture, String>;
    fn post(&self, event: &InputEvent) -> Result<(), String>;
    fn list_apps(&self) -> Result<Vec<AppInfo>, String>;
    /// Opens (or activates) an app by name or bundle id.
    fn open_app(&self, name: &str) -> Result<(), String>;
    /// The Accessibility element at a point, then its ancestors; empty when unknown.
    fn element_at(&self, at: Point) -> Vec<AxElement>;
    fn focused_element(&self) -> Option<Focus>;
    fn frontmost_app(&self) -> Option<AppIdentity>;
    /// The app owning the focused Accessibility element, which can differ from the frontmost
    /// app (a non-activating panel such as a password manager's quick access).
    fn focused_app(&self) -> Option<AppIdentity>;
    /// The app owning the topmost window under a point.
    fn window_owner_at(&self, at: Point) -> Option<AppIdentity>;
    /// Brings an app back to the front (after a dialog took the focus).
    fn activate(&self, pid: i32);
}

/// Native dialogs Ailfred shows itself, out of the model's reach.
pub trait Dialogs: Send + Sync {
    /// "Allow Ailfred to see and control this Mac for 30 minutes?" (Allow / Deny).
    fn ask_grant(&self) -> bool;
    /// A per-action confirmation showing the action and its target.
    fn confirm(&self, message: &str) -> bool;
}

pub trait Clock: Send + Sync {
    /// Unix epoch milliseconds.
    fn now_ms(&self) -> u64;
    fn sleep_ms(&self, ms: u64);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default()
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}
