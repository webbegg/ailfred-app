//! The local server: lets apps on this Mac (a web view, a Swift app) show and continue the
//! conversation in the chat, as AG-UI 1.0 events over HTTP on `127.0.0.1` (`docs/local-server.md`).
//! Off until turned on in Settings.
//!
//! - [`hub`]: the conversation's stream, fed by the chat (main thread), one run at a time.
//! - [`routes`]: the guards and the routes, without sockets.
//! - [`head`]: a request's head, read off the socket's bytes.
//! - [`conn`]: the connection layer (accepting, the time and size limits, writing answers),
//!   which computer control's server stands on too.
//! - [`http`]: the sockets (this server on `conn`) and the discovery file.
//!
//! The chat stays the truth: commands reach it on the main thread through the app's handle
//! ([`AppCore`]), and what it does reaches the hub through its feed.

pub(crate) mod conn;
mod head;
mod http;
mod hub;
mod routes;
#[cfg(test)]
pub(crate) mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ailfred_protocol::command::Command;
use rune::AppHandle;

pub use http::{Running, forget, start};
pub use hub::{Feed, Frame, Sink};
pub use routes::{Answer, Core, parse_command};
#[cfg(test)]
pub(crate) use routes::{COMMANDS, Gate, Head, Outcome, serve};

use crate::Host;

/// How long a command waits for the chat to take it before the server says it's busy (and so
/// that it wasn't applied).
const COMMAND_WAIT: Duration = Duration::from_secs(10);

/// Now, in ms since the epoch: when an event came.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The settings' switch: on, `running` is started (once); off, it's stopped.
pub fn switch(running: &mut Option<Running>, on: bool, start: impl FnOnce() -> std::io::Result<Running>) {
    if !on {
        if let Some(r) = running.take() {
            r.stop();
        }
        return;
    }
    if running.is_some() {
        return;
    }
    match start() {
        Ok(r) => {
            eprintln!("Ailfred: the local server is at {}", r.url());
            *running = Some(r);
        }
        Err(e) => eprintln!("Ailfred: the local server couldn't start: {e}"),
    }
}

/// Who needs the chat to feed the hub: the local server, the web view, or both. The chat feeds
/// one hub while either does, and stops once neither does (the next one is a fresh hub).
#[derive(Default)]
pub struct Feeding {
    feed: Option<Feed>,
    /// The chat was given `feed`.
    fed: bool,
}

impl Feeding {
    /// The hub, made if there's none yet: what the server starts on.
    pub fn feed(&mut self) -> Feed {
        self.feed.get_or_insert_with(Feed::default).clone()
    }

    /// The hub while it's fed.
    #[cfg(test)]
    pub fn fed(&self) -> Option<&Feed> {
        self.feed.as_ref().filter(|_| self.fed)
    }

    /// Whether anything needs the hub now (the server runs, or the web view is open). Some: the
    /// chat must be told (`Chat::set_feed`), with the hub or with none.
    pub fn need(&mut self, wanted: bool) -> Option<Option<Feed>> {
        match (wanted, self.fed) {
            (true, false) => {
                self.fed = true;
                Some(Some(self.feed()))
            }
            (false, true) => {
                self.fed = false;
                self.feed = None;
                Some(None)
            }
            // Made for a server that didn't start: nothing has it.
            (false, false) => {
                self.feed = None;
                None
            }
            (true, true) => None,
        }
    }
}

/// The app as the server's core: joins and commands go to the chat on the main thread.
pub struct AppCore(pub AppHandle<Host>);

impl Core for AppCore {
    fn join(&self, watcher: mpsc::SyncSender<Frame>) {
        self.0.update(move |host, _| host.join_server(watcher));
    }

    fn command(&self, command: Command) -> Answer {
        handed(COMMAND_WAIT, |ticket| self.0.update(move |host, _| host.serve_command(command, ticket)))
    }
}

/// Where a command handed to the main thread is: waiting, taken (the chat applies it), or given
/// up on (the server stopped waiting). It moves once, out of waiting, by whichever side comes
/// first.
const WAITING: u8 = 0;
const TAKEN: u8 = 1;
const GIVEN_UP: u8 = 2;

/// A command on its way to the chat: the chat takes it before applying it, and answers.
pub struct Ticket {
    state: Arc<AtomicU8>,
    answer: mpsc::SyncSender<Answer>,
}

impl Ticket {
    /// The chat takes the command now. False: the server gave up waiting, answered 503, and the
    /// command must not be applied.
    pub fn take(&self) -> bool {
        self.state.compare_exchange(WAITING, TAKEN, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    /// What to answer the client.
    pub fn answer(self, answer: Answer) {
        let _ = self.answer.send(answer);
    }
}

/// Hands a command over through `post` (to the main thread) and waits up to `wait` for the chat
/// to take it. Not taken by then, it never will be: 503, not applied, and the client may send it
/// again as it was. Taken, its answer comes, however late.
pub fn handed(wait: Duration, post: impl FnOnce(Ticket)) -> Answer {
    let state = Arc::new(AtomicU8::new(WAITING));
    let (answer, answered) = mpsc::sync_channel(1);
    post(Ticket { state: state.clone(), answer });
    if let Ok(answer) = answered.recv_timeout(wait) {
        return answer;
    }
    if state.compare_exchange(WAITING, GIVEN_UP, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        return Answer::error(503, "Ailfred is busy: the command wasn't applied; send it again");
    }
    answered.recv().unwrap_or_else(|_| Answer::error(500, "the command broke as it was applied"))
}
