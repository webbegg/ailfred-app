//! The model: what writes the replies. With an Anthropic API key (the Keychain's, else
//! `ANTHROPIC_API_KEY`) Claude answers through the Messages API over `/usr/bin/curl`; without
//! one, the installed Claude Code (`claude -p`) does, on the user's own login. Both stream the
//! API's events, which [`stream`] turns into [`Event`]s. Neither runs tools: it's chat only.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::json;

use crate::reply::Event;

mod api;
mod claude_code;
mod errors;
mod process;
mod stream;
#[cfg(test)]
mod tests;

pub use process::Run;

pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";
/// Ailfred's own pi (`pi.rs`), on the model of the user's pi setup.
pub const PI: &str = "pi";
/// The models to pick from: id and label.
pub const MODELS: [(&str, &str); 3] = [(DEFAULT_MODEL, "Sonnet 5.5"), ("claude-opus-5-5", "Opus 5.5"), (PI, "pi")];
pub const MAX_TOKENS: u32 = 8192;
pub const SYSTEM_PROMPT: &str = "You are Ailfred, a concise personal assistant on the user's Mac. \
Answer briefly, in plain markdown, and say so when you don't know.";

/// Where the replies come from.
#[derive(Clone, PartialEq)]
pub enum Backend {
    Api { key: String, source: KeySource },
    ClaudeCode { path: PathBuf },
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    Keychain,
    Env,
}

// The key never reaches a log line.
impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Backend::Api { source, .. } => write!(f, "Api({source:?}, key hidden)"),
            Backend::ClaudeCode { path } => write!(f, "ClaudeCode({})", path.display()),
            Backend::Missing => write!(f, "Missing"),
        }
    }
}

/// The Keychain's key first, then the environment's, then Claude Code (`claude`, if found);
/// blank keys count as none.
pub fn choose(keychain_key: Option<String>, env_key: Option<String>, claude: Option<PathBuf>) -> Backend {
    let key = |k: Option<String>| k.map(|k| k.trim().to_owned()).filter(|k| !k.is_empty());
    if let Some(key) = key(keychain_key) {
        return Backend::Api { key, source: KeySource::Keychain };
    }
    if let Some(key) = key(env_key) {
        return Backend::Api { key, source: KeySource::Env };
    }
    match claude {
        Some(path) => Backend::ClaudeCode { path },
        None => Backend::Missing,
    }
}

/// The backend now: reads the Keychain, so only at send time (off the UI thread).
fn detect() -> Backend {
    let env = std::env::var("ANTHROPIC_API_KEY").ok();
    choose(crate::keychain::get(), env, claude_code::find())
}

/// What the Settings window says replies come from.
pub fn status(keychain: bool, env: bool, claude: Option<&Path>) -> String {
    if keychain {
        "Replies come from the Anthropic API (key in Keychain)".into()
    } else if env {
        "Replies come from the Anthropic API (ANTHROPIC_API_KEY)".into()
    } else if let Some(path) = claude {
        format!("Replies come from Claude Code at {}", path.display())
    } else {
        "No model: add an Anthropic API key in Settings, or install Claude Code".into()
    }
}

/// Settings' status line: the Keychain item's presence is asked without reading the key.
pub fn status_now() -> String {
    let env = std::env::var("ANTHROPIC_API_KEY").is_ok_and(|k| !k.trim().is_empty());
    status(crate::keychain::exists(), env, claude_code::find().as_deref())
}

/// One earlier exchange of the conversation: what the user sent and what came back.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub text: String,
    pub pngs: Vec<Arc<Vec<u8>>>,
    pub reply: String,
}

/// One message to answer, with the conversation before it.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub model: String,
    pub history: Vec<Turn>,
    pub text: String,
    pub pngs: Vec<Arc<Vec<u8>>>,
    /// Claude Code's session of this conversation, once it has one.
    pub session: Option<String>,
    /// The user's answer to the model's question (pi waits for it), not a new message.
    pub answer: bool,
}

/// What a run reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Event(Event),
    /// Claude Code's session id, to resume it with the next message.
    Session(String),
}

/// Where a reply's items go (any thread).
pub type Sink = Arc<dyn Fn(Item) + Send + Sync>;

/// What writes the replies, one at a time: [`Claude`], or Ailfred's pi (`pi_setup::Pi`).
pub trait Model: Send {
    /// Starts the reply to `request`, its items going to `sink` until `Done` or `Fail`; a reply
    /// streaming is stopped first.
    fn send(&mut self, request: Request, sink: Sink);
    /// Stop: the reply streaming ends, and what it still had goes nowhere.
    fn stop(&mut self);
    /// Cmd+N: what comes next starts afresh (Claude is told the history each time).
    fn new_conversation(&mut self) {}
}

/// Claude, through the API or Claude Code (chosen at each send): a run per reply.
#[derive(Default)]
pub struct Claude {
    run: Option<Run>,
}

impl Model for Claude {
    fn send(&mut self, request: Request, sink: Sink) {
        self.stop();
        self.run = Some(start(request, move |item| sink(item)));
    }

    fn stop(&mut self) {
        if let Some(run) = self.run.take() {
            run.cancel();
        }
    }
}

/// Runs `request` on `backend` in the calling thread, reporting to `sink`: `Done` or `Fail` ends
/// it (unless `run` was cancelled, which ends it silently).
fn drive(run: &Run, backend: Backend, request: &Request, sink: &mut dyn FnMut(Item)) {
    match backend {
        Backend::Api { key, .. } => api::run(run, &key, request, sink),
        Backend::ClaudeCode { path } => claude_code::run(run, &path, &claude_code::private_dir_now(), request, sink),
        Backend::Missing => sink(Item::Event(Event::Fail(errors::MISSING.into()))),
    }
}

/// Starts answering `request` on its own thread: the backend is chosen there (the Keychain may
/// take a moment), `sink` hears every item until `Done` or `Fail`. The [`Run`] ends it.
pub fn start(request: Request, mut sink: impl FnMut(Item) + Send + 'static) -> Run {
    let run = Run::new();
    let own = run.clone();
    std::thread::spawn(move || drive(&own, detect(), &request, &mut sink));
    run
}

/// Runs `cmd` (curl or claude) with `input` on stdin and decodes its stdout stream into `sink`.
/// A stream that ends without `Done` or `Fail` fails in words, unless it was cancelled.
fn pump(run: &Run, kind: stream::Kind, cmd: Command, input: Vec<u8>, sink: &mut dyn FnMut(Item)) {
    let mut decoder = stream::Decoder::new(kind);
    let outcome = process::run(run, cmd, input, |line| {
        for item in decoder.line(line) {
            if !run.cancelled() {
                sink(item);
            }
        }
    });
    if run.cancelled() || decoder.finished() {
        return;
    }
    let message = match outcome {
        Err(e) => errors::not_started(kind, &e),
        Ok(outcome) => errors::broke_off(kind, decoder.stray(), &outcome.stderr, outcome.success),
    };
    sink(Item::Event(Event::Fail(message)));
}

/// Standard base64 of `bytes`, padded.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A user message's content blocks: its captures as base64 PNG images, then its text.
pub fn user_content(text: &str, pngs: &[Arc<Vec<u8>>]) -> serde_json::Value {
    let mut blocks: Vec<serde_json::Value> = pngs
        .iter()
        .map(|png| json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": base64(png)}}))
        .collect();
    if !text.is_empty() {
        blocks.push(json!({"type": "text", "text": text}));
    }
    serde_json::Value::Array(blocks)
}
