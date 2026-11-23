//! The sockets: a small HTTP/1.1 server of our own on `127.0.0.1:0`, and the discovery file.
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

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::json;

use super::head::{self, MAX_HEAD, Request};
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
/// How often a waiting stream, or the accepting thread with nothing to accept, looks whether
/// the server stopped.
const LOOK: Duration = Duration::from_millis(200);
const ACCEPT_LOOK: Duration = Duration::from_millis(20);

/// How long a client may take: to send its first byte, its head (in all, however slowly it
/// comes), its body, and to take each thing written to it (in all, however slowly it reads).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub first: Duration,
    pub head: Duration,
    pub body: Duration,
    pub write: Duration,
}

impl Limits {
    pub const DEFAULT: Limits = Limits { first: Duration::from_secs(1), head: Duration::from_secs(5), body: Duration::from_secs(30), write: Duration::from_secs(10) };
}

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
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
    history::ensure_private_dir(dir)?;
    let file = dir.join(FILE);
    history::write_atomic(&file, &discovery(&url, &token))?;
    // The web view may have kept the hub fed since the server last stopped on it.
    feed.with(|hub| hub.reopen());
    let stopped = Arc::new(AtomicBool::new(false));
    let server = Arc::new(Server { gate: Gate::new(port, token), core, stopped: stopped.clone(), limits, open: Arc::default() });
    match thread::Builder::new().name("local-server".into()).spawn(move || accept(&listener, &server)) {
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
    /// The connections open.
    open: Arc<AtomicUsize>,
}

/// Takes each connection, until the server stops, and hands it to a thread of its own; one
/// past `MAX_CONNECTIONS` is closed at once.
fn accept(listener: &TcpListener, server: &Arc<Server>) {
    while !server.stopped.load(Ordering::SeqCst) {
        let socket = match listener.accept() {
            Ok((socket, _)) => socket,
            // Nothing waiting, or a failure (too many files open, say): look again in a moment.
            Err(_) => {
                thread::sleep(ACCEPT_LOOK);
                continue;
            }
        };
        let Some(place) = Permit::take(&server.open, MAX_CONNECTIONS) else { continue };
        let server = server.clone();
        let spawned = thread::Builder::new().name("local-connection".into()).spawn(move || {
            let _place = place;
            connection(socket, &server);
        });
        if let Err(e) = spawned {
            eprintln!("Ailfred: the local server couldn't take a connection: {e}");
        }
    }
}

/// One connection: its head, the guards, then its answer or its stream; then it closes.
fn connection(mut socket: TcpStream, server: &Server) {
    let limits = server.limits;
    if socket.set_nonblocking(false).and_then(|_| socket.set_write_timeout(Some(limits.write))).is_err() {
        return;
    }
    let _ = socket.set_nodelay(true);
    let (request, rest) = match read_head(&mut socket, limits) {
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
    let mut body = Body { socket: reader, rest, at: 0, left: request.length.unwrap_or(0), deadline: Instant::now() + limits.body };
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

/// The head, its first byte within `limits.first` and all of it within `limits.head`, and the
/// bytes read past it (the body's first); Err with what to answer (None: nothing, the client
/// is gone or never spoke).
fn read_head(socket: &mut TcpStream, limits: Limits) -> Result<(Request, Vec<u8>), Option<Answer>> {
    let started = Instant::now();
    let (first, deadline) = (started + limits.first.min(limits.head), started + limits.head);
    let (mut bytes, mut chunk) = (Vec::with_capacity(1024), [0u8; 8192]);
    loop {
        let now = Instant::now();
        if bytes.is_empty() && now >= first {
            return Err(None);
        }
        let left = if bytes.is_empty() { first } else { deadline }.saturating_duration_since(now);
        if left.is_zero() {
            return Err(Some(Answer::error(408, "the request's head came too slowly")));
        }
        socket.set_read_timeout(Some(left)).map_err(|_| None)?;
        let room = (MAX_HEAD - bytes.len()).min(chunk.len());
        let n = match socket.read(&mut chunk[..room]) {
            Ok(0) => return Err(None),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
            Err(_) => return Err(None),
        };
        let seen = bytes.len();
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = head::end(&bytes, seen).map_err(Some)? {
            let request = head::parse(&bytes[..end]).map_err(Some)?;
            return Ok((request, bytes.split_off(end)));
        }
    }
}

/// A body let in: the bytes read with the head first, then the socket's, `left` more at most,
/// before `deadline`. It ending early is an error.
struct Body {
    socket: TcpStream,
    rest: Vec<u8>,
    at: usize,
    left: u64,
    deadline: Instant,
}

impl Read for Body {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 || out.is_empty() {
            return Ok(0);
        }
        let max = out.len().min(usize::try_from(self.left).unwrap_or(usize::MAX));
        let n = if self.at < self.rest.len() {
            let n = max.min(self.rest.len() - self.at);
            out[..n].copy_from_slice(&self.rest[self.at..self.at + n]);
            self.at += n;
            n
        } else {
            loop {
                let left = self.deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(ErrorKind::TimedOut.into());
                }
                self.socket.set_read_timeout(Some(left))?;
                match self.socket.read(&mut out[..max]) {
                    Ok(0) => return Err(ErrorKind::UnexpectedEof.into()),
                    Ok(n) => break n,
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
                    Err(e) => return Err(e),
                }
            }
        };
        self.left -= n as u64;
        Ok(n)
    }
}

/// The discovery file's content.
fn discovery(url: &str, token: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "url": url, "token": token, "pid": std::process::id() })).expect("JSON")
}

/// Writes all of `bytes` within `limit` in all, however the client reads: each write may take
/// only what's left of it.
fn write_within(socket: &mut TcpStream, bytes: &[u8], limit: Duration) -> io::Result<()> {
    let deadline = Instant::now() + limit;
    let mut at = 0;
    while at < bytes.len() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ErrorKind::TimedOut.into());
        }
        socket.set_write_timeout(Some(left))?;
        match socket.write(&bytes[at..]) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => at += n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Writes `answer` with its status and JSON body, within `limit`, then closes the connection.
fn respond(mut socket: TcpStream, answer: &Answer, limit: Duration) {
    let body = answer.body.to_string();
    let bearer = if answer.status == 401 { "WWW-Authenticate: Bearer\r\n" } else { "" };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{bearer}Connection: close\r\n\r\n",
        answer.status,
        reason(answer.status),
        body.len()
    );
    let _ = write_within(&mut socket, format!("{head}{body}").as_bytes(), limit);
    let _ = socket.shutdown(Shutdown::Write);
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

/// A status's reason phrase.
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "",
    }
}
