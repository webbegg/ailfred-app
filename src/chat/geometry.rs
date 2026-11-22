//! Where the panel rests and hides on a display, and how large an image is shown.

use super::*;

pub(super) const GAP: f32 = 12.0;
/// Room past the panel for its shadow.
pub(super) const PAD: f32 = 32.0;

/// What the menu bar and the Dock take at the display's edges (points).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub bottom: f32,
    pub right: f32,
}

/// The menu bar's and Dock's room on a display: `visible` within `frame` (AppKit rects,
/// `[x, y, w, h]` with y up).
pub fn insets(frame: [f32; 4], visible: [f32; 4]) -> Insets {
    let ([fx, fy, fw, fh], [vx, vy, vw, vh]) = (frame, visible);
    Insets { top: (fy + fh - (vy + vh)).max(0.0), bottom: (vy - fy).max(0.0), right: (fx + fw - (vx + vw)).max(0.0) }
}

/// The panel's rect at rest on a `screen` of that size (points, the overlay covers the display):
/// at its right edge, its full height, past the menu bar and the Dock.
pub fn side_rect(screen: [f32; 2], insets: Insets) -> [f32; 4] {
    let [sw, sh] = screen;
    [sw - GAP - insets.right - CHAT_W, insets.top + GAP, CHAT_W, (sh - insets.top - insets.bottom - 2.0 * GAP).max(0.0)]
}

/// The panel's top and right edges' distances from the window's (points): pinned by its right
/// edge, it rests and hides relative to the window whatever screen size it was last told.
pub fn side_pins(insets: Insets) -> [f32; 2] {
    [insets.top + GAP, GAP + insets.right]
}

/// The panel's horizontal offset from its rest: open, there; closed, past the screen's right
/// edge, shadow included.
pub fn offset(open: bool, insets: Insets) -> f32 {
    if open { 0.0 } else { CHAT_W + GAP + insets.right + PAD }
}

/// An image `w` × `h` pixels shown within `max` points, keeping its shape, never enlarged.
pub fn thumb_size(w: u32, h: u32, max: (f32, f32)) -> (f32, f32) {
    if w == 0 || h == 0 {
        return (0.0, 0.0);
    }
    let (w, h) = (w as f32, h as f32);
    let k = (max.0 / w).min(max.1 / h).min(1.0);
    ((w * k).round(), (h * k).round())
}
