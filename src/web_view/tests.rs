use ailfred_protocol::command::Command;
use serde_json::{Value, json};

use super::bridge::{Kind, Message, Navigation, PAGE_URL, answer_js, navigation, openable, parse, receive_js};
use super::{MISSING, edit_action, page_from};
use crate::server::Answer;

#[test]
fn the_page_joins_sends_commands_with_an_id_and_opens_links() {
    assert!(matches!(parse(r#"{"type":"join"}"#), Some(Message::Join)));
    match parse(r#"{"type":"command","id":"q1","command":{"type":"stop","threadId":"t","runId":"r"}}"#) {
        Some(Message::Command { id, command: Ok(command) }) => {
            assert_eq!(id, "q1");
            assert_eq!(command, Command::Stop { thread_id: "t".into(), run_id: "r".into() });
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(parse(r#"{"type":"open","url":"https://example.com"}"#), Some(Message::Open(url)) if url == "https://example.com"));
}

#[test]
fn a_command_that_isnt_one_is_answered_400_as_the_route_would() {
    for text in [r#"{"type":"command","id":"q1","command":{"type":"jump"}}"#, r#"{"type":"command","id":"q1"}"#, r#"{"type":"command","id":"q1","command":"stop"}"#] {
        match parse(text) {
            Some(Message::Command { id, command: Err(answer) }) => {
                assert_eq!((id.as_str(), answer.status), ("q1", 400), "{text}");
                assert!(answer.body["error"].as_str().unwrap().starts_with("not a command"), "{answer:?}");
            }
            other => panic!("{text}: {other:?}"),
        }
    }
}

#[test]
fn junk_and_anything_else_the_page_posts_is_ignored() {
    for text in [
        "",
        "{nope",
        "null",
        "[]",
        "\"join\"",
        r#"{"type":"JOIN"}"#,
        r#"{"type":"reload"}"#,
        r#"{"kind":"join"}"#,
        r#"{"type":7}"#,
        r#"{"type":"command","command":{"type":"stop","threadId":"t","runId":"r"}}"#,
        r#"{"type":"command","id":7,"command":{"type":"stop","threadId":"t","runId":"r"}}"#,
        r#"{"type":"open"}"#,
        r#"{"type":"open","url":7}"#,
    ] {
        assert!(parse(text).is_none(), "{text}");
    }
}

/// The JavaScript's arguments, read back as the page would: `window.ailfred.<name>(…);`.
fn arguments(js: &str, name: &str) -> Value {
    let inner = js.strip_prefix(&format!("window.ailfred.{name}(")).and_then(|s| s.strip_suffix(");")).unwrap_or_else(|| panic!("{js}"));
    serde_json::from_str(&format!("[{inner}]")).unwrap()
}

/// What a script must not hold raw: `</script>` would end an inline script, and U+2028/U+2029
/// end a line in older JavaScript.
fn safe(js: &str) {
    assert!(!js.contains('<') && !js.contains('\u{2028}') && !js.contains('\u{2029}'), "{js}");
}

const NASTY: &str = "</script><script>alert(\"x\")</script> it's \u{2028}a\u{2029}b \\ \n";

#[test]
fn an_answer_is_one_call_with_its_id_status_and_body_escaped() {
    let js = answer_js(NASTY, &Answer::error(409, NASTY));
    safe(&js);
    assert_eq!(arguments(&js, "answer"), json!([NASTY, 409, { "error": NASTY }]));
    let js = answer_js("q1", &Answer::accepted("t", "r"));
    assert_eq!(arguments(&js, "answer"), json!(["q1", 202, { "threadId": "t", "runId": "r" }]));
}

#[test]
fn a_frame_is_one_receive_call_with_its_events_in_order_escaped() {
    let a = json!({ "type": "TEXT_MESSAGE_CONTENT", "messageId": "m", "delta": NASTY });
    let b = json!({ "type": "RUN_FINISHED", "threadId": "t", "runId": "r" });
    let frame = format!("data: {a}\n\ndata: {b}\n\n");
    let js = receive_js(&frame).expect("a call");
    safe(&js);
    assert_eq!(arguments(&js, "receive"), json!([[a, b]]));
    assert_eq!(receive_js(&": ping\n\n".to_owned()), None, "nothing to receive");
}

#[test]
fn only_http_and_https_links_open() {
    for url in ["http://example.com", "https://example.com/a?b#c", "HTTPS://EXAMPLE.COM"] {
        assert!(openable(url), "{url}");
    }
    for url in [
        "",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "ailfred://toggle",
        "x-apple.systempreferences:com.apple.preference.security",
        "about:blank",
        "data:text/html,hi",
        "//example.com",
        "http:",
        "http://",
        "https:example.com",
        " https://example.com",
        "https://exa mple.com",
        "https://example.com/\n",
    ] {
        assert!(!openable(url), "{url:?}");
    }
}

#[test]
fn only_the_first_load_and_its_reload_navigate_and_a_link_opens_in_the_browser() {
    assert_eq!(navigation(false, Kind::Other, PAGE_URL), Navigation::Allow, "the page's load");
    assert_eq!(navigation(true, Kind::Reload, PAGE_URL), Navigation::Allow, "a reload of it");
    assert_eq!(navigation(true, Kind::Other, PAGE_URL), Navigation::Cancel, "loaded again");
    assert_eq!(navigation(false, Kind::Other, "https://example.com"), Navigation::Cancel, "not the page");
    assert_eq!(navigation(true, Kind::Link, "https://example.com"), Navigation::Open("https://example.com".into()));
    assert_eq!(navigation(false, Kind::Link, "http://example.com"), Navigation::Open("http://example.com".into()));
    for url in ["file:///etc/passwd", "javascript:alert(1)", "ailfred://toggle"] {
        assert_eq!(navigation(true, Kind::Link, url), Navigation::Cancel, "{url}");
    }
    assert_eq!(navigation(true, Kind::Other, "https://example.com"), Navigation::Cancel, "a script's");
    assert_eq!(navigation(true, Kind::Reload, "https://example.com"), Navigation::Cancel);
}

#[test]
fn command_keys_edit_text_and_close_the_window_as_an_edit_menu_would() {
    let keys = [("x", false, "cut:"), ("c", false, "copy:"), ("v", false, "paste:"), ("a", false, "selectAll:"), ("z", false, "undo:"), ("z", true, "redo:"), ("Z", true, "redo:"), ("w", false, "performClose:")];
    for (key, shift, action) in keys {
        assert_eq!(edit_action(key, shift), Some(action), "⌘{key} shift {shift}");
    }
    for (key, shift) in [("q", false), ("r", false), ("", false), ("cv", false)] {
        assert_eq!(edit_action(key, shift), None, "⌘{key}");
    }
}

#[test]
fn the_page_is_the_first_one_found_else_a_note_on_how_to_build_it() {
    let dir = std::env::temp_dir().join(format!("ailfred-web-view-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (bundled, built) = (dir.join("bundled.html"), dir.join("built.html"));
    std::fs::write(&built, "<p>built</p>").unwrap();
    assert_eq!(page_from(&[bundled.clone(), built.clone()]), "<p>built</p>");
    std::fs::write(&bundled, "<p>bundled</p>").unwrap();
    assert_eq!(page_from(&[bundled, built]), "<p>bundled</p>");
    assert_eq!(page_from(&[dir.join("none.html")]), MISSING);
    assert!(MISSING.contains("npm --prefix web run build"));
    std::fs::remove_dir_all(&dir).unwrap();
}
