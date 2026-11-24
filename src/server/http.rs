//! The sockets: a small HTTP/1.1 server of our own on `127.0.0.1:0` (on the connection layer,
//! `conn`), and the discovery file.
//!
//! - One thread accepts, and never serves: each connection gets a thread of its own, at most
//!   `MAX_CONNECTIONS` at once (one more is closed at once).
//! - Its first byte comes within `Limits::first`, else it's closed; the head within
//!   `Limits::head` in all, at most 16 KiB (`head`). The guards run on it alone; a refusal is
//!   written and the connection closed, its body never read.
//! - A body let in is read as it comes, up to its `Content-Length`, within `Limits::body`. A
//!   command whose body comes after the server stopped is never handed to the chat (503).
//! - Every answer says `Connection: close`, and the connection closes after it: no keep-alive,
//!   no pipelining.
//! - Each answer, and each frame of the event stream, is written within `Limits::write` in all;
//!   a stream pings after `PING_EVERY` without a frame. A client too slow is dropped.

use std::io::{self, ErrorKind, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::json;

pub use super::conn::Limits;
use super::conn::{self, Body, Plan, write_within};
use super::hub::{Feed, Frame, PING};
use super::routes::{Answer, Core, Gate, Outcome, Permit, Route, serve};
use crate::history;

/// What the discovery file is called, in the app's folder.
pub const FILE: &str = "server.json";
/// How long a stream waits, with nothing to send, before it pings (and so notices a client
/// that's gone).
pub const PING_EVERY: Duration = Duration::from_secs(15);
/// Connections open at once; one more is closed as it comes.
pub const MAX_CONNECTIONS: usize = 32;
/// How often a waiting stream looks whether the server stopped.
const LOOK: Duration = Duration::from_millis(200);

/// The server running: stopping it (or dropping it) closes its port, takes no new request,
/// ends the streams and removes the discovery file.
pub struct Running {
    url: String,
    stopped: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
    /// The folder of its discovery file.
    dir: PathBuf,
    feed: Feed,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running").field("url", &self.url).finish_non_exhaustive()
    }
}

impl Running {
    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Stops it (dropping it does the same).
    pub fn stop(self) {}
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.feed.with(|hub| hub.close());
        // The accepting thread owns the listener: once it ends, the port is closed.
        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
        forget(&self.dir);
    }
}

/// Removes the discovery file in `dir`, if there is one: at quit, and at launch (a crash or a
/// kill leaves it behind).
pub fn forget(dir: &Path) {
    let file = dir.join(FILE);
    if let Err(e) = std::fs::remove_file(&file)
        && e.kind() != ErrorKind::NotFound
    {
        eprintln!("Ailfred: removing {} failed: {e}", file.display());
    }
}

/// Serves `core` on a fresh loopback port with a fresh token, its streams watching `feed` (its
/// clients join it again if a server stopped on it before), and
/// writes where and with what key into `dir`/`server.json` (0600, its folder 0700).
pub fn start(core: Arc<dyn Core>, feed: Feed, dir: &Path) -> io::Result<Running> {
    start_with(core, feed, dir, Limits::DEFAULT)
}

/// `start`, with these `limits`.
pub fn start_with(core: Arc<dyn Core>, feed: Feed, dir: &Path, limits: Limits) -> io::Result<Running> {
    let token = crate::computer::server::new_token()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
    history::ensure_private_dir(dir)?;
    let file = dir.join(FILE);
    history::write_atomic(&file, &discovery(&url, &token))?;
    // The web view may have kept the hub fed since the server last stopped on it.
    feed.with(|hub| hub.reopen());
    let stopped = Arc::new(AtomicBool::new(false));
    let server = Arc::new(Server { gate: Gate::new(port, token), core, stopped: stopped.clone(), limits });
    let plan = Plan { accepting: "local-server", connection: "local-connection", connections: MAX_CONNECTIONS, limits };
    match conn::spawn(listener, plan, Arc::default(), stopped.clone(), move |socket| connection(socket, &server)) {
        Ok(accepting) => Ok(Running { url, stopped, accepting: Some(accepting), dir: dir.to_owned(), feed }),
        Err(e) => {
            let _ = std::fs::remove_file(&file);
            Err(e)
        }
    }
}

/// What every connection's thread shares.
struct Server {
    gate: Gate,
    core: Arc<dyn Core>,
    stopped: Arc<AtomicBool>,
    limits: Limits,
}

/// One connection: its head, the guards, then its answer or its stream; then it closes.
fn connection(mut socket: TcpStream, server: &Server) {
    let limits = server.limits;
    let (request, rest) = match conn::read_head(&mut socket, limits) {
        Ok(read) => read,
        Err(Some(answer)) => return respond(socket, &answer, limits.write),
        Err(None) => return,
    };
    if server.stopped.load(Ordering::SeqCst) {
        return;
    }
    let admitted = match server.gate.admit(&request.head()) {
        Ok(admitted) => admitted,
        Err(answer) => return respond(socket, &answer, limits.write),
    };
    if admitted.route == Route::Commands && request.continues && socket.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").is_err() {
        return;
    }
    let Ok(reader) = socket.try_clone() else { return };
    let mut body = Body::new(reader, rest, request.length.unwrap_or(0), limits.body);
    let live = Live { core: server.core.as_ref(), stopped: &server.stopped };
    match serve(admitted, &mut body, &live) {
        Outcome::Answer(answer) => respond(socket, &answer, limits.write),
        Outcome::Stream { first, frames, permit } => stream(socket, first, frames, permit, &server.stopped, limits.write),
    }
}

/// The server's core while this server runs: a command whose body came after it stopped (and
/// perhaps started again, a new server on the same chat) isn't handed over.
struct Live<'a> {
    core: &'a dyn Core,
    stopped: &'a AtomicBool,
}

impl Core for Live<'_> {
    fn join(&self, watcher: std::sync::mpsc::SyncSender<Frame>) {
        if !self.stopped.load(Ordering::SeqCst) {
            self.core.join(watcher);
        }
    }

    fn command(&self, command: ailfred_protocol::command::Command) -> Answer {
        if self.stopped.load(Ordering::SeqCst) {
            return Answer::error(503, "the local server stopped: the command wasn't applied");
        }
        self.core.command(command)
    }
}

/// The discovery file's content.
fn discovery(url: &str, token: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "url": url, "token": token, "pid": std::process::id() })).expect("JSON")
}

/// Writes `answer` with its status and JSON body, within `limit`, then closes the connection.
fn respond(socket: TcpStream, answer: &Answer, limit: Duration) {
    let mut headers = vec![("Content-Type", "application/json"), ("Cache-Control", "no-store")];
    if answer.status == 401 {
        headers.push(("WWW-Authenticate", "Bearer"));
    }
    conn::respond(socket, answer.status, &headers, answer.body.to_string().as_bytes(), limit);
}

/// Writes the stream: its head and `first` (the snapshot), then each frame, a ping after
/// `PING_EVERY` without one, until the hub lets go, the server stops, or the client is gone or
/// too slow (a frame not taken within `limit`; its run goes on); then it closes.
fn stream(mut socket: TcpStream, first: Frame, frames: Receiver<Frame>, permit: Permit, stopped: &AtomicBool, limit: Duration) {
    let _permit = permit;
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n";
    if write_within(&mut socket, head.as_bytes(), limit).and_then(|_| write_within(&mut socket, first.as_bytes(), limit)).is_err() {
        return;
    }
    let mut quiet = Instant::now();
    while !stopped.load(Ordering::SeqCst) {
        let frame = match frames.recv_timeout(LOOK) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) if quiet.elapsed() >= PING_EVERY => PING.to_owned(),
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if write_within(&mut socket, frame.as_bytes(), limit).is_err() {
            return;
        }
        quiet = Instant::now();
    }
    let _ = socket.shutdown(Shutdown::Write);
}
