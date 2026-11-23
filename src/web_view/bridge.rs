//! The bridge between the web view's page and the app, without WebKit (`docs/web-view.md`): the
//! page's messages read, the JavaScript that answers it and feeds it the hub's frames, the links
//! it may open and the navigations it may make.

use std::time::Instant;

use ailfred_protocol::command::Command;
use serde_json::Value;

use crate::chat::Chat;
use crate::server::{Answer, Frame, parse_command};

/// What the page posted (`window.webkit.messageHandlers.ailfred.postMessage(text)`).
#[derive(Debug)]
pub enum Message {
    /// The page is ready (again): it gets the snapshot, then live events.
    Join,
    /// A `POST /v1/commands` body, its checks before the chat's done (a refusal is `Err`); its
    /// answer goes back to `id`.
    Command { id: String, command: Result<Command, Answer> },
    /// A link to open in the default browser.
    Open(String),
}

/// The page's `text`, or None for anything else (bad JSON too): ignored.
pub fn parse(text: &str) -> Option<Message> {
    let Ok(Value::Object(mut message)) = serde_json::from_str::<Value>(text) else { return None };
    match message.get("type")?.as_str()? {
        "join" => Some(Message::Join),
        "open" => Some(Message::Open(message.get("url")?.as_str()?.to_owned())),
        "command" => {
            let id = message.get("id")?.as_str()?.to_owned();
            // Checked as `POST /v1/commands` checks its body; none is a body that isn't one.
            let body = serde_json::to_vec(&message.remove("command").unwrap_or(Value::Null)).ok()?;
            Some(Message::Command { id, command: parse_command(&body) })
        }
        _ => None,
    }
}

/// A command the page sent, done in `chat` as `POST /v1/commands` does it; what to answer.
pub fn run(chat: &mut Chat, command: Result<Command, Answer>, now: Instant) -> Answer {
    match command {
        Ok(command) => chat.command(command, now),
        Err(refused) => refused,
    }
}

/// `window.ailfred.answer(id, status, body)` for command `id`.
pub fn answer_js(id: &str, answer: &Answer) -> String {
    let id = Value::String(id.to_owned()).to_string();
    format!("window.ailfred.answer({}, {}, {});", script_safe(&id), answer.status, script_safe(&answer.body.to_string()))
}

/// `window.ailfred.receive([...events])` for one of the hub's frames (one or more `data:`
/// events); None if it holds none.
pub fn receive_js(frame: &Frame) -> Option<String> {
    let events: Vec<&str> = frame.split("\n\n").filter_map(|event| event.strip_prefix("data: ")).collect();
    if events.is_empty() {
        return None;
    }
    Some(format!("window.ailfred.receive([{}]);", script_safe(&events.join(","))))
}

/// `json` (serde's, valid JSON) as JavaScript that's safe anywhere a script goes: `<` (so no
/// `</script>` or `<!--`), U+2028 and U+2029 (line ends in older JavaScript) escaped. Each can
/// only stand inside a JSON string, where its `\u` escape means the same.
fn script_safe(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
}

/// A link the page may open in the browser: `http:` or `https:`, nothing else.
pub fn openable(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else { return false };
    let web = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    let host = rest.strip_prefix("//").is_some_and(|after| !after.is_empty());
    web && host && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// What starts a navigation, as the navigation delegate tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The user activated a link.
    Link,
    Reload,
    Other,
}

/// What the navigation delegate does with a navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Navigation {
    Allow,
    Cancel,
    /// Cancelled, and the link opens in the browser.
    Open(String),
}

/// The page loaded with `loadHTMLString:baseURL:` and no base is `about:blank`.
pub const PAGE_URL: &str = "about:blank";

/// Navigation to `url` (by `kind`), `loaded` once the page's first load was let through: only
/// that load goes (and a reload of it); a link activated for `http(s)` opens in the browser.
pub fn navigation(loaded: bool, kind: Kind, url: &str) -> Navigation {
    let page = url == PAGE_URL;
    match kind {
        Kind::Link if openable(url) => Navigation::Open(url.to_owned()),
        Kind::Other if page && !loaded => Navigation::Allow,
        Kind::Reload if page => Navigation::Allow,
        _ => Navigation::Cancel,
    }
}
