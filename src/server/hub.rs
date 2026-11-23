//! The conversation's stream: what the chat reports (a new conversation, a reply starting, its
//! events, the user's Stop) as AG-UI frames for every client watching, one run at a time.
//!
//! Runs go out whole and in the order they started: pi's steered replies overlap (the next
//! one's `Taken` comes before the previous one's `Done`), so a later run's events are held and
//! sent once the earlier one's `RUN_FINISHED` or `RUN_ERROR` is out. Each run's `RUN_STARTED`
//! carries the user message it answers (`input`). A client joining gets the conversation's
//! `MESSAGES_SNAPSHOT` without the runs still going, then the one going from its
//! `RUN_STARTED`, then what comes; every client gets the same when the conversation changes in
//! place (a Retry, a declined question).
//!
//! It never waits for a client: each has a bounded channel, and one whose channel is full (it
//! reads too slowly) is dropped; it can join again. The web view, in the app's own process,
//! watches through a sink of its own instead, never dropped: it stays until it leaves, and the
//! server stopping doesn't end it.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard};

use ailfred_protocol::agui::{self, Content, PROTOCOL_VERSION, RunAgentInput};
use ailfred_protocol::command::user_message;
use ailfred_protocol::encode::{ASK, Encoder};
use ailfred_protocol::saved;

use crate::reply::Event;

/// What the event stream writes next, as written: `data: <json>` and a blank line (a joining
/// client's first is several), or a ping.
pub type Frame = String;

/// The keep-alive comment a stream gets when nothing else came for a while.
pub const PING: &str = ": ping\n\n";

/// Where the web view's frames go, in order: it takes each at once, never waiting.
pub type Sink = Box<dyn FnMut(Frame) + Send>;

/// The hub, shared by the chat (main thread) and the server's streams.
#[derive(Clone, Default)]
pub struct Feed(Arc<Mutex<Hub>>);

impl Feed {
    /// `f` on the hub, alone.
    pub fn with<R>(&self, f: impl FnOnce(&mut Hub) -> R) -> R {
        f(&mut lock(&self.0))
    }
}

fn lock(hub: &Mutex<Hub>) -> MutexGuard<'_, Hub> {
    hub.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What's going on the stream of one conversation (thread).
#[derive(Default)]
pub struct Hub {
    thread: String,
    /// The runs not sent whole yet, in the order they were made.
    runs: Vec<Run>,
    /// Counts the runs that started, to keep them in that order.
    starts: u64,
    /// Every run id of this thread so far.
    seen: HashSet<String>,
    /// The run each reply stream (token) is, ended ones too.
    names: HashMap<u64, String>,
    /// The next number a minted run id takes.
    minted: u64,
    watchers: Vec<SyncSender<Frame>>,
    /// The web view's sink, while it watches.
    local: Option<Sink>,
    /// The server stopped: none of its clients joins any more (the web view still may).
    closed: bool,
}

/// One run: the reply streams that are it (a queued one gets a new stream when it goes), its
/// encoder, and its frames so far.
struct Run {
    id: String,
    /// The message it answers.
    message: u64,
    tokens: Vec<u64>,
    encoder: Encoder,
    frames: Vec<Frame>,
    /// How many of its frames went out.
    sent: usize,
    /// When it started, among the thread's runs (None: nothing came yet).
    started: Option<u64>,
    over: bool,
}

impl Hub {
    /// The conversation it streams.
    #[cfg(test)]
    pub fn thread(&self) -> &str {
        &self.thread
    }

    /// A new conversation, `thread`: what was going is forgotten, and every client gets its
    /// `snapshot`.
    pub fn conversation(&mut self, thread: &str, snapshot: &agui::Event) {
        self.thread = thread.to_owned();
        self.runs.clear();
        self.seen.clear();
        self.names.clear();
        self.broadcast(&agui::sse(snapshot));
    }

    /// Run id `run` was used in this thread already.
    pub fn seen(&self, run: &str) -> bool {
        self.seen.contains(run)
    }

    /// A command took run id `run` without starting a run (a resume that only declines): no
    /// later one may use it.
    pub fn claim(&mut self, run: &str) {
        self.seen.insert(run.to_owned());
    }

    /// The same conversation, changed in place (a question declined, a reply retried): every
    /// client gets `snapshot` (made without `live_messages`) and the runs going from their
    /// start, as one joining would. Nothing is forgotten.
    pub fn resync(&mut self, snapshot: &agui::Event) {
        let first = self.first(snapshot);
        self.broadcast(&first);
    }

    /// Reply stream `token` starts for message `message` (which said `text` with captures
    /// `pngs`): the same run as that message's run that hasn't started yet, else a new run,
    /// named `run` (a client's, unused) or minted.
    pub fn reply(&mut self, token: u64, message: u64, text: &str, pngs: &[&[u8]], run: Option<String>) {
        if let Some(waiting) = self.runs.iter_mut().find(|r| r.message == message && r.started.is_none()) {
            waiting.tokens.push(token);
            self.names.insert(token, waiting.id.clone());
            return;
        }
        let id = match run.filter(|r| !r.is_empty() && !self.seen.contains(r)) {
            Some(id) => id,
            None => self.mint(token),
        };
        self.seen.insert(id.clone());
        self.names.insert(token, id.clone());
        let input = RunAgentInput {
            thread_id: self.thread.clone(),
            run_id: id.clone(),
            protocol_version: Some(PROTOCOL_VERSION.into()),
            parent_run_id: None,
            state: None,
            messages: vec![user_message(&format!("{id}-user"), text, pngs)],
            tools: None,
            context: None,
            forwarded_props: None,
            resume: None,
        };
        let encoder = Encoder::new(self.thread.clone(), id.clone()).with_input(input);
        self.runs.push(Run { id, message, tokens: vec![token], encoder, frames: Vec::new(), sent: 0, started: None, over: false });
    }

    /// A run id for stream `token` no run of the thread has: `{thread}-{token}-{n}`.
    fn mint(&mut self, token: u64) -> String {
        loop {
            self.minted += 1;
            let id = format!("{}-{token}-{}", self.thread, self.minted);
            if !self.seen.contains(&id) {
                return id;
            }
        }
    }

    /// The next event of reply stream `token`, at `at` (ms since the epoch); the interrupt ids
    /// of the questions it sent.
    pub fn event(&mut self, token: u64, event: &Event, at: u64) -> Vec<String> {
        let Some(i) = self.going(token) else { return Vec::new() };
        let out = self.runs[i].encoder.encode(event, at);
        let asked = out
            .iter()
            .filter_map(|e| match e {
                agui::Event::Custom { name, value, .. } if name == ASK => value["interruptId"].as_str().map(str::to_owned),
                _ => None,
            })
            .collect();
        self.take(i, out);
        asked
    }

    /// The user stopped the streams `tokens` at `at`: their runs end cancelled.
    pub fn stop(&mut self, tokens: &[u64], at: u64) {
        for &token in tokens {
            if let Some(i) = self.going(token) {
                let out = self.runs[i].encoder.stop(at);
                self.take(i, out);
            }
        }
    }

    /// The run going that reply stream `token` is.
    fn going(&self, token: u64) -> Option<usize> {
        self.runs.iter().position(|r| !r.over && r.tokens.contains(&token))
    }

    /// Run `i`'s next events: framed and kept, a step's result cut to its tail; out at once if
    /// it's the run on the stream, else held.
    fn take(&mut self, i: usize, events: Vec<agui::Event>) {
        if events.is_empty() {
            return;
        }
        let run = &mut self.runs[i];
        if run.started.is_none() {
            self.starts += 1;
            run.started = Some(self.starts);
        }
        for mut event in events {
            match &mut event {
                agui::Event::ToolCallResult { content: Content::Text(text), .. } => *text = saved::tail(text),
                agui::Event::RunFinished { .. } | agui::Event::RunError { .. } => run.over = true,
                _ => {}
            }
            run.frames.push(agui::sse(&event));
        }
        self.pump();
    }

    /// Sends what the run on the stream (the first started that isn't sent whole) has; once it
    /// ended, it goes and the next one's turn comes.
    fn pump(&mut self) {
        while let Some(i) = self.runs.iter().enumerate().filter_map(|(i, r)| Some((i, r.started?))).min_by_key(|(_, s)| *s).map(|(i, _)| i) {
            let frames: Vec<Frame> = self.runs[i].frames[self.runs[i].sent..].to_vec();
            self.runs[i].sent = self.runs[i].frames.len();
            for frame in &frames {
                self.broadcast(frame);
            }
            if !self.runs[i].over {
                return;
            }
            self.runs.remove(i);
        }
    }

    /// `frame` to every client, never waiting; one that's gone, or that has `STREAM_FRAMES`
    /// unread, is dropped (its run goes on).
    fn broadcast(&mut self, frame: &str) {
        self.watchers.retain(|w| w.try_send(frame.to_owned()).is_ok());
        if let Some(local) = &mut self.local {
            local(frame.to_owned());
        }
    }

    /// The messages whose runs aren't sent whole yet: a snapshot leaves them out.
    pub fn live_messages(&self) -> Vec<u64> {
        self.runs.iter().map(|r| r.message).collect()
    }

    /// The run reply stream `token` is.
    pub fn run_of(&self, token: u64) -> Option<&str> {
        self.names.get(&token).map(String::as_str)
    }

    /// A client joins: `snapshot` (made without `live_messages`) and the run going from its
    /// start, as one frame; then everything as it comes. Nothing once the server stopped.
    pub fn join(&mut self, watcher: SyncSender<Frame>, snapshot: &agui::Event) {
        if self.closed {
            return;
        }
        if watcher.try_send(self.first(snapshot)).is_ok() {
            self.watchers.push(watcher);
        }
    }

    /// What a client joining gets first: `snapshot`, then the runs going from their start.
    fn first(&self, snapshot: &agui::Event) -> Frame {
        let mut first = agui::sse(snapshot);
        for run in &self.runs {
            first.extend(run.frames[..run.sent].iter().map(String::as_str));
        }
        first
    }

    /// The web view joins (again, after a reload): `sink` gets what [`Hub::join`] gives a
    /// client, and replaces the sink it had. The server stopped or not.
    pub fn join_local(&mut self, mut sink: Sink, snapshot: &agui::Event) {
        sink(self.first(snapshot));
        self.local = Some(sink);
    }

    /// The web view closed: its sink goes.
    pub fn leave_local(&mut self) {
        self.local = None;
    }

    /// The web view watches.
    #[cfg(test)]
    pub fn local_watching(&self) -> bool {
        self.local.is_some()
    }

    /// Run `run` hasn't ended.
    #[cfg(test)]
    pub fn live(&self, run: &str) -> bool {
        self.runs.iter().any(|r| r.id == run && !r.over)
    }

    /// How many clients watch.
    #[cfg(test)]
    pub fn watching(&self) -> usize {
        self.watchers.len()
    }

    /// The server stopped: every stream ends (the web view's goes on).
    pub fn close(&mut self) {
        self.closed = true;
        self.watchers.clear();
    }

    /// The server started (again) on this hub, which the web view kept fed: clients join again.
    pub fn reopen(&mut self) {
        self.closed = false;
    }
}

