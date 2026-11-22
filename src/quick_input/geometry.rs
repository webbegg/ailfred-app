//! Where the panel is, at rest and sliding in and out, and how tall its strip of chips makes it.

use super::*;

/// The panel's vertical offset from its rest, opacity and scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub dy: f32,
    pub alpha: f32,
    pub scale: f32,
}

/// Where the panel is for `open` and `tucked`: a closed or tucked panel is below the screen
/// (tucked also fades).
pub fn pose(open: bool, tucked: bool) -> Pose {
    match (open, tucked) {
        (true, false) => Pose { dy: 0.0, alpha: 1.0, scale: 1.0 },
        (false, _) => Pose { dy: TUCK, alpha: 1.0, scale: 1.0 },
        (true, true) => Pose { dy: TUCK, alpha: 0.0, scale: 1.0 },
    }
}

/// The panel's rect at rest at the bottom of a `screen` of that size, over a Dock `dock` points
/// tall.
pub fn bottom_panel(screen: [f32; 2], dock: f32) -> [f32; 4] {
    [(screen[0] - PANEL_W) / 2.0, screen[1] - dock.max(0.0) - MARGIN - PANEL_H, PANEL_W, PANEL_H]
}

/// The input's panel at the bottom of a `screen` over a Dock `dock` points tall, with `chips`
/// images in its strip (`QuickInput::panel_rect` once placed).
pub fn input_panel(screen: [f32; 2], dock: f32, chips: usize) -> [f32; 4] {
    grown(bottom_panel(screen, dock), chips)
}

/// `rest` grown upward by the strip of `chips` images.
pub(super) fn grown([x, y, w, h]: [f32; 4], chips: usize) -> [f32; 4] {
    let grow = strip_h(chips);
    [x, y - grow, w, h + grow]
}

/// Whether `n` chips show smaller, when only `full` fit at their size.
pub fn compact(n: usize, full: usize) -> bool {
    n > full
}

/// The height the input's strip of `n` chips adds above its field row: none without chips.
pub fn strip_h(n: usize) -> f32 {
    match n {
        0 => 0.0,
        n if compact(n, INPUT_FULL_CHIPS) => STRIP_TOP + THUMB_COMPACT.1,
        _ => STRIP_TOP + THUMB.1,
    }
}

/// The bottom gap the Dock takes on a screen: `visible`'s bottom over `frame`'s (AppKit
/// rects, `[x, y, w, h]` with y up). Zero when the Dock isn't at the bottom (or is hidden).
pub fn dock_inset(frame: [f32; 4], visible: [f32; 4]) -> f32 {
    (visible[1] - frame[1]).max(0.0)
}
