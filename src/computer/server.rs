//! Serves `Helper` on `127.0.0.1:<random port>/mcp` with a fresh random token each time it
//! starts (each time computer control is switched on); switched off, it stops and its port
//! closes. The token lives only in memory and in what Ailfred hands the model it starts.
//!
//! It stands on the local server's connection layer (`crate::server::conn`), with limits of its
//! own:
//!
//! - A connection reading its head waits in the layer's pool, at most `conn::MAX_WAITING` (64):
//!   one more, and the oldest waiting is shut down, not the newcomer. Only past the guards'
//!   `Host`, `Origin` and token does it take one of `MAX_CONNECTIONS` (8) served places (none
//!   free: 503). So a client without the token can't lock pi or Claude Code out by holding
//!   connections; at most 64 + 8 threads at once. One request per connection: every answer says
//!   `Connection: close` and the connection closes after it (their MCP clients open a new one).
//! - `LIMITS`: the first byte within 1 s, else closed unanswered; the head within 1 s in all
//!   (408): its clients send it in one write. At most 16 KiB and 64 headers (431), strict CRLF
//!   (400), one `Host` and one `Authorization` (400); no `Transfer-Encoding` (501), and a POST
//!   needs a `Content-Length` (411).
//! - The guards, in order, on the head alone, before any body is read: the `Host` is
//!   `127.0.0.1:<port>` or `localhost:<port>` (403: DNS rebinding), no `Origin` (403: no page in
//!   a browser), the token (401, `WWW-Authenticate: Bearer`); then room (503), a body of at most
//!   `MAX_BODY` (1 MiB, 413). Then the body, within 30 s; then `Helper` (`POST /mcp` only: 404,
//!   405).
//! - Each answer is written within 10 s, else cut off. Then what the client still sends (a body
//!   never read) is read and dropped, 64 KiB within 300 ms at most, so a refusal arrives whole,
//!   not as a connection reset.
//! - Stopped, its helper is retired first: a request it let in does nothing on the desktop, even
//!   once computer control is on again on another server.

use super::mcp::{Helper, HttpReply, HttpRequest};
use crate::server::Answer;
use crate::server::conn::{self, Body, Place, Plan, Request, Socket};
pub use crate::server::conn::Limits;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// Where the tools are served, and the token that opens them.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    pub token: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint").field("url", &self.url).field("token", &"…").finish()
    }
}

/// 32 random bytes from the system, hex encoded.
pub fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Connections open at once, each carrying one request (actions are serialized inside
/// `Helper`); one more is closed as it comes.
pub const MAX_CONNECTIONS: usize = 8;
/// How long its clients may take: pi's and Claude Code's send each head in one write, so 1 s
/// for the first byte and 1 s for the head in all; the body and each write as the local server.
pub const LIMITS: Limits = Limits { first: Duration::from_secs(1), head: Duration::from_secs(1), ..Limits::DEFAULT };
/// Requests larger than this are refused (413) before their body is read: tool calls are small.
pub const MAX_BODY: u64 = 1 << 20;

/// The tools served: stopping it (or dropping it) closes its port and retires its helper:
/// requests already in end within their limits, and none does anything on the desktop.
pub struct Serving {
    endpoint: Endpoint,
    helper: Arc<Helper>,
    stopped: Arc<AtomicBool>,
    accepting: Option<JoinHandle<()>>,
    /// The connections open.
    #[cfg(test)]
    open: Arc<AtomicUsize>,
}

impl std::fmt::Debug for Serving {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Serving").field("endpoint", &self.endpoint).finish_non_exhaustive()
    }
}

impl Serving {
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Stops it (dropping it does the same).
    pub fn stop(self) {}

    /// How many connections are open (each has a thread), now and later.
    #[cfg(test)]
    pub fn connections(&self) -> impl Fn() -> usize + use<> {
        let open = self.open.clone();
        move || open.load(Ordering::SeqCst)
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        // First: a request let in now does nothing, even once computer control is on again.
        self.helper.retire();
        self.stopped.store(true, Ordering::SeqCst);
        // The accepting thread owns the listener: once it ends, the port is closed.
        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
    }
}

/// Binds a random loopback port and serves `helper` on it, opened by `token`, until stopped.
pub fn start(helper: Arc<Helper>, token: String) -> std::io::Result<Serving> {
    start_with(helper, token, LIMITS)
}

/// `start`, with these `limits`.
pub fn start_with(helper: Arc<Helper>, token: String, limits: Limits) -> std::io::Result<Serving> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let (stopped, open) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let plan = Plan { accepting: "computer-mcp", connection: "computer-call", connections: MAX_CONNECTIONS, limits };
    let (still, serving) = (stopped.clone(), helper.clone());
    let accepting = conn::spawn(listener, plan, open.clone(), stopped.clone(), move |socket, place| connection(socket, place, &serving, port, &still, limits))?;
    Ok(Serving {
        endpoint: Endpoint { url: format!("http://127.0.0.1:{port}/mcp"), token },
        helper,
        stopped,
        accepting: Some(accepting),
        #[cfg(test)]
        open,
    })
}

/// One connection, one request: its head, the guards (then it's served: `place`), its body,
/// `helper`'s reply; then it closes.
fn connection(socket: Socket, mut place: Place, helper: &Helper, port: u16, stopped: &AtomicBool, limits: Limits) {
    let (request, rest) = match conn::read_head(&socket, limits) {
        Ok(read) => read,
        Err(Some(answer)) => return refuse(&socket, &answer, limits),
        Err(None) => return,
    };
    if let Err(answer) = guards(&request, helper, port) {
        return refuse(&socket, &answer, limits);
    }
    if !place.serve() {
        return refuse(&socket, &Answer::error(503, "too many requests at once; try again"), limits);
    }
    if request.length.is_some_and(|n| n > MAX_BODY) {
        return refuse(&socket, &Answer::error(413, "the request is larger than 1 MiB"), limits);
    }
    if request.continues && (&*socket).write_all(b"HTTP/1.1 100 Continue\r\n\r\n").is_err() {
        return;
    }
    let mut bytes = Vec::new();
    let read = Body::new(socket.clone(), rest, request.length.unwrap_or(0), limits.body).read_to_end(&mut bytes);
    let reply = match (read, String::from_utf8(bytes)) {
        _ if stopped.load(Ordering::SeqCst) => HttpReply { status: 503, body: None },
        (Ok(_), Ok(body)) => helper.handle(&HttpRequest { method: &request.method, path: &request.path, authorization: request.authorization.as_deref(), body: &body }),
        _ => HttpReply { status: 400, body: None },
    };
    respond(&socket, &reply, limits);
}

/// The guards, in order, on the head alone: the `Host` names this server on loopback (403: DNS
/// rebinding), no `Origin` (403: no page in a browser), the token (401).
fn guards(request: &Request, helper: &Helper, port: u16) -> Result<(), Answer> {
    if !request.host.as_deref().is_some_and(|h| conn::names_loopback(h, port)) {
        return Err(Answer::error(403, "the Host header must be this server's loopback address"));
    }
    if request.origin.is_some() {
        return Err(Answer::error(403, "requests from web pages aren't accepted"));
    }
    if !helper.authorized(request.authorization.as_deref()) {
        return Err(Answer::error(401, "a valid bearer token is required"));
    }
    Ok(())
}

fn refuse(socket: &TcpStream, answer: &Answer, limits: Limits) {
    respond(socket, &HttpReply { status: answer.status, body: Some(answer.body.clone()) }, limits);
}

/// Writes `reply` (JSON when it has a body), within the write limit, then closes the connection.
fn respond(socket: &TcpStream, reply: &HttpReply, limits: Limits) {
    let mut headers = Vec::new();
    if reply.body.is_some() {
        headers.push(("Content-Type", "application/json"));
    }
    if reply.status == 401 {
        headers.push(("WWW-Authenticate", "Bearer"));
    }
    let body = reply.body.as_ref().map(|b| b.to_string()).unwrap_or_default();
    conn::respond(socket, reply.status, &headers, body.as_bytes(), limits.write);
}

#[cfg(test)]
mod tests;
