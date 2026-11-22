//! The Messages API through `/usr/bin/curl`: no TLS crate. The key goes in a curl config on
//! stdin (never in the arguments, which `ps` shows), the body in a private temp file.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use super::stream::Kind;
use super::{Item, MAX_TOKENS, Request, Run, SYSTEM_PROMPT, errors, pump, user_content};
use crate::reply::Event;

pub const URL: &str = "https://api.anthropic.com/v1/messages";
pub const VERSION: &str = "2023-06-01";
const CURL: &str = "/usr/bin/curl";

/// The request's JSON body: the conversation's earlier turns, then the message.
pub fn body(request: &Request) -> serde_json::Value {
    let mut messages = Vec::new();
    for turn in &request.history {
        messages.push(json!({"role": "user", "content": user_content(&turn.text, &turn.pngs)}));
        messages.push(json!({"role": "assistant", "content": turn.reply}));
    }
    messages.push(json!({"role": "user", "content": user_content(&request.text, &request.pngs)}));
    json!({
        "model": request.model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "system": SYSTEM_PROMPT,
        "messages": messages,
    })
}

/// The body in a file only its owner can read, removed when dropped.
pub struct BodyFile {
    path: PathBuf,
}

impl BodyFile {
    pub fn write(dir: &Path, name: &str, body: &serde_json::Value) -> std::io::Result<BodyFile> {
        let path = dir.join(format!("ailfred-{}-{name}.json", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        // Owned from here, so a failed write doesn't leave the file behind.
        let owned = BodyFile { path };
        file.write_all(&serde_json::to_vec(body)?)?;
        Ok(owned)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for BodyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// curl's arguments: no key here.
pub fn args() -> Vec<String> {
    ["-sS", "-N", "--connect-timeout", "15", "-H", "content-type: application/json", "-H", &format!("anthropic-version: {VERSION}"), "-K", "-"]
        .map(String::from)
        .to_vec()
}

/// curl's config (stdin): the key, the URL and the body's file.
pub fn config(key: &str, body_file: &Path) -> String {
    // Control characters would end a line of the config, or a header; no key has any.
    let key: String = key.chars().filter(|c| !c.is_control()).collect();
    let quoted = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    format!(
        "url = {}\nheader = {}\ndata-binary = {}\n",
        quoted(URL),
        quoted(&format!("x-api-key: {key}")),
        quoted(&format!("@{}", body_file.display()))
    )
}

/// Answers `request` with the API, streaming into `sink`.
pub fn run(run: &Run, key: &str, request: &Request, sink: &mut dyn FnMut(Item)) {
    static N: AtomicU64 = AtomicU64::new(0);
    let name = N.fetch_add(1, Ordering::Relaxed).to_string();
    let file = match BodyFile::write(&std::env::temp_dir(), &name, &body(request)) {
        Ok(file) => file,
        Err(e) => return sink(Item::Event(Event::Fail(errors::not_started(Kind::Api, &e)))),
    };
    let mut cmd = Command::new(CURL);
    cmd.args(args());
    pump(run, Kind::Api, cmd, config(key, file.path()).into_bytes(), sink);
}
