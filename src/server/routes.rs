//! The server's requests, without sockets: who gets in (the guards, in order, before any body
//! is read), and what each route answers. The socket side (`http`) only reads the head, hands
//! the body over and writes what comes back.

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::time::Duration;

use ailfred_protocol::agui::Message;
use ailfred_protocol::command::{Command, user_said};
use serde_json::{Value, json};

use super::hub::Frame;

/// The largest command body, in bytes: enough for base64 screenshots.
pub const MAX_BODY: u64 = 16 << 20;
/// Requests served at once; more are turned away (503).
pub const MAX_IN_FLIGHT: usize = 8;
/// Event streams open at once.
pub const MAX_STREAMS: usize = 4;
/// The frames a stream holds for a client that reads slowly; one more, and it's dropped (it
/// can join again, and gets a fresh snapshot).
pub const STREAM_FRAMES: usize = 256;
/// How long a joining client waits for the chat to send it the conversation.
const JOIN_WAIT: Duration = Duration::from_secs(10);
/// A client's message's images, checked by their headers before any is decoded: at most this
/// many pixels a side, this many images, and this many pixels in all.
pub const MAX_SIDE: u32 = 8192;
pub const MAX_IMAGES: usize = 8;
pub const MAX_PIXELS: u64 = 40_000_000;

pub const EVENTS: &str = "/v1/events";
pub const COMMANDS: &str = "/v1/commands";

/// What the guards read of a request: its head.
#[derive(Clone, Copy, Debug, Default)]
pub struct Head<'a> {
    pub method: &'a str,
    /// Without the query.
    pub path: &'a str,
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub authorization: Option<&'a str>,
    /// Its `Content-Length`, if it said one.
    pub length: Option<u64>,
}

/// A status and its JSON body.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub status: u16,
    pub body: Value,
}

impl Answer {
    /// Refused, with a short reason: `{"error": …}`.
    pub fn error(status: u16, reason: impl Into<String>) -> Answer {
        Answer { status, body: json!({ "error": reason.into() }) }
    }

    /// Taken (202): the run's events come on the stream.
    pub fn accepted(thread: &str, run: &str) -> Answer {
        Answer { status: 202, body: json!({ "threadId": thread, "runId": run }) }
    }

    /// Taken (202) with no run: a new conversation, `thread`, whose snapshot comes on the stream.
    pub fn new_thread(thread: &str) -> Answer {
        Answer { status: 202, body: json!({ "threadId": thread }) }
    }
}

/// What the server asks of the app (its chat, on the main thread).
pub trait Core: Send + Sync {
    /// A client joins the stream: `watcher` gets the snapshot, the run going, then what comes.
    /// Dropped unused, the conversation isn't there to watch (yet).
    fn join(&self, watcher: SyncSender<Frame>);
    /// A command, and what to answer.
    fn command(&self, command: Command) -> Answer;
}

/// The guards: the port the `Host` header must name, the token, and the counts.
pub struct Gate {
    port: u16,
    token: String,
    in_flight: Arc<AtomicUsize>,
    streams: Arc<AtomicUsize>,
}

pub use super::conn::Permit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Events,
    Commands,
}

/// A request let in, holding its place.
#[derive(Debug)]
pub struct Admitted {
    pub route: Route,
    pub permit: Permit,
}

/// What a request gets: an answer, or the event stream (its first frame, the snapshot; the
/// frames to come; and its place).
pub enum Outcome {
    Answer(Answer),
    Stream { first: Frame, frames: Receiver<Frame>, permit: Permit },
}

impl Gate {
    pub fn new(port: u16, token: String) -> Gate {
        Gate { port, token, in_flight: Arc::default(), streams: Arc::default() }
    }

    /// The guards, in order, on the head alone: `guards`, then `route` (the server calls each
    /// itself, its connection served in between).
    #[cfg(test)]
    pub fn admit(&self, head: &Head) -> Result<Admitted, Answer> {
        self.guards(head)?;
        self.route(head)
    }

    /// Who may come in: the `Host` header names this server (403: DNS rebinding), no `Origin`
    /// (403: no page in a browser), the token (401). Past them, a connection is served.
    pub fn guards(&self, head: &Head) -> Result<(), Answer> {
        if !head.host.is_some_and(|h| self.ours(h)) {
            return Err(Answer::error(403, "the Host header must be this server's loopback address"));
        }
        if head.origin.is_some() {
            return Err(Answer::error(403, "requests from web pages aren't accepted"));
        }
        if !self.authorized(head.authorization) {
            return Err(Answer::error(401, "a valid bearer token is required"));
        }
        Ok(())
    }

    /// Past the guards: the route (404, 405), the body's size (413), room (503).
    pub fn route(&self, head: &Head) -> Result<Admitted, Answer> {
        let route = match (head.path, head.method) {
            (EVENTS, "GET") => Route::Events,
            (COMMANDS, "POST") => Route::Commands,
            (EVENTS | COMMANDS, _) => return Err(Answer::error(405, "method not allowed")),
            _ => return Err(Answer::error(404, "no such path")),
        };
        if route == Route::Commands && head.length.is_some_and(|n| n > MAX_BODY) {
            return Err(Answer::error(413, "the command is larger than 16 MiB"));
        }
        let (count, max) = match route {
            Route::Commands => (&self.in_flight, MAX_IN_FLIGHT),
            Route::Events => (&self.streams, MAX_STREAMS),
        };
        let permit = Permit::take(count, max).ok_or_else(|| Answer::error(503, "too many requests at once; try again"))?;
        Ok(Admitted { route, permit })
    }

    /// `host` names this server: `127.0.0.1:<port>` or `localhost:<port>`, the port in plain
    /// digits (no sign, no leading zero).
    fn ours(&self, host: &str) -> bool {
        super::conn::names_loopback(host, self.port)
    }

    fn authorized(&self, header: Option<&str>) -> bool {
        let Some((scheme, token)) = header.and_then(|h| h.trim().split_once(' ')) else { return false };
        scheme.eq_ignore_ascii_case("bearer") && constant_time_eq(token.trim().as_bytes(), self.token.as_bytes())
    }
}

/// How much of a serde error a 400 says.
const REASON_MAX: usize = 160;

/// Serves a request let in: reads a command's body (up to `MAX_BODY`), checks its images and
/// hands it to `core`; or opens the stream once the conversation came (503 if it isn't there).
pub fn serve(admitted: Admitted, body: &mut dyn Read, core: &dyn Core) -> Outcome {
    let Admitted { route, permit } = admitted;
    if route == Route::Events {
        let (watcher, frames) = sync_channel(STREAM_FRAMES);
        core.join(watcher);
        return match frames.recv_timeout(JOIN_WAIT) {
            Ok(first) => Outcome::Stream { first, frames, permit },
            Err(RecvTimeoutError::Disconnected) => Outcome::Answer(Answer::error(503, "the conversation isn't there yet; try again")),
            Err(RecvTimeoutError::Timeout) => Outcome::Answer(Answer::error(503, "Ailfred is busy; try again")),
        };
    }
    let mut bytes = Vec::new();
    if body.take(MAX_BODY + 1).read_to_end(&mut bytes).is_err() {
        return Outcome::Answer(Answer::error(400, "the body couldn't be read"));
    }
    Outcome::Answer(parse_command(&bytes).map_or_else(|refused| refused, |command| core.command(command)))
}

/// A command's body, checked as the chat can't: at most `MAX_BODY` (413), a `Command` (400), and
/// its images whole PNGs of a size the app takes (400). The web view's bridge checks its
/// commands with this too.
pub fn parse_command(bytes: &[u8]) -> Result<Command, Answer> {
    if bytes.len() as u64 > MAX_BODY {
        return Err(Answer::error(413, "the command is larger than 16 MiB"));
    }
    match serde_json::from_slice::<Command>(bytes) {
        Ok(command) => images_fit(&command).map(|()| command),
        Err(e) => {
            let mut reason = format!("not a command: {e}");
            if reason.len() > REASON_MAX {
                let mut cut = REASON_MAX;
                while !reason.is_char_boundary(cut) {
                    cut -= 1;
                }
                reason.truncate(cut);
            }
            Err(Answer::error(400, reason))
        }
    }
}

/// The images of a message a run sends (its last, the user's) are whole PNGs of a size the app
/// takes: at most `MAX_IMAGES`, and every header is read before any is decoded (0 pixels, more
/// than `MAX_SIDE` a side, or more than `MAX_PIXELS` in all is 400); then each is decoded as
/// the chat would (broken: 400). What isn't text or an inline PNG the chat refuses itself.
fn images_fit(command: &Command) -> Result<(), Answer> {
    let Command::Run(input) = command else { return Ok(()) };
    if input.resume.as_ref().is_some_and(|r| !r.is_empty()) {
        return Ok(());
    }
    let Some(Message::User { content, .. }) = input.messages.last() else { return Ok(()) };
    let Ok((_, pngs)) = user_said(content) else { return Ok(()) };
    if pngs.len() > MAX_IMAGES {
        return Err(Answer::error(400, "a message has at most 8 images"));
    }
    let mut pixels = 0u64;
    for png in &pngs {
        let (width, height) = png_size(png).ok_or_else(|| Answer::error(400, "an image has no PNG header"))?;
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(Answer::error(400, "an image must be 1 to 8192 pixels a side"));
        }
        pixels += u64::from(width) * u64::from(height);
        if pixels > MAX_PIXELS {
            return Err(Answer::error(400, "a message's images have 40 megapixels at most in all"));
        }
    }
    for png in &pngs {
        if crate::chat::decode_png(png).is_none() {
            return Err(Answer::error(400, "an image isn't a PNG that can be read"));
        }
    }
    Ok(())
}

/// A PNG's width and height, from its IHDR chunk's first bytes (after the signature).
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let header = png.get(8..24).filter(|h| h[..8] == *b"\0\0\0\x0dIHDR")?;
    let side = |at: usize| u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
    Some((side(8), side(12)))
}

/// Whether `a` and `b` are the same, in a time that doesn't say where they differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
