//! The connection layer both loopback servers stand on (the local server, `http`, and computer
//! control's, `crate::computer::server`): a small HTTP/1.1 server of our own over
//! `std::net`, each server bringing its own guards and its own [`Limits`].
//!
//! - One thread accepts, and never serves: each connection gets a thread of its own, until the
//!   server stops. A connection waits in a pool of its own while it reads its head, at most
//!   `MAX_WAITING` (64) of them: one more, and the oldest waiting is shut down (its blocked read
//!   returns at once, its thread ends), not the newcomer. A real client sends its head in one
//!   write and passes its server's guards (`Host`, `Origin`, the token) in microseconds; a
//!   client without the token can't keep it out without opening a flood of connections in that
//!   instant. Once past the guards ([`Place::serve`]), it takes one of the `Plan::connections`
//!   served places (none free: its server answers 503). So at most `MAX_WAITING` +
//!   `Plan::connections` threads at once: the accepting thread waits for an evicted one to end.
//!   Stopped, the connections still waiting are shut down.
//! - [`read_head`]: its first byte within `Limits::first`, else it's closed; the head within
//!   `Limits::head` in all, at most 16 KiB and 64 headers (`head`). The guards run on it alone.
//! - [`Body`]: a body let in is read as it comes, up to its `Content-Length`, within
//!   `Limits::body`.
//! - [`respond`]: each answer is written within `Limits::write` in all, says
//!   `Connection: close`, and the connection closes after it: no keep-alive, no pipelining.
//!   Before it closes, what the client still sends (a body never read, the rest of a head too
//!   large) is read and dropped, `LINGER_BYTES` (64 KiB) at most within `LINGER` (300 ms), so
//!   closing on bytes unread doesn't reset the connection and lose the answer. That counts
//!   against no other limit; past it, the connection closes all the same.

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::head::{self, MAX_HEAD};
pub use super::head::Request;
use super::routes::Answer;

/// Connections still reading their head (not past their server's guards yet) held at once;
/// one more, and the oldest of them is shut down.
pub const MAX_WAITING: usize = 64;
/// Once answered, what a client still sends is read and dropped before closing: this long at
/// most, and this many bytes.
pub const LINGER: Duration = Duration::from_millis(300);
pub const LINGER_BYTES: usize = 64 << 10;

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

/// A server's shape: what its threads are called, how many connections past its guards it
/// serves at once, and how long a client may take.
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

/// The connections still reading their head, oldest first: each its number and a handle on its
/// socket, to shut it down.
#[derive(Default)]
struct Waiting {
    next: u64,
    queue: VecDeque<(u64, Socket)>,
}

/// A connection's socket, shared by its thread and the waiting pool (which may shut it down)
/// without a second file descriptor: `&TcpStream` reads and writes.
pub type Socket = Arc<TcpStream>;

fn lock(waiting: &Mutex<Waiting>) -> MutexGuard<'_, Waiting> {
    waiting.lock().unwrap_or_else(|e| e.into_inner())
}

/// A connection's place: among those waiting until it passed its server's guards, then among
/// those served. Given back when dropped.
pub struct Place {
    waiting: Arc<Mutex<Waiting>>,
    number: u64,
    served: Arc<AtomicUsize>,
    max: usize,
    permit: Option<Permit>,
}

impl Place {
    /// It passed its server's guards: out of the waiting pool, into one of the served places.
    /// False if none is free (it stays waiting: its server answers 503), or if it was shut down
    /// meanwhile (the oldest waiting, past the pool).
    pub fn serve(&mut self) -> bool {
        if self.permit.is_some() {
            return true;
        }
        let mut waiting = lock(&self.waiting);
        let Some(at) = waiting.queue.iter().position(|(n, _)| *n == self.number) else { return false };
        let Some(permit) = Permit::take(&self.served, self.max) else { return false };
        waiting.queue.remove(at);
        self.permit = Some(permit);
        true
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        if self.permit.is_none() {
            lock(&self.waiting).queue.retain(|(n, _)| *n != self.number);
        }
    }
}

/// Counted in `open` while alive (its thread).
struct Alive(Arc<AtomicUsize>);

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Takes each connection on `listener`, until `stopped`, and hands it to `handle` on a thread of
/// its own (blocking, each write timing out after `limits.write`) with its [`Place`], waiting:
/// past `MAX_WAITING` waiting, the oldest is shut down. `open` counts the threads, at most
/// `MAX_WAITING` + `plan.connections`. The accepting thread owns the listener: once it ends,
/// the port is closed and the connections still waiting are shut down.
pub fn spawn(listener: TcpListener, plan: Plan, open: Arc<AtomicUsize>, stopped: Arc<AtomicBool>, handle: impl Fn(Socket, Place) + Send + Sync + 'static) -> io::Result<JoinHandle<()>> {
    listener.set_nonblocking(true)?;
    let handle = Arc::new(handle);
    let (waiting, served) = (Arc::new(Mutex::new(Waiting::default())), Arc::new(AtomicUsize::new(0)));
    let threads = MAX_WAITING + plan.connections;
    thread::Builder::new().name(plan.accepting.into()).spawn(move || {
        while !stopped.load(Ordering::SeqCst) {
            let socket = match listener.accept() {
                Ok((socket, _)) => Arc::new(socket),
                // Nothing waiting, or a failure (too many files open, say): look again in a moment.
                Err(_) => {
                    thread::sleep(ACCEPT_LOOK);
                    continue;
                }
            };
            let number = {
                let mut waiting = lock(&waiting);
                if waiting.queue.len() >= MAX_WAITING
                    && let Some((_, oldest)) = waiting.queue.pop_front()
                {
                    // Its blocked read (or write) returns at once, and its thread ends.
                    let _ = oldest.shutdown(Shutdown::Both);
                }
                waiting.next += 1;
                let number = waiting.next;
                waiting.queue.push_back((number, socket.clone()));
                number
            };
            let place = Place { waiting: waiting.clone(), number, served: served.clone(), max: plan.connections, permit: None };
            // An evicted connection's thread ends at once: wait for it.
            while open.load(Ordering::SeqCst) >= threads && !stopped.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
            open.fetch_add(1, Ordering::SeqCst);
            let (handle, alive) = (handle.clone(), Alive(open.clone()));
            let spawned = thread::Builder::new().name(plan.connection.into()).spawn(move || {
                let _alive = alive;
                if socket.set_nonblocking(false).and_then(|_| socket.set_write_timeout(Some(plan.limits.write))).is_err() {
                    return;
                }
                let _ = socket.set_nodelay(true);
                handle(socket, place);
            });
            if let Err(e) = spawned {
                eprintln!("Ailfred: {} couldn't take a connection: {e}", plan.accepting);
            }
        }
        for (_, socket) in lock(&waiting).queue.drain(..) {
            let _ = socket.shutdown(Shutdown::Both);
        }
    })
}

/// The head, its first byte within `limits.first` and all of it within `limits.head`, and the
/// bytes read past it (the body's first); Err with what to answer (None: nothing, the client
/// is gone or never spoke).
pub fn read_head(mut socket: &TcpStream, limits: Limits) -> Result<(Request, Vec<u8>), Option<Answer>> {
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
    socket: Socket,
    rest: Vec<u8>,
    at: usize,
    left: u64,
    deadline: Instant,
}

impl Body {
    /// The body `rest` starts (read with the head), `length` bytes in all, read off `socket`
    /// within `limit` from now.
    pub fn new(socket: Socket, rest: Vec<u8>, length: u64, limit: Duration) -> Body {
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
                match (&*self.socket).read(&mut out[..max]) {
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
pub fn write_within(mut socket: &TcpStream, bytes: &[u8], limit: Duration) -> io::Result<()> {
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
/// connection (once what the client still sends is dropped: `linger`).
pub fn respond(socket: &TcpStream, status: u16, headers: &[(&str, &str)], body: &[u8], limit: Duration) {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    if write_within(socket, &bytes, limit).is_ok() {
        linger(socket);
    }
}

/// Says no more comes (the client sees the answer end), then reads and drops what the client
/// still sends, until it closes, `LINGER_BYTES` at most within `LINGER`: closing on bytes unread
/// would reset the connection, and the client could lose the answer before reading it.
pub fn linger(mut socket: &TcpStream) {
    if socket.shutdown(Shutdown::Write).is_err() {
        return;
    }
    let (deadline, mut left, mut chunk) = (Instant::now() + LINGER, LINGER_BYTES, [0u8; 8192]);
    while left > 0 {
        let time = deadline.saturating_duration_since(Instant::now());
        if time.is_zero() || socket.set_read_timeout(Some(time)).is_err() {
            return;
        }
        match socket.read(&mut chunk[..left.min(8192)]) {
            Ok(0) => return,
            Ok(n) => left -= n,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(_) => return,
        }
    }
}

/// `host` (a `Host` header) names a loopback server on `port`: `127.0.0.1:<port>` or
/// `localhost:<port>`, the port in plain digits (no sign, no leading zero). Anything else may be
/// a page that rebound its name to 127.0.0.1 (DNS rebinding).
pub fn names_loopback(host: &str, port: u16) -> bool {
    let Some((name, given)) = host.trim().rsplit_once(':') else { return false };
    let plain = !given.is_empty() && !given.starts_with('0') && given.bytes().all(|b| b.is_ascii_digit());
    (name == "127.0.0.1" || name.eq_ignore_ascii_case("localhost")) && plain && given.parse::<u16>().ok() == Some(port)
}

/// For a test that floods a server with connections: one at a time (they'd compete for the
/// machine), with room for the sockets (a Terminal's shell allows 256 files open).
#[cfg(test)]
pub(crate) fn flood_room() -> MutexGuard<'static, ()> {
    static FLOOD: Mutex<()> = Mutex::new(());
    let turn = FLOOD.lock().unwrap_or_else(|e| e.into_inner());
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: plain calls on a local struct.
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 && limit.rlim_cur < 4096 {
            limit.rlim_cur = limit.rlim_max.min(4096);
            libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
        }
    }
    turn
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
