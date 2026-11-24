use std::sync::mpsc::Receiver;
use std::time::Instant;

use ailfred_protocol::agui::{self, Content, ContentPart, PartSource};
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

/// The thread clients see, the one their commands carry.
fn thread(c: &Chat) -> String {
    c.thread().to_owned()
}

/// The snapshot's thread and its messages' ids.
fn snapshot_of(e: &Value) -> (String, Vec<String>) {
    assert_eq!(e["type"], "MESSAGES_SNAPSHOT", "{e}");
    let ids = e["messages"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_owned()).collect();
    (e["metadata"]["ailfred"]["threadId"].as_str().unwrap().to_owned(), ids)
}

fn resume(c: &mut Chat, run: &str, entries: Vec<agui::Resume>) -> crate::server::Answer {
    let Command::Run(mut input) = command::answer(&thread(c), run, &[], "x", "x") else { unreachable!() };
    input.resume = Some(entries);
    c.command(Command::Run(input), Instant::now())
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

/// The runs' starts and ends among `events`: `RUN_STARTED {run}`, `RUN_FINISHED {run}`.
fn runs(events: &[Value]) -> Vec<String> {
    events.iter().filter(|e| e["type"] == "RUN_STARTED" || e["type"] == "RUN_FINISHED").map(|e| format!("{} {}", e["type"].as_str().unwrap(), e["runId"].as_str().unwrap())).collect()
}

/// The last snapshot among `events`.
fn last_snapshot(events: &[Value]) -> (String, Vec<String>) {
    snapshot_of(events.iter().rfind(|e| e["type"] == "MESSAGES_SNAPSHOT").expect("a snapshot"))
}

/// The ids a saved message and its text reply get in a snapshot: run `{thread}-{index}`.
fn said(thread: &str, index: usize) -> [String; 2] {
    let run = crate::history::run_id(thread, index);
    [format!("{run}-user"), format!("{run}-text-1")]
}

#[test]
fn after_cmd_n_the_old_conversations_retry_continue_and_answer_start_nothing_and_the_next_message_runs_to_its_end() {
    let (mut c, rx) = watched();
    let a = asked(&mut c, "a", vec![ask("Which?"), Event::Done]);
    let b = asked(&mut c, "b", vec![Event::Text("B".into()), Event::Lost("gone".into())]);
    assert_eq!(c.continuable(), Some(b));
    c.start_over();
    got(&rx);
    let asks = c.requests.len();
    // The old conversation still shows as it slides out: its buttons start nothing.
    assert_eq!(c.retry(a), None, "Retry");
    assert_eq!(c.go_on(b), None, "Continue");
    c.pick(a, 0, 0);
    assert_eq!(c.answer(a, 0), None, "an answer");
    c.stop(Instant::now());
    assert_eq!(c.requests.len(), asks, "nothing went to the model");
    assert!(got(&rx).is_empty(), "nothing reached clients");
    assert_eq!(send(&mut c, "c1", "hi", &[]).status, 202);
    let t = last_token(&c);
    c.reply_event(t, Event::Text("hello".into()), Instant::now());
    c.reply_event(t, Event::Done, Instant::now());
    let events = got(&rx);
    assert_eq!(runs(&events), ["RUN_STARTED c1", "RUN_FINISHED c1"], "{events:?}");
    let (tx, late) = watcher();
    c.join(tx);
    assert_eq!(snapshot_of(&got(&late)[0]), (thread(&c), said(&thread(&c), 0).to_vec()), "in the snapshot a client joining gets");
}

#[test]
fn a_reply_left_going_on_the_old_conversation_ends_when_the_next_message_starts_the_new_one() {
    let (mut c, rx) = watched();
    asked(&mut c, "a", vec![Event::Done]);
    c.start_over();
    got(&rx);
    // Past the guards, as a path nothing refused would: the old message's reply goes again.
    let left = c.start_reply(c.messages[0].id, "a".into(), false);
    c.reply_event(left, Event::Text("old".into()), Instant::now());
    let left_run = run_of(&c, left);
    got(&rx);
    c.cancelled.clear();
    assert_eq!(send(&mut c, "c1", "hi", &[]).status, 202);
    assert_eq!(c.cancelled, [left], "its process ends");
    let t = last_token(&c);
    c.reply_event(left, Event::Text("late".into()), Instant::now());
    c.reply_event(t, Event::Text("hello".into()), Instant::now());
    c.reply_event(t, Event::Done, Instant::now());
    let events = got(&rx);
    assert_eq!(runs(&events), [format!("RUN_FINISHED {left_run}"), "RUN_STARTED c1".into(), "RUN_FINISHED c1".into()], "{events:?}");
    let (tx, late) = watcher();
    c.join(tx);
    assert_eq!(snapshot_of(&got(&late)[0]).1, said(&thread(&c), 0), "the new message isn't taken for the old one's");
}

#[test]
fn a_retried_earlier_reply_that_is_stopped_is_put_back_in_its_place() {
    let (mut c, rx) = watched();
    for text in ["a", "b", "c"] {
        asked(&mut c, text, vec![Event::Text(text.to_uppercase()), Event::Done]);
    }
    let first = c.reply_to(c.messages[0].id).unwrap().token;
    let again = c.retry(first).unwrap();
    c.reply_event(again, Event::Text("A2".into()), Instant::now());
    got(&rx);
    c.stop(Instant::now());
    let events = got(&rx);
    let order: Vec<String> = (0..3).flat_map(|i| said(&thread(&c), i)).collect();
    assert_eq!(last_snapshot(&events).1, order, "{events:?}");
}

#[test]
fn a_retried_earlier_reply_stopped_by_answering_the_latest_question_is_put_back_in_its_place() {
    let (mut c, rx) = watched();
    asked(&mut c, "a", vec![Event::Text("A".into()), Event::Done]);
    asked(&mut c, "b", vec![Event::Text("B".into()), Event::Done]);
    let asking = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    let interrupt = format!("{}-ask-1", run_of(&c, asking));
    let first = c.reply_to(c.messages[0].id).unwrap().token;
    let again = c.retry(first).unwrap();
    c.reply_event(again, Event::Text("A2".into()), Instant::now());
    got(&rx);
    assert_eq!(answer(&mut c, "c1", &interrupt, "A").status, 202);
    let events = got(&rx);
    let (_, ids) = last_snapshot(&events);
    let t = thread(&c);
    let asked_run = crate::history::run_id(&t, 2);
    let mut order: Vec<String> = (0..2).flat_map(|i| said(&t, i)).collect();
    order.extend([format!("{asked_run}-user"), format!("{asked_run}-ask-1")]);
    assert_eq!(ids, order, "in place, without the answer, which comes as its run: {events:?}");
}

#[test]
fn a_run_id_used_before_cmd_n_stays_used_when_the_conversation_is_picked_back() {
    let (mut c, _rx) = watched();
    assert_eq!(send(&mut c, "c1", "a", &[]).status, 202);
    c.reply_event(last_token(&c), Event::Done, Instant::now());
    let old = thread(&c);
    c.start_over();
    c.load(crate::history::Loaded { id: old.clone(), created: 1, title: None, title_by_user: false, session: None, messages: Vec::new(), next_png: 0 });
    assert_eq!(thread(&c), old);
    assert_eq!(send(&mut c, "c1", "again", &[]).status, 409, "c1 was used in this thread");
    assert_eq!(c.command(command::retry(&old, "c2", "c1"), Instant::now()).status, 202, "the reply is still known by its live run id");
}

#[test]
fn a_client_run_id_shaped_as_a_saved_run_is_400_so_a_retry_names_one_reply() {
    let (mut c, _rx) = watched();
    let t = thread(&c);
    assert_eq!(send(&mut c, &format!("{t}-0"), "a", &[]).status, 400, "the saved id of message 0");
    assert_eq!(send(&mut c, &format!("{t}-12"), "a", &[]).status, 400);
    assert!(c.messages.is_empty());
    assert_eq!(send(&mut c, &format!("{t}-0a"), "a", &[]).status, 202, "not a number");
    c.reply_event(last_token(&c), Event::Done, Instant::now());
    assert_eq!(c.command(command::retry(&t, &format!("{t}-3"), &format!("{t}-0")), Instant::now()).status, 400);
    assert_eq!(c.command(command::retry(&t, "c2", &format!("{t}-0")), Instant::now()).status, 202, "by its saved id");
}

#[test]
fn picking_another_conversation_from_recent_after_cmd_n_makes_it_the_thread_clients_see() {
    let (mut c, rx) = watched();
    asked(&mut c, "a", vec![Event::Done]);
    c.start_over();
    let minted = thread(&c);
    got(&rx);
    let other = "20240101-000000-0009";
    let message = crate::history::LoadedMessage { text: "x".into(), files: Vec::new(), pngs: Vec::new(), reply: None };
    c.load(crate::history::Loaded { id: other.into(), created: 1, title: None, title_by_user: false, session: None, messages: vec![message], next_png: 0 });
    assert_eq!(last_snapshot(&got(&rx)), (other.to_owned(), vec![format!("{other}-0-user")]));
    assert_eq!(thread(&c), other);
    assert_eq!(c.command(command::send(&minted, "c1", &[], "hi", &[]), Instant::now()).status, 409, "the new one Cmd+N made is gone");
    assert_eq!(send(&mut c, "c1", "hi", &[]), crate::server::Answer::accepted(other, "c1"));
}

#[test]
fn a_declined_question_comes_back_closed_after_save_and_load() {
    let (mut c, _rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    let interrupt = format!("{}-ask-1", run_of(&c, token));
    assert_eq!(c.command(command::decline(&thread(&c), "c1", &[], &interrupt), Instant::now()).status, 202);
    asked(&mut c, "next", vec![Event::Text("N".into()), Event::Done]);
    let dir = std::env::temp_dir().join(format!("ailfred-declined-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let snapshot = c.snapshot().unwrap();
    crate::history::write_snapshot(&dir, &snapshot).unwrap();
    let mut fresh = Chat::new(None);
    fresh.load(crate::history::load(&dir, &snapshot.saved.id).expect("loads"));
    let _ = std::fs::remove_dir_all(&dir);
    let back = fresh.reply_to(fresh.messages[0].id).unwrap().token;
    let q = question(&fresh, back, 0);
    assert!(q.closed && q.answered.is_none(), "not answered");
    fresh.set_feed(Some(Feed::default()));
    let (tx, rx) = watcher();
    fresh.join(tx);
    let events = got(&rx);
    let asks = events[0]["messages"].as_array().unwrap().iter().find(|m| m["activityType"] == "ailfred:ask").unwrap();
    assert_eq!(asks["content"]["closed"], Value::Bool(true), "{asks}");
}

#[test]
fn declining_a_question_pi_waits_on_dismisses_its_dialog_and_what_pi_says_next_is_the_declines_reply() {
    let (mut c, rx) = watched();
    c.model = Some(model::PI.into());
    let token = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    let interrupt = format!("{}-ask-1", run_of(&c, token));
    got(&rx);
    let declined = c.command(command::decline(&thread(&c), "c1", &[], &interrupt), Instant::now());
    assert_eq!(declined, crate::server::Answer::accepted(&thread(&c), "c1"));
    let q = question(&c, token, 0);
    assert!(q.closed && q.answered.is_none(), "it shows as not answered");
    let (_, request) = c.requests.last().unwrap();
    assert!(request.answer && request.decline, "pi's dialog is dismissed: {request:?}");
    assert_eq!(c.messages.last().unwrap().text, DECLINED, "as the user's message, as an answer goes");
    let events = got(&rx);
    let asks = events[0]["messages"].as_array().unwrap().iter().find(|m| m["activityType"] == "ailfred:ask").unwrap();
    assert_eq!(asks["content"]["closed"], Value::Bool(true));
    assert_eq!(snapshot_of(&events[0]).1.len(), 2, "without the decline's message, which comes as its run");
    let next = last_token(&c);
    assert_ne!(next, token);
    c.reply_event(next, Event::Text("Skipped.".into()), Instant::now());
    c.reply_event(next, Event::Done, Instant::now());
    let events = got(&rx);
    assert_eq!(runs(&events), ["RUN_STARTED c1", "RUN_FINISHED c1"]);
    assert_eq!(events[0]["input"]["messages"][0]["content"].as_str(), Some(DECLINED));
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
fn a_closed_question_and_a_wrong_interrupt_id_are_409_to_an_answer_and_a_decline() {
    let (mut c, _rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    let run = run_of(&c, token);
    assert_eq!(answer(&mut c, "c1", &format!("{run}-ask-2"), "A").status, 409, "no such question");
    assert_eq!(answer(&mut c, "c2", "nope", "A").status, 409);
    assert_eq!(resume(&mut c, "c3", vec![command::decline_resume("nope")]).status, 409, "no such question to decline");
    assert!(question(&c, token, 0).open(), "still open");
    if let Some(Part::Question(q)) = c.replies.iter_mut().find(|r| r.token == token).unwrap().parts.last_mut() {
        q.closed = true;
    }
    assert_eq!(answer(&mut c, "c4", &format!("{run}-ask-1"), "A").status, 409, "closed");
    assert_eq!(resume(&mut c, "c4", vec![command::decline_resume(&format!("{run}-ask-1"))]).status, 409, "closed: nothing to decline");
    let failed = asked(&mut c, "again", vec![ask("Which?"), Event::Fail("broke".into())]);
    let run = run_of(&c, failed);
    assert_eq!(answer(&mut c, "c5", &format!("{run}-ask-1"), "A").status, 409, "its run failed: nothing waits");
    assert_eq!(c.messages.len(), 2, "no answer was sent");
}

#[test]
fn declining_a_question_closes_it_sends_nothing_to_the_model_and_clients_get_it_closed_in_a_snapshot() {
    let (mut c, rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("Which?"), Event::Done]);
    let interrupt = format!("{}-ask-1", run_of(&c, token));
    got(&rx);
    let asks = c.requests.len();
    let declined = c.command(command::decline(&thread(&c), "c1", &[], &interrupt), Instant::now());
    assert_eq!(declined, crate::server::Answer::accepted(&thread(&c), "c1"));
    let q = question(&c, token, 0);
    assert!(q.closed && q.answered.is_none(), "it shows as not answered");
    assert!(!c.replies[0].asks(), "nothing waits for it");
    assert_eq!((c.messages.len(), c.replies.len(), c.requests.len()), (1, 1, asks), "no message, no reply, nothing to the model");
    let events = got(&rx);
    assert_eq!(events.len(), 1, "{events:?}");
    let asked = events[0]["messages"].as_array().unwrap().iter().find(|m| m["activityType"] == "ailfred:ask").unwrap();
    assert_eq!(asked["content"]["closed"], Value::Bool(true), "{asked}");
    assert_eq!(c.command(command::decline(&thread(&c), "c2", &[], &interrupt), Instant::now()).status, 409, "declined already");
    assert_eq!(answer(&mut c, "c3", &interrupt, "A").status, 409, "and it takes no answer");
    assert_eq!(c.command(command::decline(&thread(&c), "c1", &[], &interrupt), Instant::now()).status, 409, "its runId is used");
}

#[test]
fn a_resume_with_answers_and_declines_sends_the_answers_and_closes_the_declined() {
    let (mut c, rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("One?"), ask("Two?"), ask("Three?"), Event::Done]);
    let run = run_of(&c, token);
    got(&rx);
    let entries = vec![command::answer_resume(&format!("{run}-ask-1"), "A"), command::decline_resume(&format!("{run}-ask-2"))];
    assert_eq!(resume(&mut c, "c1", entries).status, 202);
    assert_eq!(question(&c, token, 0).answered.as_deref(), Some("A"));
    assert!(question(&c, token, 1).closed && question(&c, token, 1).answered.is_none(), "declined");
    assert!(question(&c, token, 2).open(), "not named: open");
    assert_eq!(c.messages.last().unwrap().text, "A", "the answer goes as before");
    assert!(c.reply_to(c.messages.last().unwrap().id).unwrap().answer, "and gets its reply");
    let events = got(&rx);
    let (_, ids) = snapshot_of(&events[0]);
    assert!(!ids.iter().any(|id| id.ends_with("-1-user")), "without the answer's message, which comes as its run: {ids:?}");
    let asks: Vec<&Value> = events[0]["messages"].as_array().unwrap().iter().filter(|m| m["activityType"] == "ailfred:ask").collect();
    assert_eq!(asks.iter().map(|a| (a["content"]["answered"].as_str(), a["content"]["closed"].as_bool())).collect::<Vec<_>>(), [(Some("A"), None), (None, Some(true)), (None, None)]);
    c.reply_event(last_token(&c), Event::Text("ok".into()), Instant::now());
    let events = got(&rx);
    assert_eq!((events[0]["type"].as_str(), events[0]["runId"].as_str()), (Some("RUN_STARTED"), Some("c1")));
    let both = vec![command::decline_resume(&format!("{run}-ask-3")), command::answer_resume(&format!("{run}-ask-3"), "B")];
    assert_eq!(resume(&mut c, "c2", both).status, 409, "no longer the latest reply");
}

#[test]
fn a_question_declined_and_answered_in_one_resume_is_400() {
    let (mut c, _rx) = watched();
    let token = asked(&mut c, "pick", vec![ask("One?"), Event::Done]);
    let id = format!("{}-ask-1", run_of(&c, token));
    assert_eq!(resume(&mut c, "c1", vec![command::decline_resume(&id), command::answer_resume(&id, "A")]).status, 400);
    assert!(question(&c, token, 0).open(), "nothing applied");
}

#[test]
fn a_new_conversation_command_ends_this_one_as_cmd_n_does_and_clients_get_the_new_thread_at_once() {
    let (mut c, rx) = watched();
    asked(&mut c, "a", vec![Event::Done]);
    let going = asked(&mut c, "b", vec![Event::Text("B".into())]);
    let old = thread(&c);
    got(&rx);
    let saved = c.saved.len();
    let answer = c.command(command::new_conversation(&old), Instant::now());
    assert_eq!(answer.status, 202, "{answer:?}");
    let new = answer.body["threadId"].as_str().unwrap().to_owned();
    assert_ne!(new, old);
    assert_eq!(answer.body.get("runId"), None, "no run");
    assert_eq!(c.cancelled, [going], "the reply going stops, as with Cmd+N");
    let events = got(&rx);
    assert_eq!(events.iter().map(|e| e["type"].as_str().unwrap()).collect::<Vec<_>>(), ["TEXT_MESSAGE_END", "RUN_FINISHED", "MESSAGES_SNAPSHOT"]);
    assert_eq!(snapshot_of(&events[2]), (new.clone(), Vec::new()), "empty, under the new thread");
    assert!(c.saved[saved..].iter().all(|s| s.saved.id == old), "the old one saved as it stopped; no empty file for the new one");
    assert_eq!(c.command(command::new_conversation(&old), Instant::now()).status, 409, "the old thread");
    assert_eq!(c.command(command::send(&old, "c1", &[], "hi", &[]), Instant::now()).status, 409);
    assert_eq!(send(&mut c, "c1", "hi", &[]), crate::server::Answer::accepted(&new, "c1"), "a message goes to the new thread");
    assert_eq!((c.conv.id.as_str(), c.messages.len()), (new.as_str(), 1), "and the conversation is that one");
    c.reply_event(last_token(&c), Event::Done, Instant::now());
    let events = got(&rx);
    assert_eq!((events[0]["type"].as_str(), events[0]["threadId"].as_str()), (Some("RUN_STARTED"), Some(new.as_str())));
    assert_eq!(c.saved.last().unwrap().saved.id, new, "saved under the thread clients were told");
}

#[test]
fn a_new_conversation_command_in_the_chat_window_clears_it_there() {
    let (mut c, rx) = watched();
    c.set_windowed(true);
    asked(&mut c, "a", vec![Event::Done]);
    let old = thread(&c);
    got(&rx);
    let answer = c.command(command::new_conversation(&old), Instant::now());
    assert_eq!(answer.status, 202);
    assert!(!c.has_messages() && c.is_open(), "cleared in its window, as Cmd+N there does");
    assert_eq!(answer.body["threadId"].as_str(), Some(thread(&c).as_str()));
    assert_eq!(snapshot_of(&got(&rx)[0]), (thread(&c), Vec::new()));
}

#[test]
fn cmd_n_in_the_side_panel_shows_clients_the_new_thread_at_once_and_the_next_message_uses_it() {
    let (mut c, rx) = watched();
    asked(&mut c, "a", vec![Event::Done]);
    let old = c.conv.id.clone();
    got(&rx);
    c.start_over();
    let events = got(&rx);
    let (new, ids) = snapshot_of(events.last().expect("a snapshot at once"));
    assert!(new != old && ids.is_empty(), "{new} {ids:?}");
    assert_eq!(thread(&c), new);
    let (tx, late) = watcher();
    c.join(tx);
    assert_eq!(snapshot_of(&got(&late)[0]), (new.clone(), Vec::new()), "a client joining now gets it too");
    assert_eq!(c.command(command::send(&old, "c1", &[], "hi", &[]), Instant::now()).status, 409);
    assert_eq!(send(&mut c, "c1", "hi", &[]), crate::server::Answer::accepted(&new, "c1"));
    assert_eq!(c.conv.id, new, "the conversation is the one clients were told");
    let events = got(&rx);
    assert!(events.iter().all(|e| e["type"] != "MESSAGES_SNAPSHOT"), "no second snapshot to wipe what the client sent: {events:?}");
    c.reply_event(last_token(&c), Event::Done, Instant::now());
    assert_eq!(got(&rx)[0]["threadId"].as_str(), Some(new.as_str()));
    assert_eq!(c.saved.last().unwrap().saved.id, new);
}

#[test]
fn a_conversation_left_with_cmd_n_and_picked_back_is_its_thread_again_for_clients() {
    let (mut c, rx) = watched();
    asked(&mut c, "a", vec![Event::Done]);
    let old = c.conv.id.clone();
    c.start_over();
    got(&rx);
    // Picked from Recent: the one still here goes on (its messages aren't read again).
    c.load(crate::history::Loaded { id: old.clone(), created: 1, title: None, title_by_user: false, session: None, messages: Vec::new(), next_png: 0 });
    let (thread_id, ids) = snapshot_of(&got(&rx)[0]);
    assert_eq!((thread_id.as_str(), ids.len()), (old.as_str(), 1), "{ids:?}");
    assert_eq!(thread(&c), old);
}

#[test]
fn a_retry_command_retries_the_reply_it_names_clients_getting_a_snapshot_without_it_then_the_new_run() {
    let (mut c, rx) = watched();
    asked(&mut c, "a", vec![Event::Text("A".into()), Event::Done]);
    let b = asked(&mut c, "b", vec![Event::Text("B".into()), Event::Done]);
    got(&rx);
    let answer = c.command(command::retry(&thread(&c), "c1", &run_of(&c, b)), Instant::now());
    assert_eq!(answer, crate::server::Answer::accepted(&thread(&c), "c1"));
    assert_eq!(c.requests.last().unwrap().1.text, "b", "what the window's Retry asks");
    let events = got(&rx);
    assert_eq!(events.len(), 1, "{events:?}");
    let a_run = crate::history::run_id(&thread(&c), 0);
    assert_eq!(snapshot_of(&events[0]).1, [format!("{a_run}-user"), format!("{a_run}-text-1")], "without the retried message: its run brings it");
    let again = last_token(&c);
    c.reply_event(again, Event::Text("B2".into()), Instant::now());
    c.reply_event(again, Event::Done, Instant::now());
    let events = got(&rx);
    assert_eq!((events[0]["type"].as_str(), events[0]["runId"].as_str()), (Some("RUN_STARTED"), Some("c1")));
    assert_eq!(events[0]["input"]["messages"][0]["content"].as_str(), Some("b"));
    assert!(events.iter().all(|e| e["type"] != "MESSAGES_SNAPSHOT"), "the last message's: in place already");
    // The first reply, by the id the snapshot gives it: its new run comes last, then a snapshot
    // puts it back in its place.
    assert_eq!(c.command(command::retry(&thread(&c), "c2", &a_run), Instant::now()).status, 202);
    let events = got(&rx);
    let b_run = crate::history::run_id(&thread(&c), 1);
    assert_eq!(snapshot_of(&events[0]).1, [format!("{b_run}-user"), format!("{b_run}-text-1")]);
    let first = c.reply_to(c.messages[0].id).unwrap().token;
    c.reply_event(first, Event::Text("A2".into()), Instant::now());
    c.reply_event(first, Event::Done, Instant::now());
    let events = got(&rx);
    assert_eq!(events.iter().map(|e| e["type"].as_str().unwrap()).collect::<Vec<_>>(), ["RUN_STARTED", "TEXT_MESSAGE_START", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_END", "RUN_FINISHED", "MESSAGES_SNAPSHOT"]);
    assert_eq!(snapshot_of(&events[5]).1, [format!("{a_run}-user"), format!("{a_run}-text-1"), format!("{b_run}-user"), format!("{b_run}-text-1")]);
    assert_eq!(events[5]["messages"][1]["content"].as_str(), Some("A2"));
}

#[test]
fn a_retry_of_a_reply_the_window_offers_no_retry_on_or_with_a_used_run_id_is_409() {
    let (mut c, _rx) = watched();
    let a = asked(&mut c, "a", vec![Event::Text("A".into()), Event::Done]);
    let a_run = run_of(&c, a);
    let failed = asked(&mut c, "b", vec![Event::Fail("broke".into())]);
    let b_run = run_of(&c, failed);
    let going = asked(&mut c, "c", vec![Event::Text("C".into())]);
    assert_eq!(send(&mut c, "cq", "d", &[]).status, 202);
    let retry = |c: &mut Chat, run: &str, of: &str| {
        let thread = thread(c);
        c.command(command::retry(&thread, run, of), Instant::now()).status
    };
    let going = run_of(&c, going);
    assert_eq!(retry(&mut c, "c1", &going), 409, "streaming: it shows no Retry");
    assert_eq!(retry(&mut c, "c1", "cq"), 409, "queued");
    assert_eq!(retry(&mut c, "c1", "nope"), 409);
    assert_eq!(retry(&mut c, "c1", &a_run[..a_run.len() - 1]), 409, "matched whole");
    assert_eq!(retry(&mut c, "cq", &a_run), 409, "a used runId");
    assert_eq!(retry(&mut c, "a b", &a_run), 400);
    assert_eq!(c.command(command::retry("other", "c1", &a_run), Instant::now()).status, 409);
    assert!(c.cancelled.is_empty(), "nothing retried");
    assert_eq!(retry(&mut c, "c1", &b_run), 202, "a failed reply offers Retry");
    assert_eq!(retry(&mut c, "c2", &b_run), 409, "the reply it named is gone: a new run took its place");
    c.stop(Instant::now());
    assert_eq!(retry(&mut c, "c3", &a_run), 202, "a done one too");
    c.start_over();
    let first = crate::history::run_id(&c.conv.id, 0);
    assert_eq!(retry(&mut c, "c4", &first), 409, "the conversation is over");
}

#[test]
fn a_window_retry_gives_clients_a_snapshot_without_the_old_reply_then_the_new_run() {
    let (mut c, rx) = watched();
    let a = asked(&mut c, "a", vec![Event::Text("A".into()), Event::Done]);
    got(&rx);
    let again = c.retry(a).unwrap();
    let events = got(&rx);
    assert_eq!(events.len(), 1);
    assert_eq!(snapshot_of(&events[0]), (thread(&c), Vec::new()), "no old reply beside the new one");
    c.reply_event(again, Event::Text("A2".into()), Instant::now());
    let events = got(&rx);
    assert_eq!(events[0]["type"], "RUN_STARTED");
    assert_eq!(events[0]["input"]["messages"][0]["content"].as_str(), Some("a"));
    // Retrying the reply streaming: its run ends cancelled, then the snapshot, then the new run.
    c.retry(again).unwrap();
    let types: Vec<String> = got(&rx).iter().map(|e| e["type"].as_str().unwrap().to_owned()).collect();
    assert_eq!(types, ["TEXT_MESSAGE_END", "RUN_FINISHED", "MESSAGES_SNAPSHOT"]);
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
