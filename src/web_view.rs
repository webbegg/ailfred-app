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

/// The page's HTML: the bundle's (`Contents/Resources/web/index.html`) in an app bundle, else,
/// in a debug build, the repo's dev build (`web/dist/index.html`); else [`MISSING`].
pub fn page() -> String {
    let exe = std::env::current_exe().ok();
    let contents = exe.as_deref().and_then(Path::parent).and_then(Path::parent);
    page_from(&page_paths(crate::app_env::in_bundle(), contents, cfg!(debug_assertions), Path::new(env!("CARGO_MANIFEST_DIR"))))
}

/// Where the page is read from: run from an app bundle (its `contents` folder, if found), the
/// bundle's only, never the repo the app was built in; else the repo's dev build, in a `debug`
/// build only (`cargo run`).
pub fn page_paths(in_bundle: bool, contents: Option<&Path>, debug: bool, repo: &Path) -> Vec<PathBuf> {
    match (in_bundle, contents) {
        (true, Some(contents)) => vec![contents.join("Resources/web/index.html")],
        (true, None) => Vec::new(),
        (false, _) if debug => vec![repo.join("web/dist/index.html")],
        (false, _) => Vec::new(),
    }
}

/// The action a ⌘ key sends in the web view's window: what an Edit menu's items would; the
/// app, an accessory, has none. `key` is as typed without modifiers, `code` the key's code,
/// with ⇧ or not. A Latin key goes by what it types; any other (Cyrillic, Greek, none) by
/// where it sits, as on the ANSI layout.
pub fn edit_action(key: &str, code: u16, shift: bool) -> Option<&'static str> {
    let latin = !key.is_empty() && key.is_ascii();
    let key = if latin {
        key.to_ascii_lowercase()
    } else {
        let by_code = match code {
            7 => "x",
            8 => "c",
            9 => "v",
            0 => "a",
            6 => "z",
            13 => "w",
            _ => return None,
        };
        by_code.to_owned()
    };
    Some(match (key.as_str(), shift) {
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

/// A WKWebView context menu item kept, by its identifier: none that navigates (opens a link,
/// an image or a frame, goes back or forward, searches the web) or downloads.
pub fn menu_item_kept(identifier: &str) -> bool {
    let Some(item) = identifier.strip_prefix("WKMenuItemIdentifier") else { return true };
    !(item.starts_with("Open") || item.starts_with("Download") || matches!(item, "GoBack" | "GoForward" | "SearchWeb"))
}

/// The web view's windows as they open and close: each gets the next number, which what it
/// queues for the app (its messages, its close, its commands' answers) carries; what's for one
/// that closed is dropped, so it never reaches the next.
#[derive(Default)]
pub struct Windows {
    opened: u64,
    open: Option<u64>,
}

impl Windows {
    /// A window opens: its number.
    pub fn open(&mut self) -> u64 {
        self.opened += 1;
        self.open = Some(self.opened);
        self.opened
    }

    /// Window `n` closes; false if it isn't the one open (closed already, or an earlier one).
    pub fn close(&mut self, n: u64) -> bool {
        let open = self.is_open(n);
        if open {
            self.open = None;
        }
        open
    }

    /// Window `n` is the one open.
    pub fn is_open(&self, n: u64) -> bool {
        self.open == Some(n)
    }
}

/// The first of `paths` that reads, else [`MISSING`].
pub fn page_from(paths: &[PathBuf]) -> String {
    paths.iter().find_map(|path| std::fs::read_to_string(path).ok()).unwrap_or_else(|| MISSING.to_owned())
}
