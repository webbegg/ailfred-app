//! A finished reply as one run's AG-UI events, and back: how a saved conversation keeps its
//! replies whole (thinking and how long it took, text, agent steps, diffs, questions, how it
//! ended), in the format a client replays.
//!
//! [`reply_events`] turns the reply's parts back into the core [`Event`]s that built it, in
//! order, and feeds them to an [`Encoder`]; [`reply_from_events`] replays them through a
//! [`Replayer`]. Times are kept as timestamps from 0: a thinking ends `took` after it started
//! (to the millisecond), everything else comes at the time it's at. What isn't in the events
//! stays out: what the user answered or picked (the caller keeps the answers beside them),
//! what's open or applied, the engine's step ids (steps are numbered 1, 2, … in order).
//!
//! A step's output is kept in its `TOOL_CALL_RESULT` only (no `ailfred:step_output` events: the
//! run's end gives a step still running its result too), and only its last [`OUTPUT_MAX`]
//! bytes, after a leading "…": the end is where the errors are.

use std::time::{Duration, Instant};

use ailfred_core::reply::{Event, Part, Reply, State, StepStatus};

use crate::agui;
use crate::encode::{Encoder, STEP_OUTPUT};
use crate::replay::{Kind, ReplayError, Replayer};

/// Why a reply that ended unfinished (Ailfred quit, the model's process ended) is `Lost`.
pub const INTERRUPTED: &str = "The reply was interrupted before it finished.";

/// The most of a step's output that's saved, in bytes.
pub const OUTPUT_MAX: usize = 64 * 1024;

/// `output`, or its last [`OUTPUT_MAX`] bytes (the marker "…" in them) cut at a character.
fn tail(output: &str) -> String {
    const CUT: &str = "…";
    if output.len() <= OUTPUT_MAX {
        return output.to_owned();
    }
    let mut start = output.len() - (OUTPUT_MAX - CUT.len());
    while !output.is_char_boundary(start) {
        start += 1;
    }
    format!("{CUT}{}", &output[start..])
}

/// Why saved events don't make a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavedError {
    /// They break the run's lifecycle.
    Replay(ReplayError),
    /// The run never ends in them (none at all, or cut short).
    NotEnded,
}

impl std::fmt::Display for SavedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SavedError::Replay(e) => e.fmt(f),
            SavedError::NotEnded => write!(f, "the run doesn't end"),
        }
    }
}

impl std::error::Error for SavedError {}

impl From<ReplayError> for SavedError {
    fn from(e: ReplayError) -> Self {
        SavedError::Replay(e)
    }
}

/// `reply` as run `run_id` of thread `thread_id`, from its start to its end: done; failed (with
/// its error part's text, the last part a failed reply has); stopped by the user; or, stopped
/// `interrupted`, streaming or queued (saved mid-reply), lost, so it reads back interrupted.
pub fn reply_events(reply: &Reply, thread_id: &str, run_id: &str) -> Vec<agui::Event> {
    let mut encoder = Encoder::new(thread_id, run_id);
    let mut out = Vec::new();
    let mut at = 0u64;
    let mut steps = 0u32;
    // The results carry the steps' output.
    let mut put = |event: Event, at: u64| out.extend(encoder.encode(&event, at).into_iter().filter(|e| !matches!(e, agui::Event::Custom { name, .. } if name == STEP_OUTPUT)));
    for part in &reply.parts {
        match part {
            Part::Thinking(t) => {
                put(Event::Think(t.text.clone()), at);
                if let Some(took) = t.took {
                    // It ends then, whatever comes next (another thinking, too).
                    at += took.as_millis() as u64;
                    put(Event::Taken, at);
                }
            }
            Part::Text(text) => put(Event::Text(text.clone()), at),
            Part::Agent(agent) => {
                for step in &agent.steps {
                    steps += 1;
                    put(Event::Step { id: steps, tool: step.tool, target: step.target.clone() }, at);
                    if !step.output.is_empty() {
                        put(Event::StepOutput { id: steps, text: tail(&step.output) }, at);
                    }
                    match step.status {
                        StepStatus::Running => {}
                        StepStatus::Done => put(Event::StepEnd { id: steps, ok: true }, at),
                        StepStatus::Failed => put(Event::StepEnd { id: steps, ok: false }, at),
                    }
                }
            }
            Part::Diff(d) => put(Event::Diff { path: d.path.clone(), patch: d.patch.clone() }, at),
            Part::Question(q) => put(Event::Ask { prompt: q.prompt.clone(), options: q.options.clone(), multi: q.multi }, at),
            // The run's end says it.
            Part::Error(_) => {}
        }
    }
    let error = reply.parts.iter().rev().find_map(|p| if let Part::Error(e) = p { Some(e.clone()) } else { None });
    match reply.state {
        State::Done => put(Event::Done, at),
        State::Failed => put(Event::Fail(error.unwrap_or_default()), at),
        State::Stopped if !reply.interrupted => out.extend(encoder.stop(at)),
        State::Stopped | State::Streaming | State::Queued => put(Event::Lost(INTERRUPTED.into()), at),
    }
    out
}

/// The reply to message `id` (which said `prompt`) that `events` replay to, with its stream's
/// `token`; an error if they break the run's lifecycle or never end it. A failure without a
/// message (or only blanks) shows no error part.
pub fn reply_from_events(events: &[agui::Event], id: u64, prompt: &str, token: u64) -> Result<Reply, SavedError> {
    let mut reply = Reply::new(id, prompt);
    reply.token = token;
    let mut replayer = Replayer::new();
    let base = Instant::now();
    let mut at = events.first().and_then(|e| e.base().timestamp).unwrap_or(0);
    let start = at;
    for event in events {
        for update in replayer.replay(event)? {
            at = update.at.unwrap_or(at);
            let now = base + Duration::from_millis(at.saturating_sub(start));
            match update.kind {
                Kind::Event(e) => reply.apply(e, now),
                Kind::Stop => reply.stop(now),
            }
        }
    }
    if reply.streaming() {
        return Err(SavedError::NotEnded);
    }
    reply.parts.retain(|p| !matches!(p, Part::Error(e) if e.trim().is_empty()));
    Ok(reply)
}
