use std::sync::mpsc::Receiver;

use ailfred_protocol::saved::OUTPUT_MAX;

use super::super::hub::{Frame, Hub};
use super::super::routes::STREAM_FRAMES;
use super::{PNG, empty, got, local, tag, watcher};
use crate::reply::{Event, Tool};

/// A hub on thread `t`, watched.
fn watched() -> (Hub, Receiver<Frame>) {
    let mut hub = Hub::default();
    hub.conversation("t", &empty());
    let (tx, rx) = watcher();
    hub.join(tx, &empty());
    assert_eq!(got(&rx).iter().map(tag).collect::<Vec<_>>(), ["MESSAGES_SNAPSHOT "], "a joining client gets the conversation first");
    (hub, rx)
}

fn text(s: &str) -> Event {
    Event::Text(s.into())
}

#[test]
fn a_steered_pair_comes_out_one_run_whole_then_the_other() {
    let (mut hub, rx) = watched();
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    hub.reply(2, 11, "b", &[], Some("rb".into()));
    hub.event(1, &text("A1"), 1);
    // pi reads b while a still goes: b's Taken (and more) before a's Done.
    hub.event(2, &Event::Taken, 2);
    hub.event(2, &text("B1"), 3);
    hub.event(1, &text("A2"), 4);
    hub.event(1, &Event::Done, 5);
    hub.event(2, &Event::Done, 6);
    let tags: Vec<String> = got(&rx).iter().map(tag).collect();
    assert_eq!(
        tags,
        [
            "RUN_STARTED ra",
            "TEXT_MESSAGE_START ra",
            "TEXT_MESSAGE_CONTENT ra",
            "TEXT_MESSAGE_CONTENT ra",
            "TEXT_MESSAGE_END ra",
            "RUN_FINISHED ra",
            "RUN_STARTED rb",
            "TEXT_MESSAGE_START rb",
            "TEXT_MESSAGE_CONTENT rb",
            "TEXT_MESSAGE_END rb",
            "RUN_FINISHED rb",
        ]
    );
}

#[test]
fn a_run_started_carries_the_user_message_it_answers_with_its_captures() {
    let (mut hub, rx) = watched();
    hub.reply(1, 10, "look", &[PNG], Some("ra".into()));
    hub.event(1, &Event::Taken, 1);
    let events = got(&rx);
    let input = &events[0]["input"];
    assert_eq!((input["threadId"].as_str(), input["runId"].as_str()), (Some("t"), Some("ra")));
    let message = &input["messages"][0];
    assert_eq!((message["role"].as_str(), message["id"].as_str()), (Some("user"), Some("ra-user")));
    assert_eq!(message["content"][0]["text"].as_str(), Some("look"));
    assert_eq!(message["content"][1]["source"]["mimeType"].as_str(), Some("image/png"));
}

#[test]
fn a_new_conversation_sends_its_snapshot_and_starts_its_run_ids_afresh() {
    let (mut hub, rx) = watched();
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    hub.event(1, &text("A"), 1);
    assert!(hub.seen("ra"));
    hub.conversation("t2", &empty());
    let tags: Vec<String> = got(&rx).iter().map(tag).collect();
    assert_eq!(tags.last().map(String::as_str), Some("MESSAGES_SNAPSHOT "));
    assert_eq!(hub.thread(), "t2");
    assert!(!hub.seen("ra") && hub.live_messages().is_empty(), "nothing of the old one stays");
    hub.event(1, &text("late"), 2);
    assert!(got(&rx).is_empty(), "the old conversation's stream feeds nothing");
}

#[test]
fn a_client_joining_mid_run_gets_the_run_going_from_its_start_then_what_comes() {
    let (mut hub, early) = watched();
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    hub.reply(2, 11, "b", &[], Some("rb".into()));
    hub.event(1, &text("A1"), 1);
    hub.event(2, &Event::Taken, 2);
    assert_eq!(hub.live_messages(), [10, 11], "the snapshot leaves them out: they come as runs");
    let (tx, late) = watcher();
    hub.join(tx, &empty());
    hub.event(1, &Event::Done, 3);
    hub.event(2, &Event::Done, 4);
    let early: Vec<String> = got(&early).iter().map(tag).collect();
    let late: Vec<String> = got(&late).iter().map(tag).collect();
    assert_eq!(late[0], "MESSAGES_SNAPSHOT ");
    assert_eq!(late[1..], early[..], "the same runs, whole and in order");
}

#[test]
fn a_dropped_client_doesnt_stop_the_run_and_the_others_still_get_it() {
    let (mut hub, rx) = watched();
    let (tx, gone) = watcher();
    hub.join(tx, &empty());
    drop(gone);
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    hub.event(1, &text("A"), 1);
    assert!(hub.live("ra"), "still going");
    hub.event(1, &Event::Done, 2);
    assert!(!hub.live("ra"));
    assert_eq!(got(&rx).last().map(tag).as_deref(), Some("RUN_FINISHED ra"));
}

#[test]
fn stop_ends_the_run_cancelled_and_fail_and_lost_end_it_with_an_error() {
    let (mut hub, rx) = watched();
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    hub.event(1, &Event::Step { id: 1, tool: Tool::Run, target: "make".into() }, 1);
    hub.stop(&[1], 2);
    hub.reply(2, 11, "b", &[], Some("rb".into()));
    hub.event(2, &Event::Lost("gone".into()), 3);
    hub.reply(3, 12, "c", &[], Some("rc".into()));
    hub.event(3, &Event::Fail("broke".into()), 4);
    let events = got(&rx);
    let finished = events.iter().find(|e| e["type"] == "RUN_FINISHED").unwrap();
    assert_eq!(finished["outcome"]["type"].as_str(), Some("cancelled"));
    let errors: Vec<Option<&str>> = events.iter().filter(|e| e["type"] == "RUN_ERROR").map(|e| e["code"].as_str()).collect();
    assert_eq!(errors, [Some("ailfred:lost"), None]);
    assert!(hub.live_messages().is_empty());
}

#[test]
fn a_queued_reply_given_a_new_stream_is_still_its_run_and_a_retry_is_a_new_one() {
    let (mut hub, rx) = watched();
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    hub.reply(2, 11, "b", &[], Some("rb".into()));
    hub.event(1, &Event::Done, 1);
    // Claude: the queued one goes once a is done, on a stream of its own.
    hub.reply(3, 11, "b", &[], None);
    hub.event(3, &Event::Done, 2);
    assert_eq!(hub.run_of(3), Some("rb"));
    // Retried: a new run, its id minted, unique in the thread.
    hub.reply(4, 11, "b", &[], Some("rb".into()));
    hub.event(4, &Event::Done, 3);
    let retry = hub.run_of(4).unwrap().to_owned();
    assert!(retry != "rb" && retry.starts_with("t-"), "{retry}");
    let runs: Vec<String> = got(&rx).iter().filter(|e| e["type"] == "RUN_STARTED").map(|e| e["runId"].as_str().unwrap().to_owned()).collect();
    assert_eq!(runs, ["ra", "rb", retry.as_str()]);
}

#[test]
fn a_steps_result_on_the_stream_is_cut_to_its_last_64_kib() {
    let (mut hub, rx) = watched();
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    hub.event(1, &Event::Step { id: 1, tool: Tool::Run, target: "yes".into() }, 1);
    hub.event(1, &Event::StepOutput { id: 1, text: "y\n".repeat(40 * 1024) }, 2);
    hub.event(1, &Event::StepEnd { id: 1, ok: true }, 3);
    let events = got(&rx);
    let result = events.iter().find(|e| e["type"] == "TOOL_CALL_RESULT").unwrap();
    let content = result["content"].as_str().unwrap();
    assert!(content.len() <= OUTPUT_MAX && content.starts_with('…') && content.ends_with("y\n"), "{} bytes", content.len());
}

#[test]
fn every_frame_of_a_full_run_keeps_to_the_official_schema() {
    let (mut hub, rx) = watched();
    hub.reply(1, 10, "look", &[PNG], Some("ra".into()));
    for (at, e) in [
        Event::Think("hm".into()),
        text("Let me see."),
        Event::Step { id: 7, tool: Tool::Read, target: "a.rs".into() },
        Event::StepOutput { id: 7, text: "fn a() {}".into() },
        Event::StepEnd { id: 7, ok: false },
        Event::Diff { path: "a.rs".into(), patch: "-a\n+b\n".into() },
        Event::Ask { prompt: "Which?".into(), options: vec!["A".into(), "B".into()], multi: true },
        Event::Done,
    ]
    .iter()
    .enumerate()
    {
        hub.event(1, e, at as u64);
    }
    // `got` holds every frame to the schema.
    assert_eq!(got(&rx).len(), 17);
}

#[test]
fn the_web_view_is_never_dropped_however_much_comes_and_outlives_the_servers_streams() {
    let (mut hub, rx) = watched();
    let (sink, web) = local();
    hub.join_local(sink, &empty());
    assert_eq!(got(&web).iter().map(tag).collect::<Vec<_>>(), ["MESSAGES_SNAPSHOT "]);
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    for n in 0..2 * STREAM_FRAMES {
        hub.event(1, &text(&n.to_string()), n as u64);
    }
    assert_eq!(hub.watching(), 0, "the stream nobody read is dropped");
    drop(rx);
    assert_eq!(got(&web).len(), 2 + 2 * STREAM_FRAMES, "RUN_STARTED, TEXT_MESSAGE_START and every chunk");
    hub.close();
    hub.event(1, &Event::Done, 0);
    assert_eq!(got(&web).iter().map(tag).collect::<Vec<_>>(), ["TEXT_MESSAGE_END ra", "RUN_FINISHED ra"], "the server stopping doesn't end it");
    let (tx, late) = watcher();
    hub.join(tx, &empty());
    assert!(late.recv().is_err(), "closed, the server's clients don't join");
    hub.reopen();
    let (tx, again) = watcher();
    hub.join(tx, &empty());
    assert_eq!(got(&again).len(), 1, "started again, they do");
}

#[test]
fn the_web_view_joining_again_replaces_its_sink_and_leaving_ends_it() {
    let (mut hub, _rx) = watched();
    let (sink, old) = local();
    hub.join_local(sink, &empty());
    hub.reply(1, 10, "a", &[], Some("ra".into()));
    hub.event(1, &text("A"), 1);
    let (sink, new) = local();
    hub.join_local(sink, &empty());
    assert_eq!(old.try_iter().count(), 4, "the snapshot, then the run's three");
    assert!(matches!(old.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected)), "the old sink is gone");
    assert_eq!(got(&new).iter().map(tag).collect::<Vec<_>>(), ["MESSAGES_SNAPSHOT ", "RUN_STARTED ra", "TEXT_MESSAGE_START ra", "TEXT_MESSAGE_CONTENT ra"]);
    hub.leave_local();
    assert!(!hub.local_watching());
    assert!(new.recv().is_err(), "left, its sink is gone");
}

#[test]
fn closed_nobody_joins_and_every_stream_ends() {
    let (mut hub, rx) = watched();
    hub.close();
    assert!(rx.recv().is_err(), "the stream's sender is gone");
    let (tx, late) = watcher();
    hub.join(tx, &empty());
    assert!(late.recv().is_err());
}
