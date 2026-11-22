//! The app's own vector icons (SVG subset, drawn in their element's colour). Kit icons
//! (`rune_kit::glyphs`) cover close and sparkle.

use rune::Icon;

/// Send: an arrow pointing up.
/// Retry: an arrow coming round.
pub static RETRY: Icon = Icon {
    name: "ailfred-retry",
    view: 16.0,
    body: r#"<path d="M13 8A5 5 0 1 1 11.5 4.4M11.8 1.8V4.6H9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// Open in a window: a window with an arrow leaving it.
pub static POP_OUT: Icon = Icon {
    name: "ailfred-pop-out",
    view: 16.0,
    body: r#"<path d="M7 3H4.5A1.5 1.5 0 0 0 3 4.5v7A1.5 1.5 0 0 0 4.5 13h7a1.5 1.5 0 0 0 1.5-1.5V9M9.5 3H13v3.5M13 3L7.5 8.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// Back to the side panel: a window with its right side filled.
pub static DOCK_SIDE: Icon = Icon {
    name: "ailfred-dock-side",
    view: 16.0,
    body: r#"<rect x="2.75" y="3.25" width="10.5" height="9.5" rx="1.75" stroke="currentColor" stroke-width="1.5" fill="none"/><rect x="9" y="4.5" width="3" height="7" rx="0.75" fill="currentColor"/>"#,
};

pub static ARROW_UP: Icon = Icon {
    name: "ailfred-arrow-up",
    view: 16.0,
    body: r#"<path d="M8 13V3M3.5 7.5L8 3L12.5 7.5" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
