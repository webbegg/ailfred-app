//! The fixtures against the official AG-UI 1.0 schema, with the checker in
//! [`crate::conformance`].

use serde_json::{Value, json};

use super::{THREAD, T0, PNG, fixture, scripts};
use crate::agui::{self, Message, PROTOCOL_VERSION, RunAgentInput};
use crate::command::user_message;
use crate::conformance::Spec;
use crate::encode::Encoder;
use ailfred_core::reply::Event;

#[test]
fn every_fixture_event_and_command_keeps_to_the_official_schema() {
    let spec = Spec::load();
    let mut errors = Vec::new();
    for s in scripts() {
        for (i, line) in fixture(&format!("{}.jsonl", s.name)).iter().enumerate() {
            let value: Value = serde_json::from_str(line).unwrap();
            errors.extend(spec.errors(&value, "Event").into_iter().map(|e| format!("{}.jsonl:{}: {e}", s.name, i + 1)));
        }
    }
    for (i, line) in fixture("commands.jsonl").iter().enumerate() {
        let Value::Object(mut command) = serde_json::from_str(line).unwrap() else { panic!("an object") };
        // Our envelope: a run is a RunAgentInput tagged `run`; a stop is ours alone.
        match command.remove("type").as_ref().and_then(Value::as_str) {
            Some("run") => errors.extend(spec.errors(&Value::Object(command), "RunAgentInput").into_iter().map(|e| format!("commands.jsonl:{}: {e}", i + 1))),
            Some("stop") => assert!(command.contains_key("threadId") && command.contains_key("runId"), "a stop names its run"),
            other => panic!("commands.jsonl:{}: type {other:?}", i + 1),
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn a_run_started_with_its_input_carries_the_user_message_and_keeps_to_the_schema() {
    let message: Message = user_message("r-user", "look", &[PNG]);
    let input = RunAgentInput {
        thread_id: THREAD.into(),
        run_id: "r".into(),
        protocol_version: Some(PROTOCOL_VERSION.into()),
        parent_run_id: None,
        state: None,
        messages: vec![message.clone()],
        tools: None,
        context: None,
        forwarded_props: None,
        resume: None,
    };
    let mut encoder = Encoder::new(THREAD, "r").with_input(input);
    let events = encoder.encode(&Event::Text("hi".into()), T0);
    let agui::Event::RunStarted { input: Some(input), .. } = &events[0] else { panic!("RUN_STARTED with its input first: {:?}", events[0]) };
    assert_eq!(input.messages, [message]);
    let spec = Spec::load();
    for e in &events {
        let value: Value = serde_json::from_str(&agui::to_line(e)).unwrap();
        assert_eq!(spec.errors(&value, "Event"), Vec::<String>::new(), "{value}");
    }
}

#[test]
fn the_checker_fails_a_misspelled_field_a_missing_one_and_a_wrong_const() {
    let spec = Spec::load();
    let check = |line: &str| spec.errors(&serde_json::from_str(line).unwrap(), "Event");
    assert!(check(r#"{"type":"TEXT_MESSAGE_CONTENT","messageId":"m","delta":"x","timestamp":1}"#).is_empty());
    assert_eq!(check(r#"{"type":"TEXT_MESSAGE_CONTENT","messageID":"m","delta":"x"}"#).len(), 2, "messageID undeclared, messageId missing");
    assert_eq!(check(r#"{"type":"REASONING_MESSAGE_START","messageId":"m","role":"assistant"}"#).len(), 1);
    let outcome = r#"{"type":"RUN_FINISHED","threadId":"t","runId":"r","outcome":{"type":"interrupt","interrupts":[{"id":"i","reason":"x","prompt":"?"}]}}"#;
    assert_eq!(check(outcome), ["Event.outcome.interrupts[0]: `prompt` is not a declared property"]);
    assert_eq!(check(r#"{"type":"THINKING_START"}"#).len(), 1, "removed in 1.0");
    let three = r#"{"type":"RUN_FINISHED","threadId":"t","runId":"r","result":null,"outcome":{"type":"interrupt","interrupts":[]},"timestamp":9007199254740993}"#;
    assert_eq!(check(three).len(), 3, "result null, no interrupts, timestamp too big: {:?}", check(three));
    assert_eq!(check(r#"{"type":"RUN_STARTED","threadId":1,"runId":"r"}"#).len(), 1, "threadId not a string");
}

#[test]
fn the_checker_keeps_the_keywords_beside_a_ref() {
    let spec = Spec { defs: json!({ "Name": { "type": "string" } }).as_object().unwrap().clone() };
    let schema = json!({ "$ref": "#/$defs/Name", "enum": ["a"], "description": "only a" });
    let errors = |value: Value| {
        let mut errors = Vec::new();
        spec.check(&value, &schema, "v", &mut errors);
        errors
    };
    assert!(errors(json!("a")).is_empty());
    assert_eq!(errors(json!("b")).len(), 1, "not in the enum beside the ref");
    assert_eq!(errors(json!(1)).len(), 2, "neither the ref's type nor in the enum");
}
