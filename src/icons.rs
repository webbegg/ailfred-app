//! The app's own vector icons (SVG subset, drawn in their element's colour). Kit icons
//! (`rune_kit::glyphs`) cover close and sparkle.

use rune::Icon;

/// Send: an arrow pointing up.
pub static ARROW_UP: Icon = Icon {
    name: "capibarapp-arrow-up",
    view: 16.0,
    body: r#"<path d="M8 13V3M3.5 7.5L8 3L12.5 7.5" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
