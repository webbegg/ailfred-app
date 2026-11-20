//! The app's own vector icons (SVG subset, drawn in their element's colour). Kit icons
//! (`rune_kit::glyphs`) cover close and sparkle.

use rune::Icon;

/// Send: an arrow pointing up.
pub static ARROW_UP: Icon = Icon {
    name: "capibarapp-arrow-up",
    view: 16.0,
    body: r#"<path d="M8 13V3M3.5 7.5L8 3L12.5 7.5" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// An attached screenshot: a frame with a sun and a hill.
pub static IMAGE: Icon = Icon {
    name: "capibarapp-image",
    view: 16.0,
    body: r#"<rect x="2" y="3" width="12" height="10" rx="2" stroke="currentColor" stroke-width="1.3" fill="none"/><circle cx="6" cy="6.5" r="1.2" fill="currentColor"/><polyline points="2.5,12 6.5,8.5 9,10.5 11,9 13.5,11.5" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round" stroke-linecap="round" fill="none"/>"#,
};

/// Edit the screenshot: a pencil.
pub static PENCIL: Icon = Icon {
    name: "capibarapp-pencil",
    view: 16.0,
    body: r#"<path d="M10.5 3L13 5.5L5.5 13H3V10.5L10.5 3Z" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round" fill="none"/><path d="M9 4.5L11.5 7" stroke="currentColor" stroke-width="1.4"/>"#,
};
