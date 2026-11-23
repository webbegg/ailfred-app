use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::history::{Loaded, LoadedMessage, ReplyState, SavedReply};
use crate::reply::State;

fn scratch(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("ailfred-chat-{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn msg(id: u64, text: &str) -> Message {
    Message::text(id, text)
}

fn now() -> Instant {
    Instant::now()
}

/// Message `id` says `text`; the reply streams `reply` and ends with `end`. The reply's token.
fn exchange(c: &mut Chat, id: u64, text: &str, reply: &str, end: Event) -> u64 {
    c.push(Message::text(id, text));
    let token = c.ask(id).expect("asked");
    if !reply.is_empty() {
        c.reply_event(token, Event::Text(reply.into()), now());
    }
    c.reply_event(token, end, now());
    token
}

fn last_saved(c: &Chat) -> &history::Saved {
    &c.saved.last().expect("something was saved").saved
}

fn reply_state(c: &Chat, message: usize) -> Option<ReplyState> {
    c.saved.last()?.saved.messages.get(message)?.reply.as_ref().map(|r| r.state)
}

fn loaded_message(text: &str, reply: Option<SavedReply>) -> LoadedMessage {
    LoadedMessage { text: text.into(), files: Vec::new(), pngs: Vec::new(), reply }
}

fn loaded(id: &str, messages: Vec<LoadedMessage>, session: Option<(String, usize)>) -> Loaded {
    Loaded { id: id.into(), created: 5, title: Some("Tea".into()), title_by_user: false, session, messages, next_png: 3 }
}

fn done(text: &str) -> SavedReply {
    SavedReply { state: ReplyState::Done, text: text.into(), ..SavedReply::default() }
}

// What goes to be saved, and when.

#[test]
fn a_conversation_without_a_message_is_never_written() {
    let mut c = Chat::new(None);
    assert!(c.snapshot().is_none());
    c.clear();
    c.start_over();
    c.stop(now());
    assert!(c.saved.is_empty());
}

#[test]
fn a_message_is_saved_when_sent_with_its_reply_interrupted_and_again_when_the_reply_ends_not_per_token() {
    let mut c = Chat::new(None);
    c.push(msg(1, "why is the sky blue?"));
    let token = c.ask(1).unwrap();
    assert_eq!(c.saved.len(), 1, "saved when sent");
    assert_eq!(reply_state(&c, 0), Some(ReplyState::Interrupted), "quitting now leaves it interrupted");
    c.reply_event(token, Event::Text("Rayleigh ".into()), now());
    c.reply_event(token, Event::Text("scattering.".into()), now());
    assert_eq!(c.saved.len(), 1, "not per token");
    c.reply_event(token, Event::Done, now());
    assert_eq!(c.saved.len(), 2, "saved when the reply ends");
    assert_eq!(reply_state(&c, 0), Some(ReplyState::Done));
    let reply = last_saved(&c).messages[0].reply.as_ref().unwrap();
    assert_eq!(reply.text, "Rayleigh scattering.");
    assert_eq!(last_saved(&c).messages[0].text, "why is the sky blue?");
    c.reply_event(token, Event::Text("stray".into()), now());
    assert_eq!(c.saved.len(), 2, "a stream that's over saves nothing more");
}

#[test]
fn a_failed_stopped_or_retried_reply_is_saved_too() {
    let mut c = Chat::new(None);
    let token = exchange(&mut c, 1, "hi", "", Event::Fail("overloaded".into()));
    assert_eq!(reply_state(&c, 0), Some(ReplyState::Failed));
    assert_eq!(last_saved(&c).messages[0].reply.as_ref().unwrap().error.as_deref(), Some("overloaded"));
    let again = c.retry(token).unwrap();
    assert_eq!(reply_state(&c, 0), Some(ReplyState::Interrupted), "the retry is saved as it starts");
    c.reply_event(again, Event::Text("part".into()), now());
    let before = c.saved.len();
    c.stop(now());
    assert_eq!(c.saved.len(), before + 1);
    assert_eq!(reply_state(&c, 0), Some(ReplyState::Stopped));
    assert_eq!(last_saved(&c).messages[0].reply.as_ref().unwrap().text, "part", "what came is kept");
    c.stop(now());
    assert_eq!(c.saved.len(), before + 1, "stopping what isn't streaming saves nothing");
}

#[test]
fn a_snapshot_names_each_capture_once_and_keeps_the_names_as_the_conversation_grows() {
    let mut c = Chat::new(None);
    let id = c.conv.id.clone();
    c.push(Message::text(1, "look").with_pngs(&[vec![1, 2], vec![3]]));
    let first = c.snapshot().unwrap();
    assert_eq!(first.saved.messages[0].pngs, [format!("{id}/0.png"), format!("{id}/1.png")]);
    assert_eq!(first.pngs.len(), 2);
    assert_eq!(*first.pngs[0].1, vec![1, 2]);
    c.push(Message::text(2, "and this").with_pngs(&[vec![4]]));
    let second = c.snapshot().unwrap();
    assert_eq!(second.saved.messages[0].pngs, first.saved.messages[0].pngs, "the same files");
    assert_eq!(second.saved.messages[1].pngs, [format!("{id}/2.png")]);
    assert_eq!(second.saved.id, id);
}

#[test]
fn cmd_n_keeps_the_old_conversation_saved_and_the_next_message_starts_one_with_a_fresh_id() {
    let mut c = Chat::new(None);
    exchange(&mut c, 1, "first conversation", "ok", Event::Done);
    let old = c.conv.id.clone();
    let saved = c.saved.len();
    c.clear();
    assert_ne!(c.conv.id, old, "a fresh id at once");
    assert_eq!(c.saved.len(), saved, "an empty conversation isn't written");
    assert_eq!(last_saved(&c).id, old);
    assert_eq!(last_saved(&c).messages[0].text, "first conversation", "the old one is what was saved");
    exchange(&mut c, 2, "second conversation", "ok", Event::Done);
    assert_eq!(last_saved(&c).id, c.conv.id);
    assert_ne!(last_saved(&c).id, old);
    assert_eq!(last_saved(&c).messages.len(), 1, "it holds the new message only");
    assert!(c.saved.iter().any(|s| s.saved.id == old && s.saved.messages[0].text == "first conversation"));
}

#[test]
fn cmd_n_in_the_side_panel_ends_the_conversation_and_the_next_message_takes_a_new_id() {
    let mut c = Chat::new(None);
    exchange(&mut c, 1, "one", "ok", Event::Done);
    let old = c.conv.id.clone();
    c.start_over();
    assert_eq!(c.conv.id, old, "it still shows while it slides out");
    exchange(&mut c, 2, "two", "ok", Event::Done);
    assert_ne!(c.conv.id, old);
    assert_eq!(last_saved(&c).messages.len(), 1);
}

#[test]
fn picking_the_conversation_just_left_with_cmd_n_in_the_side_panel_goes_on_with_it() {
    let mut c = Chat::new(None);
    exchange(&mut c, 1, "first", "ok", Event::Done);
    let left = c.conv.id.clone();
    c.start_over();
    c.load(loaded(&left, vec![loaded_message("first", Some(done("ok")))], None));
    exchange(&mut c, 2, "follow-up", "sure", Event::Done);
    assert_eq!(c.conv.id, left, "still the conversation picked");
    assert_eq!(last_saved(&c).messages.len(), 2, "the follow-up joins it");
}

#[test]
fn cmd_n_while_a_reply_streams_saves_it_stopped_first() {
    let mut c = Chat::new(None);
    c.push(msg(1, "tell me a long story"));
    let token = c.ask(1).unwrap();
    c.reply_event(token, Event::Text("Once upon".into()), now());
    let old = c.conv.id.clone();
    c.clear();
    let saved = c.saved.iter().rev().find(|s| s.saved.id == old).unwrap();
    let reply = saved.saved.messages[0].reply.as_ref().unwrap();
    assert_eq!((reply.state, reply.text.as_str()), (ReplyState::Stopped, "Once upon"));
}

#[test]
fn the_session_is_saved_as_the_index_of_the_message_its_reply_ends_at() {
    let mut c = Chat::new(None);
    exchange(&mut c, 1, "one", "ok", Event::Done);
    let token = exchange(&mut c, 2, "two", "ok", Event::Done);
    c.set_session(token, "sess-7".into());
    let snapshot = c.snapshot().unwrap();
    assert_eq!(snapshot.saved.session, Some(history::SavedSession { id: "sess-7".into(), after_message: 1 }));
}

// Loading one back.

#[test]
fn a_loaded_conversation_shows_its_messages_under_fresh_ids_and_its_replies_as_they_were() {
    let mut c = Chat::new(None);
    let interrupted = SavedReply { state: ReplyState::Interrupted, text: "Half an ans".into(), ..SavedReply::default() };
    let failed = SavedReply { state: ReplyState::Failed, error: Some("overloaded".into()), ..SavedReply::default() };
    c.load(loaded("20240101-000000-0001", vec![loaded_message("one", Some(done("first"))), loaded_message("two", Some(interrupted)), loaded_message("three", Some(failed)), loaded_message("four", None)], None));
    assert_eq!(c.conv.id, "20240101-000000-0001");
    assert_eq!((c.conv.title.as_deref(), c.conv.created, c.conv.next_png), (Some("Tea"), 5, 3));
    let ids: Vec<u64> = c.messages.iter().map(|m| m.id).collect();
    assert_eq!(c.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["one", "two", "three", "four"]);
    assert!(ids.windows(2).all(|w| w[0] < w[1]) && ids[0] >= LOCAL_IDS, "ids of their own: {ids:?}");
    assert_eq!(c.reply_to(ids[0]).unwrap().state, State::Done);
    let stopped = c.reply_to(ids[1]).unwrap();
    assert_eq!(stopped.state, State::Stopped, "interrupted shows as stopped");
    assert_eq!(stopped.history_text().as_deref(), Some("Half an ans"));
    assert!(stopped.interrupted, "and says it was interrupted");
    assert_eq!(c.reply_to(ids[2]).unwrap().state, State::Failed);
    assert!(c.reply_to(ids[3]).is_none());
    assert!(!c.streaming());
    let tokens: Vec<u64> = ids.iter().filter_map(|id| c.reply_to(*id)).map(|r| r.token).collect();
    assert!(tokens.windows(2).all(|w| w[0] != w[1]) && tokens.iter().all(|t| *t > 0), "a token each, so Retry finds its reply: {tokens:?}");
    // A new message goes after them and the history it sends is the earlier replies'.
    c.push(msg(c.next_id(), "five"));
    let request = c.request_for(c.messages[4].id, "m").unwrap();
    let texts: Vec<&str> = request.history.iter().map(|t| t.reply.as_str()).collect();
    assert_eq!(texts, ["first", "Half an ans"]);
}

// "I was interrupted. Continue?"

fn interrupted(text: &str) -> SavedReply {
    SavedReply { state: ReplyState::Interrupted, text: text.into(), ..SavedReply::default() }
}

#[test]
fn the_last_reply_interrupted_offers_continue_which_asks_the_model_to_go_on_from_it() {
    let mut c = Chat::new(None);
    c.model = Some("m".into());
    c.load(loaded("20240101-000000-0005", vec![loaded_message("one", Some(done("first"))), loaded_message("two", Some(interrupted("Half an ans")))], None));
    let token = c.reply_to(c.messages[1].id).unwrap().token;
    assert_eq!(c.continuable(), Some(token));
    let next = c.go_on(token).expect("it goes on");
    assert_eq!(c.messages.last().map(|m| m.text.as_str()), Some(CONTINUE));
    let asked = c.messages.last().unwrap().id;
    let reply = c.reply_to(asked).unwrap();
    assert_eq!((reply.token, reply.streaming()), (next, true));
    let (_, request) = c.requests.last().expect("asked the model");
    assert_eq!(request.text, CONTINUE);
    let said: Vec<&str> = request.history.iter().map(|t| t.reply.as_str()).collect();
    assert_eq!(said, ["first", "Half an ans"], "with what it said before it was interrupted");
    assert_eq!(c.continuable(), None, "once");
    assert_eq!(c.saved.last().unwrap().saved.messages.len(), 3, "saved with the new message");
    assert_eq!(c.go_on(token), None);
}

#[test]
fn an_interrupted_reply_that_had_said_nothing_is_asked_again_instead() {
    let mut c = Chat::new(None);
    c.model = Some("m".into());
    c.load(loaded("20240101-000000-0006", vec![loaded_message("hello", Some(interrupted("")))], None));
    let token = c.continuable().expect("offered");
    let again = c.go_on(token).expect("asked again");
    assert_eq!(c.messages.len(), 1, "no message of its own");
    let reply = c.reply_to(c.messages[0].id).unwrap();
    assert_eq!((reply.token, reply.streaming(), reply.interrupted), (again, true, false));
    assert_eq!(c.requests.last().unwrap().1.text, "hello");
}

#[test]
fn only_the_conversations_last_reply_offers_continue() {
    let mut c = Chat::new(None);
    c.model = Some("m".into());
    c.load(loaded("20240101-000000-0007", vec![loaded_message("one", Some(interrupted("Half"))), loaded_message("two", Some(done("ok")))], None));
    assert_eq!(c.continuable(), None, "one further up");
    let older = c.reply_to(c.messages[0].id).unwrap().token;
    assert_eq!(c.go_on(older), None);
    let mut c = Chat::new(None);
    c.model = Some("m".into());
    c.load(loaded("20240101-000000-0008", vec![loaded_message("one", Some(interrupted("Half")))], None));
    c.push(msg(c.next_id(), "something else"));
    assert_eq!(c.continuable(), None, "a message after it");
    let mut c = Chat::new(None);
    c.model = Some("m".into());
    c.load(loaded("20240101-000000-0009", vec![loaded_message("one", Some(SavedReply { state: ReplyState::Stopped, text: "Half".into(), ..SavedReply::default() }))], None));
    assert_eq!(c.continuable(), None, "stopped by the user");
}

#[test]
fn a_reply_whose_model_went_away_offers_continue() {
    let mut c = Chat::new(None);
    c.model = Some("m".into());
    let token = exchange(&mut c, LOCAL_IDS, "hi", "Half", Event::Lost("The assistant stopped unexpectedly.".into()));
    assert_eq!(c.continuable(), Some(token));
    assert_eq!(reply_state(&c, 0), Some(ReplyState::Interrupted), "saved so");
}

#[test]
fn a_loaded_reply_can_be_retried_and_the_retry_asks_for_the_same_message() {
    let mut c = Chat::new(None);
    c.load(loaded("20240101-000000-0002", vec![loaded_message("hello", Some(SavedReply { state: ReplyState::Interrupted, ..SavedReply::default() }))], None));
    let token = c.replies[0].token;
    let again = c.retry(token).expect("it can be retried");
    assert_eq!(c.reply_to(c.messages[0].id).unwrap().prompt, "hello");
    assert!(c.reply_to(c.messages[0].id).unwrap().streaming());
    assert_ne!(again, token);
}

#[test]
fn the_session_id_goes_back_to_the_message_its_reply_ended_at() {
    let mut c = Chat::new(None);
    let two = vec![loaded_message("one", Some(done("a"))), loaded_message("two", Some(done("b")))];
    c.load(loaded("20240101-000000-0003", two, Some(("sess-1".into(), 1))));
    assert_eq!(c.session, Some(("sess-1".into(), c.messages[1].id)));
    c.push(msg(c.next_id(), "three"));
    assert_eq!(c.request_for(c.messages[2].id, "m").unwrap().session.as_deref(), Some("sess-1"), "the next message resumes it");
    // A session that ended earlier than the history doesn't resume.
    let mut c = Chat::new(None);
    let two = vec![loaded_message("one", Some(done("a"))), loaded_message("two", Some(done("b")))];
    c.load(loaded("20240101-000000-0004", two, Some(("sess-1".into(), 0))));
    c.push(msg(c.next_id(), "three"));
    assert_eq!(c.request_for(c.messages[2].id, "m").unwrap().session, None);
    // One pointing past the messages is dropped.
    let mut c = Chat::new(None);
    c.load(loaded("20240101-000000-0005", vec![loaded_message("one", Some(done("a")))], Some(("sess-1".into(), 9))));
    assert_eq!(c.session, None);
}

#[test]
fn picking_a_conversation_stops_what_streams_ends_its_run_and_loads_the_one_picked() {
    let mut c = Chat::new(None);
    c.push(msg(1, "a long question"));
    let token = c.ask(1).unwrap();
    c.reply_event(token, Event::Text("So far".into()), now());
    let old = c.conv.id.clone();
    c.load(loaded("20240101-000000-0006", vec![loaded_message("other", Some(done("fine")))], None));
    assert_eq!(c.cancelled, [token], "the run behind it ended");
    assert!(!c.streaming());
    assert_eq!(c.conv.id, "20240101-000000-0006");
    assert_eq!(c.messages.len(), 1);
    assert_eq!(c.messages[0].text, "other");
    c.reply_event(token, Event::Text("late".into()), now());
    assert!(c.replies.iter().all(|r| !r.copy_text().contains("late")), "its stream feeds nothing");
    let kept = c.saved.iter().rev().find(|s| s.saved.id == old).expect("the one that streamed was saved");
    assert_eq!(kept.saved.messages[0].reply.as_ref().unwrap().text, "So far");
    assert_eq!(kept.saved.messages[0].reply.as_ref().unwrap().state, ReplyState::Stopped);
}

#[test]
fn picking_the_conversation_that_is_already_there_leaves_it_streaming() {
    let mut c = Chat::new(None);
    c.push(msg(1, "a long question"));
    let token = c.ask(1).unwrap();
    let id = c.conv.id.clone();
    c.load(loaded(&id, vec![loaded_message("stale copy from disk", None)], None));
    assert!(c.streaming());
    assert!(c.cancelled.is_empty());
    assert_eq!(c.messages[0].text, "a long question", "what's in memory is the truth");
    assert_eq!(c.replies[0].token, token);
}

#[test]
fn a_conversation_goes_through_the_disk_and_comes_back_whole() {
    let dir = scratch("disk");
    let mut c = Chat::new(None);
    c.push(Message::text(1, "what is in this picture?").with_pngs(&[vec![1, 2, 3]]));
    let token = c.ask(1).unwrap();
    c.reply_event(token, Event::Think("hmm".into()), now());
    c.reply_event(token, Event::Text("A tea cup.".into()), now());
    c.reply_event(token, Event::Done, now());
    c.set_session(token, "sess-3".into());
    c.conv.title = Some("A tea cup".into());
    let snapshot = c.snapshot().unwrap();
    history::write_snapshot(&dir, &snapshot).unwrap();

    let mut fresh = Chat::new(None);
    fresh.load(history::load(&dir, &snapshot.saved.id).expect("loads"));
    assert_eq!(fresh.conv.id, c.conv.id);
    assert_eq!(fresh.conv.title.as_deref(), Some("A tea cup"));
    assert_eq!(fresh.messages[0].text, "what is in this picture?");
    assert_eq!(*fresh.messages[0].pngs[0], vec![1, 2, 3], "its capture came back by its path");
    assert_eq!(fresh.messages[0].files, c.messages[0].files);
    let reply = fresh.reply_to(fresh.messages[0].id).unwrap();
    assert_eq!((reply.state, reply.copy_text().as_str()), (State::Done, "A tea cup."));
    assert!(matches!(reply.parts.first(), Some(reply::Part::Thinking(t)) if t.text == "hmm"));
    let request = {
        fresh.push(msg(fresh.next_id(), "and a saucer?"));
        fresh.request_for(fresh.messages[1].id, "m").unwrap()
    };
    assert_eq!(request.session.as_deref(), Some("sess-3"), "the session is restored to the right message");
    assert_eq!(request.history.len(), 1);
    assert_eq!(request.history[0].pngs.len(), 1, "the capture goes with the history again");
    // Saved again, the capture keeps its file and the next one takes the next number.
    fresh.push(Message::text(fresh.next_id(), "one more").with_pngs(&[vec![9]]));
    let again = fresh.snapshot().unwrap();
    assert_eq!(again.saved.messages[0].pngs, c.snapshot().unwrap().saved.messages[0].pngs);
    assert_eq!(again.saved.messages[2].pngs, [format!("{}/1.png", fresh.conv.id)]);
}

#[test]
fn a_reopened_conversation_keeps_its_steps_diffs_and_questions_and_one_nobody_answered_is_closed() {
    let dir = scratch("questions");
    let mut c = Chat::new(None);
    c.push(msg(1, "fix it"));
    let first = c.ask(1).unwrap();
    c.reply_event(first, Event::Step { id: 1, tool: reply::Tool::Read, target: "src/lib.rs".into() }, now());
    c.reply_event(first, Event::StepEnd { id: 1, ok: true }, now());
    c.reply_event(first, Event::Diff { path: "src/lib.rs".into(), patch: "-a\n+b\n".into() }, now());
    c.reply_event(first, Event::Ask { prompt: "Apply it?".into(), options: vec!["Yes".into(), "No".into()], multi: false }, now());
    c.reply_event(first, Event::Done, now());
    c.pick(first, 2, 0);
    let second = c.answer(first, 2).expect("answered");
    c.reply_event(second, Event::Text("Applied.".into()), now());
    c.reply_event(second, Event::Ask { prompt: "Anything else?".into(), options: vec!["Tests".into(), "No".into()], multi: false }, now());
    c.reply_event(second, Event::Done, now());
    let snapshot = c.snapshot().unwrap();
    history::write_snapshot(&dir, &snapshot).unwrap();

    let mut fresh = Chat::new(None);
    fresh.load(history::load(&dir, &snapshot.saved.id).expect("loads"));
    assert_eq!(fresh.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["fix it", "Yes"]);
    let asked = fresh.reply_to(fresh.messages[0].id).unwrap();
    let token = asked.token;
    match asked.parts.as_slice() {
        [reply::Part::Agent(a), reply::Part::Diff(d), reply::Part::Question(q)] => {
            assert_eq!((a.steps[0].target.as_str(), a.steps[0].status), ("src/lib.rs", reply::StepStatus::Done));
            assert_eq!((d.path.as_str(), d.added, d.removed), ("src/lib.rs", 1, 1));
            assert_eq!(q.answered.as_deref(), Some("Yes"));
        }
        _ => panic!("a step, a diff and a question"),
    }
    fresh.pick(token, 2, 1);
    assert_eq!(fresh.answer(token, 2), None, "answered before it was saved: it stays read only");
    // The model's process is a new one: nothing waits for the last question's answer.
    let last = fresh.reply_to(fresh.messages[1].id).unwrap().token;
    fresh.pick(last, 1, 0);
    assert_eq!(fresh.answer(last, 1), None, "the last reply's question is closed");
    assert!(matches!(&fresh.reply_to(fresh.messages[1].id).unwrap().parts[1], reply::Part::Question(q) if q.closed && q.picked.is_empty()));
    assert_eq!(fresh.messages.len(), 2, "no answer sent");
    assert!(!fresh.replies.iter().any(|r| r.streaming()));
}

// Titles.

#[test]
fn a_title_is_asked_once_after_the_first_reply_is_done() {
    let mut c = Chat::new(None);
    c.push(msg(1, "how do I brew green tea?"));
    let token = c.ask(1).unwrap();
    c.reply_event(token, Event::Text("Steep it at 80 degrees.".into()), now());
    assert!(c.title_asks.is_empty(), "not while it streams");
    c.reply_event(token, Event::Done, now());
    assert_eq!(c.title_asks.len(), 1);
    let ask = &c.title_asks[0];
    assert_eq!(ask.model, "claude-haiku-5-5");
    assert!(ask.text.contains("how do I brew green tea?") && ask.text.contains("Steep it at 80 degrees."), "{}", ask.text);
    assert!(ask.history.is_empty() && ask.session.is_none());
    exchange(&mut c, 2, "and black tea?", "Hotter.", Event::Done);
    assert_eq!(c.title_asks.len(), 1, "asked once");
}

#[test]
fn no_title_is_asked_while_the_first_reply_hasnt_come_or_when_there_is_one_already() {
    let mut c = Chat::new(None);
    exchange(&mut c, 1, "hi", "", Event::Fail("overloaded".into()));
    assert!(c.title_asks.is_empty(), "a failed reply isn't a first reply");
    let stopped = {
        c.push(msg(2, "hello?"));
        let t = c.ask(2).unwrap();
        c.reply_event(t, Event::Text("par".into()), now());
        c.stop(now());
        t
    };
    assert!(c.title_asks.is_empty(), "nor a stopped one: {stopped}");
    // The first message's reply retried to the end: now.
    let first = c.replies.iter().find(|r| r.id == 1).unwrap().token;
    let again = c.retry(first).unwrap();
    c.reply_event(again, Event::Text("Hello!".into()), now());
    c.reply_event(again, Event::Done, now());
    assert_eq!(c.title_asks.len(), 1);

    let mut titled = Chat::new(None);
    titled.conv.title = Some("Mine".into());
    exchange(&mut titled, 1, "hi", "hello", Event::Done);
    assert!(titled.title_asks.is_empty(), "it has a title");
    let mut typed = Chat::new(None);
    typed.conv.title_by_user = true;
    exchange(&mut typed, 1, "hi", "hello", Event::Done);
    assert!(typed.title_asks.is_empty(), "a name the user typed wins");
}

#[test]
fn the_title_that_arrives_is_kept_and_saved_but_never_over_a_title_or_the_users() {
    let mut c = Chat::new(None);
    exchange(&mut c, 1, "hi", "hello", Event::Done);
    let (id, saved) = (c.conv.id.clone(), c.saved.len());
    c.title_arrived(&id, "Greeting".into());
    assert_eq!(c.conv.title.as_deref(), Some("Greeting"));
    assert_eq!(c.saved.len(), saved + 1);
    assert_eq!(last_saved(&c).title.as_deref(), Some("Greeting"));
    c.title_arrived(&id, "Another".into());
    assert_eq!(c.conv.title.as_deref(), Some("Greeting"), "the first stays");

    let mut typed = Chat::new(None);
    exchange(&mut typed, 1, "hi", "hello", Event::Done);
    typed.conv.title_by_user = true;
    typed.conv.title = Some("Mine".into());
    let id = typed.conv.id.clone();
    typed.title_arrived(&id, "Auto".into());
    assert_eq!(typed.conv.title.as_deref(), Some("Mine"));
    let mut blank_by_user = Chat::new(None);
    exchange(&mut blank_by_user, 1, "hi", "hello", Event::Done);
    blank_by_user.conv.title_by_user = true;
    let id = blank_by_user.conv.id.clone();
    blank_by_user.title_arrived(&id, "Auto".into());
    assert_eq!(blank_by_user.conv.title, None, "title_by_user wins even with no title");
}

#[test]
fn a_title_for_a_conversation_the_chat_has_left_doesnt_touch_the_one_it_has() {
    let mut c = Chat::new(None);
    exchange(&mut c, 1, "hi", "hello", Event::Done);
    let old = c.conv.id.clone();
    c.clear();
    exchange(&mut c, 2, "tea?", "yes", Event::Done);
    let saved = c.saved.len();
    c.title_arrived(&old, "Greeting".into());
    assert_eq!(c.conv.title, None, "the new conversation has no title");
    assert_eq!(c.saved.len(), saved, "and isn't written for it");
}

// Show earlier: a long conversation opens at its latest page.

fn many(n: usize) -> Loaded {
    loaded("long", (0..n).map(|i| loaded_message(&format!("m{i}"), Some(done("ok")))).collect(), None)
}

#[test]
fn a_long_conversation_opens_at_its_latest_page_and_shows_earlier_a_page_at_a_time() {
    let mut c = Chat::new(None);
    c.load(many(PAGE * 2 + 5));
    assert_eq!(c.earlier(), PAGE + 5, "the last page shows");
    assert!(c.show_earlier());
    assert_eq!(c.earlier(), 5);
    assert!(c.show_earlier());
    assert_eq!(c.earlier(), 0, "the rest, though less than a page");
    assert!(!c.show_earlier(), "nothing earlier");

    let mut short = Chat::new(None);
    short.load(many(PAGE));
    assert_eq!(short.earlier(), 0, "a page or less shows whole");
}

#[test]
fn what_is_sent_after_a_long_conversation_opens_keeps_the_earlier_ones_hidden_and_a_new_one_has_none() {
    let mut c = Chat::new(None);
    c.load(many(PAGE + 3));
    exchange(&mut c, 900, "more", "yes", Event::Done);
    assert_eq!(c.earlier(), 3, "the new message shows below; the hidden ones stay so");
    let request = c.request_for(900, "fake").unwrap();
    assert_eq!(request.history.len(), PAGE + 3, "the model still gets the whole conversation");
    c.clear();
    assert_eq!(c.earlier(), 0);
    c.load(many(PAGE + 3));
    c.start_over();
    c.reset_conversation();
    assert_eq!(c.earlier(), 0, "a new conversation hides nothing");
}
