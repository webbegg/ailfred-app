use ailfred_protocol::command::Command;
use serde_json::{Value, json};

use std::path::Path;

use super::bridge::{Kind, Message, Navigation, PAGE_URL, answer_js, from_page, navigation, openable, parse, receive_js};
use super::{MISSING, Windows, edit_action, menu_item_kept, page_from, page_paths};
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
fn only_the_main_frames_page_may_post_to_the_app() {
    assert!(from_page(true, PAGE_URL));
    for (main, url) in [(false, PAGE_URL), (true, "about:srcdoc"), (true, "https://example.com"), (true, ""), (true, "about:blank#x"), (false, "https://example.com")] {
        assert!(!from_page(main, url), "main {main} {url:?}");
    }
}

/// The ANSI layout's key codes the keys sit on.
const ANSI: [(&str, u16); 7] = [("x", 7), ("c", 8), ("v", 9), ("a", 0), ("z", 6), ("w", 13), ("q", 12)];

fn code(key: &str) -> u16 {
    ANSI.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map_or(99, |(_, c)| *c)
}

#[test]
fn command_keys_edit_text_and_close_the_window_as_an_edit_menu_would() {
    let keys = [("x", false, "cut:"), ("c", false, "copy:"), ("v", false, "paste:"), ("a", false, "selectAll:"), ("z", false, "undo:"), ("z", true, "redo:"), ("Z", true, "redo:"), ("w", false, "performClose:")];
    for (key, shift, action) in keys {
        assert_eq!(edit_action(key, code(key), shift), Some(action), "⌘{key} shift {shift}");
    }
    for (key, shift) in [("q", false), ("r", false), ("cv", false)] {
        assert_eq!(edit_action(key, code(key), shift), None, "⌘{key}");
    }
    // A Latin key means what it says, wherever it sits (Dvorak's C is on the ANSI I).
    assert_eq!(edit_action("c", 34, false), Some("copy:"));
    assert_eq!(edit_action("j", 8, false), None, "Dvorak's J on the ANSI C");
}

#[test]
fn on_a_layout_that_isnt_latin_command_keys_go_by_where_the_key_sits() {
    // Russian: ч с м ф я ц on X C V A Z W; Greek: χ ψ ω α ζ ς.
    for (keys, name) in [(["ч", "с", "м", "ф", "я", "ц"], "Russian"), (["χ", "ψ", "ω", "α", "ζ", "ς"], "Greek")] {
        let got: Vec<_> = keys.iter().zip([7, 8, 9, 0, 6, 13]).map(|(k, c)| edit_action(k, c, false)).collect();
        assert_eq!(got, [Some("cut:"), Some("copy:"), Some("paste:"), Some("selectAll:"), Some("undo:"), Some("performClose:")], "{name}");
    }
    assert_eq!(edit_action("я", 6, true), Some("redo:"));
    assert_eq!(edit_action("й", 12, false), None, "Q does nothing");
    assert_eq!(edit_action("", 8, false), Some("copy:"), "no characters: by the key");
}

#[test]
fn the_context_menu_keeps_nothing_that_navigates_or_downloads() {
    for id in [
        "WKMenuItemIdentifierOpenLink",
        "WKMenuItemIdentifierOpenLinkInNewWindow",
        "WKMenuItemIdentifierDownloadLinkedFile",
        "WKMenuItemIdentifierOpenImageInNewWindow",
        "WKMenuItemIdentifierDownloadImage",
        "WKMenuItemIdentifierOpenFrameInNewWindow",
        "WKMenuItemIdentifierOpenMediaInNewWindow",
        "WKMenuItemIdentifierDownloadMedia",
        "WKMenuItemIdentifierGoBack",
        "WKMenuItemIdentifierGoForward",
        "WKMenuItemIdentifierSearchWeb",
    ] {
        assert!(!menu_item_kept(id), "{id}");
    }
    for id in ["WKMenuItemIdentifierCopy", "WKMenuItemIdentifierCopyLink", "WKMenuItemIdentifierPaste", "WKMenuItemIdentifierReload", "WKMenuItemIdentifierInspectElement", "WKMenuItemIdentifierLookUp", ""] {
        assert!(menu_item_kept(id), "{id}");
    }
}

#[test]
fn what_a_closed_window_queued_never_reaches_the_next_one() {
    let mut windows = Windows::default();
    let first = windows.open();
    assert!(windows.is_open(first));
    assert!(windows.close(first), "it closes");
    assert!(!windows.is_open(first) && !windows.close(first), "once");
    let second = windows.open();
    assert_ne!(first, second);
    assert!(windows.is_open(second) && !windows.is_open(first), "the first's messages and answers are dropped");
    assert!(!windows.close(first), "a late close of the first leaves the second open");
    assert!(windows.is_open(second));
}

#[test]
fn a_bundled_app_reads_its_page_from_the_bundle_only_and_a_debug_build_from_the_repo() {
    let (contents, repo) = (Path::new("/A.app/Contents"), Path::new("/repo"));
    let (bundled, built) = (contents.join("Resources/web/index.html"), repo.join("web/dist/index.html"));
    assert_eq!(page_paths(true, Some(contents), true, repo), std::slice::from_ref(&bundled), "a debug bundle");
    assert_eq!(page_paths(true, Some(contents), false, repo), [bundled], "a release bundle");
    assert_eq!(page_paths(true, None, true, repo), Vec::<std::path::PathBuf>::new(), "a bundle whose folder isn't found");
    assert_eq!(page_paths(false, None, true, repo), [built], "`cargo run`");
    assert_eq!(page_paths(false, None, false, repo), Vec::<std::path::PathBuf>::new(), "a release build outside a bundle");
}

#[test]
fn a_bundle_without_its_page_says_it_isnt_built_even_with_the_repos_build_there() {
    let dir = std::env::temp_dir().join(format!("ailfred-web-view-bundle-{}", std::process::id()));
    let (contents, repo) = (dir.join("A.app/Contents"), dir.join("repo"));
    std::fs::create_dir_all(repo.join("web/dist")).unwrap();
    std::fs::write(repo.join("web/dist/index.html"), "<p>the repo's</p>").unwrap();
    assert_eq!(page_from(&page_paths(true, Some(&contents), true, &repo)), MISSING);
    assert_eq!(page_from(&page_paths(false, None, true, &repo)), "<p>the repo's</p>");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_page_knows_the_guards_questions_by_the_same_start_the_server_refuses_answers_to() {
    let question = include_str!("../../web/src/components/Question.tsx");
    assert!(question.contains(&format!("export const LOCAL_ONLY = \"{}\";", crate::reply::LOCAL_ONLY)), "web/src/components/Question.tsx");
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
