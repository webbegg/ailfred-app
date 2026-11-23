use std::sync::mpsc::Receiver;
use std::time::Instant;

use ailfred_protocol::agui::{self, Content, ContentPart, PartSource, ResumeStatus};
use ailfred_protocol::command::{self, Command};
use ailfred_protocol::replay::{Kind, Replayer, parse_value};
use ailfred_protocol::{saved, snapshot};
use ailfred_core::turn::base64;
use serde_json::Value;

use super::tests::sent;
use super::*;
use crate::reply::{Part, Tool};
use crate::server::tests::{PNG, got, tag, watcher};
use crate::server::{Feed, Frame};

/// A chat on Claude, its hub watched by a client that got the snapshot already.
fn watched() -> (Chat, Receiver<Frame>) {
    let mut c = Chat::new(None);
    c.model = Some(model::DEFAULT_MODEL.into());
    c.set_feed(Some(Feed::default()));
    let (tx, rx) = watcher();
    c.join(tx);
    assert_eq!(got(&rx).iter().map(tag).collect::<Vec<_>>(), ["MESSAGES_SNAPSHOT "]);
    (c, rx)
}

fn thread(c: &Chat) -> String {
    c.conv.id.clone()
}

fn send(c: &mut Chat, run: &str, text: &str, pngs: &[&[u8]]) -> crate::server::Answer {
    let thread = thread(c);
    c.command(command::send(&thread, run, &[], text, pngs), Instant::now())
}

/// The run reply stream `token` is on the stream.
fn run_of(c: &Chat, token: u64) -> String {
    c.feed.as_ref().unwrap().with(|hub| hub.run_of(token).map(str::to_owned)).expect("a run")
}

/// Message `text` typed here, its reply streamed: done with `events`; its token.
fn asked(c: &mut Chat, text: &str, events: Vec<Event>) -> u64 {
    let id = sent(c, text);
    let token = c.ask(id).unwrap();
    for e in events {
        c.reply_event(token, e, Instant::now());
    }
    token
}

fn last_token(c: &Chat) -> u64 {
    c.reply_to(c.messages.last().unwrap().id).unwrap().token
}

fn question(c: &Chat, token: u64, n: usize) -> &reply::Question {
    let r = c.replies.iter().find(|r| r.token == token).unwrap();
    r.parts.iter().filter_map(|p| if let Part::Question(q) = p { Some(q) } else { None }).nth(n).unwrap()
}

fn ask(prompt: &str) -> Event {
    Event::Ask { prompt: prompt.into(), options: vec!["A".into(), "B".into()], multi: true }
}

fn answer(c: &mut Chat, run: &str, interrupt: &str, text: &str) -> crate::server::Answer {
    let thread = thread(c);
    c.command(command::answer(&thread, run, &[], interrupt, text), Instant::now())
}

#[test]
fn a_run_with_text_and_an_image_reaches_the_chat_as_one_message_and_its_events_take_the_clients_run_id() {
    let (mut c, rx) = watched();
    let answer = send(&mut c, "c1", "look", &[PNG]);
    assert_eq!(answer, crate::server::Answer::accepted(&thread(&c), "c1"));
    let m = c.messages.last().unwrap();
    assert_eq!((c.messages.len(), m.text.as_str(), m.pngs.len()), (1, "look", 1));
    assert_eq!(m.pngs[0].as_slice(), PNG);
    let (_, request) = c.requests.last().unwrap();
    assert_eq!((request.text.as_str(), request.pngs.len(), request.model.as_str()), ("look", 1, model::DEFAULT_MODEL), "sent as if typed here");
    c.reply_event(last_token(&c), Event::Text("hi".into()), Instant::now());
    let events = got(&rx);
    assert_eq!((events[0]["type"].as_str(), events[0]["runId"].as_str()), (Some("RUN_STARTED"), Some("c1")));
    let content = &events[0]["input"]["messages"][0]["content"];
    assert_eq!(content[1]["source"]["value"].as_str(), Some(base64(PNG).as_str()), "the message, its capture too, comes before its run");
}

#[test]
fn window_runs_get_minted_ids_unique_in_the_thread() {
    let (mut c, _rx) = watched();
    let a = asked(&mut c, "a", vec![Event::Done]);
    let b = asked(&mut c, "b", vec![Event::Done]);
    let (ra, rb) = (run_of(&c, a), run_of(&c, b));
    assert!(ra != rb && ra.starts_with(&thread(&c)) && rb.starts_with(&thread(&c)), "{ra} {rb}");
    assert_eq!(send(&mut c, &ra, "again", &[]).status, 409, "a minted id is taken too");
}

#[test]
fn a_run_for_another_conversation_or_with_a_used_run_id_is_409() {
    let (mut c, _rx) = watched();
    let wrong = c.command(command::send("other", "c1", &[], "hi", &[]), Instant::now());
    assert_eq!(wrong.status, 409);
    assert!(wrong.body["error"].is_string());
    assert_eq!(send(&mut c, "c1", "hi", &[]).status, 202);
    c.reply_event(last_token(&c), Event::Done, Instant::now());
    assert_eq!(send(&mut c, "c1", "hi again", &[]).status, 409);
    assert_eq!(c.messages.len(), 1, "nothing sent");
}

#[test]
fn a_run_must_end_with_a_users_message_of_text_and_png_images() {
    let (mut c, _rx) = watched();
    let thread = thread(&c);
    let with = |content: Option<Content>| {
        let Command::Run(mut input) = command::send(&thread, "c1", &[], "hi", &[]) else { unreachable!() };
        input.messages = match content {
            Some(content) => vec![agui::Message::User { id: "u".into(), content, name: None }],
            None => vec![agui::Message::Assistant { id: "a".into(), content: Some("hi".into()), tool_calls: None, name: None }],
        };
        Command::Run(input)
    };
    let jpeg = Content::Parts(vec![ContentPart::Image { source: PartSource::Data { value: base64(b"jpeg"), mime_type: "image/jpeg".into() } }]);
    for command in [with(None), with(Some(jpeg)), with(Some(Content::Text(" ".into())))] {
        let answer = c.command(command, Instant::now());
        assert_eq!(answer.status, 400, "{answer:?}");
    }
    assert!(c.messages.is_empty());
}

#[test]
fn with_pi_a_clients_message_steers_the_run_going_and_comes_out_after_it() {
    let (mut c, rx) = watched();
    c.model = Some(model::PI.into());
    let a = asked(&mut c, "a", vec![Event::Text("A".into())]);
    assert_eq!(send(&mut c, "c2", "b", &[]).status, 202);
    assert!(c.requests.last().unwrap().1.steer, "a steer, as typed here");
    let b = last_token(&c);
    let now = Instant::now();
    c.reply_event(b, Event::Taken, now);
    c.reply_event(b, Event::Text("B".into()), now);
    c.reply_event(a, Event::Done, now);
    c.reply_event(b, Event::Done, now);
    let runs: Vec<String> = got(&rx).iter().filter(|e| e["type"] == "RUN_STARTED" || e["type"] == "RUN_FINISHED").map(|e| format!("{} {}", e["type"].as_str().unwrap(), e["runId"].as_str().unwrap())).collect();
    let ra = run_of(&c, a);
    assert_eq!(runs, [format!("RUN_STARTED {ra}"), format!("RUN_FINISHED {ra}"), "RUN_STARTED c2".into(), "RUN_FINISHED c2".into()]);
}

#[test]
fn a_resume_answers_the_open_question_as_the_window_does() {
    let (mut c, _rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    let run = run_of(&c, token);
    let taken = answer(&mut c, "c2", &format!("{run}-ask-1"), "A, B");
    assert_eq!(taken.status, 202, "{taken:?}");
    assert_eq!(question(&c, token, 0).answered.as_deref(), Some("A, B"), "several picks as one string");
    assert_eq!(c.messages.last().unwrap().text, "A, B", "the answer goes as the user's message");
    assert!(c.reply_to(c.messages.last().unwrap().id).unwrap().answer, "its reply answers, as the window's does");
    assert_eq!(answer(&mut c, "c3", &format!("{run}-ask-1"), "A").status, 409, "answered already");
}

#[test]
fn a_question_can_be_answered_by_its_snapshot_id_too() {
    let (mut c, _rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    let run = crate::history::run_id(&thread(&c), 0);
    assert_eq!(answer(&mut c, "c2", &format!("{run}-ask-1"), "B").status, 202);
    assert_eq!(question(&c, token, 0).answered.as_deref(), Some("B"));
}

#[test]
fn a_question_asked_while_the_server_was_off_is_answered_by_the_id_the_snapshot_gives_it() {
    let mut c = Chat::new(None);
    c.model = Some(model::DEFAULT_MODEL.into());
    let token = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    c.set_feed(Some(Feed::default()));
    let (tx, rx) = watcher();
    c.join(tx);
    let events = got(&rx);
    let asks = events[0]["messages"].as_array().unwrap().iter().find(|m| m["activityType"] == "ailfred:ask").unwrap();
    let interrupt = asks["content"]["interruptId"].as_str().unwrap().to_owned();
    let taken = answer(&mut c, "c1", &interrupt, "A");
    assert_eq!(taken.status, 202, "{interrupt}: {taken:?}");
    assert_eq!(question(&c, token, 0).answered.as_deref(), Some("A"));
}

#[test]
fn a_closed_question_a_wrong_interrupt_id_and_a_decline_are_409() {
    let (mut c, _rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    let run = run_of(&c, token);
    assert_eq!(answer(&mut c, "c1", &format!("{run}-ask-2"), "A").status, 409, "no such question");
    assert_eq!(answer(&mut c, "c2", "nope", "A").status, 409);
    let Command::Run(mut input) = command::answer(&thread(&c), "c3", &[], &format!("{run}-ask-1"), "A") else { unreachable!() };
    input.resume.as_mut().unwrap()[0].status = ResumeStatus::Cancelled;
    let declined = c.command(Command::Run(input), Instant::now());
    assert_eq!(declined.status, 409, "declining isn't supported yet");
    assert!(question(&c, token, 0).open(), "still open");
    if let Some(Part::Question(q)) = c.replies.iter_mut().find(|r| r.token == token).unwrap().parts.last_mut() {
        q.closed = true;
    }
    assert_eq!(answer(&mut c, "c4", &format!("{run}-ask-1"), "A").status, 409, "closed");
    let failed = asked(&mut c, "again", vec![ask("Which?"), Event::Fail("broke".into())]);
    let run = run_of(&c, failed);
    assert_eq!(answer(&mut c, "c5", &format!("{run}-ask-1"), "A").status, 409, "its run failed: nothing waits");
    assert_eq!(c.messages.len(), 2, "no answer was sent");
}

#[test]
fn a_partial_resume_answers_only_the_questions_it_gives() {
    let (mut c, _rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("One?"), ask("Two?"), Event::Done]);
    let run = run_of(&c, token);
    assert_eq!(answer(&mut c, "c1", &format!("{run}-ask-2"), "B").status, 202);
    assert!(question(&c, token, 0).open());
    assert_eq!(question(&c, token, 1).answered.as_deref(), Some("B"));
    assert_eq!(c.messages.last().unwrap().text, "B");
}

#[test]
fn stop_stops_the_run_it_names_and_only_that() {
    let (mut c, _rx) = watched();
    let token = asked(&mut c, "a", vec![Event::Text("A".into())]);
    let run = run_of(&c, token);
    let stop = |thread: &str, run: &str| Command::Stop { thread_id: thread.into(), run_id: run.into() };
    assert_eq!(c.command(stop("other", &run), Instant::now()).status, 409);
    assert_eq!(c.command(stop(&thread(&c), "nope"), Instant::now()).status, 409, "not the run going");
    assert!(c.cancelled.is_empty());
    assert_eq!(c.command(stop(&thread(&c), &run), Instant::now()).status, 202);
    assert_eq!(c.cancelled, [token], "as the window's Stop");
    assert_eq!(c.command(stop(&thread(&c), &run), Instant::now()).status, 409, "over already");
}

#[test]
fn stop_names_the_run_streaming_and_a_queued_run_is_409() {
    let (mut c, _rx) = watched();
    let a = asked(&mut c, "a", vec![Event::Text("A".into())]);
    assert_eq!(send(&mut c, "cb", "b", &[]).status, 202);
    let b = last_token(&c);
    assert_eq!(c.reply_mut(b).unwrap().state, reply::State::Queued, "on Claude it waits for A");
    let stop = |c: &Chat, run: &str| Command::Stop { thread_id: thread(c), run_id: run.into() };
    let queued = stop(&c, "cb");
    assert_eq!(c.command(queued, Instant::now()).status, 409, "B isn't streaming");
    assert!(c.cancelled.is_empty() && c.reply_mut(a).unwrap().streaming(), "A goes on");
    assert_eq!(c.reply_mut(b).unwrap().state, reply::State::Queued, "B still waits");
    let streaming = stop(&c, &run_of(&c, a));
    assert_eq!(c.command(streaming, Instant::now()).status, 202);
    assert_eq!(c.cancelled, [a], "as the window's Stop");
    assert!(c.streaming(), "and B goes, as after the window's Stop");
}

#[test]
fn a_run_id_is_at_most_128_letters_digits_dots_underscores_colons_or_dashes() {
    let (mut c, _rx) = watched();
    let long = "x".repeat(129);
    for bad in ["", "a b", "naïve", "a/b", "a\n", long.as_str()] {
        assert_eq!(send(&mut c, bad, "hi", &[]).status, 400, "run {bad:?}");
        let stop = Command::Stop { thread_id: thread(&c), run_id: bad.into() };
        assert_eq!(c.command(stop, Instant::now()).status, 400, "stop {bad:?}");
    }
    assert!(c.messages.is_empty());
    let longest = format!("A.z_0:9-{}", "x".repeat(120));
    assert_eq!(send(&mut c, &longest, "hi", &[]).status, 202);
}

#[test]
fn an_interrupt_id_is_matched_whole_so_a_run_id_with_ask_in_it_reaches_no_other_question() {
    let (mut c, _rx) = watched();
    assert_eq!(send(&mut c, "r-ask-1", "pick", &[]).status, 202);
    let token = last_token(&c);
    for e in [ask("One?"), ask("Two?"), Event::Done] {
        c.reply_event(token, e, Instant::now());
    }
    for (n, never_sent) in ["r-ask-1", "r-ask-1-ask-02", "r-ask-1-ask-+2", "r-ask-1-ask-3", "r-ask-1-ask-1-ask-2"].into_iter().enumerate() {
        assert_eq!(answer(&mut c, &format!("x{n}"), never_sent, "A").status, 409, "{never_sent}");
    }
    assert!(question(&c, token, 0).open() && question(&c, token, 1).open(), "neither was answered");
    assert_eq!(answer(&mut c, "y", "r-ask-1-ask-2", "B").status, 202);
    assert!(question(&c, token, 0).open());
    assert_eq!(question(&c, token, 1).answered.as_deref(), Some("B"));
}

#[test]
fn a_client_joining_mid_run_rebuilds_the_same_reply_as_the_window() {
    let (mut c, _rx) = watched();
    asked(&mut c, "a", vec![Event::Text("A".into()), Event::Done]);
    let b = asked(&mut c, "b", vec![Event::Text("Looking".into()), Event::Step { id: 3, tool: Tool::Read, target: "x.rs".into() }, Event::StepOutput { id: 3, text: "fn x".into() }]);
    let (tx, late) = watcher();
    c.join(tx);
    for e in [Event::StepEnd { id: 3, ok: true }, Event::Text("Done.".into()), Event::Done] {
        c.reply_event(b, e, Instant::now());
    }
    let events: Vec<agui::Event> = got(&late).iter().map(|v| parse_value(v).unwrap().unwrap()).collect();
    let agui::Event::MessagesSnapshot { messages, .. } = &events[0] else { panic!("a snapshot first") };
    let turns = snapshot::conversation_from_snapshot(messages);
    assert_eq!(turns.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["a"], "without the run going");
    let (mut replayer, mut rebuilt, now) = (Replayer::new(), Reply::new(0, "b"), Instant::now());
    for e in &events[1..] {
        for update in replayer.replay(e).unwrap() {
            match update.kind {
                Kind::Event(e) => rebuilt.apply(e, now),
                Kind::Stop => rebuilt.stop(now),
            }
        }
    }
    let lines = |r: &Reply| saved::reply_events(r, "t", "r").iter().map(agui::to_line).collect::<Vec<_>>();
    assert_eq!(lines(&rebuilt), lines(c.reply_to(c.messages[1].id).unwrap()));
}

#[test]
fn a_question_nothing_waits_for_arrives_closed_in_the_snapshot() {
    let (mut c, _rx) = watched();
    asked(&mut c, "pick", vec![ask("Which?")]);
    c.stop(Instant::now());
    let (tx, late) = watcher();
    c.join(tx);
    let events = got(&late);
    let asked = events[0]["messages"].as_array().unwrap().iter().find(|m| m["activityType"] == "ailfred:ask").unwrap();
    assert_eq!(asked["content"]["closed"], Value::Bool(true));
}

#[test]
fn a_new_conversation_sends_its_snapshot_and_the_old_thread_is_refused() {
    let (mut c, rx) = watched();
    asked(&mut c, "a", vec![Event::Done]);
    let old = thread(&c);
    c.clear();
    let events = got(&rx);
    let last = events.last().unwrap();
    assert_eq!((last["type"].as_str(), last["messages"].as_array().map(Vec::len)), (Some("MESSAGES_SNAPSHOT"), Some(0)));
    assert_ne!(thread(&c), old);
    assert_eq!(last["metadata"]["ailfred"]["threadId"].as_str(), Some(thread(&c).as_str()), "a client learns the thread to send to");
    assert_eq!(c.command(command::send(&old, "c1", &[], "hi", &[]), Instant::now()).status, 409);
    assert_eq!(send(&mut c, "c1", "hi", &[]).status, 202);
}

#[test]
fn a_dropped_client_doesnt_stop_the_run() {
    let (mut c, rx) = watched();
    drop(rx);
    let token = asked(&mut c, "a", vec![Event::Text("A".into())]);
    assert!(c.cancelled.is_empty() && c.reply_mut(token).unwrap().streaming());
}

#[test]
fn without_the_server_the_chat_feeds_nothing_and_refuses_commands() {
    let mut c = Chat::new(None);
    c.model = Some(model::DEFAULT_MODEL.into());
    asked(&mut c, "a", vec![Event::Done]);
    let thread = thread(&c);
    assert_eq!(c.command(command::send(&thread, "c1", &[], "hi", &[]), Instant::now()).status, 503);
}
