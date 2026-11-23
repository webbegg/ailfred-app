//! The chat and the apps watching it through the local server (`crate::server`): what happens
//! here reaches its hub (a new conversation, a reply starting, its events, Stop), and what a
//! client sends comes in as if typed here.
//!
//! The chat's conversation is the truth: a run's `messages` aren't replayed into the model;
//! its last one, the user's, is sent as a message typed here would be (a steer, queued or
//! asked at once, to the model picked). A `resume` answers the open questions of the latest
//! reply instead, each by an interrupt id it was sent.

use std::sync::mpsc::SyncSender;
use std::time::Instant;

use ailfred_protocol::agui::{self, Resume, ResumeStatus};
use ailfred_protocol::command::{Command, Unsaid, user_said};
use ailfred_protocol::snapshot::{self, Turn};
use serde_json::{Value, json};

use super::{Chat, Message};
use crate::history;
use crate::reply::{Part, State};
use crate::server::{Answer, Feed, Frame, now_ms};

impl Chat {
    /// The local server's hub (None: it stopped). Set, it gets the conversation as it is.
    pub fn set_feed(&mut self, feed: Option<Feed>) {
        self.feed = feed;
        self.announce();
    }

    /// The conversation is a new one (or was loaded): the hub streams it from its snapshot.
    pub(super) fn announce(&self) {
        if let Some(feed) = &self.feed {
            let snapshot = self.messages_snapshot(&[]);
            feed.with(|hub| hub.conversation(&self.conv.id, &snapshot));
        }
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

    pub(super) fn note_stop(&self, tokens: &[u64]) {
        if let Some(feed) = &self.feed {
            feed.with(|hub| hub.stop(tokens, now_ms()));
        }
    }

    /// The conversation as a `MESSAGES_SNAPSHOT`, without the messages `skip` (their runs are
    /// still going: they come as runs). Run ids are the saved ones, not the live ones; the
    /// thread (AG-UI's snapshot has no field for it) is in `metadata.ailfred.threadId`.
    pub fn messages_snapshot(&self, skip: &[u64]) -> agui::Event {
        let runs: Vec<String> = (0..self.messages.len()).map(|i| history::run_id(&self.conv.id, i)).collect();
        let pngs: Vec<Vec<&[u8]>> = self.messages.iter().map(|m| m.pngs.iter().map(|p| p.as_slice()).collect()).collect();
        let turns: Vec<Turn> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| !skip.contains(&m.id))
            .map(|(i, m)| Turn { run: &runs[i], text: &m.text, pngs: &pngs[i], reply: self.reply_to(m.id) })
            .collect();
        let mut snapshot = snapshot::messages_snapshot(&turns, now_ms());
        if let agui::Event::MessagesSnapshot { base, .. } = &mut snapshot
            && let Value::Object(metadata) = json!({ "ailfred": { "threadId": self.conv.id } })
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

    /// A client's command, done as the window would do it; what to answer.
    pub fn command(&mut self, command: Command, now: Instant) -> Answer {
        let Some(feed) = self.feed.clone() else { return Answer::error(503, "the local server is off") };
        let (thread, run) = match &command {
            Command::Run(input) => (&input.thread_id, &input.run_id),
            Command::Stop { thread_id, run_id } => (thread_id, run_id),
        };
        if !run_id_ok(run) {
            return Answer::error(400, "runId must be 1 to 128 characters of A-Z, a-z, 0-9, '.', '_', ':' and '-'");
        }
        if *thread != self.conv.id {
            return Answer::error(409, "threadId isn't the conversation in the chat");
        }
        let run = run.clone();
        let done = match command {
            Command::Stop { .. } => {
                let streams = feed.with(|hub| self.replies.iter().any(|r| r.streaming() && hub.run_of(r.token) == Some(run.as_str())));
                if !streams {
                    return Answer::error(409, "runId isn't the run streaming");
                }
                self.stop(now);
                Ok(())
            }
            Command::Run(input) => {
                if feed.with(|hub| hub.seen(&run)) {
                    return Answer::error(409, "runId was used in this thread already");
                }
                match input.resume.filter(|r| !r.is_empty()) {
                    Some(resume) => self.client_answer(&run, &resume),
                    None => self.client_message(&run, &input.messages),
                }
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

    /// `resume` answers the open questions of the latest reply, as run `run`: each by an
    /// interrupt id it was sent (its live run's, or its snapshot's), resolved, with
    /// `payload.answer`.
    fn client_answer(&mut self, run: &str, resume: &[Resume]) -> Result<(), Answer> {
        let conflict = |reason: &str| Err(Answer::error(409, reason));
        if self.over {
            return conflict("the conversation is over: send a message to start a new one");
        }
        let Some(last) = self.messages.last() else { return conflict("no question waits for an answer") };
        let Some(reply) = self.reply_to(last.id) else { return conflict("no question waits for an answer") };
        let waits = !matches!(reply.state, State::Failed | State::Stopped);
        let mut answers: Vec<(usize, String)> = Vec::new();
        for entry in resume {
            if entry.status == ResumeStatus::Cancelled {
                return conflict("declining a question isn't supported yet");
            }
            let Some(&(_, part)) = self.asked.get(&entry.interrupt_id).filter(|(token, _)| *token == reply.token) else {
                return conflict("interruptId names no question of the latest reply");
            };
            if !waits || !matches!(&reply.parts[part], Part::Question(q) if q.open()) {
                return conflict("that question takes no answer: answered, or closed");
            }
            let Some(text) = entry.payload.as_ref().and_then(|p| p.get("answer")).and_then(Value::as_str).filter(|t| !t.trim().is_empty()) else {
                return Err(Answer::error(400, "payload.answer must be a non-empty string"));
            };
            if answers.iter().any(|(p, _)| *p == part) {
                return Err(Answer::error(400, "a question is answered twice"));
            }
            answers.push((part, text.to_owned()));
        }
        let token = reply.token;
        self.client_run = Some(run.to_owned());
        let started = self.answer_with(token, &answers);
        self.client_run = None;
        started.map(|_| ()).ok_or_else(|| Answer::error(409, "no question took the answer"))
    }
}

/// A client's run id: 1 to 128 characters of `[A-Za-z0-9._:-]`.
fn run_id_ok(run: &str) -> bool {
    (1..=128).contains(&run.len()) && run.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_:".contains(&b))
}
