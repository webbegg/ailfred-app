//! A conversation as one `MESSAGES_SNAPSHOT`, for a client that joins it (the server sends it,
//! T20), and back: the "MessagesSnapshot replay" a client does.
//!
//! Per message, a `user` message `{run}-user` (its text, its captures as base64 PNG image
//! parts), then its reply, part by part, under the ids its run's events use (crate docs):
//!
//! | part | message |
//! |---|---|
//! | thinking | `reasoning` `{run}-thinking-{n}` |
//! | text | `assistant` `{run}-text-{n}` with the text as its content |
//! | agent steps | `assistant` `{run}-steps-{n}` with a `toolCalls` entry per step (`{run}-step-{k}`, `k` counting the reply's steps from 1, as its saved events do; arguments `{"target":…}`), then a `tool` message `{run}-step-{k}-result` per step that ended: its output, and `error` if it failed |
//! | diff | `activity` `ailfred:diff` `{run}-diff-{n}` `{"patch","path"}` |
//! | question | `activity` `ailfred:ask` `{run}-ask-{n}` `{"interruptId","message","multi","options"}`, and `"answered"` once it is |
//! | error | `activity` `ailfred:error` `{run}-error-{n}` `{"message"}` |
//!
//! What a snapshot doesn't carry: how a reply ended, but for an error (a reply with one comes
//! back failed, any other done: stopped, interrupted or still going alike, its steps that
//! hadn't ended still running); how long it thought (none); what was picked in a question not
//! yet answered; what's open or applied; the engine's step ids; a reply with no part (it reads
//! as no reply). A message before any user message joins a turn with no text.

use std::time::Instant;

use ailfred_core::reply::{Event, Part, Reply, State, StepStatus};
use serde_json::{Value, json};

use crate::agui::{self, Base, CallKind, Content, ContentPart, FunctionCall, Message, PartSource, ToolCall};
use crate::command::user_message;
use crate::encode::{ASK, DIFF, tool_name};
use crate::replay::{step_of, text_of};

/// The activity type of a reply's error.
pub const ERROR: &str = "ailfred:error";
/// A failed step's `error`.
pub const STEP_FAILED: &str = "The step failed.";

/// One message of a conversation and its reply, to put in a snapshot.
pub struct Turn<'a> {
    /// The run its reply is: the ids derive from it.
    pub run: &'a str,
    pub text: &'a str,
    /// Its captures, as PNG bytes.
    pub pngs: &'a [&'a [u8]],
    pub reply: Option<&'a Reply>,
}

/// One message of a conversation and its reply, read back from a snapshot.
pub struct Restored {
    pub text: String,
    pub pngs: Vec<Vec<u8>>,
    pub reply: Option<Reply>,
}

/// The conversation `turns` as a `MESSAGES_SNAPSHOT` stamped `at` (ms since the epoch).
pub fn messages_snapshot(turns: &[Turn], at: u64) -> agui::Event {
    let mut messages = Vec::new();
    for turn in turns {
        messages.push(user_message(&format!("{}-user", turn.run), turn.text, turn.pngs));
        if let Some(reply) = turn.reply {
            reply_messages(reply, turn.run, &mut messages);
        }
    }
    agui::Event::MessagesSnapshot { messages, base: Base::at(at) }
}

fn reply_messages(reply: &Reply, run: &str, out: &mut Vec<Message>) {
    let (mut thoughts, mut texts, mut agents, mut steps, mut diffs, mut asks, mut errors) = (0, 0, 0, 0, 0, 0, 0);
    let activity = |id: String, kind: &str, content: Value| {
        let Value::Object(content) = content else { unreachable!("an object literal") };
        Message::Activity { id, activity_type: kind.into(), content }
    };
    for part in &reply.parts {
        match part {
            Part::Thinking(t) => {
                thoughts += 1;
                out.push(Message::Reasoning { id: format!("{run}-thinking-{thoughts}"), content: t.text.clone() });
            }
            Part::Text(text) => {
                texts += 1;
                out.push(Message::Assistant { id: format!("{run}-text-{texts}"), content: Some(text.clone()), tool_calls: None, name: None });
            }
            Part::Agent(agent) => {
                agents += 1;
                let call = |k: usize| format!("{run}-step-{}", steps + k + 1);
                let calls = agent.steps.iter().enumerate().map(|(k, s)| {
                    let function = FunctionCall { name: tool_name(s.tool).into(), arguments: json!({ "target": s.target }).to_string() };
                    ToolCall { id: call(k), kind: CallKind::Function, function }
                });
                out.push(Message::Assistant { id: format!("{run}-steps-{agents}"), content: None, tool_calls: Some(calls.collect()), name: None });
                for (k, s) in agent.steps.iter().enumerate().filter(|(_, s)| s.status != StepStatus::Running) {
                    let error = (s.status == StepStatus::Failed).then(|| STEP_FAILED.to_owned());
                    out.push(Message::Tool { id: format!("{}-result", call(k)), content: Content::Text(s.output.clone()), tool_call_id: call(k), error });
                }
                steps += agent.steps.len();
            }
            Part::Diff(d) => {
                diffs += 1;
                out.push(activity(format!("{run}-diff-{diffs}"), DIFF, json!({ "patch": d.patch, "path": d.path })));
            }
            Part::Question(q) => {
                asks += 1;
                let id = format!("{run}-ask-{asks}");
                let mut content = json!({ "interruptId": id, "message": q.prompt, "multi": q.multi, "options": q.options });
                if let Some(answer) = &q.answered {
                    content["answered"] = json!(answer);
                }
                out.push(activity(id, ASK, content));
            }
            Part::Error(e) => {
                errors += 1;
                out.push(activity(format!("{run}-error-{errors}"), ERROR, json!({ "message": e })));
            }
        }
    }
}

/// The conversation a snapshot's `messages` hold: each user message, and what follows it until
/// the next as its reply (numbered by its place, from 0, with no token).
pub fn conversation_from_snapshot(messages: &[Message]) -> Vec<Restored> {
    let now = Instant::now();
    let mut out: Vec<Restored> = Vec::new();
    // The reply being read's tool calls, and the step each is.
    let mut calls: Vec<(String, u32)> = Vec::new();
    for message in messages {
        if let Message::User { content, .. } = message {
            let (text, pngs) = said(content);
            out.push(Restored { text, pngs, reply: None });
            calls.clear();
            continue;
        }
        if out.is_empty() {
            out.push(Restored { text: String::new(), pngs: Vec::new(), reply: None });
        }
        let index = out.len() - 1;
        let turn = &mut out[index];
        let reply = turn.reply.get_or_insert_with(|| Reply::new(index as u64, turn.text.clone()));
        match message {
            Message::Reasoning { content, .. } => {
                // A thinking of its own, even right after another.
                reply.apply(Event::Taken, now);
                reply.apply(Event::Think(content.clone()), now);
            }
            Message::Assistant { content, tool_calls, .. } => {
                if let Some(text) = content {
                    reply.apply(Event::Text(text.clone()), now);
                }
                for call in tool_calls.iter().flatten() {
                    let id = calls.len() as u32 + 1;
                    calls.push((call.id.clone(), id));
                    let (tool, target) = step_of(&call.function.name, &call.function.arguments);
                    reply.apply(Event::Step { id, tool, target }, now);
                }
            }
            Message::Tool { content, tool_call_id, error, .. } => {
                let Some(&(_, id)) = calls.iter().find(|(c, _)| c == tool_call_id) else { continue };
                let output = text_of(content);
                if !output.is_empty() {
                    reply.apply(Event::StepOutput { id, text: output }, now);
                }
                reply.apply(Event::StepEnd { id, ok: error.is_none() }, now);
            }
            Message::Activity { activity_type, content, .. } => {
                let field = |k: &str| content.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
                match activity_type.as_str() {
                    DIFF => reply.apply(Event::Diff { path: field("path"), patch: field("patch") }, now),
                    ASK => {
                        let options = content.get("options").and_then(Value::as_array).map(|o| o.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                        let multi = content.get("multi").and_then(Value::as_bool).unwrap_or(false);
                        reply.apply(Event::Ask { prompt: field("message"), options, multi }, now);
                        if let (Some(Part::Question(q)), Some(answer)) = (reply.parts.last_mut(), content.get("answered").and_then(Value::as_str)) {
                            q.answered = Some(answer.to_owned());
                        }
                    }
                    ERROR => reply.apply(Event::Fail(field("message")), now),
                    _ => {}
                }
            }
            Message::User { .. } | Message::Developer { .. } | Message::System { .. } => {}
        }
    }
    for turn in &mut out {
        if let Some(reply) = &mut turn.reply {
            end(reply, now);
        }
    }
    out
}

/// A reply read whole: its thinking ends, and it's done unless it failed, its steps as they were.
fn end(reply: &mut Reply, now: Instant) {
    reply.apply(Event::Taken, now);
    if reply.streaming() {
        reply.state = State::Done;
    }
}

/// A user message's text, and the PNGs among its parts.
fn said(content: &Content) -> (String, Vec<Vec<u8>>) {
    let pngs = match content {
        Content::Text(_) => Vec::new(),
        Content::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Image { source: PartSource::Data { value, mime_type } } if mime_type == "image/png" => unbase64(value),
                _ => None,
            })
            .collect(),
    };
    (text_of(content), pngs)
}

/// Standard base64 back into bytes; None if it isn't.
fn unbase64(coded: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(coded.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in coded.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ailfred_core::turn::base64;

    #[test]
    fn base64_reads_back_what_it_wrote() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff, 0xfe, 0x00]] {
            assert_eq!(unbase64(&base64(bytes)).as_deref(), Some(bytes));
        }
        assert_eq!(unbase64("no spaces"), None);
    }
}
