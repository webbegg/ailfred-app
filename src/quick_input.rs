//! The quick input: a floating text field that slides up from the bottom of the display under
//! the pointer, above every other window. Ctrl+Option+Space or the menu opens it; Esc, a click
//! elsewhere closes it, sliding back down. Enter sends: the panel itself turns into the chat
//! panel (chat.rs), which this window also holds, springing to the right edge as it reshapes.
//!
//! While it's open (or the chat is), the capture shortcut or menu takes a screenshot for it:
//! dragging out an area tucks the input away, and the picked area, marked up or not, is pasted
//! with Enter as a chip with a small preview (click it to see it enlarged; hover for its X).

use std::time::Duration;

use rune::tok;
use rune::{
    Anim, AppHandle, Color, Curve, Cursor, Cx, El, Key, KeyEvent, Shadow, Spring, SystemMaterial, TextEdit, TextStyle, Transition, Tx, col, el, icon,
    row, text_input,
};
use rune_kit::glyphs;

use crate::Host;
use crate::chat::{self, Chat};
use crate::icons;

mod geometry;
mod labels;
#[cfg(test)]
mod tests;
mod view;

use geometry::grown;
pub use geometry::{Pose, bottom_panel, compact, dock_inset, input_panel, pose, strip_h};
pub use labels::{chord_symbols, hint, sent_log};
pub use view::{chip, on_focus, on_key, surface, view};

/// The fixed global chord that toggles the panel (Option+Space alone is Claude's quick entry).
pub const SHORTCUT: &str = "ctrl+alt+space";
/// The panel's widest and narrowest, its height, and its row's side padding, in points.
pub const PANEL_W: f32 = 620.0;
pub const PANEL_H: f32 = 52.0;
const ROW_PAD: f32 = 12.0;
pub const RADIUS: f32 = 14.0;
/// Room around the panel for its shadow.
pub const PAD: f32 = 32.0;
/// The panel's gap to the bottom of the screen (above the Dock, if there's one). The window
/// itself reaches the bottom, so a closed or tucked panel waits out of view below.
pub const MARGIN: f32 = 40.0;
/// How far a closed or tucked panel drops at the bottom: out of the window, shadow included. It
/// opens by rising this far, from outside the screen.
pub const TUCK: f32 = PANEL_H + MARGIN + PAD;
/// How long the window takes to fade out once closed: long enough for the slide down to play.
pub const EXIT: Duration = Duration::from_millis(240);
/// How long a panel waits out of view, the window just shown, before sliding in: a frame or
/// two, so the slide plays on screen.
pub const ARRIVAL_DELAY: Duration = Duration::from_millis(40);
/// The slide from the screen edge.
const GLIDE: Spring = tok::motion::GLIDE;
/// Sending presses the whole panel in to this scale, this fast, before it goes.
pub const PRESSED_SCALE: f32 = 0.98;
pub const PRESS: Duration = Duration::from_millis(110);

/// The accent, #D97757 (Rune colours are linear).
pub const ACCENT: Color = Color { r: 0.694, g: 0.184, b: 0.095, a: 1.0 };
/// The panel's frost over the system material, #1E1E1E.
const FROST: Color = Color { r: 0.013, g: 0.013, b: 0.013, a: 0.82 };

/// A capture attached to the input, by the id of the capture area it came from (the same area
/// re-exported with marks replaces it).
pub struct Attached {
    pub id: u64,
    /// The image, ready to send.
    pub png: Vec<u8>,
    /// The same, to show in its chip (and enlarged).
    pub preview: chat::Image,
}

pub struct QuickInput {
    query: TextEdit,
    open: bool,
    /// Out of view while a capture area is dragged.
    tucked: bool,
    /// At the bottom, the Dock's height under the panel's gap: a closed or tucked panel drops
    /// past it too, out of the screen.
    dock: f32,
    shots: Vec<Attached>,
    /// The capture whose chip was clicked, shown enlarged.
    preview: Option<u64>,
    /// Where closing goes, so the host hides the window after the slide (None in tests).
    host: Option<AppHandle<Host>>,
    /// The panel's rect at rest, in points on the display the window covers.
    rect: [f32; 4],
    /// The chat side panel, in the same window: the input's panel turns into it on send.
    pub chat: Chat,
    /// Which of two keys the input's panel has (the chat's has the other): sending hands the
    /// input's key, and so its element, to the chat, which then reshapes from it.
    slot: bool,
    /// Just sent: the chat panel still carries the input's row, fading out, until `land`.
    morphing: bool,
    /// The next sent message's id (its captures join it once decoded).
    next_message: u64,
    /// The next pasted image's id.
    next_shot: u64,
    /// Enter or the send button: the panel is pressed in, sending once it's down (`release`).
    pressed: bool,
    /// Presses so far: a release goes with its own press only.
    presses: u64,
    /// Pasted images still decoding, by id: they join only while the input is the same one.
    pending: Vec<u64>,
    /// The capture shortcut's symbols, shown in the empty field's hint.
    capture_chord: Option<String>,
    /// The chat is in its own window: what's sent goes there (the host's), not into `chat`.
    windowed: bool,
}

/// What Esc in a field (the input's or the chat's) does.
#[derive(Debug, PartialEq)]
pub enum Escape {
    /// Closes the enlarged capture; the panel stays.
    Preview,
    /// Closes the panel.
    Close,
}

/// In the chat's composer, more chips than this show smaller, so they keep to one line.
pub const CHAT_FULL_CHIPS: usize = 3;
/// In the input's strip above the field, the same, with its wider room.
const INPUT_FULL_CHIPS: usize = 6;
/// A chip is its image, as large as fits in this (points), or this compact.
const THUMB: (f32, f32) = (80.0, 44.0);
const THUMB_COMPACT: (f32, f32) = (48.0, 28.0);
/// The room above the chips in the input's strip.
const STRIP_TOP: f32 = 10.0;
/// The field never gets narrower than this, however many chips there are.
const FIELD_MIN_W: f32 = 180.0;

impl QuickInput {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        QuickInput {
            query: TextEdit::new(""),
            open: false,
            tucked: false,
            dock: 0.0,
            shots: Vec::new(),
            preview: None,
            chat: Chat::new(host.clone()),
            host,
            rect: [0.0, 0.0, PANEL_W, PANEL_H],
            slot: false,
            morphing: false,
            next_message: 0,
            next_shot: 0,
            pressed: false,
            presses: 0,
            pending: Vec::new(),
            capture_chord: None,
            windowed: false,
        }
    }

    /// The chat goes into its own window: it's handed over, landed, and an empty one stays,
    /// closed. What's sent from now on goes to the window.
    pub fn take_chat(&mut self) -> Chat {
        self.end_morph();
        self.windowed = true;
        let mut chat = std::mem::replace(&mut self.chat, Chat::new(self.host.clone()));
        chat.set_windowed(true);
        chat
    }

    /// The chat's back from its window, closed, to slide in.
    pub fn put_chat(&mut self, mut chat: Chat) {
        chat.set_windowed(false);
        self.chat = chat;
        self.windowed = false;
    }

    /// The panel's rect at rest (points on the window's display). Applied without motion, with
    /// the panel out of view.
    pub fn set_rect(&mut self, rect: [f32; 4]) {
        self.rect = rect;
    }

    fn input_key(&self) -> &'static str {
        if self.slot { "panel-b" } else { "panel-a" }
    }

    fn chat_key(&self) -> &'static str {
        if self.slot { "panel-a" } else { "panel-b" }
    }

    /// Enter: what's typed and attached goes to the chat, and the input goes. With the chat
    /// closed, the input's panel becomes the chat's (it keeps its row, fading, until `land`);
    /// with it open, the message joins it and the input slides down. The message's id, text and
    /// capture PNGs (to decode); None if there's nothing to send.
    pub fn send(&mut self) -> Option<(u64, String, Vec<Vec<u8>>)> {
        let text = self.query.text.trim().to_owned();
        if text.is_empty() && self.shots.is_empty() {
            return None;
        }
        let id = self.next_message;
        self.next_message += 1;
        // The chips stay (by id) for the fading row; their images go to be decoded.
        let pngs: Vec<Vec<u8>> = self.shots.iter_mut().map(|a| std::mem::take(&mut a.png)).collect();
        let message = chat::Message { id, text: text.clone(), images: Vec::new() };
        self.query.focused = false;
        self.pending.clear();
        // The host takes it to the chat's window.
        if self.windowed {
            self.close();
            self.query.clear();
            return Some((id, text, pngs));
        }
        if self.chat.is_open() {
            self.close();
            self.query.clear();
            self.chat.push(message);
        } else {
            self.open = false;
            self.tucked = false;
            self.preview = None;
            self.slot = !self.slot;
            self.morphing = true;
            self.chat.morph(message);
        }
        self.chat.ask(id);
        Some((id, text, pngs))
    }

    /// The panel is the chat's now: the input's row goes (the next input starts empty) and the
    /// chat's content fades in.
    pub fn land(&mut self) {
        self.end_morph();
        self.chat.land();
        // Cmd+N meanwhile: the new input keeps the keys.
        if self.open {
            self.chat.blur();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The sent row is done with: the next input starts empty. At the landing, or sooner when a
    /// new input opens before it (Cmd+N), whose own text and images the landing then keeps.
    fn end_morph(&mut self) {
        if std::mem::replace(&mut self.morphing, false) {
            self.query.clear();
            self.shots.clear();
            self.tell_chips();
        }
    }

    /// Rises into place with the field focused.
    pub fn open(&mut self) {
        self.end_morph();
        self.open = true;
        self.tucked = false;
        self.query.focused = true;
        self.chat.blur();
    }

    /// The chat slides in, its field taking the keys.
    pub fn open_chat(&mut self) {
        self.query.focused = false;
        self.chat.open();
    }

    /// Goes away, discarding its captures; the text stays for next time. True if it was open.
    pub fn close(&mut self) -> bool {
        self.tucked = false;
        self.pressed = false;
        self.pending.clear();
        self.shots.clear();
        self.tell_chips();
        self.preview = None;
        std::mem::replace(&mut self.open, false)
    }

    /// Before opening: below the screen at the bottom (over a Dock `dock` points tall), whatever
    /// it was doing before, so `open` rises from the screen's edge. Applied without motion.
    pub fn at_bottom(&mut self, dock: f32) {
        self.end_morph();
        self.dock = dock.max(0.0);
        self.tucked = false;
        self.query.focused = true;
    }

    /// Where the panel is now.
    pub fn current_pose(&self) -> Pose {
        let mut p = pose(self.open, self.tucked);
        if self.pressed {
            p.scale = PRESSED_SCALE;
        }
        if p.dy == TUCK { Pose { dy: TUCK + self.dock + strip_h(self.shots.len()), ..p } } else { p }
    }

    /// The panel's left and bottom edges' distances from the window's (points): pinned by its
    /// bottom edge, it rests and hides relative to the window whatever screen it was last placed
    /// for, or none yet (launch).
    pub fn pins(&self) -> [f32; 2] {
        [self.rect[0], self.dock + MARGIN]
    }

    /// The panel's rect now: its rest, grown upward by the chip strip (the field row stays put).
    pub fn panel_rect(&self) -> [f32; 4] {
        grown(self.rect, self.shots.len())
    }

    /// Tells the host how many images it holds now (a capture leaves their strip alone).
    fn tell_chips(&self) {
        if let Some(host) = &self.host {
            let n = self.shots.len();
            host.update(move |host, _| host.set_quick_chips(n));
        }
    }

    /// Slides out of view (true) or back into place (false).
    pub fn set_tucked(&mut self, tucked: bool) {
        self.tucked = tucked;
        if !tucked {
            self.query.focused = true;
        }
    }

    /// Esc in the field: an enlarged capture closes first, then the input.
    pub fn escape(&mut self) -> Escape {
        if self.preview.take().is_some() { Escape::Preview } else { Escape::Close }
    }

    /// Shows capture `id` enlarged (its chip's image was clicked).
    pub fn open_preview(&mut self, id: u64) {
        if self.shots.iter().any(|a| a.id == id) {
            self.preview = Some(id);
        }
    }

    /// The capture shown enlarged: the input's, else the chat's.
    pub fn preview_image(&self) -> Option<&chat::Image> {
        let own = self.preview.and_then(|id| self.shots.iter().find(|a| a.id == id));
        own.map(|a| &a.preview).or_else(|| self.chat.preview_image())
    }

    fn close_preview(&mut self) {
        self.preview = None;
        self.chat.close_preview();
    }

    /// Backspace in an empty field removes the last capture; its id, if one went.
    pub fn backspace_empty(&mut self) -> Option<u64> {
        let id = self.shots.last()?.id;
        self.remove(id);
        Some(id)
    }

    /// Cmd+V in the field: an image on the clipboard takes the next id and, once decoded, joins
    /// the field as a chip (`attach`). False lets the field paste the text.
    pub fn paste(&mut self, content: &rune::ClipboardContent) -> bool {
        let taken = self.take_paste(content);
        let any = !taken.is_empty();
        for (id, pasted) in taken {
            chat::decode_pasted(&self.host, chat::Field::Input, id, pasted);
        }
        any
    }

    fn take_paste(&mut self, content: &rune::ClipboardContent) -> Vec<(u64, chat::Pasted)> {
        let taken = chat::take_ids(&mut self.next_shot, chat::pasted_images(content));
        self.pending.extend(taken.iter().map(|(id, _)| *id));
        taken
    }

    /// Pasted image `id`, decoded: it joins unless the input closed or sent since the paste.
    pub fn pasted(&mut self, id: u64, png: Vec<u8>, preview: chat::Image) {
        if let Some(at) = self.pending.iter().position(|&p| p == id) {
            self.pending.remove(at);
            self.attach(id, png, preview);
        }
    }

    /// The capture shortcut, as its symbols (`chord_symbols`), for the hint; None when unset.
    pub fn set_capture_chord(&mut self, chord: Option<String>) {
        self.capture_chord = chord;
    }

    /// Adds a capture, or replaces the one with the same id.
    pub fn attach(&mut self, id: u64, png: Vec<u8>, preview: chat::Image) {
        match self.shots.iter_mut().find(|a| a.id == id) {
            Some(shot) => (shot.png, shot.preview) = (png, preview),
            None => self.shots.push(Attached { id, png, preview }),
        }
        self.tell_chips();
    }

    /// Drops a capture (its chip's X). True if it was attached.
    pub fn remove(&mut self, id: u64) -> bool {
        if self.preview == Some(id) {
            self.preview = None;
        }
        let before = self.shots.len();
        self.shots.retain(|a| a.id != id);
        self.tell_chips();
        self.shots.len() != before
    }

    #[cfg(test)]
    fn ids(&self) -> Vec<u64> {
        self.shots.iter().map(|a| a.id).collect()
    }

    /// Something to send: text or a capture.
    pub fn send_ready(&self) -> bool {
        !self.query.text.trim().is_empty() || !self.shots.is_empty()
    }

    /// The typed text, trimmed, and the captures that go with it; the field and the attachments
    /// clear. Nothing typed and nothing attached submits nothing.
    #[cfg(test)]
    pub fn take_submission(&mut self) -> Option<(String, Vec<Attached>)> {
        let text = self.query.text.trim().to_owned();
        if text.is_empty() && self.shots.is_empty() {
            return None;
        }
        self.query.clear();
        Some((text, std::mem::take(&mut self.shots)))
    }

    /// Asks the host to close the panel (which calls `close` and fades the window out).
    fn dismiss(&mut self) {
        if let Some(host) = &self.host {
            host.update(|host, _| host.hide_quick_input());
        }
    }

    /// Enter or the send button, with something to send: the panel presses in (true), and the
    /// host sends it once it's down (`release`). Again meanwhile, or nothing to send: false.
    pub fn press(&mut self) -> Option<u64> {
        if !self.open || self.pressed || !self.send_ready() {
            return None;
        }
        self.pressed = true;
        self.presses += 1;
        Some(self.presses)
    }

    /// Press `press` is down: what's typed and attached goes (`send`), unless the input closed
    /// meanwhile.
    pub fn release(&mut self, press: u64) {
        if !self.pressed || press != self.presses {
            return;
        }
        self.pressed = false;
        if self.open {
            self.send_now();
        }
    }

    /// Presses the panel in, the host sending once it's down.
    fn submit(&mut self) {
        if let Some(press) = self.press()
            && let Some(host) = &self.host
        {
            host.update(move |host, _| host.send_after_press(press));
        }
    }

    /// Sends what's typed and attached (`send`); the host keeps up and decodes the captures.
    fn send_now(&mut self) {
        if let Some((id, text, pngs)) = self.send() {
            eprintln!("{}", sent_log(&text, &pngs));
            let windowed = self.windowed;
            if let Some(host) = &self.host {
                host.update(move |host, _| {
                    if windowed { host.sent_to_window(id, text, pngs) } else { host.sent_from_quick_input(id, pngs) }
                });
            }
        }
    }

    /// The field's own keys: Esc closes an enlarged capture, then the input (`escape`);
    /// Backspace in an empty field drops the last capture.
    fn field_key(&mut self, ev: &KeyEvent) -> bool {
        match ev.key {
            Key::Escape => {
                if self.escape() == Escape::Close {
                    self.dismiss();
                }
            }
            Key::Backspace if self.query.text.is_empty() => return self.backspace_empty().is_some(),
            _ => return false,
        }
        true
    }
}
