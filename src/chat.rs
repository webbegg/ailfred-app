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

use std::sync::Arc;
use std::time::Instant;

use crate::Host;
use crate::icons;
use crate::mock_model;
use crate::model;
use crate::reply::{self, Event, Reply};
use crate::reply_view;
use crate::settings;
use crate::quick_input::{self, ACCENT, Attached, Escape};

mod geometry;
mod images;
#[cfg(test)]
pub(crate) mod tests;
mod view;

pub use geometry::{Insets, insets, offset, side_pins, side_rect, thumb_size};
pub use images::{Field, Image, Pasted, decode_pasted, lightbox, paste_lands, pasted_images, take_ids};
#[cfg(test)]
pub use images::{decode_png, image_file_png};
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
}

impl Message {
    /// A message with `text` and no captures.
    pub fn text(id: u64, text: impl Into<String>) -> Message {
        Message { id, text: text.into(), images: Vec::new(), pngs: Vec::new() }
    }

    /// The same with the captures `pngs` (copied: the originals go to be decoded).
    pub fn with_pngs(mut self, pngs: &[Vec<u8>]) -> Message {
        self.pngs = pngs.iter().map(|p| Arc::new(p.clone())).collect();
        self
    }
}

pub struct Chat {
    messages: Vec<Message>,
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
    /// starts a new one.
    over: bool,
    /// Where Esc and the close button go (None in tests).
    host: Option<AppHandle<Host>>,
    /// The next pasted image's id.
    next_shot: u64,
    /// The model's replies, one per message at most (by its id).
    replies: Vec<Reply>,
    /// The last reply stream started: its events come back with it.
    next_token: u64,
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
    /// The streams whose runs were cancelled (the host ends their processes), for tests.
    #[cfg(test)]
    cancelled: Vec<u64>,
}

impl Chat {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        Chat { messages: Vec::new(), draft: TextEdit::new(""), shots: Vec::new(), preview: None, open: false, insets: Insets::default(), screen: [1440.0, 900.0], landed: true, over: false, host, next_shot: 0, replies: Vec::new(), next_token: 0, scratch: TextEdit::new(""), away: false, windowed: false, session: None, #[cfg(test)] cancelled: Vec::new() }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn has_messages(&self) -> bool {
        !self.messages.is_empty()
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
        self.stop(Instant::now());
        self.new_conversation();
        self.session = None;
        self.messages.clear();
        self.replies.clear();
        self.draft.clear();
        self.shots.clear();
        (self.preview, self.over, self.away) = (None, false, false);
        self.draft.focused = true;
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
    /// messages whose reply came (done, or stopped with something) as the history.
    pub fn request_for(&self, id: u64, model: &str) -> Option<model::Request> {
        let at = self.messages.iter().position(|m| m.id == id)?;
        let turns: Vec<(u64, model::Turn)> = self.messages[..at]
            .iter()
            .filter_map(|m| {
                let reply = self.replies.iter().find(|r| r.id == m.id)?.history_text()?;
                Some((m.id, model::Turn { text: m.text.clone(), pngs: m.pngs.clone(), reply }))
            })
            .collect();
        // Resumed only when it ends where this message's history does: a retry's session
        // already holds the message and its old reply.
        let last = turns.last().map(|(id, _)| *id);
        let session = self.session.as_ref().filter(|(_, ends_at)| Some(*ends_at) == last).map(|(s, _)| s.clone());
        let history = turns.into_iter().map(|(_, turn)| turn).collect();
        let message = &self.messages[at];
        Some(model::Request { model: model.into(), history, text: message.text.clone(), pngs: message.pngs.clone(), session, answer: false })
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

    /// Cmd+N: slides out, the conversation over (see `over`).
    pub fn start_over(&mut self) {
        self.stop(Instant::now());
        self.new_conversation();
        self.open = false;
        self.over = true;
        self.preview = None;
    }

    /// Message `id` gets a reply from the model, which the host streams in; the reply's
    /// token, which its events carry. A reply streaming is stopped first.
    pub fn ask(&mut self, id: u64) -> Option<u64> {
        let prompt = self.messages.iter().find(|m| m.id == id)?.text.clone();
        Some(self.start_reply(id, prompt, false))
    }

    /// Asks message `id` again: from `prompt` (`answer`: the user's answer to a question).
    fn start_reply(&mut self, id: u64, prompt: String, answer: bool) -> u64 {
        self.stop(Instant::now());
        self.next_token += 1;
        let token = self.next_token;
        // Claude's questions with options come from the mock only (so do their answers); pi asks
        // its own and waits for them; everything is the mock's with `AILFRED_MOCK_MODEL=1`.
        let model = settings::model_now();
        let script = by_mock(answer, &model, mock_model::requested()).then(|| if answer { mock_model::answer_script(&prompt) } else { mock_model::script(&prompt) });
        let mut reply = Reply::new(id, prompt);
        (reply.token, reply.answer) = (token, answer);
        self.replies.retain(|r| r.id != id);
        self.replies.push(reply);
        if let Some(host) = &self.host {
            match script {
                Some(script) => host.update(move |host, _| host.stream_reply(token, script)),
                None => {
                    if let Some(request) = self.request_for(id, &model) {
                        let request = model::Request { answer, ..request };
                        host.update(move |host, _| host.ask_model(token, request));
                    }
                }
            }
        }
        token
    }

    /// The next event of the reply stream `token`; one stopped or replaced takes none.
    pub fn reply_event(&mut self, token: u64, event: Event, now: Instant) {
        if let Some(reply) = self.reply_mut(token) {
            reply.apply(event, now);
        }
    }

    /// A reply is streaming.
    pub fn streaming(&self) -> bool {
        self.replies.iter().any(Reply::streaming)
    }

    /// Stop: the reply streaming keeps what came, and its run (the process behind it) ends.
    pub fn stop(&mut self, now: Instant) {
        let tokens: Vec<u64> = self.replies.iter().filter(|r| r.streaming()).map(|r| r.token).collect();
        for reply in &mut self.replies {
            reply.stop(now);
        }
        if tokens.is_empty() {
            return;
        }
        #[cfg(test)]
        self.cancelled.extend(&tokens);
        if let Some(host) = &self.host {
            host.update(move |host, _| host.cancel_runs(&tokens));
        }
    }

    /// Retry: the reply `token` starts over; its new token.
    pub fn retry(&mut self, token: u64) -> Option<u64> {
        let r = self.replies.iter().find(|r| r.token == token)?;
        let (id, prompt, answer) = (r.id, r.prompt.clone(), r.answer);
        Some(self.start_reply(id, prompt, answer))
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
        if self.question_mut(token, part).is_none() {
            return &mut self.scratch;
        }
        &mut self.question_mut(token, part).expect("just found").other
    }

    /// The user answers question `part` of reply `token`: the answer goes as their message and
    /// gets its reply (its token). None if nothing is picked or written.
    pub fn answer(&mut self, token: u64, part: usize) -> Option<u64> {
        let text = self.reply_mut(token)?.answer(part)?;
        let id = self.next_id();
        self.push(Message::text(id, text.clone()));
        Some(self.start_reply(id, text, true))
    }

    fn next_id(&self) -> u64 {
        // Past every id so far: the input's (small) ones arrive after typed ones too.
        self.messages.iter().map(|m| m.id + 1).max().unwrap_or(0).max(LOCAL_IDS)
    }

    /// Adds a message at the end of the conversation, the field focused for the next.
    pub fn push(&mut self, message: Message) {
        if std::mem::replace(&mut self.over, false) {
            self.session = None;
            self.messages.clear();
            self.replies.clear();
            self.draft.clear();
            self.shots.clear();
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
