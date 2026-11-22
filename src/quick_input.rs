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

/// The panel's vertical offset from its rest, opacity and scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub dy: f32,
    pub alpha: f32,
    pub scale: f32,
}

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

/// Where the panel is for `open` and `tucked`: a closed or tucked panel is below the screen
/// (tucked also fades).
pub fn pose(open: bool, tucked: bool) -> Pose {
    match (open, tucked) {
        (true, false) => Pose { dy: 0.0, alpha: 1.0, scale: 1.0 },
        (false, _) => Pose { dy: TUCK, alpha: 1.0, scale: 1.0 },
        (true, true) => Pose { dy: TUCK, alpha: 0.0, scale: 1.0 },
    }
}

/// The panel's rect at rest at the bottom of a `screen` of that size, over a Dock `dock` points
/// tall.
pub fn bottom_panel(screen: [f32; 2], dock: f32) -> [f32; 4] {
    [(screen[0] - PANEL_W) / 2.0, screen[1] - dock.max(0.0) - MARGIN - PANEL_H, PANEL_W, PANEL_H]
}

/// The field's hint: what it's for, given `shots` attached; with none, how to take a screenshot
/// with the capture shortcut `chord`, if one is set.
pub fn hint(shots: usize, chord: Option<&str>) -> String {
    match (shots, chord) {
        (0, Some(chord)) => format!("{}  {chord} to take a screenshot", placeholder(0)),
        _ => placeholder(shots).to_owned(),
    }
}

/// What the log says of a message sent: its size, never its text (the log is a file on disk).
pub fn sent_log(text: &str, pngs: &[Vec<u8>]) -> String {
    let bytes: usize = pngs.iter().map(Vec::len).sum();
    format!("Ailfred: quick input: {} chars with {} capture(s), {bytes} bytes of PNG", text.chars().count(), pngs.len())
}

/// A shortcut's keycaps (`Shortcut::keycaps`, as Settings shows them) as one label: `⇧⌘Space`.
pub fn chord_symbols(keycaps: &[String]) -> String {
    keycaps.concat()
}

fn placeholder(shots: usize) -> &'static str {
    match shots {
        0 => "Ask anything…",
        1 => "Ask about this screenshot…",
        _ => "Ask about these screenshots…",
    }
}

/// The input's panel at the bottom of a `screen` over a Dock `dock` points tall, with `chips`
/// images in its strip (`QuickInput::panel_rect` once placed).
pub fn input_panel(screen: [f32; 2], dock: f32, chips: usize) -> [f32; 4] {
    grown(bottom_panel(screen, dock), chips)
}

/// `rest` grown upward by the strip of `chips` images.
fn grown([x, y, w, h]: [f32; 4], chips: usize) -> [f32; 4] {
    let grow = strip_h(chips);
    [x, y - grow, w, h + grow]
}

/// Whether `n` chips show smaller, when only `full` fit at their size.
pub fn compact(n: usize, full: usize) -> bool {
    n > full
}

/// The height the input's strip of `n` chips adds above its field row: none without chips.
pub fn strip_h(n: usize) -> f32 {
    match n {
        0 => 0.0,
        n if compact(n, INPUT_FULL_CHIPS) => STRIP_TOP + THUMB_COMPACT.1,
        _ => STRIP_TOP + THUMB.1,
    }
}

/// A capture's chip: just its image at its own shape (clicked, `open` shows it enlarged), with
/// an X over its top-right corner on hover (`remove`) that takes no room; `compact` (many of
/// them), smaller.
pub fn chip<S: 'static>(shot: &Attached, compact: bool, remove: impl Fn(&mut S) + 'static, open: impl Fn(&mut S) + 'static) -> El {
    let id = shot.id;
    let preview = shot
        .preview
        .fitted(if compact { THUMB_COMPACT } else { THUMB }, 6.0)
        .key(format!("shot-preview-{id}"))
        .border(1.0, Color::WHITE.alpha(0.16))
        .cursor(Cursor::Pointer)
        .label("Show larger")
        .on_press(open);
    let close = el()
        .key(format!("shot-x-{id}"))
        .size(18.0, 18.0)
        .radius(9.0)
        .center()
        .fill(Color::BLACK.alpha(0.7))
        .border(1.0, Color::WHITE.alpha(0.2))
        .hover(Color::BLACK.alpha(0.9))
        .cursor(Cursor::Pointer)
        .group_opacity(0.0, 1.0)
        .label("Remove image")
        .child(icon(&glyphs::CLOSE, 9.0).color(Color::WHITE))
        .on_press(remove)
        .pin(None, Some(3.0), Some(3.0), None);
    el().key(format!("shot-{id}"))
        .hover_group()
        .transition(Transition::scale(0.9).combined(Transition::opacity()))
        .child(preview)
        .child(close)
}

fn send_button(ready: bool, t: &tok::Theme) -> El {
    let button = el()
        .key("send")
        .size(28.0, 28.0)
        .radius(8.0)
        .continuous_corners()
        .center()
        .label("Send")
        .shortcut_hint("↵");
    if ready {
        button
            .fill(ACCENT)
            .hover(Color::WHITE.alpha(0.10))
            .press(Color::BLACK.alpha(0.12))
            .press_scale(0.94)
            .cursor(Cursor::Pointer)
            .child(icon(&icons::ARROW_UP, 16.0).color(Color::WHITE))
            .on_press(|s: &mut QuickInput| s.submit())
    } else {
        button.fill(Color::WHITE.alpha(0.08)).disabled(true).child(icon(&icons::ARROW_UP, 16.0).color(t.text_secondary))
    }
}

/// The input's and the chat's panel: the system material under the frost, its border, shadows
/// and corners, holding `layers` pinned to its top left (each at its own size, so a panel that
/// reshapes reveals them rather than reflowing them). Keyed `key`; takes clicks if `hit`.
pub fn surface(key: &str, hit: bool, layers: Vec<El>) -> El {
    let mut frost = el()
        .key("frost")
        .w_full()
        .h_full()
        .radius(RADIUS)
        .continuous_corners()
        .clip_rounded(RADIUS)
        .fill(FROST)
        .border_gradient(1.0, Color::WHITE.alpha(0.14), Color::WHITE.alpha(0.05));
    for layer in layers {
        frost = frost.child(layer);
    }
    let panel = el()
        .key(key)
        .radius(RADIUS)
        .continuous_corners()
        .box_shadow(Shadow::core_animation(0.0, 0.5, 1.5, Color::BLACK.alpha(0.3)))
        .box_shadow(Shadow::core_animation(0.0, 10.0, 22.0, Color::BLACK.alpha(0.4)))
        .system_material(SystemMaterial::Popover)
        .child(frost);
    if hit { panel.hit() } else { panel }
}

/// The window covers the display: the input's panel at its rect, and the chat's at the right
/// edge (the input's panel becomes it on send).
pub fn view(s: &QuickInput, cx: &mut Cx<QuickInput>) -> El {
    crate::settings::apply_motion(cx);
    let t = tok::theme(true);
    let Pose { dy, alpha, scale } = s.current_pose();
    let motion = Anim::from(GLIDE);
    let [.., w, h] = s.panel_rect();
    let [x, bottom] = s.pins();
    // Just sent: the row is in the chat's panel, fading; the input's new panel waits empty, out
    // of view, landing at once (its element was the chat's, maybe mid-slide).
    let layers = if s.morphing { Vec::new() } else { vec![input_row(s, &t)] };
    let mut input = surface(s.input_key(), s.open, layers)
        .size(w, h)
        // By its bottom edge: placed for another screen, or none yet at launch, closed it still
        // waits below the window's edge, never on screen.
        .pin(Some(x), None, None, Some(bottom))
        .offset(0.0, dy)
        .offset_motion(motion)
        .opacity(alpha)
        .opacity_motion(motion)
        .scale(scale)
        .scale_motion(if s.pressed { Anim::new(Curve::ease_in_out(PRESS.as_secs_f32())) } else { motion })
        .motion(GLIDE);
    if s.morphing {
        input = input.instant_changes();
    }
    let chat = chat::panel(&s.chat, s.chat_key(), |s: &mut QuickInput| &mut s.chat, s.morphing.then(|| input_row(s, &t)));
    let root = el().w_full().h_full().child(chat).child(input);
    match s.preview_image() {
        Some(image) => root.child(chat::lightbox(image, s.chat.screen(), |s: &mut QuickInput| s.close_preview())),
        None => root,
    }
}

/// The chips in a strip above the field row, then the row (the sparkle, the field, the send
/// button), at the panel's size, pinned to its top left.
fn input_row(s: &QuickInput, t: &tok::Theme) -> El {
    let field = text_input(&s.query, TextStyle::new(15.0, 400.0).lh(22.0))
        .key("query")
        .label("Ask anything")
        .single_line()
        .grow(1.0)
        .min_w(FIELD_MIN_W)
        .color(t.text)
        .caret(t.text, ACCENT.alpha(0.35))
        .placeholder(hint(s.shots.len(), s.capture_chord.as_deref()), t.text_secondary.alpha(0.8))
        .animate_layout()
        .on_edit(|s: &mut QuickInput| &mut s.query)
        .on_input_key(|s: &mut QuickInput, ev: &KeyEvent, _: &mut Tx| s.field_key(ev))
        .on_input_paste(|s: &mut QuickInput, content: &rune::ClipboardContent, _: &mut Tx| s.paste(content))
        .on_submit(|s: &mut QuickInput, _: &mut Tx| s.submit());
    let compact = compact(s.shots.len(), INPUT_FULL_CHIPS);
    let mut chips = row().key("chips").w_full().h(strip_h(s.shots.len())).px(ROW_PAD).pt(STRIP_TOP).gap(8.0).items_end();
    for shot in &s.shots {
        let id = shot.id;
        let (remove, open) = (move |s: &mut QuickInput| _ = s.remove(id), move |s: &mut QuickInput| s.open_preview(id));
        chips = chips.child(chip(shot, compact, remove, open));
    }
    let field_row = row()
        .key("field-row")
        .role("textbox")
        .w_full()
        .h(PANEL_H)
        .px(ROW_PAD)
        .gap(8.0)
        .center_y()
        .child(icon(&glyphs::SPARKLE, 18.0).color(ACCENT).mr(2.0))
        .child(field)
        .child(send_button(s.send_ready(), t).ml(2.0));
    let [_, _, w, h] = s.panel_rect();
    let mut layer = col().key("input-layer").size(w, h).pin(Some(0.0), Some(0.0), None, None);
    if !s.shots.is_empty() {
        layer = layer.child(chips);
    }
    layer.child(field_row)
}

pub fn on_key(s: &mut QuickInput, event: &KeyEvent, _: &mut Tx) -> bool {
    if s.open { s.field_key(event) } else { chat::on_key(&mut s.chat, event) }
}

/// Clicking elsewhere closes it, unless a capture took the keys (the host decides).
pub fn on_focus(s: &mut QuickInput, focused: bool, _: &mut Tx) {
    if !focused && s.is_open() && let Some(host) = &s.host {
        host.update(|host, _| host.quick_input_blurred());
    }
}

/// The bottom gap the Dock takes on a screen: `visible`'s bottom over `frame`'s (AppKit
/// rects, `[x, y, w, h]` with y up). Zero when the Dock isn't at the bottom (or is hidden).
pub fn dock_inset(frame: [f32; 4], visible: [f32; 4]) -> f32 {
    (visible[1] - frame[1]).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close_to(a: Color, b: Color) -> bool {
        [a.r - b.r, a.g - b.g, a.b - b.b, a.a - b.a].iter().all(|d| d.abs() < 0.002)
    }

    #[test]
    fn colours_are_the_srgb_ones_in_linear() {
        assert!(close_to(ACCENT, rune::color::hex("#D97757").unwrap()), "{ACCENT:?}");
        assert!(close_to(FROST, rune::color::hex("#1E1E1E").unwrap().alpha(FROST.a)), "{FROST:?}");
    }

    #[test]
    fn starts_closed_below_the_screen_so_it_slides_in_from_outside() {
        let s = QuickInput::new(None);
        assert!(!s.is_open());
        assert_eq!(pose(false, false), Pose { dy: TUCK, alpha: 1.0, scale: 1.0 });
        const { assert!(TUCK >= PANEL_H + MARGIN + PAD, "fully below the bottom edge, shadow included") };
    }

    #[test]
    fn opening_rises_to_rest_opaque_with_the_field_focused() {
        let mut s = QuickInput::new(None);
        s.open();
        assert!(s.is_open());
        assert!(s.query.focused, "the field takes the keys");
        assert_eq!(pose(true, false), Pose { dy: 0.0, alpha: 1.0, scale: 1.0 });
    }

    fn tiny() -> chat::Image {
        chat::Image::from_rgba(1, 1, vec![0; 4])
    }

    #[test]
    fn closing_reports_whether_it_was_open_and_keeps_the_text() {
        let mut s = QuickInput::new(None);
        assert!(!s.close(), "already closed");
        s.open();
        s.query = TextEdit::new("draft");
        assert!(s.close());
        assert!(!s.is_open());
        assert_eq!(s.query.text, "draft");
    }

    #[test]
    fn submitting_takes_trimmed_text_and_clears_the_field() {
        let mut s = QuickInput::new(None);
        s.open();
        s.query = TextEdit::new("  hello there \n");
        let (text, shots) = s.take_submission().unwrap();
        assert_eq!((text.as_str(), shots.len()), ("hello there", 0));
        assert_eq!(s.query.text, "");
    }

    #[test]
    fn submitting_nothing_does_nothing() {
        let mut s = QuickInput::new(None);
        s.query = TextEdit::new("   ");
        assert!(s.take_submission().is_none());
        assert_eq!(s.query.text, "   ");
    }

    #[test]
    fn send_is_ready_with_text_or_a_capture() {
        let mut s = QuickInput::new(None);
        assert!(!s.send_ready());
        s.query = TextEdit::new("  ");
        assert!(!s.send_ready(), "blank text isn't something to send");
        s.query = TextEdit::new("hi");
        assert!(s.send_ready());
        s.query = TextEdit::new("");
        s.attach(1, vec![1], tiny());
        assert!(s.send_ready());
    }

    #[test]
    fn captures_attach_and_go_with_the_submission() {
        let mut s = QuickInput::new(None);
        s.open();
        s.attach(1, vec![1], tiny());
        s.attach(2, vec![2], tiny());
        assert_eq!(s.ids(), vec![1, 2]);
        s.query = TextEdit::new("what is this?");
        let (text, shots) = s.take_submission().unwrap();
        assert_eq!(text, "what is this?");
        assert_eq!(shots.iter().map(|a| a.png.clone()).collect::<Vec<_>>(), vec![vec![1], vec![2]]);
        assert!(s.ids().is_empty(), "attachments clear on submit");
    }

    #[test]
    fn the_same_area_again_replaces_its_capture_in_place() {
        let mut s = QuickInput::new(None);
        s.attach(1, vec![1], tiny());
        s.attach(2, vec![2], tiny());
        s.attach(1, vec![9], tiny());
        assert_eq!(s.ids(), vec![1, 2], "no second chip, same place");
        assert_eq!(s.shots[0].png, vec![9], "with the marked-up image");
    }

    #[test]
    fn a_chips_x_removes_only_that_capture() {
        let mut s = QuickInput::new(None);
        s.attach(1, vec![1], tiny());
        s.attach(2, vec![2], tiny());
        assert!(s.remove(1));
        assert_eq!(s.ids(), vec![2]);
        assert!(!s.remove(1), "already gone");
    }

    #[test]
    fn backspace_in_an_empty_field_removes_the_last_capture() {
        let mut s = QuickInput::new(None);
        assert_eq!(s.backspace_empty(), None, "nothing to remove");
        s.attach(1, vec![1], tiny());
        s.attach(2, vec![2], tiny());
        assert_eq!(s.backspace_empty(), Some(2));
        assert_eq!(s.ids(), vec![1]);
        assert_eq!(s.backspace_empty(), Some(1));
        assert!(s.ids().is_empty());
    }

    #[test]
    fn an_image_pasted_into_the_input_takes_the_next_id_and_text_is_left_to_the_field() {
        use crate::chat::tests::{a_png, clip};
        let mut s = QuickInput::new(None);
        assert_eq!(s.take_paste(&clip("just text", None)), vec![]);
        assert_eq!(s.take_paste(&clip("caption", Some(a_png()))), vec![(1, chat::Pasted::Png(a_png()))], "the image, not its text");
        assert_eq!(s.take_paste(&clip("", Some(a_png()))).first().map(|(id, _)| *id), Some(2));
    }

    #[test]
    fn a_chips_image_opens_enlarged_and_esc_closes_it_before_the_input() {
        let mut s = QuickInput::new(None);
        s.open();
        s.attach(1, vec![1], tiny());
        s.attach(2, vec![2], tiny());
        assert!(s.preview_image().is_none());
        s.open_preview(2);
        assert!(s.preview_image().is_some());
        assert_eq!(s.escape(), Escape::Preview, "the enlarged image closes first");
        assert!(s.preview_image().is_none());
        assert_eq!(s.ids(), vec![1, 2], "its capture stays attached");
        assert!(s.is_open(), "the input stays");
        assert_eq!(s.escape(), Escape::Close, "then the input closes");
    }

    #[test]
    fn removing_its_capture_or_closing_closes_the_enlarged_image() {
        let mut s = QuickInput::new(None);
        s.open();
        s.attach(1, vec![1], tiny());
        s.attach(2, vec![2], tiny());
        s.open_preview(2);
        assert!(s.remove(1), "another one");
        assert!(s.preview_image().is_some());
        assert_eq!(s.backspace_empty(), Some(2));
        assert!(s.preview_image().is_none(), "its own");
        assert_eq!(s.escape(), Escape::Close, "nothing enlarged left to close first");
        s.open();
        s.attach(3, vec![3], tiny());
        s.open_preview(3);
        s.close();
        assert!(s.preview_image().is_none());
    }

    #[test]
    fn the_chats_enlarged_image_shows_in_the_shared_window() {
        let mut s = QuickInput::new(None);
        s.chat.attach(5, vec![5], tiny());
        s.chat.open_preview(5);
        assert!(s.preview_image().is_some());
        s.close_preview();
        assert!(s.preview_image().is_none());
    }

    #[test]
    fn closing_discards_the_captures_and_keeps_the_text() {
        let mut s = QuickInput::new(None);
        s.open();
        s.attach(1, vec![1], tiny());
        s.query = TextEdit::new("draft");
        s.close();
        assert!(s.ids().is_empty());
        assert_eq!(s.query.text, "draft");
    }

    #[test]
    fn many_chips_go_compact() {
        assert!(!compact(INPUT_FULL_CHIPS, INPUT_FULL_CHIPS));
        assert!(compact(INPUT_FULL_CHIPS + 1, INPUT_FULL_CHIPS));
        let room = PANEL_W - 2.0 * ROW_PAD;
        assert!(INPUT_FULL_CHIPS as f32 * (THUMB.0 + 8.0) <= room, "the full ones fit on one line");
    }

    #[test]
    fn chips_sit_in_a_strip_above_the_field_which_stays_put() {
        assert_eq!(strip_h(0), 0.0, "no chips, no strip");
        assert_eq!(strip_h(1), STRIP_TOP + THUMB.1);
        assert_eq!(strip_h(INPUT_FULL_CHIPS), STRIP_TOP + THUMB.1);
        assert_eq!(strip_h(INPUT_FULL_CHIPS + 1), STRIP_TOP + THUMB_COMPACT.1, "smaller when many");
        let mut s = QuickInput::new(None);
        let r = [10.0, 500.0, PANEL_W, PANEL_H];
        s.set_rect(r);
        assert_eq!(s.panel_rect(), r);
        s.attach(1, vec![1], tiny());
        let grow = strip_h(1);
        assert_eq!(s.panel_rect(), [10.0, 500.0 - grow, PANEL_W, PANEL_H + grow], "grows upward");
        s.open();
        s.set_tucked(true);
        assert_eq!(s.current_pose().dy, TUCK + grow, "tucked, the strip too goes out of the screen");
    }

    #[test]
    fn the_hint_says_what_the_field_is_for() {
        assert_eq!(placeholder(0), "Ask anything…");
        assert_eq!(placeholder(1), "Ask about this screenshot…");
        assert_eq!(placeholder(3), "Ask about these screenshots…");
        assert_eq!(hint(0, Some("⇧⌘Space")), "Ask anything…  ⇧⌘Space to take a screenshot");
        assert_eq!(hint(0, None), "Ask anything…", "no capture shortcut set");
        assert_eq!(hint(1, Some("⇧⌘Space")), "Ask about this screenshot…");
    }

    #[test]
    fn a_shortcut_reads_as_settings_shows_it() {
        let caps = |s: &str| s.parse::<rune::Shortcut>().unwrap().keycaps();
        assert_eq!(chord_symbols(&caps("shift+super+space")), "⇧⌘Space");
        assert_eq!(chord_symbols(&caps("super+shift+4")), "⇧⌘4", "in the Mac's order");
        assert_eq!(chord_symbols(&caps("super+slash")), "⌘/");
        assert_eq!(chord_symbols(&caps("alt+f5")), "⌥F5");
    }

    #[test]
    fn a_capture_alone_can_be_submitted() {
        let mut s = QuickInput::new(None);
        s.attach(1, vec![0], tiny());
        let (text, shots) = s.take_submission().unwrap();
        assert_eq!((text.as_str(), shots.len()), ("", 1));
    }

    #[test]
    fn tucking_at_the_bottom_slides_it_out_of_the_screen_and_back() {
        let mut s = QuickInput::new(None);
        s.open();
        s.set_tucked(true);
        assert_eq!(pose(s.open, s.tucked), Pose { dy: TUCK, alpha: 0.0, scale: 1.0 }, "below the screen");
        s.set_tucked(false);
        assert_eq!(pose(s.open, s.tucked).dy, 0.0);
    }

    #[test]
    fn a_closed_input_stays_closed_when_tucked_and_reopens_untucked() {
        let mut s = QuickInput::new(None);
        s.set_tucked(true);
        assert_eq!(pose(s.open, s.tucked).dy, TUCK);
        s.open();
        assert!(!s.tucked);
    }

    #[test]
    fn over_a_dock_it_still_rises_from_the_screens_edge() {
        let mut s = QuickInput::new(None);
        s.at_bottom(70.0);
        assert_eq!(s.current_pose().dy, TUCK + 70.0, "below the screen, past the Dock");
        s.open();
        assert_eq!(s.current_pose().dy, 0.0, "rests above the Dock");
        s.set_tucked(true);
        assert_eq!(s.current_pose().dy, TUCK + 70.0, "tucks out of the screen too");
        s.close();
        assert_eq!(s.current_pose().dy, TUCK + 70.0);
    }

    #[test]
    fn at_the_bottom_the_panel_is_centred_above_the_gap_and_dock() {
        assert_eq!(bottom_panel([1920.0, 1080.0], 70.0), [650.0, 1080.0 - 70.0 - MARGIN - PANEL_H, PANEL_W, PANEL_H]);
        assert_eq!(bottom_panel([1920.0, 1080.0], 0.0)[1], 1080.0 - MARGIN - PANEL_H);
    }

    #[test]
    fn the_dock_inset_is_the_visible_areas_bottom_gap() {
        let frame = [0.0, 0.0, 1512.0, 982.0];
        assert_eq!(dock_inset(frame, [0.0, 70.0, 1512.0, 879.0]), 70.0, "Dock at the bottom");
        assert_eq!(dock_inset(frame, [0.0, 0.0, 1512.0, 949.0]), 0.0, "hidden Dock: only the menu bar");
        assert_eq!(dock_inset(frame, [64.0, 0.0, 1448.0, 949.0]), 0.0, "Dock on the left");
        // A second display above the first: AppKit y is global.
        assert_eq!(dock_inset([0.0, 982.0, 1920.0, 1080.0], [0.0, 1052.0, 1920.0, 985.0]), 70.0);
    }

    fn sent(text: &str, shot: Option<u64>) -> QuickInput {
        let mut s = QuickInput::new(None);
        s.set_rect(bottom_panel([1512.0, 982.0], 0.0));
        s.open();
        s.query = TextEdit::new(text);
        if let Some(id) = shot {
            s.attach(id, vec![1, 2, 3], tiny());
        }
        s
    }

    #[test]
    fn closed_it_hides_past_the_windows_bottom_whatever_screen_it_was_placed_for() {
        let top = |s: &QuickInput, window_h: f32| {
            let [_, bottom] = s.pins();
            window_h - bottom - s.panel_rect()[3] + s.current_pose().dy
        };
        // Never placed (launch), or placed for a smaller display than it shows on.
        let fresh = QuickInput::new(None);
        let mut small = QuickInput::new(None);
        small.at_bottom(70.0);
        small.set_rect(bottom_panel([1440.0, 900.0], 70.0));
        small.attach(1, vec![1], tiny());
        for s in [&fresh, &small] {
            for window_h in [900.0, 1080.0, 1440.0] {
                assert!(top(s, window_h) >= window_h + PAD, "closed: below a {window_h}-tall window");
            }
        }
        // Open, it's at its rect.
        let mut s = QuickInput::new(None);
        s.at_bottom(70.0);
        s.set_rect(bottom_panel([1920.0, 1080.0], 70.0));
        s.attach(1, vec![1], tiny());
        s.open();
        assert_eq!([s.pins()[0], top(&s, 1080.0)], [s.panel_rect()[0], s.panel_rect()[1]]);
    }

    #[test]
    fn sending_presses_the_whole_panel_in_then_goes_once_its_down() {
        let mut s = sent("what is this?", None);
        let press = s.press().unwrap();
        assert_eq!(s.current_pose().scale, PRESSED_SCALE, "pressed in");
        assert!(!s.morphing && s.is_open(), "not sent yet");
        assert_eq!(s.press(), None, "Enter again meanwhile sends nothing more");
        s.release(press);
        assert!(s.morphing && s.chat.is_open(), "sent: it becomes the chat");
        assert_eq!(s.current_pose().scale, 1.0);
        assert_eq!(s.press(), None, "nothing left to send");
        // Closed while pressed: nothing goes.
        let mut s = sent("draft", None);
        let press = s.press().unwrap();
        s.close();
        s.release(press);
        assert!(!s.morphing && !s.chat.is_open());
        assert_eq!(QuickInput::new(None).press(), None, "empty: no press");
    }

    #[test]
    fn a_release_goes_with_its_own_press_only() {
        let mut s = sent("a", None);
        let first = s.press().unwrap();
        s.close();
        s.open();
        let second = s.press().unwrap();
        s.release(first);
        assert!(!s.morphing && s.is_open(), "the first press's timer doesn't send the second early");
        assert_eq!(s.current_pose().scale, PRESSED_SCALE, "still pressed");
        s.release(second);
        assert!(s.morphing);
    }

    #[test]
    fn cmd_n_before_the_chat_lands_keeps_what_goes_into_the_new_input() {
        let mut s = sent("hi", Some(1));
        s.send();
        s.at_bottom(0.0);
        s.open();
        assert!(s.query.text.is_empty() && s.ids().is_empty(), "the new input starts empty");
        assert_eq!(s.panel_rect()[3], PANEL_H, "not grown by the sent images");
        s.query = TextEdit::new("next");
        s.attach(99, vec![9], tiny());
        s.land();
        assert_eq!((s.query.text.as_str(), s.ids()), ("next", vec![99]), "the landing doesn't clear it");
    }

    #[test]
    fn an_image_still_decoding_when_the_input_closes_or_sends_is_dropped() {
        use crate::chat::tests::{a_png, clip};
        let mut s = QuickInput::new(None);
        s.open();
        let id = s.take_paste(&clip("", Some(a_png())))[0].0;
        s.close();
        s.open();
        s.pasted(id, vec![1], tiny());
        assert!(s.ids().is_empty(), "not into the next input");
        let id = s.take_paste(&clip("", Some(a_png())))[0].0;
        s.pasted(id, vec![1], tiny());
        assert_eq!(s.ids(), vec![id], "the same input: it joins");
        let late = s.take_paste(&clip("", Some(a_png())))[0].0;
        s.query = TextEdit::new("go");
        s.send();
        s.land();
        s.open();
        s.pasted(late, vec![1], tiny());
        assert!(s.ids().is_empty(), "not after it sent");
    }

    #[test]
    fn the_log_never_holds_what_was_typed() {
        let line = sent_log("my secret question", &[vec![1, 2, 3]]);
        assert!(!line.contains("secret"), "{line}");
        assert!(line.contains("18 chars") && line.contains("1 capture") && line.contains("3 bytes"), "{line}");
    }

    #[test]
    fn a_new_input_opened_before_the_chat_lands_keeps_the_keys() {
        let mut s = sent("hi", None);
        s.send();
        s.open();
        s.land();
        assert!(s.query.focused && !s.chat.draft_focused());
    }

    #[test]
    fn the_panel_rests_at_its_rect() {
        let mut s = QuickInput::new(None);
        let r = [10.0, 20.0, 500.0, PANEL_H];
        s.set_rect(r);
        assert_eq!(s.rect, r);
    }

    #[test]
    fn sending_hands_the_inputs_panel_to_the_chat_which_keeps_its_row_until_it_lands() {
        let mut s = sent("what is this?", Some(4));
        let panel = s.input_key();
        let (id, text, pngs) = s.send().expect("something to send");
        assert_eq!((text.as_str(), pngs), ("what is this?", vec![vec![1, 2, 3]]));
        assert!(!s.is_open() && s.chat.is_open());
        assert_eq!(s.chat_key(), panel, "the same element: it reshapes into the chat");
        assert_ne!(s.input_key(), panel, "the next input gets the other");
        assert!(s.morphing && !s.chat.landed());
        assert_eq!((s.query.text.as_str(), s.ids()), ("what is this?", vec![4]), "the row shows what was sent while it fades");
        assert!(!s.query.focused && s.chat.draft_focused(), "the chat's field has the keys");
        s.land();
        assert!(!s.morphing && s.chat.landed());
        assert_eq!((s.query.text.as_str(), s.ids().len()), ("", 0), "the next input starts empty");
        s.chat.close();
        s.open();
        s.query = TextEdit::new("again");
        assert_eq!(s.send().map(|m| m.0), Some(id + 1));
    }

    #[test]
    fn with_the_chat_in_its_window_sending_closes_the_input_and_leaves_the_message_to_the_host() {
        let mut s = sent("first", None);
        assert!(s.send().is_some());
        let mut chat = s.take_chat();
        assert!(chat.windowed() && chat.landed(), "handed over landed, mid-flight or not");
        assert!(!s.chat.is_open() && !s.morphing);
        assert_eq!(chat.reply_to(0).map(|r| r.prompt.as_str()), Some("first"), "the conversation went with it");
        s.open();
        s.query = TextEdit::new("more");
        let (id, text, _) = s.send().expect("sent");
        assert_eq!((id, text.as_str()), (1, "more"));
        assert!(!s.is_open() && !s.morphing && !s.chat.is_open(), "the input slides down, no side panel");
        assert!(s.chat.reply_to(1).is_none(), "the message isn't the side panel's");
        chat.stop(std::time::Instant::now());
        s.put_chat(chat);
        assert!(!s.chat.windowed() && !s.chat.is_open());
        s.open();
        s.query = TextEdit::new("side");
        assert!(s.send().is_some());
        assert!(s.chat.is_open() && s.chat.reply_to(2).is_some(), "back to the side panel");
    }

    #[test]
    fn sending_with_the_chat_open_adds_to_it_and_the_input_slides_down() {
        let mut s = sent("more", None);
        s.chat.open();
        let chat = s.chat_key();
        assert!(s.send().is_some());
        assert!(!s.is_open() && s.chat.is_open());
        assert_eq!(s.chat_key(), chat);
        assert!(!s.morphing && s.query.text.is_empty());
    }

    #[test]
    fn nothing_typed_or_attached_sends_nothing() {
        let mut s = sent("  ", None);
        assert!(s.send().is_none());
        assert!(s.is_open() && !s.chat.is_open());
    }

    #[test]
    fn the_shortcut_parses() {
        assert!(SHORTCUT.parse::<rune::Shortcut>().is_ok());
    }
}
