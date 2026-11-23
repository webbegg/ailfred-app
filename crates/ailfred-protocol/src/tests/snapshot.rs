//! A conversation as the `MESSAGES_SNAPSHOT` a joining client gets, and back.

use std::time::Instant;

use ailfred_core::reply::{Event, Part, Reply, State, Tool};
use serde_json::Value;

use super::conformance::Spec;
use super::{PNG, digest};
use crate::agui::{self, to_line};
use crate::saved::reply_events;
use crate::snapshot::{Turn, conversation_from_snapshot, messages_snapshot};

/// The reply `events` build, all at once (so thinking took no time: a snapshot keeps none).
fn reply_of(id: u64, prompt: &str, events: Vec<Event>) -> Reply {
    let now = Instant::now();
    let mut reply = Reply::new(id, prompt);
    for event in events {
        reply.apply(event, now);
    }
    reply
}

fn every_part(id: u64, prompt: &str) -> Reply {
    let step = |id, tool, target: &str| Event::Step { id, tool, target: target.into() };
    let ask = |prompt: &str, multi| Event::Ask { prompt: prompt.into(), options: vec!["a.rs".into(), "b.rs".into()], multi };
    let mut reply = reply_of(
        id,
        prompt,
        vec![
            Event::Think("Let me see.".into()),
            Event::Text("Looking.".into()),
            step(3, Tool::Read, "src/main.rs"),
            Event::StepOutput { id: 3, text: "fn main() {}\n".into() },
            Event::StepEnd { id: 3, ok: true },
            step(4, Tool::Run, "cargo test"),
            Event::StepOutput { id: 4, text: "error[E0425]\n".into() },
            Event::StepEnd { id: 4, ok: false },
            Event::Text("Here's the fix:".into()),
            Event::Diff { path: "src/lib.rs".into(), patch: "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-let x = 1;\n+let x = 2;\n".into() },
            step(5, Tool::Edit, "src/lib.rs"),
            Event::StepEnd { id: 5, ok: true },
            ask("Which ones?", true),
            ask("Sure?", false),
            Event::Done,
        ],
    );
    if let Some(Part::Question(q)) = reply.parts.iter_mut().find(|p| matches!(p, Part::Question(_))) {
        q.pick(1);
    }
    let first = reply.parts.iter().position(|p| matches!(p, Part::Question(_))).unwrap();
    reply.answer(first, "").expect("answered");
    reply
}

#[test]
fn a_conversation_with_every_part_kind_goes_through_a_messages_snapshot_and_back() {
    let first = every_part(1, "fix it");
    let failed = reply_of(2, "and the docs?", vec![Event::Text("Let me".into()), Event::Step { id: 1, tool: Tool::Search, target: "docs".into() }, Event::Fail("overloaded".into())]);
    let turns = [
        Turn { run: "c-0", text: "fix it", pngs: &[PNG, PNG], reply: Some(&first) },
        Turn { run: "c-1", text: "and the docs?", pngs: &[], reply: Some(&failed) },
        Turn { run: "c-2", text: "still there?", pngs: &[], reply: None },
    ];
    let event = messages_snapshot(&turns, 1_767_225_600_000);
    let errors = Spec::load().errors(&serde_json::from_str::<Value>(&to_line(&event)).unwrap(), "Event");
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let agui::Event::MessagesSnapshot { messages, .. } = event else { panic!("a snapshot") };

    let back = conversation_from_snapshot(&messages);
    assert_eq!(back.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["fix it", "and the docs?", "still there?"]);
    assert_eq!(back[0].pngs, [PNG.to_vec(), PNG.to_vec()], "the captures, decoded");
    assert!(back[1].pngs.is_empty());
    assert_eq!(digest(back[0].reply.as_ref().unwrap()), digest(&first));
    assert_eq!(digest(back[1].reply.as_ref().unwrap()), digest(&failed));
    assert_eq!(back[1].reply.as_ref().unwrap().state, State::Failed);
    assert!(back[2].reply.is_none());
    assert_eq!(back[0].reply.as_ref().unwrap().prompt, "fix it", "a reply answers the message before it");
}

#[test]
fn a_snapshots_ids_are_the_ones_its_runs_events_use() {
    let reply = every_part(1, "fix it");
    let agui::Event::MessagesSnapshot { messages, .. } = messages_snapshot(&[Turn { run: "c-0", text: "fix it", pngs: &[], reply: Some(&reply) }], 0) else { panic!("a snapshot") };
    let ids: Vec<String> = messages.iter().map(|m| serde_json::to_value(m).unwrap()["id"].as_str().unwrap().to_owned()).collect();
    assert_eq!(
        ids,
        [
            "c-0-user",
            "c-0-thinking-1",
            "c-0-text-1",
            "c-0-steps-1",
            "c-0-step-1-result",
            "c-0-step-2-result",
            "c-0-text-2",
            "c-0-diff-1",
            "c-0-steps-2",
            "c-0-step-3-result",
            "c-0-ask-1",
            "c-0-ask-2",
        ]
    );
    // The saved run's events name its messages, calls and questions alike.
    let lines: Vec<String> = reply_events(&reply, "c", "c-0").iter().map(to_line).collect();
    for id in ["c-0-thinking-1", "c-0-text-2", "c-0-step-3-result", "c-0-diff-1", "c-0-ask-2"] {
        assert!(lines.iter().any(|l| l.contains(&format!("\"{id}\""))), "{id} in\n{}", lines.join("\n"));
    }
    let agui::Message::Assistant { tool_calls: Some(calls), content: None, .. } = &messages[3] else { panic!("the steps' calls") };
    assert_eq!(serde_json::to_value(&calls[1]).unwrap(), serde_json::json!({ "id": "c-0-step-2", "type": "function", "function": { "name": "run", "arguments": "{\"target\":\"cargo test\"}" } }));
    let agui::Message::Tool { error, tool_call_id, .. } = &messages[5] else { panic!("a step's result") };
    assert_eq!((tool_call_id.as_str(), error.is_some()), ("c-0-step-2", true), "a failed step says so");
    let agui::Message::Activity { activity_type, content, .. } = &messages[10] else { panic!("a question") };
    assert_eq!((activity_type.as_str(), content.get("answered").and_then(Value::as_str)), ("ailfred:ask", Some("b.rs")));
}

#[test]
fn what_a_snapshot_doesnt_carry_comes_back_as_said_in_the_docs() {
    // Stopped or interrupted comes back done; a step still running stays running.
    let mut stopped = reply_of(1, "q", vec![Event::Text("Half".into())]);
    stopped.stop(Instant::now());
    let streaming = reply_of(2, "q", vec![Event::Step { id: 1, tool: Tool::Run, target: "ls".into() }]);
    let turns = [Turn { run: "r-0", text: "q", pngs: &[], reply: Some(&stopped) }, Turn { run: "r-1", text: "q", pngs: &[], reply: Some(&streaming) }];
    let agui::Event::MessagesSnapshot { messages, .. } = messages_snapshot(&turns, 0) else { panic!("a snapshot") };
    let back = conversation_from_snapshot(&messages);
    assert_eq!(back[0].reply.as_ref().unwrap().state, State::Done);
    let Some(Part::Agent(a)) = back[1].reply.as_ref().unwrap().parts.first() else { panic!("an agent run") };
    assert_eq!(a.steps[0].status, ailfred_core::reply::StepStatus::Running);
    // A message before any user message joins a turn of its own, with no text.
    let lone = [agui::Message::Assistant { id: "x".into(), content: Some("hi".into()), tool_calls: None, name: None }];
    let back = conversation_from_snapshot(&lone);
    assert_eq!((back.len(), back[0].text.as_str()), (1, ""));
    assert_eq!(back[0].reply.as_ref().unwrap().copy_text(), "hi");
}
