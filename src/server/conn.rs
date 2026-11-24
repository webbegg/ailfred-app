//! The connection layer both loopback servers stand on (the local server, `http`, and computer
//! control's, `crate::computer::server`): a small HTTP/1.1 server of our own over
//! `std::net`, each server bringing its own guards and its own [`Limits`].
//!
//! - One thread accepts, and never serves: each connection gets a thread of its own, at most
//!   `Plan::connections` at once (one more is closed as it comes), until the server stops.
//! - [`read_head`]: its first byte within `Limits::first`, else it's closed; the head within
//!   `Limits::head` in all, at most 16 KiB and 64 headers (`head`). The guards run on it alone.
//! - [`Body`]: a body let in is read as it comes, up to its `Content-Length`, within
//!   `Limits::body`.
//! - [`respond`]: each answer is written within `Limits::write` in all, says
//!   `Connection: close`, and the connection closes after it: no keep-alive, no pipelining.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::head::{self, MAX_HEAD};
pub use super::head::Request;
use super::routes::Answer;

/// How often the accepting thread, with nothing to accept, looks whether the server stopped.
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

/// A server's shape: what its threads are called, how many connections it takes at once, and
/// how long a client may take.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub accepting: &'static str,
    pub connection: &'static str,
    pub connections: usize,
    pub limits: Limits,
}

/// A place among the requests in flight (or the streams, or the connections), given back when
/// dropped.
#[derive(Debug)]
pub struct Permit(Arc<AtomicUsize>);

impl Permit {
    /// One of the `max` places `count` keeps, if one is free.
    pub fn take(count: &Arc<AtomicUsize>, max: usize) -> Option<Permit> {
        if count.fetch_add(1, Ordering::SeqCst) >= max {
            count.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Permit(count.clone()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Takes each connection on `listener`, until `stopped`, and hands it to `handle` on a thread of
/// its own (blocking, each write timing out after `limits.write`), counted in `open`; one past
/// `plan.connections` is closed at once. The accepting thread owns the listener: once it ends,
/// the port is closed.
pub fn spawn(listener: TcpListener, plan: Plan, open: Arc<AtomicUsize>, stopped: Arc<AtomicBool>, handle: impl Fn(TcpStream) + Send + Sync + 'static) -> io::Result<JoinHandle<()>> {
    listener.set_nonblocking(true)?;
    let handle = Arc::new(handle);
    thread::Builder::new().name(plan.accepting.into()).spawn(move || {
        while !stopped.load(Ordering::SeqCst) {
            let socket = match listener.accept() {
                Ok((socket, _)) => socket,
                // Nothing waiting, or a failure (too many files open, say): look again in a moment.
                Err(_) => {
                    thread::sleep(ACCEPT_LOOK);
                    continue;
                }
            };
            let Some(place) = Permit::take(&open, plan.connections) else { continue };
            let handle = handle.clone();
            let spawned = thread::Builder::new().name(plan.connection.into()).spawn(move || {
                let _place = place;
                if socket.set_nonblocking(false).and_then(|_| socket.set_write_timeout(Some(plan.limits.write))).is_err() {
                    return;
                }
                let _ = socket.set_nodelay(true);
                handle(socket);
            });
            if let Err(e) = spawned {
                eprintln!("Ailfred: {} couldn't take a connection: {e}", plan.accepting);
            }
        }
    })
}

/// The head, its first byte within `limits.first` and all of it within `limits.head`, and the
/// bytes read past it (the body's first); Err with what to answer (None: nothing, the client
/// is gone or never spoke).
pub fn read_head(socket: &mut TcpStream, limits: Limits) -> Result<(Request, Vec<u8>), Option<Answer>> {
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
pub struct Body {
    socket: TcpStream,
    rest: Vec<u8>,
    at: usize,
    left: u64,
    deadline: Instant,
}

impl Body {
    /// The body `rest` starts (read with the head), `length` bytes in all, read off `socket`
    /// within `limit` from now.
    pub fn new(socket: TcpStream, rest: Vec<u8>, length: u64, limit: Duration) -> Body {
        Body { socket, rest, at: 0, left: length, deadline: Instant::now() + limit }
    }
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

/// Writes all of `bytes` within `limit` in all, however the client reads: each write may take
/// only what's left of it.
pub fn write_within(socket: &mut TcpStream, bytes: &[u8], limit: Duration) -> io::Result<()> {
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

/// Writes an answer, `status` with `headers` and `body`, within `limit`, then closes the
/// connection.
pub fn respond(mut socket: TcpStream, status: u16, headers: &[(&str, &str)], body: &[u8], limit: Duration) {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    let _ = write_within(&mut socket, &bytes, limit);
    let _ = socket.shutdown(Shutdown::Write);
}

/// `host` (a `Host` header) names a loopback server on `port`: `127.0.0.1:<port>` or
/// `localhost:<port>`, the port in plain digits (no sign, no leading zero). Anything else may be
/// a page that rebound its name to 127.0.0.1 (DNS rebinding).
pub fn names_loopback(host: &str, port: u16) -> bool {
    let Some((name, given)) = host.trim().rsplit_once(':') else { return false };
    let plain = !given.is_empty() && !given.starts_with('0') && given.bytes().all(|b| b.is_ascii_digit());
    (name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost")) && plain && given.parse::<u16>().ok() == Some(port)
}

/// A status's reason phrase.
pub fn reason(status: u16) -> &'static str {
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
