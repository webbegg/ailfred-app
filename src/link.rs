//! Links that reach Ailfred (`ailfred://…`, from `open`, a browser, Shortcuts or a script).
//! They come from anyone who can run `open`: only the exact forms below do anything.

/// The scheme the app's bundle declares (`scripts/bundle-mac`).
pub const SCHEME: &str = "ailfred";

/// What a link asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// `ailfred://toggle`: shows or hides the quick input, as its shortcut does.
    Toggle,
}

/// The link `url` names; None for anything else.
pub fn parse(url: &str) -> Option<Link> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    rest.eq_ignore_ascii_case("toggle").then_some(Link::Toggle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_is_the_only_link() {
        assert_eq!(parse("ailfred://toggle"), Some(Link::Toggle));
        assert_eq!(parse("ailfred://toggle/"), Some(Link::Toggle), "a trailing slash, as some callers add");
        assert_eq!(parse("AILFRED://Toggle"), Some(Link::Toggle), "schemes and this host are case-insensitive");
    }

    #[test]
    fn anything_else_does_nothing() {
        for url in [
            "",
            "ailfred://",
            "ailfred:toggle",
            "ailfred://toggles",
            "ailfred://capture",
            "ailfred://toggle?x=1",
            "ailfred://toggle#x",
            "ailfred://toggle/more",
            "ailfred://user@toggle",
            "other://toggle",
            "https://toggle",
        ] {
            assert_eq!(parse(url), None, "{url:?}");
        }
    }

    #[test]
    fn the_installed_app_declares_the_scheme() {
        let script = include_str!("../scripts/bundle-mac");
        let declared = format!("\"CFBundleURLSchemes\":[\"{SCHEME}\"]");
        assert!(script.contains(&declared), "bundle-mac declares no {declared}");
    }
}
