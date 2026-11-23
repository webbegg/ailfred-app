//! A reply saved as its events and read back: every part kind and every end comes back the same,
//! and every event it's saved as keeps to the official schema.

use std::time::{Duration, Instant};

use ailfred_core::reply::{Event, Part, Reply, State, StepStatus, Tool};
use serde_json::Value;

use crate::conformance::Spec;
use super::digest;
use crate::agui::{self, to_line};
use crate::encode::STEP_OUTPUT;
use crate::replay::ReplayError;
use crate::saved::{SavedError, reply_events, reply_from_events};

const THREAD: &str = "20260101-000000-abcd";
const RUN: &str = "20260101-000000-abcd-0";

fn at(ms: u64) -> Instant {
    static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *BASE.get_or_init(Instant::now) + Duration::from_millis(ms)
}

/// The reply `events` build, 100 ms apart.
fn reply_of(events: Vec<Event>) -> Reply {
    let mut reply = Reply::new(4, "why?");
    for (i, event) in events.into_iter().enumerate() {
        reply.apply(event, at(i as u64 * 100));
    }
    reply
}

/// `reply` saved as events, each checked against the official schema.
fn saved(reply: &Reply) -> Vec<agui::Event> {
    let events = reply_events(reply, THREAD, RUN);
    let spec = Spec::load();
    let errors: Vec<String> = events.iter().flat_map(|e| spec.errors(&serde_json::from_str::<Value>(&to_line(e)).unwrap(), "Event")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    events
}

/// `reply` saved and read back.
fn back(reply: &Reply) -> Reply {
    let events = saved(reply);
    let back = reply_from_events(&events, reply.id, &reply.prompt, 9).unwrap_or_else(|e| panic!("{e}: {}", lines(&events)));
    assert_eq!((back.id, back.prompt.as_str(), back.token), (reply.id, reply.prompt.as_str(), 9));
    back
}

fn lines(events: &[agui::Event]) -> String {
    events.iter().map(to_line).collect::<Vec<_>>().join("\n")
}

fn step(id: u32, tool: Tool, target: &str) -> Event {
    Event::Step { id, tool, target: target.into() }
}

fn every_part() -> Vec<Event> {
    vec![
        Event::Think("Let me ".into()),
        Event::Think("see.".into()),
        Event::Taken,
        Event::Think("More.".into()),
        Event::Text("Looking.".into()),
        step(7, Tool::Read, "src/main.rs"),
        Event::StepOutput { id: 7, text: "fn main() {}\n".into() },
        step(8, Tool::Run, "cargo test"),
        Event::StepEnd { id: 7, ok: true },
        Event::StepOutput { id: 8, text: "error[E0425]\n".into() },
        Event::StepEnd { id: 8, ok: false },
        step(9, Tool::Other, "memory"),
        Event::StepEnd { id: 9, ok: true },
        Event::Text("Here's the fix:".into()),
        Event::Diff { path: "src/lib.rs".into(), patch: "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-let x = 1;\n+let x = 2;\n".into() },
        Event::Ask { prompt: "Which ones?".into(), options: vec!["a.rs".into(), "b.rs".into()], multi: true },
        Event::Ask { prompt: "Sure?".into(), options: Vec::new(), multi: false },
    ]
}

#[test]
fn every_part_kind_comes_back_from_its_saved_events() {
    let mut events = every_part();
    events.push(Event::Done);
    let reply = reply_of(events);
    let [Part::Thinking(first), Part::Thinking(second), ..] = reply.parts.as_slice() else { panic!("two thinkings first") };
    assert_eq!((first.took, second.took), (Some(Duration::from_millis(200)), Some(Duration::from_millis(100))));
    assert_eq!(digest(&back(&reply)), digest(&reply));
}

#[test]
fn every_end_comes_back_the_same() {
    // Done, failed (a step still running fails), interrupted (the model went away).
    let ends = [Event::Done, Event::Fail("overloaded".into()), Event::Lost("pi exited (code 1)".into())];
    for end in ends {
        let mut events = every_part();
        events.push(step(10, Tool::Run, "cargo build"));
        events.push(end.clone());
        let reply = reply_of(events);
        let back = back(&reply);
        assert_eq!(digest(&back), digest(&reply), "{end:?}");
        assert!(!back.streaming());
    }
    // Stopped by the user, thinking open and a step running.
    let mut reply = reply_of(vec![Event::Text("Building.".into()), step(1, Tool::Run, "cargo build"), Event::Think("hm".into())]);
    reply.stop(at(500));
    assert!(matches!(reply.parts.last(), Some(Part::Thinking(t)) if t.took == Some(Duration::from_millis(300))));
    assert_eq!(digest(&back(&reply)), digest(&reply));
    assert!(!back(&reply).interrupted);
}

#[test]
fn a_reply_saved_mid_stream_or_still_queued_comes_back_interrupted() {
    let streaming = reply_of(vec![Event::Think("hm".into()), Event::Text("Half an ".into()), step(1, Tool::Run, "ls")]);
    assert!(streaming.streaming());
    let back_streaming = back(&streaming);
    assert_eq!((back_streaming.state, back_streaming.interrupted), (State::Stopped, true));
    let [Part::Thinking(t), Part::Text(text), Part::Agent(a)] = back_streaming.parts.as_slice() else { panic!("thinking, text, agent") };
    assert_eq!((t.text.as_str(), text.as_str(), a.steps[0].status), ("hm", "Half an ", StepStatus::Failed));
    let mut queued = Reply::new(5, "next");
    queued.state = State::Queued;
    let back_queued = back(&queued);
    assert_eq!((back_queued.state, back_queued.interrupted, back_queued.parts.len()), (State::Stopped, true, 0));
}

#[test]
fn a_failure_without_a_message_comes_back_failed_with_no_empty_error() {
    let mut reply = reply_of(vec![Event::Text("a".into())]);
    reply.state = State::Failed;
    let back = back(&reply);
    assert_eq!(back.state, State::Failed);
    assert_eq!(digest(&back), digest(&reply));
}

#[test]
fn steps_are_saved_numbered_in_the_order_they_came_whatever_the_engine_called_them() {
    let reply = reply_of(vec![step(42, Tool::Read, "a"), step(42, Tool::Read, "b"), Event::Done]);
    let calls: Vec<String> = saved(&reply).iter().filter_map(|e| if let agui::Event::ToolCallStart { tool_call_id, .. } = e { Some(tool_call_id.clone()) } else { None }).collect();
    assert_eq!(calls, [format!("{RUN}-step-1"), format!("{RUN}-step-2")]);
    assert_eq!(digest(&back(&reply)), digest(&reply));
}

fn has_step_output(events: &[agui::Event]) -> bool {
    events.iter().any(|e| matches!(e, agui::Event::Custom { name, .. } if name == STEP_OUTPUT))
}

#[test]
fn a_long_step_output_is_saved_as_its_last_64_kib_marked_cut() {
    // 200 KiB of two-byte characters, the error at the end.
    let long = format!("{}error: boom\n", "é".repeat(100 * 1024));
    let reply = reply_of(vec![step(1, Tool::Run, "cargo test"), Event::StepOutput { id: 1, text: long.clone() }, Event::StepEnd { id: 1, ok: false }, Event::Done]);
    let events = saved(&reply);
    assert!(!has_step_output(&events), "the result carries the output");
    let saved: Vec<String> = events.iter().filter_map(|e| if let agui::Event::ToolCallResult { content, .. } = e { Some(crate::replay::text_of(content)) } else { None }).collect();
    let [saved] = saved.as_slice() else { panic!("one result") };
    assert!(saved.len() <= 64 * 1024 && saved.len() > 60 * 1024, "{} bytes", saved.len());
    let back = back(&reply);
    let [Part::Agent(a)] = back.parts.as_slice() else { panic!("one agent run") };
    let output = &a.steps[0].output;
    assert_eq!(output, saved);
    let tail = output.strip_prefix('…').expect("marked cut");
    assert!(long.ends_with(tail) && tail.ends_with("error: boom\n"), "its end");
    assert_eq!(a.steps[0].status, StepStatus::Failed);
}

#[test]
fn step_output_is_saved_in_the_results_only_and_a_running_steps_survives() {
    let mut events = every_part();
    events.push(Event::Done);
    let done = reply_of(events);
    assert!(!has_step_output(&saved(&done)));
    assert_eq!(digest(&back(&done)), digest(&done));
    // Saved mid-stream, a step still running.
    let streaming = reply_of(vec![step(1, Tool::Run, "cargo build"), Event::StepOutput { id: 1, text: "Compiling\n".into() }]);
    assert!(!has_step_output(&saved(&streaming)));
    let back = back(&streaming);
    let [Part::Agent(a)] = back.parts.as_slice() else { panic!("one agent run") };
    assert_eq!((a.steps[0].output.as_str(), a.steps[0].status), ("Compiling\n", StepStatus::Failed));
}

#[test]
fn a_failure_with_only_blank_words_comes_back_failed_with_no_error_part() {
    let reply = reply_of(vec![Event::Text("a".into()), Event::Fail("   ".into())]);
    let back = back(&reply);
    assert_eq!(back.state, State::Failed);
    assert!(!back.parts.iter().any(|p| matches!(p, Part::Error(_))), "as the text path drops it");
}

#[test]
fn events_that_dont_make_a_whole_run_are_an_error() {
    let mut events = every_part();
    events.push(Event::Done);
    let whole = saved(&reply_of(events));
    let read = |events: &[agui::Event]| reply_from_events(events, 1, "q", 1).err();
    assert_eq!(read(&[]), Some(SavedError::NotEnded));
    assert_eq!(read(&whole[..whole.len() - 1]), Some(SavedError::NotEnded), "cut short");
    assert_eq!(read(&whole[1..]), Some(SavedError::Replay(ReplayError::NotStarted)));
    let mut twice = whole.clone();
    twice.push(whole[0].clone());
    assert_eq!(read(&twice), Some(SavedError::Replay(ReplayError::AfterEnd)));
    assert_eq!(read(&whole), None);
}
