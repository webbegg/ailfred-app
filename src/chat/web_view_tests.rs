//! The chat and the web view: the hub fed while it's open, with the server off, and its
//! commands answered as `POST /v1/commands` answers them.

use std::sync::Mutex;
use std::sync::mpsc::SyncSender;
use std::time::Instant;

use ailfred_protocol::command::{self, Command};
use ailfred_protocol::replay::{Kind, Replayer, parse_value};
use serde_json::Value;

use super::tests::sent;
use super::*;
use crate::reply::Part;
use crate::server::tests::{PNG, got, local, watcher};
use crate::server::{Answer, COMMANDS, Core, Feed, Feeding, Frame, Gate, Head, Outcome, serve};
use crate::web_view::{self, Message};

fn on_claude() -> Chat {
    let mut c = Chat::new(None);
    c.model = Some(model::DEFAULT_MODEL.into());
    c
}

fn types(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["type"].as_str().unwrap_or_default()).collect()
}

#[test]
fn with_the_server_off_an_open_web_view_keeps_the_hub_fed_and_joining_mid_reply_gets_the_snapshot_then_the_run() {
    let mut feeding = Feeding::default();
    let mut c = on_claude();
    let first = sent(&mut c, "a");
    let token = c.ask(first).unwrap();
    c.reply_event(token, Event::Text("A".into()), Instant::now());
    c.reply_event(token, Event::Done, Instant::now());
    assert!(feeding.need(false).is_none(), "nothing needs it yet");
    // The web view opens; the server stays off.
    c.set_feed(feeding.need(true).expect("the chat is told"));
    assert!(feeding.need(true).is_none(), "once");
    assert!(feeding.fed().is_some());
    let second = sent(&mut c, "b");
    let token = c.ask(second).unwrap();
    c.reply_event(token, Event::Text("B".into()), Instant::now());
    let (sink, web) = local();
    c.join_local(sink);
    let events = got(&web);
    assert_eq!(types(&events), ["MESSAGES_SNAPSHOT", "RUN_STARTED", "TEXT_MESSAGE_START", "TEXT_MESSAGE_CONTENT"]);
    let snapshot = &events[0]["messages"];
    assert_eq!(snapshot.as_array().map(Vec::len), Some(2), "the first message and its reply, not the one going: {snapshot}");
    assert_eq!(events[1]["input"]["messages"][0]["content"].as_str(), Some("b"), "the run going, from its start");
    c.reply_event(token, Event::Done, Instant::now());
    assert_eq!(types(&got(&web)), ["TEXT_MESSAGE_END", "RUN_FINISHED"], "then live");
    // A reload joins again: a fresh snapshot, with both replies now.
    let (sink, again) = local();
    c.join_local(sink);
    let events = got(&again);
    assert_eq!(types(&events), ["MESSAGES_SNAPSHOT"]);
    assert_eq!(events[0]["messages"].as_array().map(Vec::len), Some(4));
}

/// `events` replayed as a client would, from the run's `RUN_STARTED` on: its lifecycle holds,
/// and the reply it rebuilds says `text`, ended.
fn replays_to(events: &[Value], text: &str) {
    let (mut replayer, mut rebuilt, now) = (Replayer::new(), Reply::new(0, ""), Instant::now());
    for e in events {
        let event = parse_value(e).unwrap().unwrap();
        for update in replayer.replay(&event).unwrap_or_else(|err| panic!("{e}: {err}")) {
            match update.kind {
                Kind::Event(e) => rebuilt.apply(e, now),
                Kind::Stop => rebuilt.stop(now),
            }
        }
    }
    assert!(!rebuilt.streaming(), "it ended");
    let said: String = rebuilt.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect();
    assert_eq!(said, text);
}

#[test]
fn opening_the_web_view_mid_reply_gives_the_page_that_run_from_its_start_then_live_to_its_end() {
    let mut feeding = Feeding::default();
    let mut c = on_claude();
    let id = sent(&mut c, "a");
    let token = c.ask(id).unwrap();
    c.reply_event(token, Event::Text("A".into()), Instant::now());
    // The web view opens mid-reply, the server off: a fresh hub.
    c.set_feed(feeding.need(true).unwrap());
    let (sink, web) = local();
    c.join_local(sink);
    c.reply_event(token, Event::Text("B".into()), Instant::now());
    c.reply_event(token, Event::Text("C".into()), Instant::now());
    c.reply_event(token, Event::Done, Instant::now());
    let events = got(&web);
    assert_eq!(
        types(&events),
        ["MESSAGES_SNAPSHOT", "RUN_STARTED", "TEXT_MESSAGE_START", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_END", "RUN_FINISHED"]
    );
    assert_eq!(events[0]["messages"].as_array().map(Vec::len), Some(0), "without the run going: {}", events[0]);
    assert_eq!(events[1]["input"]["messages"][0]["content"].as_str(), Some("a"), "the run from its start");
    replays_to(&events[1..], "ABC");

    // Closed and opened again mid-reply: the same, and Stop works.
    let id = sent(&mut c, "b");
    let token = c.ask(id).unwrap();
    c.reply_event(token, Event::Text("X".into()), Instant::now());
    c.leave_local();
    c.set_feed(feeding.need(false).unwrap());
    c.reply_event(token, Event::Text("Y".into()), Instant::now());
    c.set_feed(feeding.need(true).unwrap());
    let (sink, web) = local();
    c.join_local(sink);
    c.reply_event(token, Event::Text("Z".into()), Instant::now());
    let events = got(&web);
    assert_eq!(types(&events), ["MESSAGES_SNAPSHOT", "RUN_STARTED", "TEXT_MESSAGE_START", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_CONTENT"]);
    assert_eq!(events[0]["messages"].as_array().map(Vec::len), Some(2), "the first message and its reply only");
    assert_eq!(events[2..].iter().filter_map(|e| e["delta"].as_str()).collect::<String>(), "XYZ");
    let run = events[1]["runId"].as_str().unwrap().to_owned();
    let thread = c.conv.id.clone();
    assert_eq!(c.command(Command::Stop { thread_id: thread, run_id: run }, Instant::now()).status, 202, "Stop finds it");
    let mut all = events[1..].to_vec();
    all.extend(got(&web));
    assert_eq!(all.last().unwrap()["type"], "RUN_FINISHED", "{all:?}");
    replays_to(&all, "XYZ");
}

#[test]
fn a_server_starting_mid_reply_gives_a_client_that_run_from_its_start_then_live_to_its_end() {
    let mut feeding = Feeding::default();
    let mut c = on_claude();
    let id = sent(&mut c, "a");
    let token = c.ask(id).unwrap();
    c.reply_event(token, Event::Text("A".into()), Instant::now());
    // The server starts on the hub, then the chat is told.
    let _server = feeding.feed();
    c.set_feed(feeding.need(true).unwrap());
    let (tx, client) = watcher();
    c.join(tx);
    c.reply_event(token, Event::Text("B".into()), Instant::now());
    c.reply_event(token, Event::Done, Instant::now());
    let events = got(&client);
    assert_eq!(types(&events), ["MESSAGES_SNAPSHOT", "RUN_STARTED", "TEXT_MESSAGE_START", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_END", "RUN_FINISHED"]);
    replays_to(&events[1..], "AB");
}

#[test]
fn closing_the_web_view_with_the_server_off_stops_the_feed() {
    let mut feeding = Feeding::default();
    let mut c = on_claude();
    c.set_feed(feeding.need(true).unwrap());
    let (sink, web) = local();
    c.join_local(sink);
    assert_eq!(types(&got(&web)), ["MESSAGES_SNAPSHOT"]);
    c.leave_local();
    c.set_feed(feeding.need(false).expect("the chat is told"));
    assert!(c.feed.is_none() && feeding.fed().is_none());
    assert!(web.recv().is_err(), "its sink is gone");
    let id = sent(&mut c, "a");
    c.ask(id).unwrap();
    let thread = c.conv.id.clone();
    assert_eq!(c.command(command::send(&thread, "c1", &[], "hi", &[]), Instant::now()).status, 503, "fed nothing, as with the server off");
}

#[test]
fn the_web_view_closing_while_the_server_runs_keeps_the_feed_and_a_server_that_didnt_start_leaves_none() {
    let mut feeding = Feeding::default();
    let started = feeding.feed();
    assert!(feeding.need(false).is_none() && feeding.fed().is_none(), "the server didn't start: nobody was told");
    let mut c = on_claude();
    let server = feeding.feed();
    c.set_feed(feeding.need(true).unwrap());
    let (sink, web) = local();
    c.join_local(sink);
    c.leave_local();
    assert!(feeding.need(true).is_none(), "the server still runs: the chat keeps feeding");
    assert!(server.with(|hub| !hub.local_watching()));
    drop((started, web));
}

/// The chat behind a server, as the app's core: commands go to it as they come.
struct OnChat(Mutex<Chat>);

impl Core for OnChat {
    fn join(&self, watcher: SyncSender<Frame>) {
        self.0.lock().unwrap().join(watcher);
    }

    fn command(&self, command: Command) -> Answer {
        self.0.lock().unwrap().command(command, Instant::now())
    }
}

/// What `POST /v1/commands` answers `body`.
fn by_http(core: &OnChat, body: &str) -> Answer {
    let head = Head { method: "POST", path: COMMANDS, host: Some("127.0.0.1:4000"), origin: None, authorization: Some("Bearer t"), length: None };
    match serve(Gate::new(4000, "t".into()).admit(&head).unwrap(), &mut body.as_bytes(), core) {
        Outcome::Answer(answer) => answer,
        Outcome::Stream { .. } => panic!("a stream"),
    }
}

/// What the web view's bridge answers `body` posted as a command.
fn by_bridge(c: &mut Chat, body: &str) -> Answer {
    match web_view::parse(&format!(r#"{{"type":"command","id":"q1","command":{body}}}"#)) {
        Some(Message::Command { id, command }) => {
            assert_eq!(id, "q1");
            web_view::run(c, command, Instant::now())
        }
        other => panic!("{body}: {other:?}"),
    }
}

#[test]
fn the_web_views_commands_get_what_post_v1_commands_answers_for_the_same_body() {
    let mut web = on_claude();
    web.set_feed(Some(Feed::default()));
    let mut twin = on_claude();
    twin.conv = web.conv.clone();
    twin.set_feed(Some(Feed::default()));
    let http = OnChat(Mutex::new(twin));
    let thread = web.conv.id.clone();
    let json = |c: Command| serde_json::to_string(&c).unwrap();
    let broken = &PNG[..40];
    let cases = [
        (json(command::send(&thread, "c1", &[], "hi", &[])), 202),
        (json(command::send(&thread, "c1", &[], "again", &[])), 409),
        (json(command::send(&thread, "c2", &[], "look", &[broken])), 400),
        (json(command::send(&thread, "c3", &[], "look", &[PNG; 9])), 400),
        (r#"{"type":"jump"}"#.to_owned(), 400),
        (json(Command::Stop { thread_id: thread.clone(), run_id: "c2".into() }), 409),
        (json(Command::Stop { thread_id: thread.clone(), run_id: "c1".into() }), 202),
        (json(Command::Stop { thread_id: thread.clone(), run_id: "c1".into() }), 409),
    ];
    let same = |web: &mut Chat, cases: Vec<(String, u16)>| {
        for (body, status) in cases {
            let (bridged, served) = (by_bridge(web, &body), by_http(&http, &body));
            assert_eq!(bridged, served, "{body}");
            assert_eq!(bridged.status, status, "{body}: {bridged:?}");
        }
    };
    same(&mut web, cases.to_vec());
    {
        let twin = http.0.lock().unwrap();
        assert_eq!((web.messages.len(), twin.messages.len()), (1, 1), "the same message reached each chat");
        assert_eq!(web.replies[0].state, twin.replies[0].state, "and the same Stop");
    }
    // Retry the stopped reply, which then asks; decline its question.
    let ask = Event::Ask { prompt: "Which?".into(), options: vec!["A".into()], multi: false };
    let decline = |run: &str, interrupt: &str| json(command::decline(&thread, run, &[], interrupt));
    same(&mut web, vec![
        (json(command::retry(&thread, "c4", "nope")), 409),
        (json(command::retry(&thread, "c4", "c1")), 202),
        (json(command::retry(&thread, "c5", "c1")), 409),
        (json(command::retry(&thread, "c4", "c4")), 409),
        (r#"{"type":"retry","threadId":"t"}"#.to_owned(), 400),
        (decline("c5", "c4-ask-1"), 409),
    ]);
    for chat in [&mut web, &mut *http.0.lock().unwrap()] {
        let token = chat.replies[0].token;
        chat.reply_event(token, ask.clone(), Instant::now());
        chat.reply_event(token, Event::Done, Instant::now());
    }
    same(&mut web, vec![(decline("c5", "c4-ask-1"), 202), (decline("c6", "c4-ask-1"), 409), (json(command::new_conversation("other")), 409)]);
    // A new conversation: the same answer but for the fresh thread's id, each chat's own.
    let body = json(command::new_conversation(&thread));
    let (bridged, served) = (by_bridge(&mut web, &body), by_http(&http, &body));
    assert_eq!((bridged.status, served.status), (202, 202));
    let twin = http.0.into_inner().unwrap();
    assert_eq!(bridged.body, serde_json::json!({ "threadId": web.thread() }));
    assert_eq!(served.body, serde_json::json!({ "threadId": twin.thread() }));
    assert!(web.thread() != thread && twin.thread() != thread);
}
