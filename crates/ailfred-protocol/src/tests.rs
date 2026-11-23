//! The fixtures every client replays: scripted runs encoded to `fixtures/*.jsonl`, and the
//! checks that they stay what the code makes, read back the same, rebuild the same reply, and
//! keep to the official schema (`conformance`).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ailfred_core::reply::{Event, Part, Reply, Tool};
use serde_json::{Value, json};

use crate::agui::{self, PROTOCOL_VERSION, RunAgentInput, to_line};
use crate::command::{self, Command};
use crate::encode::Encoder;
use crate::replay::{Kind, Replayer, parse_line};
use crate::snapshot::{Turn, history, messages_snapshot};

mod conformance;
mod saved;
mod snapshot;

const THREAD: &str = "thread-1";

/// When the scripts start: 2026-01-01, in ms since the epoch.
const T0: u64 = 1_767_225_600_000;

/// A 1×1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f, 0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

/// One scripted run: its fixture's name, its run id, and what came when (ms after `T0`).
struct Script {
    name: &'static str,
    run: &'static str,
    events: Vec<(u64, Kind)>,
}

fn scripts() -> Vec<Script> {
    let ev = Kind::Event;
    let text = |s: &str| ev(Event::Text(s.into()));
    let step = |id, tool, target: &str| ev(Event::Step { id, tool, target: target.into() });
    let output = |id, text: &str| ev(Event::StepOutput { id, text: text.into() });
    let script = |name, run, events| Script { name, run, events };
    vec![
        script(
            "answer",
            "run-answer",
            vec![
                (0, ev(Event::Think(String::new()))),
                (100, ev(Event::Think("The user wants ".into()))),
                (150, ev(Event::Think("a greeting.".into()))),
                (400, text("Hello")),
                (450, text(", ")),
                (500, text("**world**.")),
                (600, ev(Event::Done)),
            ],
        ),
        script(
            "steps",
            "run-steps",
            vec![
                (0, text("Let me look.\n")),
                (100, step(1, Tool::Read, "src/main.rs")),
                (150, output(1, "fn main() {\n")),
                (180, output(1, "}\n")),
                (200, ev(Event::StepEnd { id: 1, ok: true })),
                (300, step(2, Tool::Run, "cargo test")),
                (350, output(2, "error[E0425]: cannot find value `x`\n")),
                (400, ev(Event::StepEnd { id: 2, ok: false })),
                (500, text("The test fails; searching for it.")),
                (600, step(3, Tool::Search, "fn main")),
                (700, ev(Event::Done)),
            ],
        ),
        script(
            "diff",
            "run-diff",
            vec![
                (0, text("Here's the fix:")),
                (100, ev(Event::Diff { path: "src/lib.rs".into(), patch: "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-let x = 1;\n+let x = 2;\n".into() })),
                (200, ev(Event::Done)),
            ],
        ),
        script(
            "question",
            "run-question",
            vec![
                (0, text("I can change a few files.")),
                (100, ev(Event::Ask { prompt: "Which ones?".into(), options: vec!["src/a.rs".into(), "src/b.rs".into(), "src/c.rs".into()], multi: true })),
                (100, ev(Event::Done)),
            ],
        ),
        script("stop", "run-stop", vec![(0, text("Building it.")), (100, step(1, Tool::Run, "cargo build")), (200, output(1, "Compiling ailfred\n")), (300, Kind::Stop)]),
        script("error", "run-error", vec![(0, text("Let me")), (100, ev(Event::Fail("overloaded".into())))]),
        script("lost", "run-lost", vec![(0, text("Half an ans")), (100, ev(Event::Lost("pi exited (code 1)".into())))]),
        script("steered", "run-steered", vec![(0, ev(Event::Taken)), (50, text("Got it, ")), (80, text("switching.")), (200, ev(Event::Done))]),
    ]
}

fn encoded(s: &Script) -> Vec<agui::Event> {
    let mut encoder = Encoder::new(THREAD, s.run);
    let mut out = Vec::new();
    for (at, kind) in &s.events {
        out.extend(match kind {
            Kind::Event(e) => encoder.encode(e, T0 + at),
            Kind::Stop => encoder.stop(T0 + at),
        });
    }
    out
}

/// The `question` fixture's run, what the user said to it, and its reply as the core has it.
const QUESTION_RUN: &str = "run-question";
const QUESTION_TEXT: &str = "Tidy up the error handling.";

fn question_reply() -> Reply {
    direct(scripts().iter().find(|s| s.name == "question").expect("the question script"))
}

fn commands() -> Vec<Command> {
    // The `question` fixture's conversation: what the user asked, and the reply with the question.
    let reply = question_reply();
    let asked = [Turn { run: QUESTION_RUN, text: QUESTION_TEXT, pngs: &[], reply: Some(&reply) }];
    vec![
        command::send(THREAD, "run-image", &[], "What's in this screenshot?", &[PNG]),
        command::answer(THREAD, "run-answered", &asked, "run-question-ask-1", "src/a.rs, src/c.rs"),
        Command::Stop { thread_id: THREAD.into(), run_id: "run-stop".into() },
    ]
}

fn jsonl(lines: impl IntoIterator<Item = String>) -> String {
    lines.into_iter().map(|l| l + "\n").collect()
}

/// Every fixture file's name and what the code makes for it.
fn made() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = scripts().iter().map(|s| (format!("{}.jsonl", s.name), jsonl(encoded(s).iter().map(to_line)))).collect();
    files.push(("commands.jsonl".into(), jsonl(commands().iter().map(|c| serde_json::to_string(c).unwrap()))));
    files
}

fn crate_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn updating() -> bool {
    std::env::var("UPDATE_FIXTURES").is_ok_and(|v| v == "1")
}

/// The lines of fixture `file`: what's checked in, or, while they're rewritten, what the code
/// makes (so no test reads a file half written).
fn fixture(file: &str) -> Vec<String> {
    let content = if updating() {
        made().into_iter().find(|(f, _)| f == file).expect("a fixture").1
    } else {
        std::fs::read_to_string(crate_path(&format!("fixtures/{file}"))).unwrap_or_else(|e| panic!("fixtures/{file}: {e}"))
    };
    content.lines().map(str::to_owned).collect()
}

/// Checked-in `rel` equals `made`, or becomes it with `UPDATE_FIXTURES=1`.
fn golden(rel: &str, made: &str) {
    let path = crate_path(rel);
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, made).unwrap();
        return;
    }
    let held = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(held == made, "{rel} differs from what the code makes; rewrite it with UPDATE_FIXTURES=1 if that's intended\n--- held\n{held}\n--- made\n{made}");
}

#[test]
fn each_fixture_is_what_its_script_encodes_to() {
    for (file, content) in made() {
        golden(&format!("fixtures/{file}"), &content);
    }
}

#[test]
fn every_fixture_line_reads_back_into_our_types_and_writes_the_same_line() {
    for s in scripts() {
        for line in fixture(&format!("{}.jsonl", s.name)) {
            let event = parse_line(&line).unwrap_or_else(|e| panic!("{}: {e}: {line}", s.name)).expect("a known event");
            assert_eq!(to_line(&event), line);
        }
    }
    for line in fixture("commands.jsonl") {
        let command: Command = serde_json::from_str(&line).unwrap();
        assert_eq!(serde_json::to_string(&command).unwrap(), line);
    }
}

fn instant(at: u64) -> Instant {
    static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *BASE.get_or_init(Instant::now) + Duration::from_millis(at.saturating_sub(T0))
}

fn apply(reply: &mut Reply, at: u64, kind: Kind) {
    match kind {
        Kind::Event(e) => reply.apply(e, instant(at)),
        Kind::Stop => reply.stop(instant(at)),
    }
}

/// What a reply shows, in words, to compare two (a `Reply` has no `PartialEq`).
fn digest(r: &Reply) -> String {
    let mut out = format!("{:?} interrupted={}\n", r.state, r.interrupted);
    for part in &r.parts {
        let line = match part {
            Part::Thinking(t) => format!("thinking {:?} took {:?}", t.text, t.took),
            Part::Text(t) => format!("text {t:?}"),
            Part::Agent(a) => a.steps.iter().map(|s| format!("step {:?} {:?} {:?} {:?}", s.tool, s.target, s.output, s.status)).collect::<Vec<_>>().join("\n"),
            Part::Diff(d) => format!("diff {:?} {:?}", d.path, d.patch),
            Part::Question(q) => format!("question {:?} {:?} multi={} answered={:?}", q.prompt, q.options, q.multi, q.answered),
            Part::Error(e) => format!("error {e:?}"),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// The reply script `s` builds, applied straight.
fn direct(s: &Script) -> Reply {
    let mut reply = Reply::new(1, "q");
    for (at, kind) in s.events.clone() {
        apply(&mut reply, T0 + at, kind);
    }
    reply
}

/// The reply a client builds from `lines` of AG-UI events.
fn replayed(lines: &[String]) -> Reply {
    let mut reply = Reply::new(1, "q");
    let mut replayer = Replayer::new();
    for line in lines {
        let event = parse_line(line).unwrap().expect("a known event");
        for update in replayer.replay(&event).unwrap_or_else(|e| panic!("{e}: {line}")) {
            apply(&mut reply, update.at.unwrap_or(T0), update.kind);
        }
    }
    reply
}

/// `events` applied straight and replayed from their encoding build the same reply.
fn assert_round_trips(events: Vec<(u64, Kind)>) {
    let s = Script { name: "scratch", run: "run-x", events };
    let lines: Vec<String> = encoded(&s).iter().map(to_line).collect();
    assert_eq!(digest(&replayed(&lines)), digest(&direct(&s)), "{}", lines.join("\n"));
}

#[test]
fn replaying_a_fixture_builds_the_same_reply_as_its_script() {
    for s in scripts() {
        let replayed = replayed(&fixture(&format!("{}.jsonl", s.name)));
        assert_eq!(digest(&replayed), digest(&direct(&s)), "{}", s.name);
    }
}

fn ask(prompt: &str) -> Kind {
    Kind::Event(Event::Ask { prompt: prompt.into(), options: vec!["A".into(), "B".into()], multi: false })
}

#[test]
fn a_question_replays_where_it_was_asked_even_with_text_after_it() {
    let text = |s: &str| Kind::Event(Event::Text(s.into()));
    assert_round_trips(vec![(0, text("a")), (100, ask("Which?")), (200, text("b")), (300, Kind::Event(Event::Done))]);
}

#[test]
fn a_question_before_a_failure_still_shows_when_replayed() {
    assert_round_trips(vec![(0, ask("Which?")), (100, Kind::Event(Event::Fail("overloaded".into())))]);
    assert_round_trips(vec![(0, ask("Which?")), (100, Kind::Stop)]);
}

#[test]
fn thinking_ended_by_a_taken_replays_as_two_thinkings_with_their_times() {
    let think = |s: &str| Kind::Event(Event::Think(s.into()));
    assert_round_trips(vec![(0, think("a")), (100, Kind::Event(Event::Taken)), (250, think("b")), (400, Kind::Event(Event::Done))]);
    assert_round_trips(vec![(0, think("a")), (150, ask("Which?")), (200, think("b")), (450, Kind::Event(Event::Done))]);
}

/// The runs in `commands.jsonl`: the image run, then the answering run.
fn runs() -> Vec<RunAgentInput> {
    fixture("commands.jsonl")
        .iter()
        .filter_map(|line| match serde_json::from_str(line).unwrap() {
            Command::Run(input) => Some(*input),
            Command::Stop { .. } => None,
        })
        .collect()
}

#[test]
fn the_answering_run_carries_the_history_a_snapshot_of_its_conversation_holds() {
    let reply = question_reply();
    let turns = [Turn { run: QUESTION_RUN, text: QUESTION_TEXT, pngs: &[], reply: Some(&reply) }];
    let [image, answering] = runs().try_into().expect("two runs");
    let agui::Event::MessagesSnapshot { messages: snapshot, .. } = messages_snapshot(&turns, T0) else { panic!("a snapshot") };
    assert_eq!(answering.messages, history(&turns), "its complete history, under the snapshot's ids");
    assert_eq!(history(&turns), snapshot);
    let sent = Command::Run(Box::new(answering.clone()));
    let round: Command = serde_json::from_str(&serde_json::to_string(&sent).unwrap()).unwrap();
    assert_eq!(round, sent);
    assert_eq!(answering.protocol_version.as_deref(), Some(PROTOCOL_VERSION));
    // It answers the interrupt the question's run ended with.
    let finished = parse_line(fixture("question.jsonl").last().unwrap()).unwrap().unwrap();
    let agui::Event::RunFinished { outcome: Some(agui::RunOutcome::Interrupt { interrupts }), .. } = finished else { panic!("an interrupt") };
    let resume = answering.resume.as_deref().expect("its answer");
    assert_eq!(resume.iter().map(|r| r.interrupt_id.as_str()).collect::<Vec<_>>(), [interrupts[0].id.as_str()]);
    assert_eq!(sent, command::answer(THREAD, &answering.run_id, &turns, &interrupts[0].id, "src/a.rs, src/c.rs"));
    // A run with nothing before it holds only its own message.
    let ids: Vec<String> = image.messages.iter().map(|m| serde_json::to_value(m).unwrap()["id"].as_str().unwrap().to_owned()).collect();
    assert_eq!(ids, [format!("{}-user", image.run_id)]);
}

/// `schema` as the clients get it: pretty JSON, without the `null` schemars allows for an
/// `Option` (an absent field is left out, never sent as `null`), and with no keyword beside a
/// `$ref`.
fn schema_json(schema: schemars::Schema) -> String {
    let mut schema = schema.to_value();
    without_nulls(&mut schema);
    ref_alone(&mut schema);
    serde_json::to_string_pretty(&schema).unwrap() + "\n"
}

/// Moves the keywords beside a `$ref` into an `allOf` with it, as draft-07 code generators
/// (quicktype, json-schema-to-typescript) ignore a `$ref`'s siblings; a description or title
/// stays outside, where they read it.
fn ref_alone(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            map.values_mut().for_each(ref_alone);
            if map.len() > 1
                && let Some(target) = map.remove("$ref")
            {
                let mut siblings = std::mem::take(map);
                for key in ["description", "title"] {
                    if let Some(v) = siblings.remove(key) {
                        map.insert(key.into(), v);
                    }
                }
                let mut parts = vec![json!({ "$ref": target })];
                if let Some(Value::Array(more)) = siblings.remove("allOf") {
                    parts.extend(more);
                }
                if !siblings.is_empty() {
                    parts.push(Value::Object(siblings));
                }
                map.insert("allOf".into(), Value::Array(parts));
            }
        }
        Value::Array(items) => items.iter_mut().for_each(ref_alone),
        _ => {}
    }
}

fn without_nulls(schema: &mut Value) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::Array(types)) = map.get_mut("type") {
                types.retain(|t| t != "null");
                if let [only] = types.as_slice() {
                    let only = only.clone();
                    map.insert("type".into(), only);
                }
            }
            if let Some(Value::Array(any)) = map.get_mut("anyOf") {
                any.retain(|s| s.get("type").and_then(Value::as_str) != Some("null"));
                if let [Value::Object(only)] = any.as_slice() {
                    let only = only.clone();
                    map.remove("anyOf");
                    map.extend(only);
                }
            }
            map.values_mut().for_each(without_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(without_nulls),
        _ => {}
    }
}

#[test]
fn the_json_schemas_are_what_the_types_make() {
    golden("schema/event.schema.json", &schema_json(schemars::schema_for!(agui::Event)));
    golden("schema/command.schema.json", &schema_json(schemars::schema_for!(Command)));
}

/// Where in `schema` an object puts keywords beside a `$ref` (which draft-07 code generators
/// ignore).
fn ref_siblings(schema: &Value, path: &str, out: &mut Vec<String>) {
    match schema {
        Value::Object(map) => {
            if map.contains_key("$ref") && map.len() > 1 {
                out.push(path.to_owned());
            }
            map.iter().for_each(|(k, v)| ref_siblings(v, &format!("{path}/{k}"), out));
        }
        Value::Array(items) => items.iter().enumerate().for_each(|(i, v)| ref_siblings(v, &format!("{path}/{i}"), out)),
        _ => {}
    }
}

#[test]
fn no_exported_schema_puts_keywords_beside_a_ref() {
    let mut found = Vec::new();
    for (name, schema) in [("event", schema_json(schemars::schema_for!(agui::Event))), ("command", schema_json(schemars::schema_for!(Command)))] {
        ref_siblings(&serde_json::from_str(&schema).unwrap(), name, &mut found);
    }
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn the_event_types_a_line_is_known_by_are_the_ones_the_types_read() {
    let schema = schemars::schema_for!(agui::Event);
    let types: Vec<&str> = schema.as_value()["oneOf"].as_array().unwrap().iter().filter_map(|v| v["properties"]["type"]["const"].as_str()).collect();
    assert_eq!(types, agui::Event::TYPES);
}

#[test]
fn a_line_of_an_unknown_type_is_skipped_by_a_replaying_client() {
    let lines = [r#"{"type":"RUN_STARTED","threadId":"t","runId":"r"}"#, r#"{"type":"STATE_SNAPSHOT","snapshot":{}}"#, r#"{"type":"TEXT_MESSAGE_START","messageId":"m"}"#];
    let events: Vec<agui::Event> = lines.iter().filter_map(|l| parse_line(l).unwrap()).collect();
    let mut r = Replayer::new();
    let kinds: Vec<Kind> = events.iter().flat_map(|e| r.replay(e).unwrap()).map(|u| u.kind).collect();
    assert_eq!(kinds, [Kind::Event(Event::Taken), Kind::Event(Event::Text(String::new()))]);
    let custom = agui::Event::Custom { name: "someone:else".into(), value: Value::Null, base: agui::Base::default() };
    assert!(r.replay(&custom).unwrap().is_empty(), "custom events it doesn't know are ignored");
}
