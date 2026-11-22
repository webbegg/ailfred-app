//! A reply to what's saved of it, and back. Only thinking, text and the error are kept: agent
//! steps, diffs and questions come from the mock model only.

use std::time::{Duration, Instant};

use super::{INTERRUPTED, ReplyState, SavedReply};
use crate::reply::{Part, Reply, State, Thinking};

fn joined<'a>(texts: impl Iterator<Item = &'a str>) -> String {
    texts.filter(|t| !t.trim().is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// What to save of `reply`: one still streaming is written `interrupted` (if the app quits now,
/// that's what's left of it).
pub fn saved_reply(reply: &Reply) -> SavedReply {
    let state = match reply.state {
        State::Streaming => ReplyState::Interrupted,
        State::Done => ReplyState::Done,
        State::Stopped => ReplyState::Stopped,
        State::Failed => ReplyState::Failed,
    };
    let thinking = joined(reply.parts.iter().filter_map(|p| if let Part::Thinking(t) = p { Some(t.text.as_str()) } else { None }));
    let text = joined(reply.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }));
    let error = joined(reply.parts.iter().filter_map(|p| if let Part::Error(e) = p { Some(e.as_str()) } else { None }));
    SavedReply { state, thinking: Some(thinking).filter(|t| !t.is_empty()), text, error: Some(error).filter(|e| !e.is_empty()) }
}

/// The reply to message `id` (which said `prompt`) as it was saved, with its stream's `token`:
/// finished, so Stop has nothing to end and Retry asks again. One that was interrupted shows as
/// stopped, with its text and the note saying so.
pub fn reply_from(saved: &SavedReply, id: u64, prompt: &str, token: u64) -> Reply {
    let mut reply = Reply::new(id, prompt);
    reply.token = token;
    if let Some(thought) = saved.thinking.as_ref().filter(|t| !t.is_empty()) {
        reply.parts.push(Part::Thinking(Thinking { text: thought.clone(), started: Instant::now(), took: Some(Duration::ZERO), open: false }));
    }
    if !saved.text.trim().is_empty() {
        reply.parts.push(Part::Text(saved.text.clone()));
    }
    if let Some(error) = saved.error.as_ref().filter(|e| !e.is_empty()) {
        reply.parts.push(Part::Error(error.clone()));
    }
    reply.state = match saved.state {
        ReplyState::Done => State::Done,
        ReplyState::Failed => State::Failed,
        ReplyState::Stopped => State::Stopped,
        ReplyState::Interrupted => {
            reply.parts.push(Part::Error(INTERRUPTED.into()));
            State::Stopped
        }
    };
    reply
}
