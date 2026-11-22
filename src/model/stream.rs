//! Turns a model's stream, line by line, into [`Item`]s. The API sends server-sent events
//! (`event:` and `data:` lines, or a plain JSON body for an HTTP error); Claude Code's
//! `stream-json` wraps the same events in `{"type":"stream_event","event":…}` and adds a
//! `system`/`init` line (the session id) and a final `result`.

use serde_json::Value;

use super::{Item, errors};
use crate::reply::Event;

/// Which stream: the API's own, or Claude Code's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Api,
    Cli,
}

pub struct Decoder {
    kind: Kind,
    done: bool,
    stray: String,
}

impl Decoder {
    pub fn new(kind: Kind) -> Decoder {
        Decoder { kind, done: false, stray: String::new() }
    }

    /// What `line` says; nothing once the stream has ended (`Done` or `Fail` came).
    pub fn line(&mut self, line: &str) -> Vec<Item> {
        let line = line.trim();
        // SSE's `event:` name repeats the data's own `type`; `:` lines are comments.
        if self.done || line.is_empty() || line.starts_with("event:") || line.starts_with(':') {
            return Vec::new();
        }
        let data = line.strip_prefix("data:").map_or(line, str::trim);
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            if !self.stray.is_empty() {
                self.stray.push(' ');
            }
            self.stray.push_str(line);
            return Vec::new();
        };
        let items = match value["type"].as_str() {
            Some("stream_event") => self.event(&value["event"]),
            Some("system") => session(&value, value["subtype"] == "init"),
            Some("result") => self.result(&value),
            Some(_) => self.event(&value),
            None => Vec::new(),
        };
        if items.iter().any(|i| matches!(i, Item::Event(Event::Done | Event::Fail(_)))) {
            self.done = true;
        }
        items
    }

    /// One of the API's events.
    fn event(&self, event: &Value) -> Vec<Item> {
        let one = |e: Event| vec![Item::Event(e)];
        match event["type"].as_str() {
            Some("content_block_delta") => {
                let delta = &event["delta"];
                match (delta["type"].as_str(), delta["text"].as_str(), delta["thinking"].as_str()) {
                    (Some("text_delta"), Some(text), _) if !text.is_empty() => one(Event::Text(text.into())),
                    (Some("thinking_delta"), _, Some(thought)) if !thought.is_empty() => one(Event::Think(thought.into())),
                    _ => Vec::new(),
                }
            }
            // Claude Code goes on to its `result`; the API's stream ends here.
            Some("message_stop") if self.kind == Kind::Api => one(Event::Done),
            Some("error") => {
                let error = &event["error"];
                one(Event::Fail(errors::api_error(error["type"].as_str().unwrap_or(""), error["message"].as_str().unwrap_or("something went wrong"))))
            }
            _ => Vec::new(),
        }
    }

    /// Claude Code's last line: the session, then how it went.
    fn result(&self, value: &Value) -> Vec<Item> {
        let mut items = session(value, true);
        if value["is_error"].as_bool().unwrap_or(false) {
            let message = value["result"].as_str().unwrap_or("something went wrong");
            items.push(Item::Event(Event::Fail(errors::cli_error(value["api_error_status"].as_u64(), message))));
        } else {
            items.push(Item::Event(Event::Done));
        }
        items
    }

    /// `Done` or `Fail` came.
    pub fn finished(&self) -> bool {
        self.done
    }

    /// Lines that weren't JSON, joined: what a CLI that broke off may have said.
    pub fn stray(&self) -> &str {
        &self.stray
    }
}

/// The session id of a Claude Code line that carries it for good (`init`, `result`).
fn session(value: &Value, wanted: bool) -> Vec<Item> {
    match value["session_id"].as_str() {
        Some(id) if wanted && !id.is_empty() => vec![Item::Session(id.into())],
        _ => Vec::new(),
    }
}
