//! The web view: a native window holding a WKWebView that shows the conversation with the React
//! page in `web/`, live, and sends what the user does there to the chat (`docs/web-view.md`).
//!
//! - [`bridge`]: the page's messages, the JavaScript sent back, which links and navigations go;
//!   without WebKit, tested.
//! - [`glue`]: the window, the web view and its delegates (objc2); thin, checked by hand.
//!
//! It watches the local server's hub in the app's own process, without HTTP: the chat feeds it
//! while the web view is open, the server switched on or not.

mod bridge;
mod glue;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

pub use bridge::{Message, answer_js, parse, receive_js, run};
pub use glue::{Window, open_url};

/// What the window shows when the page wasn't built.
pub const MISSING: &str = "<!doctype html><meta charset=utf-8><title>Ailfred</title>\
<body style=\"font: 14px -apple-system, sans-serif; margin: 2em; color: #444\">\
<p>The web view's page isn't built.</p><p>Run <code>npm --prefix web run build</code>, then open the web view again.</p>";

/// The page's HTML: the bundle's (`Contents/Resources/web/index.html`), else the dev build's
/// (`web/dist/index.html` in the repo), else [`MISSING`].
pub fn page() -> String {
    let mut paths = Vec::new();
    if crate::app_env::in_bundle()
        && let Some(contents) = std::env::current_exe().ok().as_deref().and_then(Path::parent).and_then(Path::parent)
    {
        paths.push(contents.join("Resources/web/index.html"));
    }
    paths.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("web/dist/index.html"));
    page_from(&paths)
}

/// The action a ⌘ key sends in the web view's window (`key` as typed without modifiers, with
/// ⇧ or not): what an Edit menu's items would; the app, an accessory, has none.
pub fn edit_action(key: &str, shift: bool) -> Option<&'static str> {
    Some(match (key.to_ascii_lowercase().as_str(), shift) {
        ("x", false) => "cut:",
        ("c", false) => "copy:",
        ("v", false) => "paste:",
        ("a", false) => "selectAll:",
        ("z", false) => "undo:",
        ("z", true) => "redo:",
        ("w", false) => "performClose:",
        _ => return None,
    })
}

/// The first of `paths` that reads, else [`MISSING`].
pub fn page_from(paths: &[PathBuf]) -> String {
    paths.iter().find_map(|path| std::fs::read_to_string(path).ok()).unwrap_or_else(|| MISSING.to_owned())
}
