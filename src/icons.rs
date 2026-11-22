//! The app's own vector icons (SVG subset, drawn in their element's colour). Kit icons
//! (`rune_kit::glyphs`) cover close and sparkle.

use rune::Icon;

/// Send: an arrow pointing up.
/// Retry: an arrow coming round.
pub static RETRY: Icon = Icon {
    name: "capibarapp-retry",
    view: 16.0,
    body: r#"<path d="M13 8A5 5 0 1 1 11.5 4.4M11.8 1.8V4.6H9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

pub static ARROW_UP: Icon = Icon {
    name: "capibarapp-arrow-up",
    view: 16.0,
    body: r#"<path d="M8 13V3M3.5 7.5L8 3L12.5 7.5" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
