//! A reply to what's saved of it, and back. A reply is saved twice over: as its run's AG-UI
//! events (`ailfred_protocol::saved`), whole, with the answers to its questions beside them;
//! and as its thinking, text and error, which every version reads. Loading prefers the events;
//! when there are none (a file from before them), or they don't replay into a finished reply,
//! the text fields make it, as they always did.

use std::time::{Duration, Instant};

use ailfred_protocol::saved;

use super::{INTERRUPTED, ReplyState, SavedReply};
use crate::reply::{Part, Reply, State, Thinking};

fn joined<'a>(texts: impl Iterator<Item = &'a str>) -> String {
    texts.filter(|t| !t.trim().is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// The run id the reply to message `index` of conversation `conversation` is saved under.
pub fn run_id(conversation: &str, index: usize) -> String {
    format!("{conversation}-{index}")
}

/// What to save of `reply`, as run `run` of thread `thread`: one still streaming, or queued, is
/// written `interrupted` (if the app quits now, that's what's left of it), and so is one
/// interrupted already.
pub fn saved_reply(reply: &Reply, thread: &str, run: &str) -> SavedReply {
    let state = match reply.state {
        State::Queued | State::Streaming => ReplyState::Interrupted,
        State::Stopped if reply.interrupted => ReplyState::Interrupted,
        State::Done => ReplyState::Done,
        State::Stopped => ReplyState::Stopped,
        State::Failed => ReplyState::Failed,
    };
    let thinking = joined(reply.parts.iter().filter_map(|p| if let Part::Thinking(t) = p { Some(t.text.as_str()) } else { None }));
    let text = joined(reply.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }));
    let error = joined(reply.parts.iter().filter_map(|p| if let Part::Error(e) = p { Some(e.as_str()) } else { None }));
    let events = if errors_fit(reply) { saved::reply_events(reply, thread, run) } else { Vec::new() };
    let answers = reply.parts.iter().filter_map(|p| if let Part::Question(q) = p { Some(q.answered.clone()) } else { None }).collect();
    SavedReply { state, thinking: Some(thinking).filter(|t| !t.is_empty()), text, error: Some(error).filter(|e| !e.is_empty()), events, answers }
}

/// Whether the events can carry its error: a run ends with it, so only a failed reply's last
/// part. (One loaded stopped with an error, from a damaged state, keeps to its text.)
fn errors_fit(reply: &Reply) -> bool {
    let errors = reply.parts.iter().filter(|p| matches!(p, Part::Error(_))).count();
    errors == 0 || (errors == 1 && reply.state == State::Failed && matches!(reply.parts.last(), Some(Part::Error(_))))
}

/// The reply to message `id` (which said `prompt`) as it was saved, with its stream's `token`:
/// finished, so Stop has nothing to end and Retry asks again. From its events if they replay,
/// its questions answered as they were; one without an answer is closed, as the model's process
/// that asked it is gone. Else from its text.
pub fn reply_from(saved: &SavedReply, id: u64, prompt: &str, token: u64) -> Reply {
    if !saved.events.is_empty() {
        match saved::reply_from_events(&saved.events, id, prompt, token) {
            Ok(mut reply) => {
                let questions = reply.parts.iter_mut().filter_map(|p| if let Part::Question(q) = p { Some(q) } else { None });
                for (i, q) in questions.enumerate() {
                    q.answered = saved.answers.get(i).cloned().flatten();
                    q.closed = q.answered.is_none();
                }
                return reply;
            }
            Err(e) => eprintln!("Ailfred: a saved reply's events don't replay ({e}); it shows its text"),
        }
    }
    reply_from_text(saved, id, prompt, token)
}

/// The same, from its thinking, text and error. One that was interrupted shows as stopped and
/// `interrupted`, with its text (an older save of one, stopped with the note `INTERRUPTED` as
/// its error, too).
fn reply_from_text(saved: &SavedReply, id: u64, prompt: &str, token: u64) -> Reply {
    let mut reply = Reply::new(id, prompt);
    reply.token = token;
    if let Some(thought) = saved.thinking.as_ref().filter(|t| !t.is_empty()) {
        reply.parts.push(Part::Thinking(Thinking { text: thought.clone(), started: Instant::now(), took: Some(Duration::ZERO), open: false }));
    }
    if !saved.text.trim().is_empty() {
        reply.parts.push(Part::Text(saved.text.clone()));
    }
    let noted = saved.state == ReplyState::Stopped && saved.error.as_deref() == Some(INTERRUPTED);
    if let Some(error) = saved.error.as_ref().filter(|e| !e.is_empty() && !noted) {
        reply.parts.push(Part::Error(error.clone()));
    }
    reply.state = match saved.state {
        ReplyState::Done => State::Done,
        ReplyState::Failed => State::Failed,
        ReplyState::Stopped | ReplyState::Interrupted => State::Stopped,
    };
    reply.interrupted = saved.state == ReplyState::Interrupted || noted;
    reply
}
