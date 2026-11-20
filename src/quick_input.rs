//! The quick input: a floating text field that slides up from the bottom of the display under
//! the pointer, above every other window. Ctrl+Option+Space or the menu opens it; Esc, a click
//! elsewhere or Enter closes it, sliding back down. Submitting has no action yet: it logs.
//!
//! While it's open, captures land in it as "Image" chips in the field row (hover one for its X),
//! dragging out a capture area tucks it off the screen, and once an area is picked it rises again
//! beside that area, so the prompt reads as being about it.

use std::time::Duration;

use rune::tok::{self, type_};
use rune::{Anim, AppHandle, Color, Cx, El, Key, KeyEvent, Spring, TextEdit, Tx, col, el, row, spacer, text, text_input};

use crate::Host;

/// The fixed global chord that toggles the panel (Option+Space alone is Claude's quick entry).
pub const SHORTCUT: &str = "ctrl+alt+space";
/// The panel's size and inner padding, in points.
pub const PANEL_W: f32 = 720.0;
const INNER: f32 = 10.0;
const ROW_H: f32 = 44.0;
pub const PANEL_H: f32 = ROW_H + 2.0 * INNER;
/// Room around the panel for its shadow.
pub const PAD: f32 = 24.0;
/// The panel's gap to the bottom of the screen. The window itself reaches the bottom edge, so a
/// tucked panel slides out of the screen rather than out of the window above it.
pub const MARGIN: f32 = 40.0;
/// How far a closed or tucked panel drops: below the screen's bottom edge, shadow included. It
/// opens by rising this far, from outside the screen.
pub const TUCK: f32 = PANEL_H + MARGIN + PAD;
/// The overlay's own size: the panel, its shadow room above, the gap below. The panel sits
/// `PAD` points in from the window's top-left.
pub const WINDOW: (f32, f32) = (PANEL_W + 2.0 * PAD, PAD + PANEL_H + MARGIN);
/// The panel's gap to a capture area it sits beside, and to the screen's edges.
const BESIDE_GAP: f32 = 24.0;
const EDGE_GAP: f32 = 8.0;
/// How long the window takes to fade out once closed: long enough for the slide down to play.
pub const EXIT: Duration = Duration::from_millis(240);
/// The slide's spring: smooth, no overshoot.
const MOTION: Spring = tok::motion::GLIDE;

const ACCENT: Color = Color { r: 0.85, g: 0.47, b: 0.34, a: 1.0 };

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
    /// Out of the screen while a capture area is dragged.
    tucked: bool,
    shots: Vec<Attached>,
    /// The capture area on screen that the input sits beside, while there is one.
    area: Option<u64>,
    /// Where closing goes, so the host hides the window after the slide (None in tests).
    host: Option<AppHandle<Host>>,
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
const FIELD_MIN_W: f32 = 200.0;

impl QuickInput {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        QuickInput { query: TextEdit::new(""), open: false, tucked: false, shots: Vec::new(), area: None, host }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Rises into place with the field focused.
    pub fn open(&mut self) {
        self.open = true;
        self.tucked = false;
        self.query.focused = true;
    }

    /// Slides back down, discarding its captures; the text stays for next time. True if it was
    /// open.
    pub fn close(&mut self) -> bool {
        self.tucked = false;
        self.shots.clear();
        self.area = None;
        std::mem::replace(&mut self.open, false)
    }

    /// Area `id` is on screen and the input beside it (None: no area any more).
    pub fn set_area(&mut self, id: Option<u64>) {
        self.area = id;
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

    /// Slides out of the screen (true) or back into place (false).
    pub fn set_tucked(&mut self, tucked: bool) {
        self.tucked = tucked;
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

    /// The typed text, trimmed, and the captures that go with it; the field and the attachments
    /// clear. Nothing typed and nothing attached submits nothing.
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

    fn submit(&mut self) {
        if let Some((text, shots)) = self.take_submission() {
            let bytes: usize = shots.iter().map(|a| a.png.len()).sum();
            eprintln!("Capibarapp: quick input: {text:?} with {} capture(s), {bytes} bytes of PNG", shots.len());
            self.dismiss();
        }
    }

    /// A chip's X: the capture goes, and so does its area if it's still on screen.
    fn unattach(&mut self, id: u64) {
        if self.area == Some(id) {
            self.area = None;
        }
        if self.remove(id) && let Some(host) = &self.host {
            host.update(move |host, _| host.forget_capture_area(id));
        }
    }

    /// Esc in the field: see `escape`.
    fn on_escape(&mut self) {
        match self.escape() {
            Escape::Area(id) => {
                if let Some(host) = &self.host {
                    host.update(move |host, _| host.forget_capture_area(id));
                }
            }
            Escape::Close => self.dismiss(),
        }
    }
}

/// The panel's vertical offset from its rest and its opacity: closed it waits below the screen,
/// so opening slides it in from outside; tucked it's out of the screen too and fades as it goes,
/// so rising again somewhere else (beside a capture area) reads as arriving there.
pub fn pose(open: bool, tucked: bool) -> (f32, f32) {
    match (open, tucked) {
        (false, _) => (TUCK, 1.0),
        (true, true) => (TUCK, 0.0),
        (true, false) => (0.0, 1.0),
    }
}

/// Where the overlay window goes so the panel sits beside capture area `sel` (`[x, y, w, h]`,
/// points on a `screen` of that size): right of it, else left, else below, else above, else
/// over its bottom; vertically centred on it beside, left-aligned with it above or below; always
/// on screen. Returns the window's top-left (the panel is `PAD` in from it).
pub fn beside(sel: [f32; 4], screen: [f32; 2]) -> [f32; 2] {
    let ([x, y, w, h], [sw, sh], (pw, ph)) = (sel, screen, (PANEL_W, PANEL_H));
    let clamp_x = |v: f32| v.min(sw - pw - EDGE_GAP).max(EDGE_GAP);
    let clamp_y = |v: f32| v.min(sh - ph - EDGE_GAP).max(EDGE_GAP);
    let middle = clamp_y(y + h / 2.0 - ph / 2.0);
    let [px, py] = if x + w + BESIDE_GAP + pw <= sw - EDGE_GAP {
        [x + w + BESIDE_GAP, middle]
    } else if x - BESIDE_GAP - pw >= EDGE_GAP {
        [x - BESIDE_GAP - pw, middle]
    } else if y + h + BESIDE_GAP + ph <= sh - EDGE_GAP {
        [clamp_x(x), y + h + BESIDE_GAP]
    } else if y - BESIDE_GAP - ph >= EDGE_GAP {
        [clamp_x(x), y - BESIDE_GAP - ph]
    } else {
        [clamp_x(x + w / 2.0 - pw / 2.0), clamp_y(y + h - ph - BESIDE_GAP)]
    };
    [px - PAD, py - PAD]
}

/// Whether `n` chips show as icons only.
fn compact(n: usize) -> bool {
    n > LABELLED_CHIPS
}

/// The panel's own rect (points on the screen) when it sits beside `sel`.
pub fn panel_beside(sel: [f32; 4], screen: [f32; 2]) -> [f32; 4] {
    let [x, y] = beside(sel, screen);
    [x + PAD, y + PAD, PANEL_W, PANEL_H]
}

/// A small picture glyph: a frame with a sun.
fn image_glyph(color: Color) -> El {
    el().size(14.0, 11.0).radius(2.5).border(1.4, color).child(el().size(3.0, 3.0).radius(1.5).fill(color).pin(Some(2.5), Some(2.0), None, None))
}

/// A capture's chip: "Image" with an X on hover, or (`compact`, many of them) just the glyph,
/// which the X replaces on hover.
fn chip(shot: &Attached, compact: bool, t: &tok::Theme) -> El {
    let id = shot.id;
    let close = el()
        .key(format!("shot-x-{id}"))
        .size(16.0, 16.0)
        .radius(8.0)
        .center()
        .fill(Color::WHITE.alpha(0.18))
        .hover(Color::WHITE.alpha(0.12))
        .group_opacity(0.0, 1.0)
        .child(text("×").font(type_::LABEL.lh(14.0)).color(t.text))
        .on_press(move |s: &mut QuickInput| s.unattach(id));
    if compact {
        return el()
            .key(format!("shot-{id}"))
            .hover_group()
            .size(28.0, 28.0)
            .radius(8.0)
            .fill(Color::WHITE.alpha(0.08))
            .child(el().size(28.0, 28.0).center().group_opacity(1.0, 0.0).child(image_glyph(t.text)).pin(Some(0.0), Some(0.0), None, None))
            .child(el().size(28.0, 28.0).center().child(close).pin(Some(0.0), Some(0.0), None, None));
    }
    row()
        .key(format!("shot-{id}"))
        .hover_group()
        .h(28.0)
        .pl(10.0)
        .pr(6.0)
        .gap(6.0)
        .radius(8.0)
        .center_y()
        .fill(Color::WHITE.alpha(0.08))
        .child(image_glyph(t.text))
        .child(text("Image").font(type_::LABEL).color(t.text))
        .child(close)
}

pub fn view(s: &QuickInput, _: &mut Cx<QuickInput>) -> El {
    let t = tok::theme(true);
    let (dy, alpha) = pose(s.open, s.tucked);
    let field = text_input(&s.query, type_::BODY.lh(24.0))
        .key("query")
        .single_line()
        .grow(1.0)
        .min_w(FIELD_MIN_W)
        .color(t.text)
        .placeholder("What can I help you with?", t.text_secondary)
        .on_edit(|s: &mut QuickInput| &mut s.query)
        .on_input_key(|s: &mut QuickInput, ev: &KeyEvent, _: &mut Tx| {
            if ev.key == Key::Escape {
                s.on_escape();
                return true;
            }
            false
        })
        .on_submit(|s: &mut QuickInput, _: &mut Tx| s.submit());
    let ready = !s.query.text.trim().is_empty() || !s.shots.is_empty();
    let send = el()
        .key("send")
        .size(32.0, 32.0)
        .radius(8.0)
        .center()
        .fill(if ready { ACCENT } else { ACCENT.alpha(0.45) })
        .child(text("↑").font(type_::TITLE).color(Color::WHITE))
        .on_press(|s: &mut QuickInput| s.submit());
    let mut field_row = row()
        .key("field-row")
        .w_full()
        .h(ROW_H)
        .px(6.0)
        .gap(10.0)
        .center_y()
        .child(text("✳").font(type_::TITLE.lh(24.0)).color(ACCENT).mr(4.0));
    let compact = compact(s.shots.len());
    for shot in &s.shots {
        field_row = field_row.child(chip(shot, compact, &t));
    }
    let field_row = field_row.child(field).child(send);
    let panel = col()
        .key("panel")
        .hit()
        .w_full()
        .h(PANEL_H)
        .px(INNER)
        .center_y()
        .radius(16.0)
        .fill(t.card)
        .border(1.0, Color::WHITE.alpha(0.08))
        .shadow(10.0, 28.0, Color::BLACK.alpha(0.45))
        .offset(0.0, dy)
        .offset_motion(Anim::from(MOTION))
        .opacity(alpha)
        .opacity_motion(Anim::from(MOTION))
        .motion(MOTION)
        .child(field_row);
    col().w_full().h_full().px(PAD).pt(PAD).child(panel).child(spacer())
}

pub fn on_key(s: &mut QuickInput, event: &KeyEvent, _: &mut Tx) -> bool {
    if event.key == Key::Escape {
        s.on_escape();
        return true;
    }
    false
}

/// Clicking elsewhere closes it, unless a capture took the keys (the host decides).
pub fn on_focus(s: &mut QuickInput, focused: bool, _: &mut Tx) {
    if !focused && s.is_open() && let Some(host) = &s.host {
        host.update(|host, _| host.quick_input_blurred());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed_below_the_screen_so_it_slides_in_from_outside() {
        let s = QuickInput::new(None);
        assert!(!s.is_open());
        assert_eq!(pose(false, false), (TUCK, 1.0));
        assert!(TUCK >= PANEL_H + MARGIN + PAD, "fully below the bottom edge, shadow included");
    }

    #[test]
    fn opening_rises_to_rest_opaque_with_the_field_focused() {
        let mut s = QuickInput::new(None);
        s.open();
        assert!(s.is_open());
        assert!(s.query.focused, "the field takes the keys");
        assert_eq!(pose(true, false), (0.0, 1.0));
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
        s.query = TextEdit::new("draft");
        s.close();
        assert!(s.ids().is_empty());
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
    fn a_capture_alone_can_be_submitted() {
        let mut s = QuickInput::new(None);
        s.attach(1, vec![0]);
        let (text, shots) = s.take_submission().unwrap();
        assert_eq!((text.as_str(), shots.len()), ("", 1));
    }

    #[test]
    fn tucking_slides_it_out_of_the_screen_and_back() {
        let mut s = QuickInput::new(None);
        s.open();
        s.set_tucked(true);
        assert_eq!(pose(s.open, s.tucked), (TUCK, 0.0), "below the screen");
        assert!(TUCK >= PANEL_H + MARGIN, "clears the bottom edge");
        s.set_tucked(false);
        assert_eq!(pose(s.open, s.tucked), (0.0, 1.0));
    }

    #[test]
    fn a_closed_input_stays_closed_when_tucked_and_reopens_untucked() {
        let mut s = QuickInput::new(None);
        s.set_tucked(true);
        assert_eq!(pose(s.open, s.tucked), (TUCK, 1.0));
        s.open();
        assert!(!s.tucked);
    }

    /// The panel's own rect for a window at `at`.
    fn panel_at(at: [f32; 2]) -> [f32; 4] {
        [at[0] + PAD, at[1] + PAD, PANEL_W, PANEL_H]
    }

    #[test]
    fn the_input_sits_right_of_the_area_centred_on_it() {
        let screen = [1920.0, 1080.0];
        let [x, y, _, h] = panel_at(beside([100.0, 200.0, 600.0, 500.0], screen));
        assert_eq!(x, 100.0 + 600.0 + BESIDE_GAP);
        assert_eq!(y + h / 2.0, 200.0 + 250.0);
    }

    #[test]
    fn without_room_on_the_right_it_goes_left_then_below() {
        let screen = [1920.0, 1080.0];
        let [x, _, w, _] = panel_at(beside([1100.0, 200.0, 600.0, 500.0], screen));
        assert_eq!(x + w, 1100.0 - BESIDE_GAP, "left of it");
        let [x, y, _, _] = panel_at(beside([500.0, 100.0, 900.0, 500.0], screen));
        assert_eq!((x, y), (500.0, 600.0 + BESIDE_GAP), "below, left-aligned");
        let [_, y, _, h] = panel_at(beside([500.0, 400.0, 900.0, 640.0], screen));
        assert_eq!(y + h, 400.0 - BESIDE_GAP, "above");
    }

    #[test]
    fn the_input_stays_on_screen() {
        let screen = [1920.0, 1080.0];
        // Right of an area at the very top: centred would poke above the screen.
        let [_, y, _, _] = panel_at(beside([100.0, 0.0, 200.0, 20.0], screen));
        assert_eq!(y, EDGE_GAP);
        // Below an area near the right edge: left-aligned would run off it.
        let [x, _, w, _] = panel_at(beside([1300.0, 100.0, 600.0, 500.0], screen));
        assert!(x + w <= 1920.0 - EDGE_GAP);
        // The whole screen: over its bottom.
        let [_, y, _, h] = panel_at(beside([0.0, 0.0, 1920.0, 1080.0], screen));
        assert!(y + h <= 1080.0 - EDGE_GAP);
    }

    #[test]
    fn the_overlay_has_room_for_the_panel() {
        assert!(WINDOW.1 >= PAD + PANEL_H + MARGIN);
        assert!(WINDOW.0 >= PANEL_W + 2.0 * PAD);
    }

    #[test]
    fn the_shortcut_parses() {
        assert!(SHORTCUT.parse::<rune::Shortcut>().is_ok());
    }
}
