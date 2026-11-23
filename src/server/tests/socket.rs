use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use ailfred_protocol::command::{self, Command};
use serde_json::Value;

use super::super::hub::{Feed, Frame};
use super::super::routes::{Answer, Core};
use super::super::http::{FILE, forget};
use super::super::{Running, start, switch};
use super::empty;
use crate::reply::Event;

/// A core whose conversation is the hub alone: a run's message becomes reply stream 1.
pub(super) struct Fake(pub(super) Feed);

impl Core for Fake {
    fn join(&self, watcher: SyncSender<Frame>) {
        self.0.with(|hub| hub.join(watcher, &empty()));
    }

    fn command(&self, command: Command) -> Answer {
        let Command::Run(input) = command else { return Answer::error(409, "no") };
        self.0.with(|hub| hub.reply(1, 1, "hi", &[], Some(input.run_id.clone())));
        Answer::accepted(&input.thread_id, &input.run_id)
    }
}

/// A fresh folder for the app's files.
pub(super) fn scratch() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!("ailfred-server-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn started(dir: &Path) -> (Running, Feed) {
    let feed = Feed::default();
    feed.with(|hub| hub.conversation("t", &empty()));
    (start(Arc::new(Fake(feed.clone())), feed.clone(), dir).unwrap(), feed)
}

pub(super) fn connect(address: &str) -> TcpStream {
    let stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream
}

/// The next SSE `data:` line's JSON, past chunk sizes and blank lines; None at the body's end.
fn next_event(reader: &mut BufReader<TcpStream>) -> Option<Value> {
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 || line.trim_end() == "0" {
            return None;
        }
        if let Some(json) = line.trim_end().strip_prefix("data: ") {
            return Some(serde_json::from_str(json).unwrap());
        }
    }
}

#[test]
fn over_a_real_socket_a_client_gets_the_snapshot_runs_a_command_and_reads_its_events_in_order() {
    let dir = scratch();
    let (running, feed) = started(&dir);
    let file = dir.join(FILE);
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
    let found: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    let (url, token) = (found["url"].as_str().unwrap().to_owned(), found["token"].as_str().unwrap().to_owned());
    assert_eq!(url, running.url());
    assert_eq!((token.len(), found["pid"].as_u64()), (64, Some(u64::from(std::process::id()))));
    assert!(!format!("{running:?}").contains(&token), "never printed");
    let address = url.trim_start_matches("http://").to_owned();

    let mut foreign = connect(&address);
    write!(foreign, "GET /v1/events HTTP/1.1\r\nHost: evil.example:80\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n").unwrap();
    let mut refused = String::new();
    foreign.read_to_string(&mut refused).unwrap();
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");

    let mut events = connect(&address);
    write!(events, "GET /v1/events HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\n\r\n").unwrap();
    let mut reader = BufReader::new(events.try_clone().unwrap());
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(next_event(&mut reader).unwrap()["type"], "MESSAGES_SNAPSHOT");

    let body = serde_json::to_string(&command::send("t", "client1", &[], "hi", &[])).unwrap();
    let mut post = connect(&address);
    write!(post, "POST /v1/commands HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut answer = String::new();
    post.read_to_string(&mut answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 202"), "{answer}");
    let taken: Value = serde_json::from_str(answer.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!((taken["threadId"].as_str(), taken["runId"].as_str()), (Some("t"), Some("client1")));

    feed.with(|hub| {
        hub.event(1, &Event::Text("hello".into()), 1);
        hub.event(1, &Event::Done, 2);
    });
    let kinds: Vec<String> = (0..5).map(|_| next_event(&mut reader).unwrap()["type"].as_str().unwrap().to_owned()).collect();
    assert_eq!(kinds, ["RUN_STARTED", "TEXT_MESSAGE_START", "TEXT_MESSAGE_CONTENT", "TEXT_MESSAGE_END", "RUN_FINISHED"]);

    running.stop();
    assert!(!file.exists(), "the discovery file goes with it");
    assert!(next_event(&mut reader).is_none(), "the open stream ends");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_discovery_file_a_crash_left_behind_is_removed_at_launch() {
    let dir = scratch();
    forget(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(FILE), br#"{"url":"http://127.0.0.1:1","token":"old","pid":1}"#).unwrap();
    forget(&dir);
    assert!(!dir.join(FILE).exists(), "no client finds a server that's gone");
    assert!(dir.exists(), "only the file goes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_switch_starts_the_server_once_and_stops_it() {
    let dir = scratch();
    let mut running = None;
    switch(&mut running, true, || Ok(started(&dir).0));
    let url = running.as_ref().map(|r| r.url().to_owned()).expect("on");
    assert!(dir.join(FILE).exists());
    switch(&mut running, true, || panic!("started twice"));
    assert_eq!(running.as_ref().map(Running::url), Some(url.as_str()));
    switch(&mut running, false, || panic!("started to stop"));
    assert!(running.is_none() && !dir.join(FILE).exists(), "off: stopped, its file gone");
    let _ = std::fs::remove_dir_all(&dir);
}
