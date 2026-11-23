//! The server's limits over a real socket: what a client without the token, a slow client, or
//! too many of them, can't do to the others.

use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ailfred_protocol::agui::{self, Base};
use ailfred_protocol::command::{self, Command, user_message};
use serde_json::Value;

use super::super::Running;
use super::super::http::{FILE, Limits, MAX_CONNECTIONS, start_with};
use super::super::hub::{Feed, Frame};
use super::super::routes::{Answer, Core, MAX_STREAMS};
use super::empty;
use super::socket::{Fake, connect, scratch};

/// The longest head, in bytes.
const HEAD_MAX: usize = 16 << 10;

/// Limits short enough for a test.
const QUICK: Limits = Limits { first: Duration::from_millis(500), head: Duration::from_millis(500), body: Duration::from_secs(2), write: Duration::from_millis(300) };

/// A server on a real port: its hub, where it is and its token. Dropped, it stops.
struct Served {
    running: Option<Running>,
    feed: Feed,
    address: String,
    token: String,
    dir: PathBuf,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.running.take();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn served(limits: Limits) -> Served {
    let feed = Feed::default();
    served_with(limits, Arc::new(Fake(feed.clone())), feed)
}

/// A server of `core`, its streams watching `feed`.
fn served_with(limits: Limits, core: Arc<dyn Core>, feed: Feed) -> Served {
    let dir = scratch();
    feed.with(|hub| hub.conversation("t", &empty()));
    let running = start_with(core, feed.clone(), &dir, limits).unwrap();
    let found: Value = serde_json::from_slice(&std::fs::read(dir.join(FILE)).unwrap()).unwrap();
    let address = running.url().trim_start_matches("http://").to_owned();
    Served { running: Some(running), feed, address, token: found["token"].as_str().unwrap().into(), dir }
}

impl Served {
    /// A request with the token, for a path there isn't: answered 404 at once.
    fn valid(&self) -> String {
        format!("GET /v1/nothing HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\r\n", self.address, self.token)
    }

    /// Opening the event stream, with the token.
    fn events(&self) -> String {
        format!("GET /v1/events HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\n\r\n", self.address, self.token)
    }

    /// A command without the token, saying a body of `length` bytes comes (it never does).
    fn tokenless(&self, length: &str) -> String {
        format!("POST /v1/commands HTTP/1.1\r\nHost: {}\r\nContent-Length: {length}\r\n\r\n", self.address)
    }

    /// A new connection that sent `request`.
    fn send(&self, request: &str) -> TcpStream {
        let mut stream = connect(&self.address);
        stream.write_all(request.as_bytes()).unwrap();
        stream
    }

    fn watching(&self) -> usize {
        self.feed.with(|hub| hub.watching())
    }
}

/// Everything the server sends until it closes, if it closes within `wait`.
fn until_closed(stream: &mut TcpStream, wait: Duration) -> io::Result<String> {
    stream.set_read_timeout(Some(wait))?;
    let mut got = Vec::new();
    stream.read_to_end(&mut got)?;
    Ok(String::from_utf8_lossy(&got).into_owned())
}

/// The answer's status line, if it comes within `wait`.
fn first_line(stream: &mut TcpStream, wait: Duration) -> io::Result<String> {
    stream.set_read_timeout(Some(wait))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(line)
}

fn status(answer: &str) -> &str {
    answer.lines().next().unwrap_or_default()
}

/// The server closed the connection: an end, or a reset.
fn closed(got: &io::Result<String>) -> bool {
    match got {
        Ok(_) => true,
        Err(e) => e.kind() == ErrorKind::ConnectionReset,
    }
}

/// `f` holds within `wait`, tried every 50 ms.
fn eventually(wait: Duration, mut f: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    loop {
        if f() {
            return true;
        }
        if started.elapsed() > wait {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn without_the_token_a_huge_content_length_is_refused_at_once_and_the_server_goes_on() {
    let s = served(Limits::DEFAULT);
    let started = Instant::now();
    let mut huge = s.send(&s.tokenless("4611686018427387904"));
    let answer = until_closed(&mut huge, Duration::from_secs(2)).expect("answered and closed");
    assert!(status(&answer).starts_with("HTTP/1.1 401"), "{answer}");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    let mut next = s.send(&s.valid());
    let served = until_closed(&mut next, Duration::from_secs(1)).expect("the next request is served");
    assert!(status(&served).starts_with("HTTP/1.1 404"), "{served}");
}

#[test]
fn without_the_token_a_body_that_never_comes_holds_up_no_one() {
    let s = served(Limits::DEFAULT);
    let mut stalled = s.send(&s.tokenless("100000"));
    let started = Instant::now();
    let mut other = s.send(&s.valid());
    let answer = first_line(&mut other, Duration::from_secs(1)).expect("served meanwhile");
    assert!(answer.starts_with("HTTP/1.1 404") && started.elapsed() < Duration::from_secs(1), "{answer:?} in {:?}", started.elapsed());
    let refused = until_closed(&mut stalled, Duration::from_secs(2)).expect("refused and closed, its body unread");
    assert!(status(&refused).starts_with("HTTP/1.1 401"), "{refused}");
}

#[test]
fn a_head_sent_a_byte_at_a_time_is_cut_off_when_the_heads_time_is_up() {
    let s = served(QUICK);
    let mut slow = connect(&s.address);
    let mut writer = slow.try_clone().unwrap();
    let head = format!("GET /v1/events HTTP/1.1\r\nHost: {}\r\nX-Slow: {}", s.address, "a".repeat(100));
    let started = Instant::now();
    let dripping = std::thread::spawn(move || {
        for byte in head.bytes() {
            if writer.write_all(&[byte]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let cut = until_closed(&mut slow, Duration::from_secs(3));
    let after = started.elapsed();
    assert!(closed(&cut), "still open after {after:?}: {cut:?}");
    assert!(after >= QUICK.head && after < QUICK.head * 3, "{after:?}");
    dripping.join().unwrap();
}

#[test]
fn a_connection_that_sends_nothing_is_closed_after_the_first_bytes_time_and_its_place_freed() {
    let limits = Limits { first: Duration::from_millis(200), ..Limits::DEFAULT };
    let s = served(limits);
    let started = Instant::now();
    let mut silent: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| connect(&s.address)).collect();
    for socket in &mut silent {
        let got = until_closed(socket, Duration::from_secs(3));
        assert!(closed(&got), "still open after {:?}: {got:?}", started.elapsed());
    }
    let after = started.elapsed();
    assert!(after >= limits.first && after < limits.head, "closed after {after:?}, not the head's {:?}", limits.head);
    let mut next = s.send(&s.valid());
    let served = first_line(&mut next, Duration::from_secs(1)).expect("their places came free");
    assert!(served.starts_with("HTTP/1.1 404"), "{served:?}");
}

#[test]
fn a_stream_client_reading_a_large_frame_slowly_is_dropped_after_about_the_write_limit() {
    let s = served(QUICK);
    let mut slow = s.send(&s.events());
    assert!(first_line(&mut slow, Duration::from_secs(2)).is_ok_and(|l| l.starts_with("HTTP/1.1 200")));
    assert!(eventually(Duration::from_secs(2), || s.watching() == 1), "joined");
    let done = Arc::new(AtomicBool::new(false));
    let reading = {
        let done = done.clone();
        let mut slow = slow.try_clone().unwrap();
        std::thread::spawn(move || {
            let mut chunk = vec![0u8; 64 << 10];
            slow.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            while !done.load(Ordering::SeqCst) {
                match slow.read(&mut chunk) {
                    Ok(0) => return,
                    Ok(_) => {}
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                    Err(_) => return,
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };
    // Some of it well within each write's limit, but 8 MB at about 640 KB/s takes seconds.
    let big = agui::Event::MessagesSnapshot { messages: vec![user_message("m", &"x".repeat(8 << 20), &[])], base: Base::at(0) };
    let started = Instant::now();
    s.feed.with(|hub| hub.conversation("t", &big));
    let dropped = eventually(Duration::from_secs(5), || {
        s.feed.with(|hub| hub.conversation("t", &empty()));
        s.watching() == 0
    });
    let after = started.elapsed();
    done.store(true, Ordering::SeqCst);
    assert!(dropped, "still streaming after {after:?}");
    assert!(after >= QUICK.write && after < QUICK.write * 10, "dropped after {after:?}");
    reading.join().unwrap();
}

/// A core that keeps the commands it was handed, and answers 202.
#[derive(Default)]
struct Keeping(Mutex<Vec<Command>>);

impl Core for Keeping {
    fn join(&self, _: SyncSender<Frame>) {}

    fn command(&self, command: Command) -> Answer {
        self.0.lock().unwrap().push(command);
        Answer::accepted("t", "r")
    }
}

#[test]
fn a_body_that_finishes_after_the_server_went_off_and_on_again_is_503_and_never_applied() {
    let core = Arc::new(Keeping::default());
    let mut s = served_with(QUICK, core.clone(), Feed::default());
    let body = serde_json::to_string(&command::send("t", "c1", &[], "hi", &[])).unwrap();
    let (start, end) = body.split_at(body.len() / 2);
    let head = format!("POST /v1/commands HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{start}", s.address, s.token, body.len());
    let mut slow = s.send(&head);
    std::thread::sleep(Duration::from_millis(100));
    s.running.take().expect("on").stop();
    let again = served_with(QUICK, core.clone(), Feed::default());
    slow.write_all(end.as_bytes()).unwrap();
    let answer = until_closed(&mut slow, Duration::from_secs(3)).expect("answered");
    assert!(status(&answer).starts_with("HTTP/1.1 503"), "{answer}");
    assert!(core.0.lock().unwrap().is_empty(), "never applied");
    drop(again);
}

#[test]
fn a_connection_past_the_cap_is_closed_at_once() {
    let s = served(Limits::DEFAULT);
    let held: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| connect(&s.address)).collect();
    let mut extra = connect(&s.address);
    let started = Instant::now();
    let got = until_closed(&mut extra, Duration::from_secs(2));
    assert!(closed(&got) && got.as_ref().map_or(true, String::is_empty), "{got:?}");
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    drop(held);
    let free = eventually(Duration::from_secs(2), || first_line(&mut s.send(&s.valid()), Duration::from_millis(500)).is_ok_and(|l| l.starts_with("HTTP/1.1 404")));
    assert!(free, "their places come free once they're gone");
}

#[test]
fn a_head_over_16_kib_gets_431() {
    let s = served(Limits::DEFAULT);
    let start = format!("GET /v1/events HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nX-Pad: ", s.address, s.token);
    // 16 KiB and still no blank line (all of it read: closing on bytes unread would reset).
    let mut big = s.send(&format!("{start}{}", "a".repeat(HEAD_MAX - start.len())));
    let answer = until_closed(&mut big, Duration::from_secs(2)).expect("answered and closed");
    assert!(status(&answer).starts_with("HTTP/1.1 431"), "{answer}");
}

#[test]
fn a_chunked_body_gets_501() {
    let s = served(Limits::DEFAULT);
    let mut chunked = s.send(&format!("POST /v1/commands HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nTransfer-Encoding: chunked\r\n\r\n", s.address, s.token));
    let answer = until_closed(&mut chunked, Duration::from_secs(2)).expect("answered and closed");
    assert!(status(&answer).starts_with("HTTP/1.1 501"), "{answer}");
}

#[test]
fn after_a_refusal_the_connection_ends_and_a_request_behind_it_gets_nothing() {
    let s = served(Limits::DEFAULT);
    let mut refused = s.send(&format!("GET /v1/events HTTP/1.1\r\nHost: {}\r\n\r\n{}", s.address, s.valid()));
    let answer = until_closed(&mut refused, Duration::from_secs(2)).expect("the socket reaches its end");
    assert!(status(&answer).starts_with("HTTP/1.1 401"), "{answer}");
    assert!(answer.contains("Connection: close\r\n"), "{answer}");
    assert_eq!(answer.matches("HTTP/1.1 ").count(), 1, "no keep-alive, no pipelining: {answer}");
}

#[test]
fn a_stream_client_that_never_reads_is_dropped_its_place_freed_and_a_new_one_joins() {
    let s = served(QUICK);
    let deaf: Vec<TcpStream> = (0..MAX_STREAMS).map(|_| s.send(&s.events())).collect();
    assert!(eventually(Duration::from_secs(2), || s.watching() == MAX_STREAMS), "all joined");
    let big = agui::Event::MessagesSnapshot { messages: vec![user_message("m", &"x".repeat(32 << 10), &[])], base: Base::at(0) };
    let mut sent = 0;
    while s.watching() > 0 && sent < 1000 {
        s.feed.with(|hub| hub.conversation("t", &big));
        sent += 1;
    }
    assert_eq!(s.watching(), 0, "after {sent} frames, the hub let go of clients that don't read");
    let joined = eventually(Duration::from_secs(3), || first_line(&mut s.send(&s.events()), Duration::from_millis(500)).is_ok_and(|l| l.starts_with("HTTP/1.1 200")));
    assert!(joined, "their places came free: a new stream joins");
    drop(deaf);
}

#[test]
fn switched_off_its_port_refuses_connections_even_with_a_request_stalled() {
    let mut s = served(Limits::DEFAULT);
    let _idle = connect(&s.address);
    let mut stalled = s.send(&s.tokenless("100000"));
    let refused = first_line(&mut stalled, Duration::from_secs(2)).expect("refused");
    assert!(refused.starts_with("HTTP/1.1 401"), "{refused:?}");
    s.running.take().expect("on").stop();
    let after = TcpStream::connect(&s.address);
    assert!(after.as_ref().is_err_and(|e| e.kind() == ErrorKind::ConnectionRefused), "{after:?}");
}
