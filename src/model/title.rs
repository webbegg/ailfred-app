//! The automatic title of a conversation: one small call to a cheap model once its first reply is
//! in, from the start of the message and of the reply, with a timeout. No history, no session
//! left behind. Any failure is no title.

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use super::{Backend, Item, Request, Run, api, claude_code, detect};
use crate::reply::Event;

pub const MODEL: &str = "claude-haiku-5-5";
pub const MAX_TOKENS: u32 = 30;
pub const TIMEOUT: Duration = Duration::from_secs(20);
/// How much of the first message, and of its reply, the call gets.
pub const INPUT_CAP: usize = 1000;
/// The longest title kept.
pub const MAX_CHARS: usize = 60;
pub const SYSTEM_PROMPT: &str = "You name conversations. Reply with only a title of two to six words for the conversation you are shown: no quotes, no final punctuation.";

/// The call that titles a conversation that began with `first` and was answered `reply`.
pub fn request(first: &str, reply: &str) -> Request {
    let cap = |text: &str| text.trim().chars().take(INPUT_CAP).collect::<String>();
    let first = if first.trim().is_empty() { "(only a screenshot)".to_owned() } else { cap(first) };
    let text = format!("The conversation's first message:\n{first}\n\nThe reply:\n{}\n\nGive the conversation a title.", cap(reply));
    Request { model: MODEL.into(), history: Vec::new(), text, pngs: Vec::new(), session: None, answers: None, decline: false, steer: false }
}

/// Quotes, periods and blanks around a title go.
fn is_trim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’' | '«' | '»' | '.')
}

/// A model's answer as a title: its first line without quotes, at most `MAX_CHARS`; None if
/// nothing is left.
pub fn clean(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let whole = line.trim_matches(is_trim);
    let title: String = whole.chars().take(MAX_CHARS).collect();
    Some(title.trim_end_matches(is_trim).to_owned()).filter(|t| !t.is_empty())
}

fn drive(run: &Run, backend: Backend, dir: &Path, request: &Request, sink: &mut dyn FnMut(Item)) {
    match backend {
        Backend::Api { key, .. } => api::run_title(run, &key, request, sink),
        Backend::ClaudeCode { path } => claude_code::run_title(run, &path, dir, request, sink),
        Backend::Missing => {}
    }
}

/// Asks `backend` (Claude Code run from `dir`): the title if the call ended well.
pub fn ask(run: &Run, backend: Backend, dir: &Path, request: &Request) -> Option<String> {
    let (mut text, mut done) = (String::new(), false);
    drive(run, backend, dir, request, &mut |item| match item {
        Item::Event(Event::Text(more)) => text.push_str(&more),
        Item::Event(Event::Done) => done = true,
        _ => {}
    });
    if run.cancelled() || !done { None } else { clean(&text) }
}

/// `ask`, its process ended once `timeout` is up.
pub fn with_timeout(backend: Backend, dir: &Path, request: &Request, timeout: Duration) -> Option<String> {
    let run = Run::new();
    let (finished, waiting) = mpsc::channel::<()>();
    let watched = run.clone();
    // Wakes up when `finished` is dropped (the call ended), or when the time is up.
    std::thread::spawn(move || {
        if waiting.recv_timeout(timeout) == Err(RecvTimeoutError::Timeout) {
            watched.cancel();
        }
    });
    let title = ask(&run, backend, dir, request);
    drop(finished);
    title
}

/// Asks on its own thread, with the backend that replies use (chosen there: the Keychain may
/// take a moment); `done` hears the title, or None.
pub fn start(request: Request, timeout: Duration, done: impl FnOnce(Option<String>) + Send + 'static) {
    std::thread::spawn(move || done(with_timeout(detect(), &claude_code::private_dir_now(), &request, timeout)));
}
