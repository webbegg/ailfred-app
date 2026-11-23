//! The chat's conversation on disk: what it hands the host to save, what it takes in when one is
//! loaded (at launch, or picked from the menu), and the automatic title.

use std::time::Instant;

use super::{Chat, LOCAL_IDS, Message, PAGE};
use crate::history;
use crate::mock_model;
use crate::model;
use crate::reply::State;

impl Chat {
    /// A new conversation starts: nothing of the old one stays (it's saved already), and it has
    /// an id of its own.
    pub(super) fn reset_conversation(&mut self) {
        self.session = None;
        self.messages.clear();
        self.hidden = 0;
        self.replies.clear();
        self.others.clear();
        self.kept.clear();
        self.asked.clear();
        self.resync_at = None;
        self.draft.clear();
        self.shots.clear();
        self.conv = history::Meta::fresh();
    }

    /// The conversation as it is, to write; None while it has no message. The captures not
    /// yet kept get their file names here; a loaded reply that keeps what was saved of it
    /// writes those events and answers back.
    pub fn snapshot(&mut self) -> Option<history::Snapshot> {
        if self.messages.is_empty() {
            return None;
        }
        let mut pngs = Vec::new();
        for message in &mut self.messages {
            while message.files.len() < message.pngs.len() {
                message.files.push(format!("{}/{}.png", self.conv.id, self.conv.next_png));
                self.conv.next_png += 1;
            }
            pngs.extend(message.files.iter().cloned().zip(message.pngs.iter().cloned()));
        }
        let messages = self
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| history::SavedMessage {
                text: m.text.clone(),
                pngs: m.files.clone(),
                reply: self.replies.iter().find(|r| r.id == m.id).map(|r| {
                    let mut saved = history::saved_reply(r, &self.conv.id, &history::run_id(&self.conv.id, i));
                    if let Some(kept) = self.kept.get(&r.token) {
                        (saved.events, saved.answers) = (kept.events.clone(), kept.answers.clone());
                    }
                    saved
                }),
            })
            .collect();
        let session = self.session.as_ref().and_then(|(id, ends_at)| {
            let after_message = self.messages.iter().position(|m| m.id == *ends_at)?;
            Some(history::SavedSession { id: id.clone(), after_message })
        });
        let saved = history::Saved {
            version: history::VERSION,
            id: self.conv.id.clone(),
            created: self.conv.created,
            updated: history::now_secs(),
            title: self.conv.title.clone(),
            title_by_user: self.conv.title_by_user,
            session,
            messages,
        };
        Some(history::Snapshot { saved, pngs })
    }

    /// Hands the conversation to the host to write (its thread does; this one never does file
    /// work).
    pub(super) fn save(&mut self) {
        let Some(snapshot) = self.snapshot() else { return };
        #[cfg(test)]
        self.saved.push(snapshot.clone());
        if let Some(host) = &self.host {
            host.update(move |host, _| host.save_conversation(snapshot));
        }
    }

    /// Reply `token` just ended (done or failed): the conversation is saved, and its first
    /// reply getting done asks for a title.
    pub(super) fn reply_ended(&mut self, token: u64) {
        self.save();
        let Some(request) = self.title_request(token) else { return };
        #[cfg(test)]
        self.title_asks.push(request.clone());
        if let Some(host) = &self.host {
            let id = self.conv.id.clone();
            host.update(move |host, _| host.ask_title(id, request));
        }
    }

    /// The call that titles the conversation, if reply `token` ending now is the one to ask
    /// after: done, to the conversation's first message, which has no title yet and wasn't
    /// asked about. Asked once; never with the mock model.
    fn title_request(&mut self, token: u64) -> Option<model::Request> {
        if self.conv.title.is_some() || self.conv.title_by_user || self.conv.title_asked || mock_model::requested() {
            return None;
        }
        let first = self.messages.first()?;
        let reply = self.replies.iter().find(|r| r.token == token)?;
        if reply.id != first.id || reply.state != State::Done || reply.answer {
            return None;
        }
        let request = model::title_request(&first.text, &reply.history_text()?);
        self.conv.title_asked = true;
        Some(request)
    }

    /// An automatic title arrived for conversation `id`: this one's gets it (unless it has a
    /// title, or the user wrote it); another's, which the chat has moved on from, goes to its
    /// file.
    pub fn title_arrived(&mut self, id: &str, title: String) {
        if id == self.conv.id {
            if self.conv.title.is_none() && !self.conv.title_by_user {
                self.conv.title = Some(title);
                self.save();
            }
        } else if let Some(host) = &self.host {
            let id = id.to_owned();
            host.update(move |host, _| host.patch_title(id, title));
        }
    }

    /// Takes in a conversation read from disk, in place of this one: what streams is stopped
    /// first (and saved). Its messages get new ids; its captures are decoded off the main
    /// thread. The one that's here already stays as it is.
    pub fn load(&mut self, loaded: history::Loaded) {
        if loaded.id == self.conv.id && !self.messages.is_empty() {
            // Left with Cmd+N in the side panel, it's picked back: it goes on, its thread
            // clients' again.
            if self.over.take().is_some() {
                self.announce();
            }
            return;
        }
        self.end_replies(Instant::now());
        // pi keeps a session of its own: it starts a new one, not the one it had.
        self.new_conversation();
        self.reset_conversation();
        (self.preview, self.over, self.away) = (None, None, false);
        self.conv = history::Meta {
            id: loaded.id,
            created: loaded.created,
            title: loaded.title,
            title_by_user: loaded.title_by_user,
            next_png: loaded.next_png,
            title_asked: false,
        };
        let mut to_decode = Vec::new();
        for (id, read) in (LOCAL_IDS..).zip(loaded.messages) {
            let mut message = Message::text(id, read.text);
            message.pngs = read.pngs;
            message.files = read.files;
            if let Some(saved) = &read.reply {
                self.next_token += 1;
                let (reply, kept) = history::read_reply(saved, id, &message.text, self.next_token);
                if let Some(kept) = kept {
                    self.kept.insert(reply.token, kept);
                }
                self.replies.push(reply);
            }
            if !message.pngs.is_empty() {
                to_decode.push((id, message.pngs.iter().map(|p| p.to_vec()).collect::<Vec<_>>()));
            }
            self.messages.push(message);
        }
        // Long, it opens at its latest page.
        self.hidden = self.messages.len().saturating_sub(PAGE);
        self.session = loaded.session.and_then(|(session, index)| Some((session, self.messages.get(index)?.id)));
        self.announce();
        if let Some(host) = &self.host {
            for (id, pngs) in to_decode {
                host.update(move |host, _| host.decode_into_chat(id, pngs));
            }
        }
    }
}
