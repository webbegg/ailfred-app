//! The chat and the apps watching it through the local server (`crate::server`) or the web
//! view (`crate::web_view`, the same hub in the app's process): what happens
//! here reaches its hub (a new conversation, a reply starting, its events, Stop), and what a
//! client sends comes in as if typed here.
//!
//! The chat's conversation is the truth: a run's `messages` aren't replayed into the model;
//! its last one, the user's, is sent as a message typed here would be (a steer, queued or
//! asked at once, to the model picked). A `resume` answers or declines the open questions of the
//! latest reply instead, each by an interrupt id it was sent; one of Ailfred's guard
//! (`reply::LOCAL_ONLY`) only the window answers, a client can only decline it. A Retry or a new
//! conversation does what the window's Retry or Cmd+N does.

use std::sync::mpsc::SyncSender;
use std::time::Instant;

use ailfred_protocol::agui::{self, Resume, ResumeStatus};
use ailfred_protocol::command::{Command, Unsaid, user_said};
use ailfred_protocol::saved;
use ailfred_protocol::snapshot::{self, Turn};
use serde_json::{Value, json};

use super::{Chat, DECLINED, Message};
use crate::history;
use crate::mock_model;
use crate::model;
use crate::reply::{Part, State};
use crate::server::{Answer, Feed, Frame, Sink, now_ms};

impl Chat {
    /// The local server's hub (None: it stopped). Set, it gets the conversation as it is: its
    /// snapshot, then the replies going (and queued) as runs, each caught up with what came so
    /// far, so a client joining gets them from their `RUN_STARTED` and live to their end.
    pub fn set_feed(&mut self, feed: Option<Feed>) {
        self.feed = feed;
        self.announce();
        self.catch_up();
    }

    /// The hub, new, learns the replies going and queued, in the order they were made: each a
    /// run (named anew), the streaming ones fed their parts so far. A question among them gets
    /// the live run's interrupt id too.
    fn catch_up(&mut self) {
        let Some(feed) = self.feed.clone() else { return };
        let mut going: Vec<usize> = (0..self.replies.len()).filter(|&i| matches!(self.replies[i].state, State::Streaming | State::Queued)).collect();
        going.sort_by_key(|&i| self.replies[i].token);
        let start = now_ms();
        for at in going {
            let reply = &self.replies[at];
            let (token, id) = (reply.token, reply.id);
            let Some(message) = self.messages.iter().find(|m| m.id == id) else { continue };
            let pngs: Vec<&[u8]> = message.pngs.iter().map(|p| p.as_slice()).collect();
            feed.with(|hub| hub.reply(token, id, &message.text, &pngs, None));
            if !reply.streaming() {
                continue;
            }
            let events = saved::part_events(reply);
            // Its parts came before now, in the order and spacing they came.
            let since = start.saturating_sub(events.last().map_or(0, |(_, ms)| *ms));
            let questions: Vec<usize> = reply.parts.iter().enumerate().filter(|(_, p)| matches!(p, Part::Question(_))).map(|(i, _)| i).collect();
            let mut asked = questions.into_iter();
            let mut sent: Vec<(String, usize)> = Vec::new();
            for (event, ms) in events {
                let question = matches!(event, crate::reply::Event::Ask { .. });
                let ids = feed.with(|hub| hub.event(token, &event, since + ms));
                if question && let Some(part) = asked.next() {
                    sent.extend(ids.into_iter().map(|i| (i, part)));
                }
            }
            for (interrupt, part) in sent {
                self.asked.insert(interrupt, (token, part));
            }
        }
    }

    /// The conversation clients see, the one whose `threadId` commands carry: after Cmd+N in the
    /// side panel, the new one the next message starts.
    pub fn thread(&self) -> &str {
        self.over.as_deref().unwrap_or(&self.conv.id)
    }

    /// The conversation is a new one (or was loaded): the hub streams it from its snapshot.
    pub(super) fn announce(&self) {
        if let Some(feed) = &self.feed {
            let snapshot = self.messages_snapshot(&[]);
            feed.with(|hub| hub.conversation(self.thread(), &snapshot));
        }
    }

    /// The same conversation, changed in place: every client gets it afresh, its snapshot
    /// without the runs going, then those from their start.
    pub(super) fn resync(&self) {
        let Some(feed) = &self.feed else { return };
        feed.with(|hub| {
            let snapshot = self.messages_snapshot(&hub.live_messages());
            hub.resync(&snapshot);
        });
    }

    /// Reply stream `token` retries its message's reply (`last`: the conversation's last):
    /// clients get the conversation without that message, whose new run brings it back; one
    /// further up comes after the others, so once it ends, another snapshot puts it in place.
    pub(super) fn note_retry(&mut self, token: u64, last: bool) {
        if self.feed.is_none() {
            return;
        }
        self.resync_at = (!last).then_some(token);
        self.resync();
    }

    /// Reply stream `token` starts for message `id`: the hub knows which run it is, and takes
    /// the client's run id given for it.
    pub(super) fn note_run(&mut self, token: u64, id: u64) {
        let Some(feed) = &self.feed else { return };
        let Some(message) = self.messages.iter().find(|m| m.id == id) else { return };
        let pngs: Vec<&[u8]> = message.pngs.iter().map(|p| p.as_slice()).collect();
        let run = self.client_run.take();
        feed.with(|hub| hub.reply(token, id, &message.text, &pngs, run));
    }

    /// The next event of reply stream `token`; the interrupt ids of the questions it sent.
    pub(super) fn note_event(&self, token: u64, event: &crate::reply::Event) -> Vec<String> {
        match &self.feed {
            Some(feed) => feed.with(|hub| hub.event(token, event, now_ms())),
            None => Vec::new(),
        }
    }

    /// Reply `at`'s newest question went out: a client answers it by an id it was given, the
    /// live run's (`sent`) or the one a snapshot gives it (`{saved run}-ask-{n}`), never by one
    /// made up.
    pub(super) fn note_question(&mut self, at: usize, sent: Vec<String>) {
        let reply = &self.replies[at];
        let questions = || reply.parts.iter().enumerate().filter(|(_, p)| matches!(p, Part::Question(_)));
        let (Some((part, _)), n) = (questions().next_back(), questions().count()) else { return };
        let Some(index) = self.messages.iter().position(|m| m.id == reply.id) else { return };
        let saved = format!("{}-ask-{n}", history::run_id(&self.conv.id, index));
        let token = reply.token;
        for id in sent.into_iter().chain([saved]) {
            self.asked.insert(id, (token, part));
        }
    }

    /// The streams `tokens` stopped. Once the conversation is over (Cmd+N in the side panel),
    /// clients are on the new thread: a stop of the old one's reply never reaches it.
    pub(super) fn note_stop(&self, tokens: &[u64]) {
        if self.over.is_some() {
            self.forget_runs(tokens);
            return;
        }
        if let Some(feed) = &self.feed {
            feed.with(|hub| hub.stop(tokens, now_ms()));
        }
    }

    /// The streams `tokens` are of the conversation left with Cmd+N, not the thread clients see:
    /// their runs leave the hub, one clients saw start ending cancelled (`Hub::forget`).
    pub(super) fn forget_runs(&self, tokens: &[u64]) {
        if let Some(feed) = &self.feed {
            feed.with(|hub| hub.forget(tokens, now_ms()));
        }
    }

    /// The retried reply out of its place (`resync_at`) ended, however it did (done, failed, its
    /// process lost, stopped, or stopped by the next reply): clients get the conversation with it
    /// back in its place.
    pub(super) fn settle_resync(&mut self) {
        let Some(token) = self.resync_at else { return };
        if self.replies.iter().any(|r| r.token == token && r.live()) {
            return;
        }
        self.resync_at = None;
        self.resync();
    }

    /// The conversation as a `MESSAGES_SNAPSHOT`, without the messages `skip` (their runs are
    /// still going: they come as runs). Run ids are the saved ones, not the live ones; the
    /// thread (AG-UI's snapshot has no field for it) is in `metadata.ailfred.threadId`.
    pub fn messages_snapshot(&self, skip: &[u64]) -> agui::Event {
        // Over (Cmd+N in the side panel): the new one, empty, though the old still shows.
        let messages: &[Message] = if self.over.is_some() { &[] } else { &self.messages };
        let runs: Vec<String> = (0..messages.len()).map(|i| history::run_id(&self.conv.id, i)).collect();
        let pngs: Vec<Vec<&[u8]>> = messages.iter().map(|m| m.pngs.iter().map(|p| p.as_slice()).collect()).collect();
        let turns: Vec<Turn> = messages
            .iter()
            .enumerate()
            .filter(|(_, m)| !skip.contains(&m.id))
            .map(|(i, m)| Turn { run: &runs[i], text: &m.text, pngs: &pngs[i], reply: self.reply_to(m.id) })
            .collect();
        let mut snapshot = snapshot::messages_snapshot(&turns, now_ms());
        if let agui::Event::MessagesSnapshot { base, .. } = &mut snapshot
            && let Value::Object(metadata) = json!({ "ailfred": { "threadId": self.thread() } })
        {
            base.metadata = Some(metadata);
        }
        snapshot
    }

    /// A client joins: `watcher` gets the snapshot, the run going from its start, then what
    /// comes. Nothing without a hub (the server stopped).
    pub fn join(&self, watcher: SyncSender<Frame>) {
        let Some(feed) = &self.feed else { return };
        feed.with(|hub| {
            let snapshot = self.messages_snapshot(&hub.live_messages());
            hub.join(watcher, &snapshot);
        });
    }

    /// The web view joins (again, after a reload): `sink` gets what [`Chat::join`] gives a
    /// client, and replaces the sink it had. Nothing without a hub.
    pub fn join_local(&self, sink: Sink) {
        let Some(feed) = &self.feed else { return };
        feed.with(|hub| {
            let snapshot = self.messages_snapshot(&hub.live_messages());
            hub.join_local(sink, &snapshot);
        });
    }

    /// The web view closed: its sink leaves the hub.
    pub fn leave_local(&self) {
        if let Some(feed) = &self.feed {
            feed.with(|hub| hub.leave_local());
        }
    }

    /// A client's command, done as the window would do it; what to answer.
    pub fn command(&mut self, command: Command, now: Instant) -> Answer {
        let Some(feed) = self.feed.clone() else { return Answer::error(503, "the local server is off") };
        let (thread, run) = match &command {
            Command::Run(input) => (&input.thread_id, Some(&input.run_id)),
            Command::Stop { thread_id, run_id } | Command::Retry { thread_id, run_id, .. } => (thread_id, Some(run_id)),
            Command::NewConversation { thread_id } => (thread_id, None),
        };
        if run.is_some_and(|r| !run_id_ok(r)) {
            return Answer::error(400, "runId must be 1 to 128 characters of A-Z, a-z, 0-9, '.', '_', ':' and '-'");
        }
        if thread != self.thread() {
            return Answer::error(409, "threadId isn't the conversation in the chat");
        }
        let run = run.cloned().unwrap_or_default();
        let saved = saved_shape(&run, self.thread());
        let fresh = |run: &str| {
            if saved {
                return Err(Answer::error(400, "runId must not be {threadId}-{number}: that names a saved run in snapshots"));
            }
            if feed.with(|hub| hub.seen(run)) { Err(Answer::error(409, "runId was used in this thread already")) } else { Ok(()) }
        };
        let done = match command {
            Command::Stop { .. } => {
                let streams = feed.with(|hub| self.replies.iter().any(|r| r.streaming() && hub.run_of(r.token) == Some(run.as_str())));
                if !streams {
                    return Answer::error(409, "runId isn't the run streaming");
                }
                self.stop(now);
                Ok(())
            }
            Command::Run(input) => fresh(&run).and_then(|()| match input.resume.filter(|r| !r.is_empty()) {
                Some(resume) => self.client_answer(&feed, &run, &resume),
                None => self.client_message(&run, &input.messages),
            }),
            Command::Retry { reply_run_id, .. } => fresh(&run).and_then(|()| self.client_retry(&feed, &run, &reply_run_id)),
            Command::NewConversation { .. } => {
                // As Cmd+N: in the window, it clears there; in the side panel, the conversation
                // is over (the host slides it out). What streams stops either way.
                if self.windowed {
                    self.clear();
                } else {
                    self.start_over();
                }
                return Answer::new_thread(self.thread());
            }
        };
        match done {
            Ok(()) => Answer::accepted(&self.conv.id, &run),
            Err(refused) => refused,
        }
    }

    /// The last of `messages`, the user's, sent as if typed here, as run `run`.
    fn client_message(&mut self, run: &str, messages: &[agui::Message]) -> Result<(), Answer> {
        let Some(agui::Message::User { content, .. }) = messages.last() else {
            return Err(Answer::error(400, "the last message must be the user's"));
        };
        let (text, pngs) = user_said(content).map_err(|e| match e {
            Unsaid::NotPng => Answer::error(400, "only text and inline base64 PNG images are supported"),
            Unsaid::Empty => Answer::error(400, "the message has neither text nor an image"),
        })?;
        let id = self.next_id();
        self.client_run = Some(run.to_owned());
        self.push(Message::text(id, text.trim()).with_pngs(&pngs));
        self.ask(id);
        self.client_run = None;
        if !pngs.is_empty()
            && let Some(host) = &self.host
        {
            host.update(move |host, _| host.decode_into_chat(id, pngs));
        }
        Ok(())
    }

    /// Retries the reply of run `reply` (its live run's id, or its snapshot's) as run `run`, if
    /// the window offers Retry on it.
    fn client_retry(&mut self, feed: &Feed, run: &str, reply: &str) -> Result<(), Answer> {
        let token = self.retryable(feed, reply).ok_or_else(|| Answer::error(409, "replyRunId names no reply that can be retried: one of this conversation that ended (done, stopped or failed)"))?;
        self.client_run = Some(run.to_owned());
        self.retry(token);
        self.client_run = None;
        Ok(())
    }

    /// The stream of the reply run `run` is (the hub's name for it, or its snapshot's
    /// `{conversation}-{index}`, matched whole), if it offers Retry. None once the conversation
    /// is over.
    fn retryable(&self, feed: &Feed, run: &str) -> Option<u64> {
        if self.over.is_some() {
            return None;
        }
        feed.with(|hub| {
            self.messages.iter().enumerate().find_map(|(i, m)| {
                let r = self.reply_to(m.id).filter(|r| Chat::offers_retry(r))?;
                (hub.run_of(r.token) == Some(run) || history::run_id(&self.conv.id, i) == run).then_some(r.token)
            })
        })
    }

    /// The latest reply's question is pi's, whose dialog waits for the answer: pi wrote the reply
    /// last and is the model picked (as an answer goes to it), not the mock.
    fn pi_waits(&self) -> bool {
        self.running_model == model::PI && self.model() == model::PI && !mock_model::requested()
    }

    /// `resume` answers or declines the open questions of the latest reply, as run `run`: each
    /// by an interrupt id it was sent (its live run's, or its snapshot's), `resolved` with
    /// `payload.answer`, or `cancelled`. The answers go as one message, as in the window. A
    /// declined question closes ("Not answered"); declined alone, pi's question has its dialog
    /// dismissed and pi goes on, its words the reply to a `DECLINED` message run `run` is; any
    /// other model's run ended with the question and nothing waits, so nothing goes to it.
    /// Clients get the change in a snapshot.
    fn client_answer(&mut self, feed: &Feed, run: &str, resume: &[Resume]) -> Result<(), Answer> {
        let conflict = |reason: &str| Err(Answer::error(409, reason));
        if self.over.is_some() {
            return conflict("the conversation is over: send a message to start a new one");
        }
        let Some(last) = self.messages.last() else { return conflict("no question waits for an answer") };
        let Some(reply) = self.reply_to(last.id) else { return conflict("no question waits for an answer") };
        let waits = !matches!(reply.state, State::Failed | State::Stopped);
        let mut answers: Vec<(usize, String)> = Vec::new();
        let mut declined: Vec<usize> = Vec::new();
        for entry in resume {
            let Some(&(_, part)) = self.asked.get(&entry.interrupt_id).filter(|(token, _)| *token == reply.token) else {
                return conflict("interruptId names no question of the latest reply");
            };
            if !waits || !matches!(&reply.parts[part], Part::Question(q) if q.open()) {
                return conflict("that question takes no answer: answered, or closed");
            }
            // Ailfred's guard asks it: it approves an action on the user's accounts or computer,
            // so only the window answers it. A decline only blocks the action.
            if entry.status == ResumeStatus::Resolved && matches!(&reply.parts[part], Part::Question(q) if q.local_only()) {
                return Err(Answer::error(403, "Answer in Ailfred's window: this question approves an action on the user's accounts or computer"));
            }
            let text = match entry.status {
                ResumeStatus::Cancelled => None,
                ResumeStatus::Resolved => match entry.payload.as_ref().and_then(|p| p.get("answer")).and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
                    Some(text) => Some(text.to_owned()),
                    None => return Err(Answer::error(400, "payload.answer must be a non-empty string")),
                },
            };
            if answers.iter().any(|(p, _)| *p == part) || declined.contains(&part) {
                return Err(Answer::error(400, "a question is answered or declined twice"));
            }
            match text {
                Some(text) => answers.push((part, text)),
                None => declined.push(part),
            }
        }
        let token = reply.token;
        for &part in &declined {
            if let Some(q) = self.question_mut(token, part) {
                q.closed = true;
            }
        }
        if answers.is_empty() && self.pi_waits() {
            // pi keeps its dialog open: it's dismissed, and what pi says next is the reply to
            // the decline, sent as the user's message as an answer is.
            let id = self.next_id();
            self.client_run = Some(run.to_owned());
            self.push(Message::text(id, DECLINED));
            self.start_reply_as(id, DECLINED.into(), true, true);
            self.client_run = None;
        } else if answers.is_empty() {
            feed.with(|hub| hub.claim(run));
        } else {
            self.client_run = Some(run.to_owned());
            let started = self.answer_with(token, &answers);
            self.client_run = None;
            started.ok_or_else(|| Answer::error(409, "no question took the answer"))?;
        }
        if !declined.is_empty() {
            self.resync();
        }
        Ok(())
    }
}

/// Run id `run` is what snapshots of thread `thread` name a saved run: `{thread}-{index}`. A
/// client's may not be, so a Retry's `replyRunId` names one reply only.
fn saved_shape(run: &str, thread: &str) -> bool {
    run.strip_prefix(thread).and_then(|r| r.strip_prefix('-')).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A client's run id: 1 to 128 characters of `[A-Za-z0-9._:-]`.
fn run_id_ok(run: &str) -> bool {
    (1..=128).contains(&run.len()) && run.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_:".contains(&b))
}
