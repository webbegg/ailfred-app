//! The chat side panel: sending from the quick input turns the input's own panel into it (same
//! window, same element: it springs to the right edge, reshaping), with what was sent as the
//! first message and its own field focused. It stays open
//! when another app is clicked; Esc or its close button slide it back out, and the input's
//! shortcut brings it back. Cmd+N slides it out for good and opens a new input. Each message
//! gets a reply from the model (`model.rs`: Claude, or the mock in tests), which the host
//! streams in; Stop and Retry end the run behind it.

use rune::tok;
use rune::{
    Anim, AppHandle, Color, Curve, Cursor, El, GrowthSpec, Key, KeyEvent, Pixels, ScrollSpec, Shadow, Spring, TextEdit, TextStyle, Transition, Tx,
    auto_grow, col, el, icon, row, text, text_input,
};
use rune_kit::glyphs;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::Host;
use crate::history;
use crate::icons;
use crate::mock_model;
use crate::model;
use crate::reply::{self, Event, Reply};
use crate::reply_view;
use crate::settings;
use crate::quick_input::{self, ACCENT, Attached, Escape};

mod clients;
#[cfg(test)]
mod clients_tests;
mod geometry;
mod images;
mod saving;
#[cfg(test)]
mod saving_tests;
#[cfg(test)]
pub(crate) mod tests;
mod view;
#[cfg(test)]
mod web_view_tests;

pub use geometry::{Insets, insets, offset, side_pins, side_rect, thumb_size};
pub use images::{Field, Image, Pasted, decode_pasted, decode_png, lightbox, paste_lands, pasted_images, take_ids};
#[cfg(test)]
pub use images::image_file_png;
pub use view::{HEADER_HEIGHT, body, dock_button, header, on_key, panel};

/// The panel's width and its gap to the screen's edges, in points. Its corners are the input's
/// (`quick_input::RADIUS`): it's the same panel.
pub const CHAT_W: f32 = 400.0;
/// The slide in from the edge and back, and the input's panel reshaping into this one.
const GLIDE: Spring = tok::motion::GLIDE;
/// How long after the input starts reshaping the content fades in, and over how long; and how
/// fast the input's row fades as it goes.
pub const LANDING: std::time::Duration = std::time::Duration::from_millis(260);
const CONTENT_FADE: f32 = 0.2;
pub const ROW_FADE: f32 = 0.12;

/// Messages typed in the panel take ids from here; the host's (sent from the input) are below.
const LOCAL_IDS: u64 = 1 << 32;

/// The message Continue sends, under an interrupted reply.
pub const CONTINUE: &str = "Continue where you left off.";

/// The user's message when a client declines pi's question: pi's dialog is dismissed, and what
/// pi says next is its reply.
pub const DECLINED: &str = "Declined.";

/// What the quick input's shortcut does.
#[derive(Debug, PartialEq)]
pub enum Toggle {
    ShowInput,
    HideInput,
    ShowChat,
    HideChat,
}

/// The shortcut with the input `quick_open`, a conversation `started` (not replaced by Cmd+N),
/// shown or not (`chat_open`): an open input closes; a conversation comes back instead of a new
/// input, or goes; otherwise the input opens.
/// The chat's back in the side panel from its window: it slides in, or, emptied there, a
/// fresh input rises as Cmd+N's does.
pub fn on_dock(has_messages: bool) -> Toggle {
    if has_messages { Toggle::ShowChat } else { Toggle::ShowInput }
}

/// The mock writes this reply (`answer`: to a question; `model`: the one picked;
/// `requested`: `AILFRED_MOCK_MODEL=1`).
pub fn by_mock(answer: bool, model: &str, requested: bool) -> bool {
    requested || (answer && model != model::PI)
}

pub fn toggle(quick_open: bool, started: bool, chat_open: bool, windowed: bool) -> Toggle {
    // The chat in its window: the shortcut is the input's, to send to it.
    let started = started && !windowed;
    match (quick_open, started, chat_open) {
        (true, _, _) => Toggle::HideInput,
        (false, true, false) => Toggle::ShowChat,
        (false, true, true) => Toggle::HideChat,
        (false, false, _) => Toggle::ShowInput,
    }
}

/// One sent message: its text (maybe empty) and its captures.
pub struct Message {
    /// Its captures arrive once decoded (`set_images`).
    pub id: u64,
    pub text: String,
    pub images: Vec<Image>,
    /// Its captures' PNG bytes, for the model (the conversation's later messages send them again).
    pub pngs: Vec<Arc<Vec<u8>>>,
    /// Where each capture is kept on disk (`history`), once the conversation has been saved.
    pub files: Vec<String>,
}

impl Message {
    /// A message with `text` and no captures.
    pub fn text(id: u64, text: impl Into<String>) -> Message {
        Message { id, text: text.into(), images: Vec::new(), pngs: Vec::new(), files: Vec::new() }
    }

    /// The same with the captures `pngs` (copied: the originals go to be decoded).
    pub fn with_pngs(mut self, pngs: &[Vec<u8>]) -> Message {
        self.pngs = pngs.iter().map(|p| Arc::new(p.clone())).collect();
        self
    }
}

/// How many messages a long conversation opens with, and how many more each "Show earlier"
/// brings: the panel draws only those (a long one would cost every frame).
pub const PAGE: usize = 40;

pub struct Chat {
    messages: Vec<Message>,
    /// How many of the first messages don't show (opened long: all but the latest page).
    hidden: usize,
    draft: TextEdit,
    /// Captures pasted into the field, to go with the next message.
    shots: Vec<Attached>,
    /// The capture whose chip was clicked, shown enlarged.
    preview: Option<u64>,
    open: bool,
    insets: Insets,
    /// The display's size in points: the overlay covers it.
    screen: [f32; 2],
    /// Its content shows: false while the input's panel reshapes into it, faded in once there
    /// (`land`).
    landed: bool,
    /// Cmd+N: the conversation is over; it still shows while sliding out, and the next message
    /// starts a new one, with this id: clients see that one at once (`thread`), empty.
    over: Option<String>,
    /// A retried reply that isn't the conversation's last, by its stream: its run comes after
    /// the others on a client, so once it ends a snapshot puts it back in its place.
    resync_at: Option<u64>,
    /// Where Esc and the close button go (None in tests).
    host: Option<AppHandle<Host>>,
    /// The next pasted image's id.
    next_shot: u64,
    /// The model's replies, one per message at most (by its id).
    replies: Vec<Reply>,
    /// The last reply stream started: its events come back with it.
    next_token: u64,
    /// The questions' "something else" fields, by reply token and part: the user's own answer
    /// as they write it (the reply only gets its text when they answer).
    others: HashMap<(u64, usize), TextEdit>,
    /// What loaded replies keep of what was saved, by token: events this build didn't make them
    /// from whole, written back as they were (`history::Kept`).
    kept: HashMap<u64, history::Kept>,
    /// A text field for a question gone meanwhile.
    scratch: TextEdit,
    /// The user scrolled back from the latest: what comes keeps their place, and a button
    /// takes them back down.
    away: bool,
    /// It's in its own window (popped out), not the side panel: Esc doesn't close it.
    windowed: bool,
    /// Claude Code's session of this conversation, once it has one (`set_session`), and the
    /// message whose reply it ends at: the next message resumes it. A new conversation has none.
    session: Option<(String, u64)>,
    /// What the conversation is on disk: its id, title and the next capture's number.
    conv: history::Meta,
    /// The model of the reply streaming last: a message sent meanwhile steers pi's run, and
    /// waits for any other's.
    running_model: String,
    /// The model picker's list shows.
    models_open: bool,
    /// The local server's hub, while it's on: what happens here reaches the apps watching
    /// (`clients`).
    feed: Option<crate::server::Feed>,
    /// The run id a client's command gave the message being sent: its reply's run takes it.
    client_run: Option<String>,
    /// The questions that went out, by the interrupt ids a client may answer them with: their
    /// reply stream and part.
    asked: HashMap<String, (u64, usize)>,
    /// Replies steered into a run that was stopped: pi may have read one just before. pi says
    /// which: one it read gets `Taken` (and stops with the run), one it didn't `Dropped` (and
    /// goes again).
    cut: Vec<u64>,
    /// The streams whose runs were cancelled (the host ends their processes), for tests.
    #[cfg(test)]
    cancelled: Vec<u64>,
    /// What was handed over to be saved, and the title calls asked for, for tests.
    #[cfg(test)]
    saved: Vec<history::Snapshot>,
    #[cfg(test)]
    title_asks: Vec<model::Request>,
    /// The model picked, in tests (else the setting's).
    #[cfg(test)]
    model: Option<String>,
    /// What went to the model, by reply token, for tests.
    #[cfg(test)]
    requests: Vec<(u64, model::Request)>,
}

impl Chat {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        Chat { messages: Vec::new(), hidden: 0, draft: TextEdit::new(""), shots: Vec::new(), preview: None, open: false, insets: Insets::default(), screen: [1440.0, 900.0], landed: true, over: None, resync_at: None, host, next_shot: 0, replies: Vec::new(), next_token: 0, others: HashMap::new(), kept: HashMap::new(), scratch: TextEdit::new(""), away: false, windowed: false, session: None, conv: history::Meta::fresh(), running_model: String::new(), models_open: false, feed: None, client_run: None, asked: HashMap::new(), cut: Vec::new(), #[cfg(test)] cancelled: Vec::new(), #[cfg(test)] saved: Vec::new(), #[cfg(test)] title_asks: Vec::new(), #[cfg(test)] model: None, #[cfg(test)] requests: Vec::new() }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn has_messages(&self) -> bool {
        !self.messages.is_empty()
    }

    /// How many earlier messages don't show ("Show earlier" brings them).
    #[cfg(test)]
    pub fn earlier(&self) -> usize {
        self.hidden
    }

    /// "Show earlier": the page before the first shown; false if all show.
    pub fn show_earlier(&mut self) -> bool {
        let had = self.hidden;
        self.hidden = had.saturating_sub(PAGE);
        had > 0
    }

    /// Into its own window (true), or back into the side panel, closed, so it slides in.
    pub fn set_windowed(&mut self, on: bool) {
        self.windowed = on;
        // It arrives at the latest (`Host::land_in_window`, `QuickInput::put_chat`).
        self.away = false;
        self.landed = true;
        self.preview = None;
        self.open = on;
        self.draft.focused = on;
    }

    #[cfg(test)]
    pub fn windowed(&self) -> bool {
        self.windowed
    }

    /// Cmd+N in its window: the conversation goes at once; what still streams feeds nothing.
    pub fn clear(&mut self) {
        self.end_replies(Instant::now());
        self.new_conversation();
        self.reset_conversation();
        (self.preview, self.over, self.away) = (None, None, false);
        self.draft.focused = true;
        self.announce();
    }

    /// Its pop-out button: the host moves it into its own window.
    fn pop_out(&self) {
        if let Some(host) = &self.host {
            host.update(|host, _| host.pop_out());
        }
    }

    /// Its window's dock button: the host moves it back into the side panel.
    fn dock_back(&self) {
        if let Some(host) = &self.host {
            host.update(|host, _| host.dock_back());
        }
    }

    #[cfg(test)]
    pub fn landed(&self) -> bool {
        self.landed
    }

    /// Its draft has the keys.
    #[cfg(test)]
    pub fn draft_focused(&self) -> bool {
        self.draft.focused
    }

    /// Before showing, on a display with these `insets`: out of view past its right edge, so
    /// `open` slides in from there. Applied without motion.
    pub fn place(&mut self, insets: Insets, screen: [f32; 2]) {
        self.insets = insets;
        self.screen = screen;
        self.open = false;
        self.landed = true;
    }

    /// Its field gives up the keys (the input's has them).
    pub fn blur(&mut self) {
        self.draft.focused = false;
    }

    /// Slides in, its field focused.
    pub fn open(&mut self) {
        self.open = true;
        self.draft.focused = true;
    }

    /// The input's panel becomes this one (after `place`): open at once with `message`, its
    /// content hidden until `land`.
    pub fn morph(&mut self, message: Message) {
        self.open = true;
        self.landed = false;
        self.push(message);
    }

    /// In place after the flight: its content fades in, its field taking the keys (they passed
    /// it by while it was transparent).
    pub fn land(&mut self) {
        self.landed = true;
        if self.open {
            self.draft.focused = true;
        }
    }

    /// The conversation is over: the model starts the next afresh (pi's session).
    fn new_conversation(&self) {
        if let Some(host) = &self.host {
            host.update(|host, _| host.new_conversation());
        }
    }

    /// The request that asks the model message `id` (None: no such message): the earlier
    /// messages whose reply came (done, or stopped with something) as the history. A turn is
    /// untrusted once any reply up to it read the connected apps or the screen, one with no text
    /// of its own (left out of the history) included: what came after can carry what it read.
    pub fn request_for(&self, id: u64, model: &str) -> Option<model::Request> {
        let at = self.messages.iter().position(|m| m.id == id)?;
        let mut tainted = false;
        let turns: Vec<(u64, model::Turn)> = self.messages[..at]
            .iter()
            .filter_map(|m| {
                let saved = self.replies.iter().find(|r| r.id == m.id)?;
                tainted |= saved.untrusted();
                let reply = saved.history_text()?;
                Some((m.id, model::Turn { text: m.text.clone(), pngs: m.pngs.clone(), reply, untrusted: tainted }))
            })
            .collect();
        // Resumed only when it ends where this message's history does: a retry's session
        // already holds the message and its old reply.
        let last = turns.last().map(|(id, _)| *id);
        let session = self.session.as_ref().filter(|(_, ends_at)| Some(*ends_at) == last).map(|(s, _)| s.clone());
        let history = turns.into_iter().map(|(_, turn)| turn).collect();
        let message = &self.messages[at];
        Some(model::Request { model: model.into(), history, text: message.text.clone(), pngs: message.pngs.clone(), session, answers: None, decline: false, steer: false })
    }

    /// Reply `token` reports Claude Code's session: the next message resumes it. A stream that's
    /// gone (stopped and replaced, or over with Cmd+N) reports nothing.
    pub fn set_session(&mut self, token: u64, session: String) {
        if let Some(reply) = self.reply_mut(token) {
            let ends_at = reply.id;
            self.session = Some((session, ends_at));
        }
    }

    /// Message `id`'s captures, decoded.
    pub fn set_images(&mut self, id: u64, images: Vec<Image>) {
        if let Some(m) = self.messages.iter_mut().find(|m| m.id == id) {
            m.images = images;
        }
    }

    /// Slides out; the conversation and the draft stay. True if it was open.
    pub fn close(&mut self) -> bool {
        self.preview = None;
        std::mem::replace(&mut self.open, false)
    }

    /// The display's size in points.
    pub fn screen(&self) -> [f32; 2] {
        self.screen
    }

    /// Esc in the field: an enlarged capture closes first, then the chat.
    pub fn escape(&mut self) -> Escape {
        if self.preview.take().is_some() { Escape::Preview } else { Escape::Close }
    }

    /// Shows capture `id` enlarged (its chip's image was clicked).
    pub fn open_preview(&mut self, id: u64) {
        if self.shots.iter().any(|a| a.id == id) {
            self.preview = Some(id);
        }
    }

    /// The capture shown enlarged, if any.
    pub fn preview_image(&self) -> Option<&Image> {
        let id = self.preview?;
        self.shots.iter().find(|a| a.id == id).map(|a| &a.preview)
    }

    pub fn close_preview(&mut self) {
        self.preview = None;
    }

    /// Backspace in an empty field removes the last capture; its id, if one went.
    pub fn backspace_empty(&mut self) -> Option<u64> {
        let id = self.shots.last()?.id;
        self.remove(id);
        Some(id)
    }

    /// Cmd+N: slides out, the conversation over (see `over`); clients get the new one, empty,
    /// at once. Nothing is saved of it until its first message.
    pub fn start_over(&mut self) {
        self.end_replies(Instant::now());
        self.new_conversation();
        self.open = false;
        self.over = Some(history::Meta::fresh().id);
        self.preview = None;
        // The new one's snapshot replaces the old one's on every client, in whatever order.
        self.resync_at = None;
        self.announce();
    }

    /// Message `id` gets a reply from the model, which the host streams in; the reply's
    /// token, which its events carry. While another message's reply streams, it's queued: pi
    /// reads it in the run going (a steer), any other model gets it once the reply ends.
    pub fn ask(&mut self, id: u64) -> Option<u64> {
        let prompt = self.messages.iter().find(|m| m.id == id)?.text.clone();
        if self.replies.iter().any(|r| r.live() && r.id != id) {
            return Some(self.queue(id, prompt));
        }
        Some(self.start_reply(id, prompt, false))
    }

    /// The model picker: its list opens or closes.
    pub fn toggle_models(&mut self) {
        self.models_open = !self.models_open;
    }

    /// A model picked in the composer: it writes the next replies (kept in the settings, by the
    /// settings window). Anything not a model is ignored.
    pub fn pick_model(&mut self, id: &str) {
        if !model::MODELS.iter().any(|(m, _)| *m == id) {
            return;
        }
        self.models_open = false;
        #[cfg(test)]
        {
            self.model = Some(id.to_owned());
        }
        if let Some(host) = &self.host {
            let id = id.to_owned();
            host.update(move |host, _| host.pick_model(id));
        }
    }

    /// The model the picker names.
    pub fn model_label(&self) -> &'static str {
        let id = self.model();
        model::MODELS.iter().find(|(m, _)| *m == id).map_or("", |(_, label)| label)
    }

    fn model(&self) -> String {
        #[cfg(test)]
        if let Some(model) = &self.model {
            return model.clone();
        }
        settings::model_now()
    }

    /// Message `id`'s reply waits, queued; to pi streaming, it goes now, as a steer.
    fn queue(&mut self, id: u64, prompt: String) -> u64 {
        self.next_token += 1;
        let token = self.next_token;
        let model = self.model();
        // While pi still has to say what it read of a stopped run, there's no run to steer: it
        // waits its turn behind those.
        let steer = model == model::PI && self.running_model == model::PI && !by_mock(false, &model, mock_model::requested()) && self.cut.is_empty();
        let mut reply = Reply::new(id, prompt);
        (reply.token, reply.state, reply.steered) = (token, reply::State::Queued, steer);
        self.replace_reply(reply);
        self.note_run(token, id);
        // Quitting now keeps it, interrupted.
        self.save();
        if steer && let Some(request) = self.request_for(id, &model) {
            self.send_request(token, model::Request { steer: true, ..request });
        }
        token
    }

    /// `reply` takes the place of its message's reply, whose question fields and kept events go
    /// with it.
    fn replace_reply(&mut self, reply: Reply) {
        self.replies.retain(|r| r.id != reply.id);
        self.replies.push(reply);
        let replies = &self.replies;
        self.others.retain(|(token, _), _| replies.iter().any(|r| r.token == *token));
        self.kept.retain(|token, _| replies.iter().any(|r| r.token == *token));
        self.cut.retain(|token| replies.iter().any(|r| r.token == *token));
    }

    /// `request` goes to the model for the reply `token`.
    fn send_request(&mut self, token: u64, request: model::Request) {
        #[cfg(test)]
        self.requests.push((token, request.clone()));
        if let Some(host) = &self.host {
            host.update(move |host, _| host.ask_model(token, request));
        }
    }

    /// Asks message `id` again: from `prompt` (`answer`: the user's answer to a question). To pi
    /// it's a message: it answers no dialog.
    fn start_reply(&mut self, id: u64, prompt: String, answer: bool) -> u64 {
        self.start_reply_as(id, prompt, answer, None, false)
    }

    /// The same, the answer to pi's dialog `dialog` (the question's, the user answering it),
    /// `decline`: declining it (`DECLINED`).
    fn start_reply_as(&mut self, id: u64, prompt: String, answer: bool, dialog: Option<String>, decline: bool) -> u64 {
        self.stop_replies(Instant::now());
        self.next_token += 1;
        let token = self.next_token;
        // Claude's questions with options come from the mock only (so do their answers); pi asks
        // its own and waits for them; everything is the mock's with `AILFRED_MOCK_MODEL=1`.
        let model = self.model();
        let script = by_mock(answer, &model, mock_model::requested()).then(|| if answer { mock_model::answer_script(&prompt) } else { mock_model::script(&prompt) });
        self.running_model = if script.is_some() { String::new() } else { model.clone() };
        let mut reply = Reply::new(id, prompt);
        (reply.token, reply.answer) = (token, answer);
        self.replace_reply(reply);
        self.note_run(token, id);
        // Saved as it starts: quitting now leaves it interrupted.
        self.save();
        match script {
            Some(script) => {
                if let Some(host) = &self.host {
                    host.update(move |host, _| host.stream_reply(token, script));
                }
            }
            None => {
                if let Some(request) = self.request_for(id, &model) {
                    self.send_request(token, model::Request { answers: dialog, decline, ..request });
                }
            }
        }
        // A retried reply this one stopped goes back in its place.
        self.settle_resync();
        token
    }

    /// The next event of the reply stream `token`; one stopped or replaced takes none. A reply
    /// ending lets the next queued one go, unless it asks the user something.
    pub fn reply_event(&mut self, token: u64, event: Event, now: Instant) {
        let Some(at) = self.replies.iter().position(|r| r.token == token) else { return };
        let cut = self.cut.iter().position(|&t| t == token).map(|i| self.cut.remove(i)).is_some();
        if event == Event::Dropped {
            // pi let it go unread with its run: it goes again, as a message of its own.
            let reply = &mut self.replies[at];
            if reply.state == reply::State::Queued && reply.steered {
                reply.steered = false;
                self.next_queued();
            }
            return;
        }
        let sent = self.note_event(token, &event);
        let question = matches!(event, Event::Ask { .. });
        let reply = &mut self.replies[at];
        let was = reply.live();
        reply.apply(event, now);
        if question {
            self.note_question(at, sent);
        }
        if cut && self.replies[at].streaming() {
            // pi read it just before the Stop reached it: it's in the run that stopped.
            self.replies[at].stop(now);
            self.note_stop(&[token]);
        }
        let reply = &self.replies[at];
        if was && !reply.live() {
            let asks = reply.asks();
            self.reply_ended(token);
            if !asks {
                self.next_queued();
            }
            self.settle_resync();
        }
    }

    /// Nothing going: the first message queued gets its reply. While pi still has a steered
    /// one, it waits for pi.
    fn next_queued(&mut self) {
        if self.replies.iter().any(Reply::live) {
            return;
        }
        let Some(r) = self.replies.iter().filter(|r| r.state == reply::State::Queued).min_by_key(|r| r.token) else { return };
        let (id, prompt) = (r.id, r.prompt.clone());
        self.start_reply(id, prompt, false);
    }

    /// A reply is streaming.
    pub fn streaming(&self) -> bool {
        self.replies.iter().any(Reply::streaming)
    }

    /// Stop: the reply streaming keeps what came, its run (the process behind it) ends, and the
    /// next message queued goes at once. One steered into pi's run waits for pi to say whether
    /// it read it (it stops with the run) or not (it goes again). The conversation is saved
    /// with it.
    pub fn stop(&mut self, now: Instant) {
        if self.stop_replies(now) {
            self.save();
        }
        self.next_queued();
        self.settle_resync();
    }

    /// The conversation is left: what streams stops and what's queued goes, saved so.
    fn end_replies(&mut self, now: Instant) {
        let queued = self.replies.len();
        self.replies.retain(|r| r.state != reply::State::Queued);
        self.cut.clear();
        if self.stop_replies(now) || self.replies.len() != queued {
            self.save();
        }
    }

    /// Stops what streams without saving (what's queued stays); whether anything did.
    fn stop_replies(&mut self, now: Instant) -> bool {
        let tokens: Vec<u64> = self.replies.iter().filter(|r| r.streaming()).map(|r| r.token).collect();
        for reply in &mut self.replies {
            reply.stop(now);
        }
        // What was steered into the run waits for pi to say whether it read it (`reply_event`).
        let steered = self.replies.iter().filter(|r| r.state == reply::State::Queued && r.steered).map(|r| r.token);
        let fresh: Vec<u64> = steered.filter(|t| !self.cut.contains(t)).collect();
        self.cut.extend(fresh);
        if tokens.is_empty() {
            return false;
        }
        self.note_stop(&tokens);
        #[cfg(test)]
        self.cancelled.extend(&tokens);
        if let Some(host) = &self.host {
            host.update(move |host, _| host.cancel_runs(&tokens));
        }
        true
    }

    /// The conversation left with Cmd+N gets a message, so the new one starts (`push`). Nothing
    /// of the old one should still go (its Retry, Continue and answers start nothing), but what
    /// does ends here, stopped and saved into it, and its runs leave the hub (`forget_runs`).
    fn end_leftovers(&mut self) {
        let now = Instant::now();
        let live: Vec<u64> = self.replies.iter().filter(|r| r.live()).map(|r| r.token).collect();
        for reply in &mut self.replies {
            reply.stop(now);
        }
        let tokens: Vec<u64> = self.replies.iter().map(|r| r.token).collect();
        self.forget_runs(&tokens);
        if live.is_empty() {
            return;
        }
        #[cfg(test)]
        self.cancelled.extend(&live);
        if let Some(host) = &self.host {
            host.update(move |host, _| host.cancel_runs(&live));
        }
        self.save();
    }

    /// The reply offering "I was interrupted. Continue?": the conversation's last, interrupted
    /// (Ailfred quit, or the model's process ended, as it streamed).
    pub fn continuable(&self) -> Option<u64> {
        let last = self.messages.last()?;
        self.reply_to(last.id).filter(|r| r.interrupted && r.state == reply::State::Stopped).map(|r| r.token)
    }

    /// Continue on the interrupted reply `token`: the user's message `CONTINUE` asks the model
    /// to go on from what it said (in the history); one that had said nothing is asked again.
    /// The new reply's token; None unless `token` is the one offering it.
    pub fn go_on(&mut self, token: u64) -> Option<u64> {
        if self.over.is_some() || self.continuable() != Some(token) {
            return None;
        }
        let reply = self.reply_mut(token)?;
        if reply.history_text().is_none() {
            return self.retry(token);
        }
        let id = self.next_id();
        self.push(Message::text(id, CONTINUE));
        Some(self.start_reply(id, CONTINUE.into(), false))
    }

    /// Retry: the reply `token` starts over; its new token. Clients get the conversation
    /// without it, then its new run. None once the conversation is over (Cmd+N): it still
    /// shows as it slides out, but starts nothing. A reply to an answer goes to pi as a message
    /// of the answer's words, never as an answer: pi's dialog open then (which may be another
    /// than the one answered, as the guard's next) is cancelled, not answered.
    pub fn retry(&mut self, token: u64) -> Option<u64> {
        if self.over.is_some() {
            return None;
        }
        let r = self.replies.iter().find(|r| r.token == token)?;
        let (id, prompt, answer) = (r.id, r.prompt.clone(), r.answer);
        let last = self.messages.last().is_some_and(|m| m.id == id);
        // The snapshot it sends puts a reply retried before back in place.
        self.resync_at = None;
        let again = self.start_reply(id, prompt, answer);
        self.note_retry(again, last);
        Some(again)
    }

    /// Reply `r` shows Retry (`reply_view`): it ended, done, stopped or failed (the footer's, or
    /// its error's); one streaming or queued doesn't.
    pub fn offers_retry(r: &Reply) -> bool {
        matches!(r.state, reply::State::Done | reply::State::Stopped | reply::State::Failed)
    }

    /// The reply to message `id`.
    pub fn reply_to(&self, id: u64) -> Option<&Reply> {
        self.replies.iter().find(|r| r.id == id)
    }

    pub fn reply_mut(&mut self, token: u64) -> Option<&mut Reply> {
        self.replies.iter_mut().find(|r| r.token == token)
    }

    fn question_mut(&mut self, token: u64, part: usize) -> Option<&mut reply::Question> {
        match self.reply_mut(token)?.parts.get_mut(part)? {
            reply::Part::Question(q) => Some(q),
            _ => None,
        }
    }

    /// Option `option` of question `part` in reply `token` clicked.
    pub fn pick(&mut self, token: u64, part: usize, option: usize) {
        if let Some(q) = self.question_mut(token, part) {
            q.pick(option);
        }
    }

    /// The "something else" field of question `part` in reply `token`.
    pub fn question_other(&mut self, token: u64, part: usize) -> &mut TextEdit {
        if !self.question_mut(token, part).is_some_and(|q| q.open()) {
            return &mut self.scratch;
        }
        self.others.entry((token, part)).or_insert_with(|| TextEdit::new(""))
    }

    /// The field as drawn: none until the user edits it.
    pub fn question_field(&self, token: u64, part: usize) -> Option<&TextEdit> {
        self.others.get(&(token, part))
    }

    /// The user answers question `part` of reply `token`: the answer goes as their message and
    /// gets its reply (its token). None if nothing is picked or written, or it doesn't take an
    /// answer (answered, or closed).
    pub fn answer(&mut self, token: u64, part: usize) -> Option<u64> {
        let other = self.others.get(&(token, part)).map_or(String::new(), |e| e.text.clone());
        self.answer_with(token, &[(part, other)])
    }

    /// The same for questions `answers` of reply `token` at once (each its part and what was
    /// written for it; nothing written: the options picked): those that take an answer get it,
    /// and the answers go as one message (blank lines between), which gets its reply. None if
    /// none took one, or the conversation is over (Cmd+N).
    pub fn answer_with(&mut self, token: u64, answers: &[(usize, String)]) -> Option<u64> {
        if self.over.is_some() {
            return None;
        }
        let reply = self.replies.iter_mut().find(|r| r.token == token)?;
        // pi's dialog the first question answered stands for: the answer goes to it alone.
        let dialog = answers.iter().find_map(|(part, _)| match reply.parts.get(*part) {
            Some(reply::Part::Question(q)) if q.open() => q.dialog.clone(),
            _ => None,
        });
        let texts: Vec<String> = answers.iter().filter_map(|(part, other)| reply.answer(*part, other)).collect();
        if texts.is_empty() {
            return None;
        }
        let text = texts.join("\n\n");
        let id = self.next_id();
        self.push(Message::text(id, text.clone()));
        Some(self.start_reply_as(id, text, true, dialog, false))
    }

    fn next_id(&self) -> u64 {
        // Past every id so far: the input's (small) ones arrive after typed ones too.
        self.messages.iter().map(|m| m.id + 1).max().unwrap_or(0).max(LOCAL_IDS)
    }

    /// Adds a message at the end of the conversation, the field focused for the next.
    pub fn push(&mut self, message: Message) {
        // Clients were given its id with its empty snapshot already (`start_over`).
        if let Some(next) = self.over.take() {
            self.end_leftovers();
            self.reset_conversation();
            self.conv.id = next;
        }
        self.messages.push(message);
        self.draft.focused = true;
    }

    /// Cmd+V in the field: an image on the clipboard takes the next id and, once decoded, joins
    /// the draft (`attach`). False lets the field paste the text.
    pub fn paste(&mut self, content: &rune::ClipboardContent) -> bool {
        let taken = self.take_paste(content);
        let any = !taken.is_empty();
        for (id, pasted) in taken {
            decode_pasted(&self.host, Field::Chat, id, pasted);
        }
        any
    }

    fn take_paste(&mut self, content: &rune::ClipboardContent) -> Vec<(u64, Pasted)> {
        take_ids(&mut self.next_shot, pasted_images(content))
    }

    /// Adds a capture to the draft, or replaces the one with the same id.
    pub fn attach(&mut self, id: u64, png: Vec<u8>, preview: Image) {
        match self.shots.iter_mut().find(|a| a.id == id) {
            Some(shot) => (shot.png, shot.preview) = (png, preview),
            None => self.shots.push(Attached { id, png, preview }),
        }
    }

    /// Drops a capture (its chip's X). True if it was attached.
    pub fn remove(&mut self, id: u64) -> bool {
        if self.preview == Some(id) {
            self.preview = None;
        }
        let before = self.shots.len();
        self.shots.retain(|a| a.id != id);
        self.shots.len() != before
    }

    #[cfg(test)]
    fn shot_ids(&self) -> Vec<u64> {
        self.shots.iter().map(|a| a.id).collect()
    }

    /// Something to send: text or a capture.
    fn send_ready(&self) -> bool {
        !self.draft.text.trim().is_empty() || !self.shots.is_empty()
    }

    /// Sends the draft, trimmed, with its captures as a message; nothing typed or attached sends
    /// nothing. The message's id and its captures' PNGs (to decode, `set_images`).
    pub fn send(&mut self) -> Option<(u64, Vec<Vec<u8>>)> {
        if !self.send_ready() {
            return None;
        }
        let text = self.draft.text.trim().to_owned();
        self.draft.clear();
        self.preview = None;
        let pngs: Vec<Vec<u8>> = std::mem::take(&mut self.shots).into_iter().map(|a| a.png).collect();
        let id = self.next_id();
        self.push(Message::text(id, text).with_pngs(&pngs));
        Some((id, pngs))
    }

    fn dismiss(&self) {
        if let Some(host) = &self.host {
            host.update(|host, _| host.hide_chat());
        }
    }

    /// Esc closes an enlarged capture, then slides the panel out (`escape`); Backspace in an
    /// empty field drops the last capture; Cmd+N starts a new chat (`Host::new_chat`).
    fn field_key(&mut self, ev: &KeyEvent) -> bool {
        // Esc while a reply streams stops it; the next Esc closes.
        if ev.key == Key::Escape && self.streaming() {
            self.stop(Instant::now());
            return true;
        }
        if ev.key == Key::Escape {
            if self.escape() == Escape::Preview {
                return true;
            }
            // A window stays; the side panel slides out.
            if self.windowed {
                return false;
            }
            self.dismiss();
            return true;
        }
        if ev.key == Key::Backspace && self.draft.text.is_empty() {
            return self.backspace_empty().is_some();
        }
        if ev.is_command('n') {
            if let Some(host) = &self.host {
                host.update(|host, _| host.new_chat());
            }
            return true;
        }
        false
    }
}
