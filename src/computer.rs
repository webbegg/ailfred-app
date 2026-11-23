//! Computer control (roadmap S6): the model sees the screen and works the Mac's apps through
//! tools Ailfred serves, within guards it can't change. Ported from Gentle Dot's helper
//! (`apps/desktop/src-tauri/src/computer`), macOS only.
//!
//! The rules are pure modules with their tests: the rate limit, screenshot geometry, key
//! combos, the apps never targeted, which actions ask first, and the session the user grants.
//! `Desktop` is the seam to the real Mac (`macos`): screenshots, input events, and what the
//! Accessibility API says is under a point. The tools that use them come in the next slices;
//! until then nothing calls them (remove this allow once they do).
#![allow(dead_code, unused_imports)]

pub mod blocklist;
pub mod coords;
pub mod encode;
pub mod keys;
mod macos;
pub mod rate;
pub mod risk;
pub mod session;
pub mod windows;

use blocklist::AppIdentity;
use coords::{Point, ScreenshotGeometry};
use keys::KeyCombo;
use risk::{AxElement, Focus};

pub use macos::{MacDesktop, open_privacy_pane, request_permission};

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
