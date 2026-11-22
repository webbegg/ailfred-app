use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::store::write_atomic;
use super::*;
use crate::reply::{Event, Reply, State};

/// A fresh empty folder under the system's temp folder.
fn scratch(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("ailfred-history-{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn message(text: &str, reply: Option<SavedReply>) -> SavedMessage {
    SavedMessage { text: text.into(), pngs: Vec::new(), reply }
}

fn answer(text: &str) -> SavedReply {
    SavedReply { state: ReplyState::Done, text: text.into(), ..SavedReply::default() }
}

/// A conversation `id` updated at `updated`, with one message per text.
fn saved(id: &str, updated: u64, texts: &[&str]) -> Saved {
    Saved { version: VERSION, id: id.into(), created: updated, updated, messages: texts.iter().map(|t| message(t, Some(answer("ok")))).collect(), ..Saved::default() }
}

fn snapshot(saved: Saved) -> Snapshot {
    Snapshot { saved, pngs: Vec::new() }
}

fn put(dir: &Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

#[test]
fn a_leftover_loose_tmp_doesnt_make_the_conversation_readable_by_others() {
    let dir = scratch("loose-tmp");
    let id = "20261122-130000-abcd";
    put(&dir, &format!("{id}.json.tmp"), "stale");
    std::fs::set_permissions(dir.join(format!("{id}.json.tmp")), std::fs::Permissions::from_mode(0o644)).unwrap();
    store::write_snapshot(&dir, &snapshot(saved(id, 1, &["hi"]))).unwrap();
    assert_eq!(mode(&dir.join(format!("{id}.json"))), 0o600);
}

// Ids.

#[test]
fn an_id_is_the_utc_date_and_time_then_four_hex_digits() {
    assert_eq!(format_id(0, 0), "19700101-000000-0000");
    assert_eq!(format_id(1_700_000_000, 0xabcd), "20231114-221320-abcd");
    assert_eq!(format_id(1_709_208_000, 1), "20240229-120000-0001", "a leap day");
    assert_eq!(format_id(951_782_399, 0xffff), "20000228-235959-ffff", "the day before 2000's leap day");
}

#[test]
fn two_ids_made_in_the_same_second_differ_and_are_safe_file_names() {
    let ids: Vec<String> = (0..50).map(|_| new_id()).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    assert!(ids.iter().all(|id| safe_id(id) && id.len() == "20231114-221320-abcd".len()));
    assert!(!safe_id("../x") && !safe_id("a/b") && !safe_id("") && !safe_id(".hidden"));
}

// Writing and reading back.

#[test]
fn a_saved_conversation_loads_back_with_its_text_replies_captures_session_and_title() {
    let dir = scratch("round");
    let mut conversation = saved("20240101-000000-0001", 100, &[]);
    conversation.title = Some("A title".into());
    conversation.title_by_user = true;
    conversation.session = Some(SavedSession { id: "sess-1".into(), after_message: 1 });
    conversation.messages = vec![
        SavedMessage { text: "look".into(), pngs: vec!["20240101-000000-0001/0.png".into(), "20240101-000000-0001/1.png".into()], reply: Some(answer("I see")) },
        message("and?", Some(SavedReply { state: ReplyState::Failed, error: Some("overloaded".into()), ..SavedReply::default() })),
    ];
    let pngs = vec![
        ("20240101-000000-0001/0.png".to_owned(), Arc::new(b"\x89PNG-zero".to_vec())),
        ("20240101-000000-0001/1.png".to_owned(), Arc::new(b"\x89PNG-one".to_vec())),
    ];
    write_snapshot(&dir, &Snapshot { saved: conversation.clone(), pngs }).unwrap();

    let loaded = load(&dir, "20240101-000000-0001").expect("it loads");
    assert_eq!((loaded.title.as_deref(), loaded.title_by_user, loaded.created), (Some("A title"), true, 100));
    assert_eq!(loaded.session, Some(("sess-1".into(), 1)));
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.messages[0].text, "look");
    assert_eq!(*loaded.messages[0].pngs[0], b"\x89PNG-zero".to_vec(), "the captures come back by their paths");
    assert_eq!(*loaded.messages[0].pngs[1], b"\x89PNG-one".to_vec());
    assert_eq!(loaded.messages[0].files, ["20240101-000000-0001/0.png", "20240101-000000-0001/1.png"]);
    assert_eq!(loaded.messages[0].reply, Some(answer("I see")));
    assert_eq!(loaded.messages[1].reply.as_ref().unwrap().error.as_deref(), Some("overloaded"));
    assert_eq!(loaded.next_png, 2, "past the numbers in use");
    let file = std::fs::read_to_string(dir.join("20240101-000000-0001.json")).unwrap();
    assert!(file.contains("\"pngs\"") && file.contains("20240101-000000-0001/0.png") && !file.contains("PNG-zero"), "captures are referenced, not embedded: {file}");
}

#[test]
fn the_folders_are_owner_only_and_so_are_the_files() {
    let dir = scratch("modes").join("Library/Application Support/com.ailfred.test/conversations");
    let id = "20240101-000000-0002";
    let pngs = vec![(format!("{id}/0.png"), Arc::new(vec![1, 2, 3]))];
    write_snapshot(&dir, &Snapshot { saved: saved(id, 1, &["hi"]), pngs }).unwrap();
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join(id)), 0o700);
    assert_eq!(mode(&dir.join(format!("{id}.json"))), 0o600);
    assert_eq!(mode(&dir.join(id).join("0.png")), 0o600);
    // A folder that was left open is closed up on the next save.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    write_snapshot(&dir, &snapshot(saved(id, 2, &["hi"]))).unwrap();
    assert_eq!(mode(&dir), 0o700);
}

#[test]
fn a_capture_already_on_disk_isnt_written_again() {
    let dir = scratch("once");
    let id = "20240101-000000-0003";
    let first = vec![(format!("{id}/0.png"), Arc::new(vec![1]))];
    write_snapshot(&dir, &Snapshot { saved: saved(id, 1, &["hi"]), pngs: first }).unwrap();
    let again = vec![(format!("{id}/0.png"), Arc::new(vec![9, 9]))];
    write_snapshot(&dir, &Snapshot { saved: saved(id, 2, &["hi"]), pngs: again }).unwrap();
    assert_eq!(std::fs::read(dir.join(id).join("0.png")).unwrap(), vec![1]);
}

#[test]
fn a_write_goes_to_a_tmp_file_and_is_renamed_so_no_tmp_is_left() {
    let dir = scratch("atomic");
    let path = dir.join("a.json");
    write_atomic(&path, b"first").unwrap();
    write_atomic(&path, b"second").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"second");
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(names, ["a.json"], "no partial file beside it");
}

#[test]
fn a_write_that_fails_leaves_what_was_there_whole_and_no_tmp() {
    let dir = scratch("atomic-fail");
    // The tmp name is taken by a folder: the write can't even start.
    let path = dir.join("a.json");
    write_atomic(&path, b"old").unwrap();
    std::fs::create_dir(dir.join("a.json.tmp")).unwrap();
    assert!(write_atomic(&path, b"new").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"old", "the old file is whole");
    std::fs::remove_dir(dir.join("a.json.tmp")).unwrap();
    // The rename fails (a folder is in the way): the tmp it wrote is cleaned up.
    let blocked = dir.join("b.json");
    std::fs::create_dir_all(blocked.join("inside")).unwrap();
    assert!(write_atomic(&blocked, b"new").is_err());
    assert!(!dir.join("b.json.tmp").exists(), "no partial file left behind");
}

#[test]
fn a_tmp_file_left_by_a_crash_is_not_a_conversation() {
    let dir = scratch("stale");
    write_snapshot(&dir, &snapshot(saved("20240101-000000-0004", 5, &["hi"]))).unwrap();
    put(&dir, "20240101-000000-0005.json.tmp", r#"{"id":"20240101-000000-0005","messages":[{"text":"half"#);
    let ids: Vec<String> = scan(&dir).into_iter().map(|e| e.id).collect();
    assert_eq!(ids, ["20240101-000000-0004"]);
}

#[test]
fn a_damaged_field_falls_back_to_its_default_and_the_rest_of_the_file_reads() {
    let dir = scratch("damaged");
    let id = "20240101-000000-0006";
    put(
        &dir,
        &format!("{id}.json"),
        r#"{"version":1,"id":"x","created":"long ago","updated":50,"title":5,"title_by_user":"yes",
            "session":{"id":"s","after_message":"first"},
            "messages":[{"text":"hi","pngs":["gone/0.png",3],"reply":{"state":"weird","text":"ok","thinking":7}}, 3, {"text":"again"}]}"#,
    );
    let loaded = load(&dir, id).expect("the file still reads");
    assert_eq!(loaded.id, id, "the file's name is its id, whatever the file says");
    assert_eq!((loaded.created, loaded.title, loaded.title_by_user), (0, None, false));
    assert_eq!(loaded.session, Some(("s".into(), 0)), "the session's damaged field alone defaults");
    assert_eq!(loaded.messages.len(), 2, "the message that isn't one is dropped");
    assert_eq!(loaded.messages[0].text, "hi");
    assert!(loaded.messages[0].pngs.is_empty(), "a capture that's gone drops alone");
    let reply = loaded.messages[0].reply.as_ref().unwrap();
    assert_eq!((reply.state, reply.text.as_str(), reply.thinking.as_deref()), (ReplyState::Stopped, "ok", None));
    assert_eq!(loaded.messages[1].text, "again");
}

#[test]
fn a_file_that_wont_parse_is_skipped_and_never_deleted() {
    let dir = scratch("broken");
    put(&dir, "20240101-000000-0007.json", "not json {");
    put(&dir, "20240101-000000-0008.json", "[1, 2]");
    put(&dir, "20240101-000000-0009.json", r#"{"version":99,"id":"x","messages":[{"text":"from the future"}]}"#);
    write_snapshot(&dir, &snapshot(saved("20240101-000000-0010", 5, &["fine"]))).unwrap();
    let ids: Vec<String> = scan(&dir).into_iter().map(|e| e.id).collect();
    assert_eq!(ids, ["20240101-000000-0010"]);
    assert!(load(&dir, "20240101-000000-0007").is_none());
    assert_eq!(std::fs::read_to_string(dir.join("20240101-000000-0007.json")).unwrap(), "not json {", "left as it was");
    assert!(dir.join("20240101-000000-0008.json").exists() && dir.join("20240101-000000-0009.json").exists());
}

#[test]
fn a_path_in_a_file_that_leaves_the_conversations_folder_is_never_read() {
    let dir = scratch("paths");
    let outside = dir.join("secret.txt");
    put(&dir, "secret.txt", "private");
    let id = "20240101-000000-0011";
    put(
        &dir,
        &format!("{id}.json"),
        &format!(r#"{{"messages":[{{"text":"hi","pngs":["../secret.txt","{}","./","{id}/../secret.txt"]}}]}}"#, outside.display()),
    );
    let loaded = load(&dir, id).unwrap();
    assert!(loaded.messages[0].pngs.is_empty());
    assert!(load(&dir, "../secret").is_none(), "an id is a file name, not a path");
}

#[test]
fn the_most_recently_updated_conversation_comes_first_and_one_without_messages_is_left_out() {
    let dir = scratch("latest");
    for (id, updated) in [("20240101-000000-0012", 10), ("20240101-000000-0013", 30), ("20240101-000000-0014", 20)] {
        write_snapshot(&dir, &snapshot(saved(id, updated, &["hi"]))).unwrap();
    }
    write_snapshot(&dir, &snapshot(saved("20240101-000000-0015", 99, &[]))).unwrap();
    let found = scan(&dir);
    let ids: Vec<&str> = found.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["20240101-000000-0013", "20240101-000000-0014", "20240101-000000-0012"]);
    assert_eq!(found[0].updated, 30);
    assert!(load(&dir, &found[0].id).is_some(), "launch restores it");
    assert!(scan(&dir.join("nothing here")).is_empty(), "no folder yet: nothing");
}

#[test]
fn a_file_without_a_time_takes_the_one_it_was_last_changed() {
    let dir = scratch("mtime");
    put(&dir, "20240101-000000-0016.json", r#"{"messages":[{"text":"hi"}]}"#);
    assert!(scan(&dir)[0].updated > 1_700_000_000);
}

// The menu's list.

fn entry(id: &str, updated: u64) -> Entry {
    Entry { id: id.into(), label: id.into(), updated }
}

#[test]
fn the_recent_list_is_newest_first_and_holds_ten() {
    let entries: Vec<Entry> = (0..12).map(|n| entry(&format!("c{n:02}"), n)).collect();
    let top = recent(entries, RECENT);
    assert_eq!(top.len(), 10);
    assert_eq!(top.first().unwrap().id, "c11");
    assert_eq!(top.last().unwrap().id, "c02", "the two oldest are out");
    let tied = recent(vec![entry("a", 5), entry("b", 5)], 10);
    let ids: Vec<&str> = tied.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["b", "a"], "the later id first when the times tie");
}

#[test]
fn a_conversation_is_listed_by_its_title_else_the_start_of_its_first_message() {
    assert_eq!(label(Some("Fixing the Dock"), "whatever"), "Fixing the Dock");
    assert_eq!(label(None, "hello there"), "hello there");
    assert_eq!(label(Some("  "), "hello"), "hello", "a blank title is none");
    let long = "a".repeat(60);
    assert_eq!(label(None, &long), format!("{}…", "a".repeat(40)));
    assert_eq!(label(None, &"b".repeat(40)), "b".repeat(40), "forty fit whole");
    assert_eq!(label(None, "two\n\nlines   here"), "two lines here");
    assert_eq!(label(None, ""), "Screenshot", "a capture alone");
    assert_eq!(label(None, &"é".repeat(50)), format!("{}…", "é".repeat(40)), "by characters, not bytes");
}

#[test]
fn an_automatic_title_goes_into_the_file_only_where_there_is_none_and_the_user_didnt_write_one() {
    let dir = scratch("title");
    let (plain, titled, by_user) = ("20240101-000000-0017", "20240101-000000-0018", "20240101-000000-0019");
    write_snapshot(&dir, &snapshot(saved(plain, 1, &["hi"]))).unwrap();
    write_snapshot(&dir, &snapshot(Saved { title: Some("Mine".into()), ..saved(titled, 1, &["hi"]) })).unwrap();
    write_snapshot(&dir, &snapshot(Saved { title_by_user: true, ..saved(by_user, 1, &["hi"]) })).unwrap();
    let entry = patch_title(&dir, plain, "Auto").unwrap().expect("changed");
    assert_eq!((entry.id.as_str(), entry.label.as_str()), (plain, "Auto"));
    assert_eq!(load(&dir, plain).unwrap().title.as_deref(), Some("Auto"));
    assert!(patch_title(&dir, titled, "Auto").unwrap().is_none());
    assert!(patch_title(&dir, by_user, "Auto").unwrap().is_none());
    assert_eq!(load(&dir, titled).unwrap().title.as_deref(), Some("Mine"));
    assert_eq!(load(&dir, by_user).unwrap().title, None);
    assert!(patch_title(&dir, "20240101-000000-0099", "Auto").unwrap().is_none(), "no such conversation");
}

// The writer thread.

fn listening(dir: &Path) -> (Writer, Arc<Mutex<Vec<Vec<String>>>>) {
    let heard = Arc::new(Mutex::new(Vec::new()));
    let sink = heard.clone();
    let writer = Writer::start(dir.to_path_buf(), move |list| sink.lock().unwrap().push(list.into_iter().map(|e| e.label).collect()));
    (writer, heard)
}

#[test]
fn the_writer_writes_in_order_off_the_callers_thread_and_reports_the_recent_list_when_it_changes() {
    let dir = scratch("writer");
    let (writer, heard) = listening(&dir);
    let started = Instant::now();
    for n in 0..12u64 {
        let id = format!("20240101-000000-{n:04}");
        writer.save(snapshot(saved(&id, n + 1, &[&format!("message {n}")])));
    }
    // Saved again: the later one is what stays.
    writer.save(snapshot(saved("20240101-000000-0003", 50, &["message three, again"])));
    assert!(started.elapsed() < Duration::from_secs(1), "saving only queues");
    assert!(writer.flush(Duration::from_secs(5)));
    assert_eq!(scan(&dir).len(), 12);
    assert_eq!(load(&dir, "20240101-000000-0003").unwrap().messages[0].text, "message three, again");
    let last = heard.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.len(), 10, "ten at most");
    assert_eq!(last[0], "message three, again", "the one saved last is first");
    let count = heard.lock().unwrap().len();
    writer.save(snapshot(saved("20240101-000000-0003", 50, &["message three, again"])));
    writer.flush(Duration::from_secs(5));
    assert_eq!(heard.lock().unwrap().len(), count, "an unchanged list is not announced again");
}

#[test]
fn what_was_on_disk_at_launch_joins_the_list_without_replacing_what_was_saved_since() {
    let dir = scratch("seed");
    let (writer, heard) = listening(&dir);
    writer.save(snapshot(Saved { title: Some("New".into()), ..saved("20240101-000000-0020", 90, &["hi"]) }));
    writer.seed(vec![Entry { id: "20240101-000000-0020".into(), label: "stale".into(), updated: 1 }, Entry { id: "20240101-000000-0021".into(), label: "Old".into(), updated: 5 }]);
    assert!(writer.flush(Duration::from_secs(5)));
    assert_eq!(heard.lock().unwrap().last().unwrap(), &["New", "Old"]);
}

#[test]
fn a_title_for_a_conversation_the_chat_has_left_goes_to_its_file_and_the_list() {
    let dir = scratch("late-title");
    let (writer, heard) = listening(&dir);
    writer.save(snapshot(saved("20240101-000000-0022", 9, &["tell me about tea"])));
    writer.set_title("20240101-000000-0022".into(), "Tea".into());
    assert!(writer.flush(Duration::from_secs(5)));
    assert_eq!(load(&dir, "20240101-000000-0022").unwrap().title.as_deref(), Some("Tea"));
    assert_eq!(heard.lock().unwrap().last().unwrap(), &["Tea"]);
}

// A reply, saved and back.

fn streaming_reply() -> Reply {
    let mut reply = Reply::new(7, "why?");
    reply.token = 3;
    let now = Instant::now();
    reply.apply(Event::Think("hmm".into()), now);
    reply.apply(Event::Text("Because".into()), now);
    reply.apply(Event::Text(" of tea.".into()), now);
    reply
}

#[test]
fn a_reply_still_streaming_is_saved_interrupted_and_loads_as_stopped_with_its_text_and_the_note() {
    let saved = saved_reply(&streaming_reply());
    assert_eq!(saved.state, ReplyState::Interrupted);
    assert_eq!((saved.thinking.as_deref(), saved.text.as_str()), (Some("hmm"), "Because of tea."));
    let json = serde_json::to_string(&saved).unwrap();
    let back: SavedReply = serde_json::from_str(&json).unwrap();
    let reply = reply_from(&back, 11, "why?", 5);
    assert_eq!((reply.state, reply.id, reply.token), (State::Stopped, 11, 5));
    assert_eq!(reply.history_text().as_deref(), Some("Because of tea."), "later messages still send what it said");
    let notes: Vec<&str> = reply.parts.iter().filter_map(|p| if let crate::reply::Part::Error(e) = p { Some(e.as_str()) } else { None }).collect();
    assert_eq!(notes, ["Interrupted: Ailfred quit before the reply finished."]);
    assert!(!reply.streaming(), "nothing streams after a launch");
}

#[test]
fn done_stopped_and_failed_replies_come_back_as_they_were() {
    let now = Instant::now();
    let mut done = streaming_reply();
    done.apply(Event::Done, now);
    let mut stopped = streaming_reply();
    stopped.stop(now);
    let mut failed = streaming_reply();
    failed.apply(Event::Fail("overloaded".into()), now);
    for (reply, state) in [(done, State::Done), (stopped, State::Stopped), (failed, State::Failed)] {
        let back = reply_from(&saved_reply(&reply), 1, "why?", 2);
        assert_eq!(back.state, state);
        assert_eq!(back.copy_text(), reply.copy_text());
        assert_eq!(back.history_text(), reply.history_text());
    }
    let failed = reply_from(&saved_reply(&{
        let mut r = Reply::new(1, "x");
        r.apply(Event::Fail("overloaded".into()), now);
        r
    }), 1, "x", 1);
    assert!(matches!(failed.parts.last(), Some(crate::reply::Part::Error(e)) if e == "overloaded"));
}
