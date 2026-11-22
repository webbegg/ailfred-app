use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::stream::{Decoder, Kind};
use super::*;
use crate::reply::Event;

/// A fresh empty folder under the system's temp folder.
fn scratch(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("ailfred-test-{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn script(path: &Path, body: &str, mode: u32) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn request(text: &str) -> Request {
    Request { model: DEFAULT_MODEL.into(), history: Vec::new(), text: text.into(), pngs: Vec::new(), session: None }
}

fn png(bytes: &[u8]) -> Arc<Vec<u8>> {
    Arc::new(bytes.to_vec())
}

fn some(s: &str) -> Option<String> {
    Some(s.to_owned())
}

// The choice of backend.

#[test]
fn a_keychain_key_wins_over_the_environments_and_claude_code() {
    let backend = choose(some("sk-keychain"), some("sk-env"), Some("/bin/claude".into()));
    assert_eq!(backend, Backend::Api { key: "sk-keychain".into(), source: KeySource::Keychain });
}

#[test]
fn without_a_keychain_key_the_environments_key_wins_over_claude_code() {
    let backend = choose(None, some("sk-env"), Some("/bin/claude".into()));
    assert_eq!(backend, Backend::Api { key: "sk-env".into(), source: KeySource::Env });
}

#[test]
fn without_any_key_claude_code_answers_and_with_nothing_the_model_is_missing() {
    assert_eq!(choose(None, None, Some("/bin/claude".into())), Backend::ClaudeCode { path: "/bin/claude".into() });
    assert_eq!(choose(None, None, None), Backend::Missing);
}

#[test]
fn a_blank_key_counts_as_no_key_and_a_padded_one_is_trimmed() {
    assert_eq!(choose(some("  "), some(""), None), Backend::Missing);
    assert_eq!(choose(some(" sk-a\n"), None, None), Backend::Api { key: "sk-a".into(), source: KeySource::Keychain });
}

#[test]
fn the_status_line_names_the_backend_the_choice_makes() {
    let claude = Path::new("/Users/me/.local/bin/claude");
    assert_eq!(status(true, true, Some(claude)), "Replies come from the Anthropic API (key in Keychain)");
    assert_eq!(status(false, true, Some(claude)), "Replies come from the Anthropic API (ANTHROPIC_API_KEY)");
    assert_eq!(status(false, false, Some(claude)), "Replies come from Claude Code at /Users/me/.local/bin/claude");
    assert_eq!(status(false, false, None), "No model: add an Anthropic API key in Settings, or install Claude Code");
    for (k, e, c) in [(true, false, false), (false, true, true), (false, false, true), (false, false, false), (true, true, true)] {
        let backend = choose(k.then(|| "k".into()), e.then(|| "e".into()), c.then(|| claude.to_path_buf()));
        let line = status(k, e, c.then_some(claude));
        let agrees = match backend {
            Backend::Api { source: KeySource::Keychain, .. } => line.contains("Keychain"),
            Backend::Api { source: KeySource::Env, .. } => line.contains("ANTHROPIC_API_KEY"),
            Backend::ClaudeCode { .. } => line.contains("Claude Code at"),
            Backend::Missing => line.starts_with("No model"),
        };
        assert!(agrees, "{line} for keychain {k}, env {e}, claude {c}");
    }
}

#[test]
fn a_backend_never_prints_its_key() {
    let shown = format!("{:?}", Backend::Api { key: "sk-secret".into(), source: KeySource::Keychain });
    assert!(!shown.contains("sk-secret"), "{shown}");
}

// What goes to the model.

#[test]
fn base64_is_the_standard_padded_one() {
    for (plain, coded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
        assert_eq!(base64(plain.as_bytes()), coded);
    }
    assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+", "the + and / alphabet");
}

#[test]
fn a_message_goes_as_its_captures_then_its_text_and_a_capture_alone_has_no_text_block() {
    let blocks = user_content("what is this?", &[png(b"foo"), png(b"foob")]);
    assert_eq!(
        blocks,
        json!([
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "Zm9v"}},
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "Zm9vYg=="}},
            {"type": "text", "text": "what is this?"},
        ])
    );
    assert_eq!(user_content("", &[png(b"foo")]).as_array().unwrap().len(), 1);
}

// The API path.

fn conversation() -> Request {
    Request {
        history: vec![
            Turn { text: "first".into(), pngs: vec![png(b"foo")], reply: "one".into() },
            Turn { text: "second".into(), pngs: Vec::new(), reply: "two".into() },
        ],
        pngs: vec![png(b"foob")],
        ..request("third")
    }
}

#[test]
fn the_api_body_has_the_model_the_system_prompt_the_earlier_turns_in_order_and_the_capture() {
    let body = api::body(&conversation());
    assert_eq!(body["model"], DEFAULT_MODEL);
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], MAX_TOKENS);
    assert_eq!(body["system"], SYSTEM_PROMPT);
    let messages = body["messages"].as_array().unwrap();
    let roles: Vec<&str> = messages.iter().map(|m| m["role"].as_str().unwrap()).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant", "user"]);
    assert_eq!(messages[0]["content"][0]["source"]["data"], "Zm9v", "the first message's capture, as base64");
    assert_eq!(messages[0]["content"][1]["text"], "first");
    assert_eq!(messages[1]["content"], "one");
    assert_eq!(messages[3]["content"], "two");
    assert_eq!(messages[4]["content"][0]["source"]["media_type"], "image/png");
    assert_eq!(messages[4]["content"][1]["text"], "third");
}

#[test]
fn curl_gets_the_key_in_its_config_on_stdin_and_never_in_its_arguments() {
    let config = api::config("sk-ant-secret", Path::new("/tmp/body.json"));
    assert!(config.contains("x-api-key: sk-ant-secret"), "{config}");
    assert!(config.contains(api::URL));
    assert!(config.contains("@/tmp/body.json"), "the body is read from its file");
    let args = api::args().join(" ");
    assert!(!args.contains("sk-ant"), "{args}");
    assert!(args.contains("-N") && args.contains("-K -"), "unbuffered, config from stdin: {args}");
    assert!(!args.contains(api::URL), "the URL is in the config");
    assert!(args.contains(&format!("anthropic-version: {}", api::VERSION)));
}

#[test]
fn a_key_with_quotes_or_line_breaks_cannot_add_lines_to_the_config() {
    let config = api::config("sk\"\nurl = \"http://evil", Path::new("/tmp/b.json"));
    assert_eq!(config.lines().filter(|l| l.starts_with("url")).count(), 1, "{config}");
}

#[test]
fn the_body_file_is_private_and_goes_when_done_with() {
    let dir = scratch("body");
    let path;
    {
        let file = api::BodyFile::write(&dir, "7", &json!({"a": 1})).unwrap();
        path = file.path().to_path_buf();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"a":1}"#);
    }
    assert!(!path.exists(), "removed afterwards");
}

// The Claude Code path.

#[test]
fn claude_is_looked_for_in_its_usual_folders_then_on_the_path() {
    let dirs = claude_code::candidate_dirs(Path::new("/Users/me"), Some(&OsString::from("/a/bin:/b/bin")));
    let dirs: Vec<&str> = dirs.iter().map(|d| d.to_str().unwrap()).collect();
    assert_eq!(dirs, ["/Users/me/.local/bin", "/opt/homebrew/bin", "/usr/local/bin", "/Users/me/.claude/local", "/a/bin", "/b/bin"]);
}

#[test]
fn the_first_folder_with_an_executable_claude_wins_and_a_file_that_cant_run_is_skipped() {
    let (a, b, c) = (scratch("a"), scratch("b"), scratch("c"));
    script(&a.join("claude"), "exit 0", 0o644);
    script(&b.join("claude"), "exit 0", 0o755);
    script(&c.join("claude"), "exit 0", 0o755);
    assert_eq!(claude_code::find_in(&[scratch("none"), a, b.clone(), c]), Some(b.join("claude")));
    assert_eq!(claude_code::find_in(&[scratch("empty")]), None);
}

#[test]
fn claude_is_found_on_the_path_when_the_usual_folders_lack_it() {
    let bin = scratch("path");
    script(&bin.join("claude"), "exit 0", 0o755);
    let dirs = claude_code::candidate_dirs(&scratch("home"), Some(bin.as_os_str()));
    // The machine may have a real claude in a usual folder: the path's is the last resort.
    assert_eq!(dirs.last(), Some(&bin));
    assert!(claude_code::find_in(&dirs[dirs.len() - 1..]) == Some(bin.join("claude")));
}

#[test]
fn claude_runs_lean_with_the_model_and_resumes_only_a_conversation_that_has_a_session() {
    let fresh = claude_code::args("claude-opus-5-5", None);
    let has = |args: &[String], flag: &str| args.iter().any(|a| a == flag);
    let after = |args: &[String], flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    for flag in ["-p", "--verbose", "--include-partial-messages", "--strict-mcp-config"] {
        assert!(has(&fresh, flag), "{flag} in {fresh:?}");
    }
    assert_eq!(after(&fresh, "--input-format"), some("stream-json"));
    assert_eq!(after(&fresh, "--output-format"), some("stream-json"));
    assert_eq!(after(&fresh, "--setting-sources"), some(""), "no settings, hooks or plugins");
    assert_eq!(after(&fresh, "--tools"), some(""), "chat only");
    assert_eq!(after(&fresh, "--system-prompt"), some(SYSTEM_PROMPT));
    assert_eq!(after(&fresh, "--model"), some("claude-opus-5-5"));
    assert!(!has(&fresh, "--resume"));
    let resumed = claude_code::args(DEFAULT_MODEL, Some("abc-123"));
    assert_eq!(after(&resumed, "--resume"), some("abc-123"));
}

#[test]
fn a_model_not_found_names_it_once() {
    let said = errors::cli_error(Some(404), "There's an issue with the selected model (claude-x). It may not exist or you may not have access to it.");
    assert_eq!(said, "The model claude-x isn't available. Pick another in Settings.");
    let api = errors::api_error("not_found_error", "model: claude-y");
    assert_eq!(api, "The model isn't available (model: claude-y). Pick another in Settings.");
}

#[test]
fn claudes_stdin_is_one_json_line_with_the_capture_and_the_text() {
    let line = claude_code::input(&Request { pngs: vec![png(b"foo")], ..request("what is this?") });
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "one line: {line:?}");
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["type"], "user");
    assert_eq!(v["message"]["role"], "user");
    assert_eq!(v["message"]["content"][0]["type"], "image");
    assert_eq!(v["message"]["content"][0]["source"]["data"], "Zm9v");
    assert_eq!(v["message"]["content"][1], json!({"type": "text", "text": "what is this?"}));
}

#[test]
fn without_a_session_claude_code_gets_the_earlier_turns_as_a_transcript() {
    let history = vec![
        Turn { text: "first question".into(), pngs: vec![png(b"foo")], reply: "first answer".into() },
        Turn { text: "second question".into(), pngs: Vec::new(), reply: "second answer".into() },
    ];
    let fresh = Request { history: history.clone(), ..request("third question") };
    let v: Value = serde_json::from_str(&claude_code::input(&fresh)).unwrap();
    let content = v["message"]["content"].as_array().unwrap();
    let text: Vec<&str> = content.iter().filter_map(|b| b["text"].as_str()).collect();
    let all = text.join("\n");
    for said in ["first question", "first answer", "second question", "second answer"] {
        assert!(all.contains(said), "{said} in {all}");
    }
    assert_eq!(text.last(), Some(&"third question"), "the new message comes last, on its own");
    assert!(all.find("first answer") < all.find("second question"), "in order");
    // Resumed, the session has them: only the message goes.
    let resumed = Request { history, session: some("s"), ..request("third question") };
    let v: Value = serde_json::from_str(&claude_code::input(&resumed)).unwrap();
    assert_eq!(v["message"]["content"], json!([{"type": "text", "text": "third question"}]));
}

#[test]
fn claude_runs_in_a_private_folder_of_the_apps_own() {
    let home = scratch("home2");
    let dir = claude_code::private_dir(&home, "com.ailfred.dev");
    assert_eq!(dir, home.join("Library/Application Support/com.ailfred.dev/claude-code"));
    claude_code::ensure_private_dir(&dir).unwrap();
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    claude_code::ensure_private_dir(&dir).unwrap();
}

// The stream.

fn items(kind: Kind, lines: &str) -> Vec<Item> {
    let mut d = Decoder::new(kind);
    lines.lines().flat_map(|l| d.line(l)).collect()
}

fn text_of(items: &[Item]) -> String {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Event(Event::Text(t)) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

fn failure(items: &[Item]) -> Option<&str> {
    items.iter().find_map(|i| match i {
        Item::Event(Event::Fail(m)) => Some(m.as_str()),
        _ => None,
    })
}

#[test]
fn a_recorded_claude_code_reply_streams_its_text_pieces_then_the_session_then_done() {
    let out = items(Kind::Cli, include_str!("../testdata/claude_code_hi.jsonl"));
    let pieces: Vec<&str> = out.iter().filter_map(|i| if let Item::Event(Event::Text(t)) = i { Some(t.as_str()) } else { None }).collect();
    assert_eq!(pieces, ["Hello there", ", fri", "end!"]);
    assert_eq!(text_of(&out), "Hello there, friend!");
    assert!(out.contains(&Item::Session("f95168ca-9f96-4ef3-8fe8-cae1e472c0b8".into())), "the session id is kept");
    assert_eq!(out.iter().filter(|i| **i == Item::Event(Event::Done)).count(), 1, "one Done, at the result");
    assert_eq!(out.last(), Some(&Item::Event(Event::Done)));
}

#[test]
fn claude_codes_message_stop_isnt_the_end_its_result_is() {
    let mut d = Decoder::new(Kind::Cli);
    let stop = r#"{"type":"stream_event","event":{"type":"message_stop"}}"#;
    assert_eq!(d.line(stop), vec![]);
    assert!(!d.finished());
    assert_eq!(d.line(r#"{"type":"result","is_error":false,"session_id":"s1"}"#), vec![Item::Session("s1".into()), Item::Event(Event::Done)]);
    assert!(d.finished());
}

#[test]
fn thinking_deltas_become_thinking() {
    let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me see"}}}"#;
    assert_eq!(items(Kind::Cli, line), vec![Item::Event(Event::Think("Let me see".into()))]);
}

#[test]
fn claude_code_not_logged_in_fails_in_words_that_say_how_to_fix_it() {
    let line = r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":401,"result":"Invalid API key · Please run /login","session_id":"s"}"#;
    let out = items(Kind::Cli, line);
    let message = failure(&out).expect("a failure");
    assert!(message.contains("`claude` in Terminal") && message.contains("log in"), "{message}");
    assert!(!out.contains(&Item::Event(Event::Done)));
}

#[test]
fn another_claude_code_error_is_shown_as_it_said_it() {
    let line = r#"{"type":"result","is_error":true,"result":"Something odd happened"}"#;
    let out = items(Kind::Cli, line);
    assert!(failure(&out).unwrap().contains("Something odd happened"));
}

const SSE: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[]}}\n\
\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\
\n\
event: ping\n\
data: {\"type\": \"ping\"}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi \"}}\n\
\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"there\"}}\n\
\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\
\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\
\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n";

#[test]
fn an_api_stream_gives_its_text_and_ends_at_message_stop_ignoring_what_follows() {
    let mut d = Decoder::new(Kind::Api);
    let mut out: Vec<Item> = SSE.lines().flat_map(|l| d.line(l)).collect();
    assert_eq!(text_of(&out), "Hi there");
    assert_eq!(out.last(), Some(&Item::Event(Event::Done)));
    assert!(d.finished());
    out = d.line(r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"late"}}"#);
    assert_eq!(out, vec![], "nothing after the end");
}

#[test]
fn an_api_overloaded_error_event_says_so() {
    let sse = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n";
    let out = items(Kind::Api, sse);
    assert_eq!(out.len(), 1);
    assert!(failure(&out).unwrap().contains("overloaded"), "{out:?}");
}

#[test]
fn a_plain_401_body_says_the_key_was_refused_and_to_check_it_in_settings() {
    let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
    let out = items(Kind::Api, body);
    let message = failure(&out).expect("a failure");
    assert!(message.contains("API key was refused") && message.contains("Settings"), "{message}");
}

#[test]
fn rate_limits_and_unknown_models_are_told_apart() {
    let rate = items(Kind::Api, r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#);
    assert!(failure(&rate).unwrap().contains("Rate limited"));
    let model = items(Kind::Api, r#"{"type":"error","error":{"type":"not_found_error","message":"model: claude-x"}}"#);
    let message = failure(&model).unwrap();
    assert!(message.contains("model") && message.contains("claude-x") && message.contains("Settings"), "{message}");
    let other = items(Kind::Api, r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages: too long"}}"#);
    assert!(failure(&other).unwrap().contains("messages: too long"));
}

#[test]
fn lines_that_arent_json_are_kept_aside_not_dropped_silently() {
    let mut d = Decoder::new(Kind::Cli);
    assert_eq!(d.line("Invalid API key · Please run /login"), vec![]);
    assert!(d.stray().contains("Please run /login"));
}

// Running a process.

#[test]
fn a_process_gets_its_stdin_and_its_lines_come_back_in_order() {
    let run = Run::new();
    let mut seen = Vec::new();
    let out = process::run(&run, Command::new("/bin/cat"), b"one\ntwo\nthree".to_vec(), |l| seen.push(l.to_owned())).unwrap();
    assert_eq!(seen, ["one", "two", "three"]);
    assert!(out.success);
}

#[test]
fn a_process_that_fails_says_what_it_wrote_to_stderr() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "echo boom >&2; exit 3"]);
    let out = process::run(&Run::new(), cmd, Vec::new(), |_| {}).unwrap();
    assert!(!out.success);
    assert_eq!(out.stderr.trim(), "boom");
}

#[test]
fn a_process_that_writes_a_lot_to_stderr_isnt_cut_off() {
    let (done, ended) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut cmd = Command::new("/bin/sh");
        // Past the cap: a reader that stops there closes the pipe, and the child dies writing.
        cmd.args(["-c", "head -c 300000 /dev/zero >&2 && echo done"]);
        let mut seen = Vec::new();
        let out = process::run(&Run::new(), cmd, Vec::new(), |l| seen.push(l.to_owned())).unwrap();
        let _ = done.send((seen, out.success));
    });
    let (seen, success) = ended.recv_timeout(Duration::from_secs(10)).expect("the run never ended");
    assert_eq!(seen, ["done"]);
    assert!(success);
}

#[test]
fn cancelling_ends_a_run_whose_process_left_a_child_holding_its_output() {
    let run = Run::new();
    let (said, pid) = std::sync::mpsc::channel();
    let (done, ended) = std::sync::mpsc::channel();
    {
        let run = run.clone();
        std::thread::spawn(move || {
            let mut cmd = Command::new("/bin/sh");
            // The leader exits at once; the sleep it started keeps stdout open.
            cmd.args(["-c", "sleep 30 & echo $!; exit 0"]);
            let _ = process::run(&run, cmd, Vec::new(), |l| {
                let _ = said.send(l.to_owned());
            });
            let _ = done.send(());
        });
    }
    let pid: i32 = pid.recv_timeout(Duration::from_secs(5)).expect("the sleep's pid").trim().parse().unwrap();
    run.cancel();
    ended.recv_timeout(Duration::from_secs(3)).expect("the run still waits on the sleep");
    // SAFETY: signal 0 only asks whether the process exists.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "the sleep it started is gone too");
}

#[test]
fn cancelling_a_run_kills_its_child_process() {
    let run = Run::new();
    let worker = {
        let run = run.clone();
        std::thread::spawn(move || {
            let mut cmd = Command::new("/bin/sleep");
            cmd.arg("30");
            process::run(&run, cmd, Vec::new(), |_| {})
        })
    };
    let started = Instant::now();
    while run.pid().is_none() {
        assert!(started.elapsed() < Duration::from_secs(5), "the child never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = run.pid().unwrap() as i32;
    // SAFETY: signal 0 only asks whether the process exists.
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0, "running before the cancel");
    assert!(!run.exited());
    let cancelled = Instant::now();
    run.cancel();
    while !run.exited() {
        assert!(cancelled.elapsed() < Duration::from_secs(1), "the child outlived the cancel");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(run.cancelled());
    // SAFETY: as above; the process is gone, so the call fails with ESRCH.
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "the process is gone");
    worker.join().unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(10), "the reader didn't wait the 30 s out");
}

#[test]
fn a_run_cancelled_before_its_process_starts_never_starts_it() {
    let run = Run::new();
    run.cancel();
    let mut cmd = Command::new("/bin/sleep");
    cmd.arg("30");
    let started = Instant::now();
    let _ = process::run(&run, cmd, Vec::new(), |_| {});
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(run.pid().is_none());
}

// The whole path, with a stand-in for `claude`.

fn collect(backend: Backend, request: Request) -> Vec<Item> {
    let mut out = Vec::new();
    drive(&Run::new(), backend, &request, &mut |item| out.push(item));
    out
}

#[test]
fn claude_code_replies_through_the_whole_path_reading_its_stdin_and_resuming_its_session() {
    let bin = scratch("fake");
    let log = bin.join("args.txt");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/claude_code_hi.jsonl");
    script(&bin.join("claude"), &format!("echo \"$@\" > '{}'\ncat > '{}.stdin'\ncat '{}'", log.display(), log.display(), fixture.display()), 0o755);
    let backend = Backend::ClaudeCode { path: bin.join("claude") };
    let out = collect(backend, Request { session: some("sess-9"), ..request("hello") });
    assert_eq!(text_of(&out), "Hello there, friend!");
    assert_eq!(out.last(), Some(&Item::Event(Event::Done)));
    assert!(std::fs::read_to_string(&log).unwrap().contains("--resume sess-9"));
    assert!(std::fs::read_to_string(format!("{}.stdin", log.display())).unwrap().contains("\"text\":\"hello\""));
}

#[test]
fn a_claude_that_breaks_off_without_a_result_fails_with_what_it_said() {
    let bin = scratch("fake2");
    script(&bin.join("claude"), "cat > /dev/null\necho 'Not logged in · Please run /login'\nexit 1", 0o755);
    let out = collect(Backend::ClaudeCode { path: bin.join("claude") }, request("hi"));
    let message = failure(&out).expect("a failure");
    assert!(message.contains("`claude` in Terminal"), "{message}");
}

#[test]
fn without_a_model_the_reply_fails_saying_how_to_get_one() {
    let out = collect(Backend::Missing, request("hi"));
    assert_eq!(out, vec![Item::Event(Event::Fail("No model to answer: add an Anthropic API key in Settings, or install Claude Code.".into()))]);
}

/// The real Claude Code, once, by hand: `cargo test a_real_claude_code -- --ignored --nocapture`.
#[test]
#[ignore = "runs the real claude CLI (a fraction of a cent on the user's login)"]
fn a_real_claude_code_reply_streams_in() {
    let path = claude_code::find().expect("claude isn't installed");
    let out = collect(Backend::ClaudeCode { path }, request("Say hi in three words"));
    eprintln!("{out:?}");
    assert!(out.iter().any(|i| matches!(i, Item::Event(Event::Text(_)))), "no text came: {out:?}");
    assert!(!text_of(&out).trim().is_empty());
    assert_eq!(out.last(), Some(&Item::Event(Event::Done)), "{out:?}");
    assert!(out.iter().any(|i| matches!(i, Item::Session(_))), "no session id: {out:?}");
}
