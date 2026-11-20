//! The quick input: a floating text field that slides up from the bottom of the display under
//! the pointer, above every other window. Ctrl+Option+Space or the menu opens it; Esc, a click
//! elsewhere closes it, sliding back down. Enter sends: the panel itself turns into the chat
//! panel (chat.rs), which this window also holds, springing to the right edge as it reshapes.
//!
//! While it's open, captures land in it as "Image" chips in the field row (hover one for its X),
//! dragging out a capture area tucks it away, and once an area is picked it arrives beside that
//! area, so the prompt reads as being about it.

use std::time::Duration;

use rune::tok;
use rune::{
    Anim, AppHandle, Color, Cursor, Cx, El, Key, KeyEvent, Shadow, Spring, SystemMaterial, TextEdit, TextStyle, Transition, Tx, el, icon,
    row, text, text_input,
};
use rune_kit::glyphs;

use crate::Host;
use crate::chat::{self, Chat};
use crate::icons;

/// The fixed global chord that toggles the panel (Option+Space alone is Claude's quick entry).
pub const SHORTCUT: &str = "ctrl+alt+space";
/// The panel's widest and narrowest, its height, and its row's side padding, in points.
pub const PANEL_W: f32 = 620.0;
const PANEL_MIN_W: f32 = 480.0;
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
/// Beside a capture area it arrives instead: from this little below, faded and slightly small.
pub const ARRIVE: f32 = 12.0;
const ARRIVE_SCALE: f32 = 0.98;
/// The panel's gap to a capture area it sits beside, and to the screen's edges.
const BESIDE_GAP: f32 = 16.0;
const EDGE_GAP: f32 = 8.0;
/// How long the window takes to fade out once closed: long enough for the slide down to play.
pub const EXIT: Duration = Duration::from_millis(240);
/// How long the panel waits out of view beside an area before rising: a frame or two, so the
/// rise starts from there and not from where it was.
pub const ARRIVAL_DELAY: Duration = Duration::from_millis(40);
/// The slide from the screen edge, and the arrival beside an area.
const GLIDE: Spring = tok::motion::GLIDE;
const SETTLE: Spring = tok::motion::SETTLE;

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
}

pub struct QuickInput {
    query: TextEdit,
    open: bool,
    /// Out of view while a capture area is dragged.
    tucked: bool,
    /// Placed beside a capture area (it arrives and leaves in place) rather than at the bottom
    /// (it slides in and out of the screen).
    beside: bool,
    /// Waiting out of view beside this area, about to rise there.
    arriving: Option<u64>,
    /// At the bottom, the Dock's height under the panel's gap: a closed or tucked panel drops
    /// past it too, out of the screen.
    dock: f32,
    /// The capture's tools have the keys: the field shows it's waiting.
    editing: bool,
    shots: Vec<Attached>,
    /// The capture area on screen that the input sits beside, while there is one.
    area: Option<u64>,
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
}

/// What Esc in the field does.
#[derive(Debug, PartialEq)]
pub enum Escape {
    /// Discards the capture area on screen and its image; the input stays.
    Area(u64),
    /// Closes the input.
    Close,
}

/// More chips than this show as icons only, so the field keeps its room.
const LABELLED_CHIPS: usize = 3;
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
            beside: false,
            arriving: None,
            dock: 0.0,
            editing: false,
            shots: Vec::new(),
            area: None,
            chat: Chat::new(host.clone()),
            host,
            rect: [0.0, 0.0, PANEL_W, PANEL_H],
            slot: false,
            morphing: false,
            next_message: 0,
        }
    }

    /// The panel's rect at rest (points on the window's display): at the bottom or beside an
    /// area. Applied without motion, with the panel out of view.
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
        if self.chat.is_open() {
            self.close();
            self.query.clear();
            self.chat.push(message);
        } else {
            self.open = false;
            self.tucked = false;
            self.editing = false;
            self.area = None;
            self.slot = !self.slot;
            self.morphing = true;
            self.chat.morph(message);
        }
        Some((id, text, pngs))
    }

    /// The panel is the chat's now: the input's row goes (the next input starts empty) and the
    /// chat's content fades in.
    pub fn land(&mut self) {
        if std::mem::replace(&mut self.morphing, false) {
            self.query.clear();
            self.shots.clear();
        }
        self.chat.land();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Rises into place with the field focused.
    pub fn open(&mut self) {
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
        self.editing = false;
        self.shots.clear();
        self.area = None;
        std::mem::replace(&mut self.open, false)
    }

    /// Before opening: below the screen at the bottom (over a Dock `dock` points tall), whatever
    /// it was doing before, so `open` rises from the screen's edge. Applied without motion.
    pub fn at_bottom(&mut self, dock: f32) {
        self.dock = dock.max(0.0);
        self.beside = false;
        self.tucked = false;
        self.arriving = None;
        self.query.focused = true;
    }

    /// Where the panel is now.
    pub fn current_pose(&self) -> Pose {
        let p = pose(self.open, self.tucked, self.beside);
        if p.dy == TUCK { Pose { dy: TUCK + self.dock, ..p } } else { p }
    }

    /// Slides out of view (true) or back into place (false).
    pub fn set_tucked(&mut self, tucked: bool) {
        self.tucked = tucked;
        self.arriving = None;
        if !tucked {
            self.query.focused = true;
        }
    }

    /// About to arrive beside capture area `id`: waits there, out of view, until `arrived`.
    pub fn arrive(&mut self, id: u64) {
        self.beside = true;
        self.tucked = true;
        self.arriving = Some(id);
        self.area = Some(id);
    }

    /// Rises beside area `id`, unless something else happened since `arrive` (a new drag).
    pub fn arrived(&mut self, id: u64) {
        if self.arriving == Some(id) && self.open {
            self.tucked = false;
            self.arriving = None;
            self.query.focused = true;
        }
    }

    /// Area `id` is on screen and the input beside it (None: no area any more).
    #[cfg(test)]
    fn set_area(&mut self, id: Option<u64>) {
        self.area = id;
    }

    /// The capture's tools have the keys (true) or gave them back.
    pub fn set_editing(&mut self, editing: bool) {
        self.editing = editing;
    }

    /// Esc in the field, one step at a time: the capture area on screen goes first (its image
    /// too), then the input closes.
    pub fn escape(&mut self) -> Escape {
        match self.area.take() {
            Some(id) => {
                self.remove(id);
                Escape::Area(id)
            }
            None => Escape::Close,
        }
    }

    /// Backspace in an empty field removes the last capture; its id, if one went.
    pub fn backspace_empty(&mut self) -> Option<u64> {
        let id = self.shots.pop()?.id;
        if self.area == Some(id) {
            self.area = None;
        }
        Some(id)
    }

    /// Adds a capture, or replaces the one from the same capture area.
    pub fn attach(&mut self, id: u64, png: Vec<u8>) {
        match self.shots.iter_mut().find(|a| a.id == id) {
            Some(shot) => shot.png = png,
            None => self.shots.push(Attached { id, png }),
        }
    }

    /// Drops a capture (its chip's X). True if it was attached.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.shots.len();
        self.shots.retain(|a| a.id != id);
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

    /// Sends what's typed and attached (`send`); the host keeps up and decodes the captures.
    fn submit(&mut self) {
        if let Some((id, text, pngs)) = self.send() {
            let bytes: usize = pngs.iter().map(Vec::len).sum();
            eprintln!("Capibarapp: quick input: {text:?} with {} capture(s), {bytes} bytes of PNG", pngs.len());
            if let Some(host) = &self.host {
                host.update(move |host, _| host.sent_from_quick_input(id, pngs));
            }
        }
    }

    /// A chip's X: the capture goes, and so does its area if it's still on screen.
    fn unattach(&mut self, id: u64) {
        if self.area == Some(id) {
            self.area = None;
        }
        if self.remove(id) {
            self.forget_area(id);
        }
    }

    fn forget_area(&self, id: u64) {
        if let Some(host) = &self.host {
            host.update(move |host, _| host.forget_capture_area(id));
        }
    }

    /// Esc in the field: see `escape`.
    fn on_escape(&mut self) {
        match self.escape() {
            Escape::Area(id) => self.forget_area(id),
            Escape::Close => self.dismiss(),
        }
    }

    /// The field's own keys: Esc steps back, Backspace in an empty field drops the last capture,
    /// ⌘E opens or closes the capture's tools.
    fn field_key(&mut self, ev: &KeyEvent) -> bool {
        match ev.key {
            Key::Escape => self.on_escape(),
            Key::Backspace if self.query.text.is_empty() => match self.backspace_empty() {
                Some(id) => self.forget_area(id),
                None => return false,
            },
            _ if ev.is_command('e') => {
                if let Some(host) = &self.host {
                    host.update(|host, _| host.toggle_capture_tools());
                }
            }
            _ => return false,
        }
        true
    }
}

/// Where the panel is for `open`, `tucked` and `beside`: at the bottom a closed or tucked panel
/// is below the screen (tucked also fades, so arriving elsewhere reads as arriving); beside an
/// area it arrives and leaves in place, from a little below, faded and slightly small.
pub fn pose(open: bool, tucked: bool, beside: bool) -> Pose {
    match (open, tucked, beside) {
        (true, false, _) => Pose { dy: 0.0, alpha: 1.0, scale: 1.0 },
        (false, _, true) | (true, true, true) => Pose { dy: ARRIVE, alpha: 0.0, scale: ARRIVE_SCALE },
        (false, _, false) => Pose { dy: TUCK, alpha: 1.0, scale: 1.0 },
        (true, true, false) => Pose { dy: TUCK, alpha: 0.0, scale: 1.0 },
    }
}

/// The panel's rect (points on a `screen` of that size) beside capture area `sel`
/// (`[x, y, w, h]`): right of it, else left, else below, else above, else over its bottom.
/// Beside it's vertically centred on it; above or below it takes the area's width (within the
/// panel's widest and narrowest) and lines up with its left edge. Always on screen.
pub fn beside(sel: [f32; 4], screen: [f32; 2]) -> [f32; 4] {
    let ([x, y, w, h], [sw, sh], ph) = (sel, screen, PANEL_H);
    let clamp_x = |v: f32, pw: f32| v.min(sw - pw - EDGE_GAP).max(EDGE_GAP);
    let clamp_y = |v: f32| v.min(sh - ph - EDGE_GAP).max(EDGE_GAP);
    let middle = clamp_y(y + h / 2.0 - ph / 2.0);
    // Above or below: as wide as the area, within the panel's narrowest and widest.
    let along = w.clamp(PANEL_MIN_W, PANEL_W);
    if x + w + BESIDE_GAP + PANEL_W <= sw - EDGE_GAP {
        [x + w + BESIDE_GAP, middle, PANEL_W, ph]
    } else if x - BESIDE_GAP - PANEL_W >= EDGE_GAP {
        [x - BESIDE_GAP - PANEL_W, middle, PANEL_W, ph]
    } else if y + h + BESIDE_GAP + ph <= sh - EDGE_GAP {
        [clamp_x(x, along), y + h + BESIDE_GAP, along, ph]
    } else if y - BESIDE_GAP - ph >= EDGE_GAP {
        [clamp_x(x, along), y - BESIDE_GAP - ph, along, ph]
    } else {
        [clamp_x(x + w / 2.0 - PANEL_W / 2.0, PANEL_W), clamp_y(y + h - ph - BESIDE_GAP), PANEL_W, ph]
    }
}

/// The panel's rect at rest at the bottom of a `screen` of that size, over a Dock `dock` points
/// tall.
pub fn bottom_panel(screen: [f32; 2], dock: f32) -> [f32; 4] {
    [(screen[0] - PANEL_W) / 2.0, screen[1] - dock.max(0.0) - MARGIN - PANEL_H, PANEL_W, PANEL_H]
}

/// The field's hint: what it's for, given `shots` attached, or that the capture's tools have
/// the keys.
pub fn placeholder(shots: usize, editing: bool) -> &'static str {
    match (editing, shots) {
        (true, _) => "Editing the screenshot · ↵ Done · Esc",
        (false, 0) => "Ask anything, or drag to capture…",
        (false, 1) => "Ask about this screenshot…",
        (false, _) => "Ask about these screenshots…",
    }
}

/// Whether `n` chips show as icons only.
pub fn compact(n: usize) -> bool {
    n > LABELLED_CHIPS
}

/// A capture's chip: "Image" with an X on hover, or (`compact`, many of them) just the icon,
/// which the X replaces on hover.
pub fn chip<S: 'static>(id: u64, compact: bool, t: &tok::Theme, remove: impl Fn(&mut S) + 'static) -> El {
    let close = el()
        .key(format!("shot-x-{id}"))
        .size(20.0, 20.0)
        .radius(10.0)
        .center()
        .hover(Color::WHITE.alpha(0.14))
        .cursor(Cursor::Pointer)
        .group_opacity(0.0, 1.0)
        .label("Remove image")
        .child(icon(&glyphs::CLOSE, 11.0).color(t.text))
        .on_press(remove);
    let base = |e: El| {
        e.key(format!("shot-{id}"))
            .hover_group()
            .h(24.0)
            .radius(6.0)
            .continuous_corners()
            .fill(Color::WHITE.alpha(0.09))
            .hover(Color::WHITE.alpha(0.05))
            .transition(Transition::scale(0.9).combined(Transition::opacity()))
    };
    if compact {
        return base(el())
            .w(24.0)
            .child(el().size(24.0, 24.0).center().group_opacity(1.0, 0.0).child(icon(&icons::IMAGE, 14.0).color(t.text)).pin(Some(0.0), Some(0.0), None, None))
            .child(el().size(24.0, 24.0).center().child(close).pin(Some(0.0), Some(0.0), None, None));
    }
    base(row())
        .pl(7.0)
        .pr(2.0)
        .gap(5.0)
        .center_y()
        .child(icon(&icons::IMAGE, 14.0).color(t.text.alpha(0.85)))
        .child(text("Image").font(TextStyle::new(11.5, 450.0)).color(t.text.alpha(0.85)))
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
    let motion = Anim::from(if s.beside { SETTLE } else { GLIDE });
    let [x, y, w, h] = s.rect;
    // Just sent: the row is in the chat's panel, fading; the input's new panel waits empty, out
    // of view, landing at once (its element was the chat's, maybe mid-slide).
    let layers = if s.morphing { Vec::new() } else { vec![input_row(s, &t)] };
    let mut input = surface(s.input_key(), s.open, layers)
        .size(w, h)
        .pin(Some(x), Some(y), None, None)
        .offset(0.0, dy)
        .offset_motion(motion.clone())
        .opacity(alpha)
        .opacity_motion(motion.clone())
        .scale(scale)
        .scale_motion(motion)
        .motion(if s.beside { SETTLE } else { GLIDE });
    if s.morphing {
        input = input.instant_changes();
    }
    let chat = chat::panel(&s.chat, s.chat_key(), |s: &mut QuickInput| &mut s.chat, s.morphing.then(|| input_row(s, &t)));
    el().w_full().h_full().child(chat).child(input)
}

/// The field row (the sparkle, the chips, the field, the send button) at the panel's size,
/// pinned to its top left.
fn input_row(s: &QuickInput, t: &tok::Theme) -> El {
    let field = text_input(&s.query, TextStyle::new(15.0, 400.0).lh(22.0))
        .key("query")
        .single_line()
        .grow(1.0)
        .min_w(FIELD_MIN_W)
        .color(t.text)
        .caret(t.text, ACCENT.alpha(0.35))
        .placeholder(placeholder(s.shots.len(), s.editing), t.text_secondary.alpha(0.8))
        .opacity(if s.editing { 0.6 } else { 1.0 })
        .animate_layout()
        .on_edit(|s: &mut QuickInput| &mut s.query)
        .on_input_key(|s: &mut QuickInput, ev: &KeyEvent, _: &mut Tx| s.field_key(ev))
        .on_submit(|s: &mut QuickInput, _: &mut Tx| s.submit());
    let compact = compact(s.shots.len());
    let mut chips = row().key("chips").gap(6.0).center_y();
    for shot in &s.shots {
        let id = shot.id;
        chips = chips.child(chip(id, compact, t, move |s: &mut QuickInput| s.unattach(id)));
    }
    let mut field_row = row()
        .key("field-row")
        .role("textbox")
        .w_full()
        .h_full()
        .px(ROW_PAD)
        .gap(8.0)
        .center_y()
        .child(icon(&glyphs::SPARKLE, 18.0).color(ACCENT).mr(2.0));
    if !s.shots.is_empty() {
        field_row = field_row.child(chips);
    }
    let field_row = field_row.child(field).child(send_button(s.send_ready(), t).ml(2.0));
    el().key("input-layer").size(s.rect[2], PANEL_H).pin(Some(0.0), Some(0.0), None, None).child(field_row)
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
        assert_eq!(pose(false, false, false), Pose { dy: TUCK, alpha: 1.0, scale: 1.0 });
        assert!(TUCK >= PANEL_H + MARGIN + PAD, "fully below the bottom edge, shadow included");
    }

    #[test]
    fn opening_rises_to_rest_opaque_with_the_field_focused() {
        let mut s = QuickInput::new(None);
        s.open();
        assert!(s.is_open());
        assert!(s.query.focused, "the field takes the keys");
        assert_eq!(pose(true, false, false), Pose { dy: 0.0, alpha: 1.0, scale: 1.0 });
        assert_eq!(pose(true, false, true), Pose { dy: 0.0, alpha: 1.0, scale: 1.0 });
    }

    #[test]
    fn beside_an_area_it_arrives_and_leaves_in_place() {
        let arriving = Pose { dy: ARRIVE, alpha: 0.0, scale: ARRIVE_SCALE };
        assert_eq!(pose(true, true, true), arriving, "tucked for a drag, or about to arrive");
        assert_eq!(pose(false, false, true), arriving, "closing");
        assert!(ARRIVE < PAD, "inside the window: no hard edge mid-screen");
        let mut s = QuickInput::new(None);
        s.open();
        s.arrive(7);
        assert!(s.tucked && s.beside);
        s.query.focused = false;
        s.arrived(7);
        assert_eq!(pose(s.open, s.tucked, s.beside).dy, 0.0, "risen");
        assert!(s.query.focused, "ready to type at once");
        s.arrive(8);
        s.set_tucked(true);
        s.arrived(8);
        assert!(s.tucked, "a drag started since: it stays out of view");
        assert_eq!(s.escape(), Escape::Area(8));
        s.close();
        s.at_bottom(0.0);
        assert_eq!(pose(s.open, s.tucked, s.beside).dy, TUCK, "the next opening slides in from the bottom again");
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
        s.attach(1, vec![1]);
        assert!(s.send_ready());
    }

    #[test]
    fn captures_attach_and_go_with_the_submission() {
        let mut s = QuickInput::new(None);
        s.open();
        s.attach(1, vec![1]);
        s.attach(2, vec![2]);
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
        s.attach(1, vec![1]);
        s.attach(2, vec![2]);
        s.attach(1, vec![9]);
        assert_eq!(s.ids(), vec![1, 2], "no second chip, same place");
        assert_eq!(s.shots[0].png, vec![9], "with the marked-up image");
    }

    #[test]
    fn a_chips_x_removes_only_that_capture() {
        let mut s = QuickInput::new(None);
        s.attach(1, vec![1]);
        s.attach(2, vec![2]);
        assert!(s.remove(1));
        assert_eq!(s.ids(), vec![2]);
        assert!(!s.remove(1), "already gone");
    }

    #[test]
    fn backspace_in_an_empty_field_removes_the_last_capture() {
        let mut s = QuickInput::new(None);
        assert_eq!(s.backspace_empty(), None, "nothing to remove");
        s.attach(1, vec![1]);
        s.attach(2, vec![2]);
        s.set_area(Some(2));
        assert_eq!(s.backspace_empty(), Some(2));
        assert_eq!(s.ids(), vec![1]);
        assert_eq!(s.escape(), Escape::Close, "its area went with it");
        assert_eq!(s.backspace_empty(), Some(1));
        assert!(s.ids().is_empty());
    }

    #[test]
    fn esc_discards_the_area_on_screen_first_then_closes() {
        let mut s = QuickInput::new(None);
        s.open();
        s.attach(1, vec![1]);
        s.attach(2, vec![2]);
        s.set_area(Some(2));
        assert_eq!(s.escape(), Escape::Area(2), "the area on screen goes");
        assert_eq!(s.ids(), vec![1], "with its image only");
        assert!(s.is_open(), "the input stays");
        assert_eq!(s.escape(), Escape::Close, "then the input closes");
    }

    #[test]
    fn closing_discards_the_captures_and_keeps_the_text() {
        let mut s = QuickInput::new(None);
        s.open();
        s.attach(1, vec![1]);
        s.set_area(Some(1));
        s.set_editing(true);
        s.query = TextEdit::new("draft");
        s.close();
        assert!(s.ids().is_empty());
        assert!(!s.editing);
        assert_eq!(s.escape(), Escape::Close, "no area left either");
        assert_eq!(s.query.text, "draft");
    }

    #[test]
    fn a_chips_x_on_the_area_on_screen_forgets_it() {
        let mut s = QuickInput::new(None);
        s.attach(1, vec![1]);
        s.set_area(Some(1));
        s.unattach(1);
        assert_eq!(s.escape(), Escape::Close);
    }

    #[test]
    fn many_chips_go_compact() {
        assert!(!compact(LABELLED_CHIPS));
        assert!(compact(LABELLED_CHIPS + 1));
    }

    #[test]
    fn the_hint_says_what_the_field_is_for() {
        assert_eq!(placeholder(0, false), "Ask anything, or drag to capture…");
        assert_eq!(placeholder(1, false), "Ask about this screenshot…");
        assert_eq!(placeholder(3, false), "Ask about these screenshots…");
        assert_eq!(placeholder(1, true), "Editing the screenshot · ↵ Done · Esc");
    }

    #[test]
    fn a_capture_alone_can_be_submitted() {
        let mut s = QuickInput::new(None);
        s.attach(1, vec![0]);
        let (text, shots) = s.take_submission().unwrap();
        assert_eq!((text.as_str(), shots.len()), ("", 1));
    }

    #[test]
    fn tucking_at_the_bottom_slides_it_out_of_the_screen_and_back() {
        let mut s = QuickInput::new(None);
        s.open();
        s.set_tucked(true);
        assert_eq!(pose(s.open, s.tucked, s.beside), Pose { dy: TUCK, alpha: 0.0, scale: 1.0 }, "below the screen");
        s.set_tucked(false);
        assert_eq!(pose(s.open, s.tucked, s.beside).dy, 0.0);
    }

    #[test]
    fn a_closed_input_stays_closed_when_tucked_and_reopens_untucked() {
        let mut s = QuickInput::new(None);
        s.set_tucked(true);
        assert_eq!(pose(s.open, s.tucked, s.beside).dy, TUCK);
        s.open();
        assert!(!s.tucked);
    }

    const SCREEN: [f32; 2] = [1920.0, 1080.0];

    #[test]
    fn the_input_sits_right_of_the_area_centred_on_it() {
        let [x, y, w, h] = beside([100.0, 200.0, 600.0, 500.0], SCREEN);
        assert_eq!((x, w), (100.0 + 600.0 + BESIDE_GAP, PANEL_W));
        assert_eq!(y + h / 2.0, 200.0 + 250.0);
    }

    #[test]
    fn without_room_on_the_right_it_goes_left_then_below_then_above() {
        let [x, _, w, _] = beside([1300.0, 200.0, 600.0, 500.0], SCREEN);
        assert_eq!(x + w, 1300.0 - BESIDE_GAP, "left of it");
        let [x, y, w, _] = beside([500.0, 100.0, 1000.0, 500.0], SCREEN);
        assert_eq!((x, y, w), (500.0, 600.0 + BESIDE_GAP, PANEL_W), "below, left-aligned, at most the widest");
        let [_, y, _, h] = beside([500.0, 400.0, 1000.0, 640.0], SCREEN);
        assert_eq!(y + h, 400.0 - BESIDE_GAP, "above");
    }

    #[test]
    fn above_or_below_it_takes_the_areas_width() {
        let [x, _, w, _] = beside([400.0, 100.0, 520.0, 500.0], [1200.0, 1000.0]);
        assert_eq!((x, w), (400.0, 520.0), "both edges line up");
        let [_, _, w, _] = beside([500.0, 100.0, 300.0, 500.0], [1200.0, 1000.0]);
        assert_eq!(w, PANEL_MIN_W, "never narrower than this");
    }

    #[test]
    fn the_input_stays_on_screen() {
        // Right of an area at the very top: centred would poke above the screen.
        let [_, y, _, _] = beside([100.0, 0.0, 200.0, 20.0], SCREEN);
        assert_eq!(y, EDGE_GAP);
        // Below a narrow area at the right edge: left-aligned would run off it.
        let [x, _, w, _] = beside([1500.0, 100.0, 410.0, 500.0], [1920.0, 1080.0]);
        assert!(x + w <= 1920.0 - EDGE_GAP, "{x} + {w}");
        // The whole screen: over its bottom.
        let [_, y, _, h] = beside([0.0, 0.0, 1920.0, 1080.0], SCREEN);
        assert!(y + h <= 1080.0 - EDGE_GAP);
    }

    #[test]
    fn opening_rises_from_below_the_screen_even_right_after_closing_beside_an_area() {
        let mut s = QuickInput::new(None);
        s.open();
        s.arrive(1);
        s.arrived(1);
        s.close();
        // Reopened before the closed input went back to the bottom.
        s.at_bottom(0.0);
        assert_eq!(s.current_pose(), Pose { dy: TUCK, alpha: 1.0, scale: 1.0 }, "waits below the screen, opaque");
        s.open();
        assert_eq!(s.current_pose(), Pose { dy: 0.0, alpha: 1.0, scale: 1.0 });
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
            s.attach(id, vec![1, 2, 3]);
        }
        s
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
