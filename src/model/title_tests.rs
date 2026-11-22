use std::time::{Duration, Instant};

use super::tests::{scratch, script};
use super::*;

fn claude_args(model: &str) -> Vec<String> {
    claude_code::title_args(model)
}

fn after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn count(text: &str, c: char) -> usize {
    text.chars().filter(|x| *x == c).count()
}

// What goes out.

#[test]
fn the_title_request_asks_the_cheap_model_with_no_history_and_at_most_a_thousand_characters_of_each_side() {
    // Letters the framing around them doesn't use.
    let request = title::request(&"x".repeat(5000), &"q".repeat(5000));
    assert_eq!(request.model, "claude-haiku-5-5");
    assert!(request.history.is_empty() && request.pngs.is_empty() && request.session.is_none());
    assert_eq!(count(&request.text, 'x'), 1000);
    assert_eq!(count(&request.text, 'q'), 1000);
    let short = title::request("hello", "hi there");
    assert!(short.text.contains("hello") && short.text.contains("hi there"));
    let wide = title::request(&"é".repeat(3000), "ok");
    assert_eq!(count(&wide.text, 'é'), 1000, "characters, not bytes");
    assert!(title::request("", "a reply").text.contains("screenshot"), "a capture alone still says what it was");
}

#[test]
fn the_api_title_body_is_one_small_message_to_haiku_capped_at_thirty_tokens() {
    let body = api::title_body(&title::request("hello", "hi"));
    assert_eq!(body["model"], "claude-haiku-5-5");
    assert_eq!(body["max_tokens"], 30);
    assert_eq!(body["stream"], true);
    assert_eq!(body["system"], title::SYSTEM_PROMPT);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "no history");
    assert_eq!(messages[0]["role"], "user");
    assert!(messages[0]["content"][0]["text"].as_str().unwrap().contains("hello"));
}

#[test]
fn claude_code_titles_lean_without_leaving_a_session_and_never_resuming_one() {
    let args = claude_args("claude-haiku-5-5");
    assert!(args.iter().any(|a| a == "--no-session-persistence"), "{args:?}");
    assert!(!args.iter().any(|a| a == "--resume"), "{args:?}");
    assert_eq!(after(&args, "--model").as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(after(&args, "--tools").as_deref(), Some(""), "chat only");
    assert_eq!(after(&args, "--setting-sources").as_deref(), Some(""), "no hooks or plugins");
    assert_eq!(after(&args, "--system-prompt").as_deref(), Some(title::SYSTEM_PROMPT));
    assert!(args.iter().any(|a| a == "--strict-mcp-config"));
}

// The title itself.

#[test]
fn a_title_comes_back_without_quotes_on_one_line_and_at_most_sixty_characters() {
    assert_eq!(title::clean("\"Fixing the Dock icon\"").as_deref(), Some("Fixing the Dock icon"));
    assert_eq!(title::clean("“Curly quotes”").as_deref(), Some("Curly quotes"));
    assert_eq!(title::clean("'Single' ").as_deref(), Some("Single"));
    assert_eq!(title::clean("`Ticks`").as_deref(), Some("Ticks"));
    assert_eq!(title::clean("  Plain title \n").as_deref(), Some("Plain title"));
    assert_eq!(title::clean("First line\nSecond line").as_deref(), Some("First line"));
    assert_eq!(title::clean("\"Quoted title.\"").as_deref(), Some("Quoted title"), "no final period");
    let long = title::clean(&"word ".repeat(40)).unwrap();
    assert!(long.chars().count() <= 60, "{long}");
    assert!(long.starts_with("word word") && !long.ends_with(' '));
    assert_eq!(title::clean(&"é".repeat(100)).unwrap().chars().count(), 60, "characters, not bytes");
    assert_eq!(title::clean(""), None);
    assert_eq!(title::clean(" \"\" "), None);
    assert_eq!(title::clean("\n\n"), None);
}

// The whole call, with a stand-in for `claude`.

fn fixture() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/claude_code_hi.jsonl").display().to_string()
}

#[test]
fn a_title_is_asked_of_claude_code_with_the_cheap_model_and_no_session_flags() {
    let bin = scratch("title-fake");
    let log = bin.join("args.txt");
    script(&bin.join("claude"), &format!("echo \"$@\" > '{}'\ncat > '{}.stdin'\ncat '{}'", log.display(), log.display(), fixture()), 0o755);
    let request = title::request("tell me about tea", "Tea is a drink.");
    let title = title::ask(&Run::new(), Backend::ClaudeCode { path: bin.join("claude") }, &scratch("title-dir"), &request);
    assert_eq!(title.as_deref(), Some("Hello there, friend!"));
    let args = std::fs::read_to_string(&log).unwrap();
    assert!(args.contains("--model claude-haiku-5-5") && args.contains("--no-session-persistence"), "{args}");
    assert!(!args.contains("--resume"), "{args}");
    let stdin = std::fs::read_to_string(format!("{}.stdin", log.display())).unwrap();
    assert!(stdin.contains("tell me about tea") && stdin.contains("Tea is a drink."), "{stdin}");
}

#[test]
fn a_title_call_that_fails_leaves_no_title() {
    let bin = scratch("title-fail");
    let dir = scratch("title-fail-dir");
    script(&bin.join("claude"), "cat > /dev/null\necho 'Not logged in · Please run /login'\nexit 1", 0o755);
    let request = title::request("hi", "hello");
    assert_eq!(title::ask(&Run::new(), Backend::ClaudeCode { path: bin.join("claude") }, &dir, &request), None);
    script(&bin.join("claude"), "cat > /dev/null\necho '{\"type\":\"result\",\"is_error\":true,\"result\":\"boom\"}'", 0o755);
    assert_eq!(title::ask(&Run::new(), Backend::ClaudeCode { path: bin.join("claude") }, &dir, &request), None);
    assert_eq!(title::ask(&Run::new(), Backend::Missing, &dir, &request), None);
    // It ended well but said nothing usable.
    script(&bin.join("claude"), "cat > /dev/null\necho '{\"type\":\"result\",\"is_error\":false}'", 0o755);
    assert_eq!(title::ask(&Run::new(), Backend::ClaudeCode { path: bin.join("claude") }, &dir, &request), None);
}

#[test]
fn a_title_call_that_takes_too_long_is_ended_and_leaves_no_title() {
    let bin = scratch("title-slow");
    let pid_file = bin.join("pid");
    script(&bin.join("claude"), &format!("echo $$ > '{}'\ncat > /dev/null\nsleep 30", pid_file.display()), 0o755);
    let request = title::request("hi", "hello");
    let started = Instant::now();
    let title = title::with_timeout(Backend::ClaudeCode { path: bin.join("claude") }, &scratch("title-slow-dir"), &request, Duration::from_millis(400));
    assert_eq!(title, None);
    assert!(started.elapsed() < Duration::from_secs(10), "it waited the sleep out: {:?}", started.elapsed());
    // On a loaded machine the timeout may land before the process even starts, which is no
    // process left either; one that did start has written its id by now.
    let waited = Instant::now();
    let pid = loop {
        match std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<i32>().ok()) {
            Some(pid) => break Some(pid),
            None if waited.elapsed() > Duration::from_secs(1) => break None,
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    if let Some(pid) = pid {
        // SAFETY: signal 0 only asks whether the process exists.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "the process is gone after the timeout");
    }
}

#[test]
fn a_title_that_arrives_in_time_is_not_cancelled_by_the_timeout() {
    let bin = scratch("title-quick");
    script(&bin.join("claude"), &format!("cat > /dev/null\ncat '{}'", fixture()), 0o755);
    let request = title::request("hi", "hello");
    let title = title::with_timeout(Backend::ClaudeCode { path: bin.join("claude") }, &scratch("title-quick-dir"), &request, Duration::from_secs(20));
    assert_eq!(title.as_deref(), Some("Hello there, friend!"));
}

/// The real Claude Code, once, by hand: `cargo test a_real_claude_code_title -- --ignored --nocapture`.
#[test]
#[ignore = "runs the real claude CLI (a fraction of a cent on the user's login)"]
fn a_real_claude_code_title_comes_back_short() {
    let path = claude_code::find().expect("claude isn't installed");
    let request = title::request("How do I make a good cup of green tea?", "Heat water to about 80 °C, steep the leaves for two minutes, and don't let it boil.");
    let title = title::with_timeout(Backend::ClaudeCode { path }, &scratch("title-real"), &request, title::TIMEOUT);
    eprintln!("{title:?}");
    let title = title.expect("no title came");
    assert!(!title.is_empty() && title.chars().count() <= 60 && !title.contains('"'), "{title}");
}
