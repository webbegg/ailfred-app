//! Computer control (roadmap S6): the model sees the screen and works the Mac's apps through
//! tools Ailfred serves, within guards it can't change. Ported from Gentle Dot's helper
//! (`apps/desktop/src-tauri/src/computer`), macOS only.
//!
//! The rules are pure modules with their tests: the rate limit, screenshot geometry, key
//! combos, the apps never targeted, which actions ask first, and the session the user grants.
//! `Desktop` is the seam to the real Mac (`macos`): screenshots, input events, and what the
//! Accessibility API says is under a point. `mcp` serves the tools over JSON-RPC behind those
//! guards, `server` on a loopback port while the switch is on, and `control` holds the session
//! the app shares with it (Settings' switch, Stop, the panic chord, the timeout). `alert` asks
//! the user natively. pi and Claude Code get the tools while the switch is on (`endpoint_now`).


pub mod alert;
pub mod blocklist;
pub mod control;
pub mod coords;
pub mod encode;
#[cfg(test)]
pub mod fake;
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

pub use macos::{MacDesktop, request_accessibility};

#[cfg(test)]
mod tests {
    use super::fake::{FakeClock, FakeDesktop, FakeDialogs};
    use super::*;
    use std::net::TcpStream;

    #[test]
    fn the_panic_chord_is_option_shift_escape() {
        let chord: rune::Shortcut = super::PANIC_SHORTCUT.parse().expect("a chord Rune reads");
        assert_eq!(chord.to_string(), "alt+shift+escape");
    }

    #[test]
    fn switched_off_the_tools_port_closes_and_switched_on_again_it_is_a_fresh_port_and_token() {
        let control = Arc::new(control::Control::new(|_| {}));
        let shared = control.clone();
        let mut running = Running::with(control, move |token| {
            mcp::Helper::new(Arc::new(FakeDesktop::default()), Arc::new(FakeDialogs::new(false, false)), Arc::new(FakeClock::new(0)), shared.clone(), token, 1)
        });
        assert!(running.endpoint().is_none() && !running.control.enabled(), "off, and not served, until switched on");
        running.switch(true).unwrap();
        let first = running.endpoint().cloned().expect("served while on");
        assert!(running.control.enabled());
        running.switch(true).unwrap();
        assert_eq!(running.endpoint(), Some(&first), "on again: the same");
        let address = first.url.trim_start_matches("http://").trim_end_matches("/mcp").to_owned();
        let _idle = TcpStream::connect(&address).unwrap();

        running.switch(false).unwrap();
        assert!(running.endpoint().is_none() && !running.control.enabled());
        let after = TcpStream::connect(&address);
        assert!(after.as_ref().is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused), "{after:?}");

        running.switch(true).unwrap();
        let second = running.endpoint().cloned().expect("served again");
        assert_ne!(second.token, first.token, "a token handed out before is dead");
    }

    #[test]
    fn switched_off_the_endpoint_is_unpublished_and_the_port_closed_before_the_session_ends() {
        // What the session's end saw: the tools' port still open, and the endpoint still handed
        // to a model started now.
        let seen = Arc::new(std::sync::Mutex::new(None::<(bool, bool)>));
        let address = Arc::new(std::sync::Mutex::new(None::<server::Endpoint>));
        let (sink, ours) = (seen.clone(), address.clone());
        let control = Arc::new(control::Control::new(move |state| {
            let Some(endpoint) = ours.lock().unwrap().clone().filter(|_| !state.active) else { return };
            let port = endpoint.url.trim_start_matches("http://").trim_end_matches("/mcp").to_owned();
            *sink.lock().unwrap() = Some((TcpStream::connect(port).is_ok(), endpoint_now().as_ref() == Some(&endpoint)));
        }));
        let shared = control.clone();
        let mut running = Running::with(control.clone(), move |token| {
            mcp::Helper::new(Arc::new(FakeDesktop::default()), Arc::new(FakeDialogs::new(false, false)), Arc::new(FakeClock::new(0)), shared.clone(), token, 1)
        });
        set_enabled(&mut running, true).unwrap();
        *address.lock().unwrap() = running.endpoint().cloned();
        control.grant(0);
        set_enabled(&mut running, false).unwrap();
        assert_eq!(*seen.lock().unwrap(), Some((false, false)), "(port open, endpoint published) when the session ended");
    }
}

use std::sync::{Arc, RwLock};

/// The panic chord: ends computer control's session from anywhere, while it is on.
pub const PANIC_SHORTCUT: &str = "alt+shift+escape";

/// The tools' helper, opened by a token.
type MakeHelper = Box<dyn Fn(String) -> mcp::Helper + Send>;

/// Computer control as the app runs it: the session it shares with the tools, and where they
/// are served while it's on (for the model it starts).
pub struct Running {
    pub control: Arc<control::Control>,
    serving: Option<server::Serving>,
    helper: MakeHelper,
}

impl Running {
    /// Off and not served, until switched on; `helper` makes the tools' helper for a token.
    pub fn with(control: Arc<control::Control>, helper: impl Fn(String) -> mcp::Helper + Send + 'static) -> Running {
        Running { control, serving: None, helper: Box::new(helper) }
    }

    /// Where the tools are served, while they are.
    pub fn endpoint(&self) -> Option<&server::Endpoint> {
        self.serving.as_ref().map(server::Serving::endpoint)
    }

    /// On: serves the tools, if they aren't yet, on a fresh loopback port with a fresh token, and
    /// turns the session's switch on. Off: stops serving (the port closes, the token handed out
    /// is dead, and what its server let in does nothing), then turns the switch off (the session
    /// ends). In that order: a request let in reads the session's epoch before its helper
    /// retires, or sees it retired.
    pub fn switch(&mut self, on: bool) -> std::io::Result<()> {
        if on && self.serving.is_none() {
            let token = server::new_token()?;
            self.serving = Some(server::start(Arc::new((self.helper)(token.clone())), token)?);
        }
        if !on && let Some(serving) = self.serving.take() {
            serving.stop();
        }
        self.control.set_enabled(on);
        Ok(())
    }
}

/// Computer control, switched off and not served until the app turns it on (`set_enabled`).
/// `notify` hears every session change. Once per launch.
pub fn start(notify: impl Fn(&session::StateEvent) + Send + Sync + 'static) -> std::io::Result<Running> {
    let control = Arc::new(control::Control::new(notify));
    let shared = control.clone();
    let helper = move |token| {
        mcp::Helper::new(Arc::new(MacDesktop::default()), Arc::new(alert::MacDialogs), Arc::new(SystemClock), shared.clone(), token, std::process::id() as i32)
    };
    // A session that times out says so when it does, not at the next tool call.
    let watched = Arc::downgrade(&control);
    std::thread::Builder::new().name("computer-timeout".into()).spawn(move || {
        while let Some(control) = watched.upgrade() {
            control.expire(SystemClock.now_ms());
            drop(control);
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    })?;
    Ok(Running::with(control, helper))
}

/// Where the tools are served while computer control is on (any thread): what a model started
/// now is handed. None while it's off.
static ENDPOINT_NOW: RwLock<Option<server::Endpoint>> = RwLock::new(None);

pub fn endpoint_now() -> Option<server::Endpoint> {
    ENDPOINT_NOW.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Turns `running` on or off (`Running::switch`), and publishes where it's served while it's on.
/// Off, it's unpublished first: no model started meanwhile is handed a port that's closing.
pub fn set_enabled(running: &mut Running, on: bool) -> std::io::Result<()> {
    if !on {
        *ENDPOINT_NOW.write().unwrap_or_else(|e| e.into_inner()) = None;
    }
    let switched = running.switch(on);
    *ENDPOINT_NOW.write().unwrap_or_else(|e| e.into_inner()) = running.endpoint().filter(|_| running.control.enabled()).cloned();
    switched
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
