//! The quick input: a floating text field that slides up from the bottom of the display under
//! the pointer, above every other window. Ctrl+Option+Space or the menu opens it; Esc, a click
//! elsewhere or Enter closes it, sliding back down. Submitting has no action yet: it logs.
//!
//! While it's open, captures land in it as thumbnails above the field, and dragging out a
//! capture area tucks it off the bottom of the screen until the drag ends.

use std::time::Duration;

use rune::tok::{self, type_};
use rune::{Anim, AppHandle, Color, Cx, El, Key, KeyEvent, Pixels, Spring, SurfaceFit, TextEdit, Tx, col, el, row, spacer, text, text_input};

use crate::Host;

/// The fixed global chord that toggles the panel (Option+Space alone is Claude's quick entry).
pub const SHORTCUT: &str = "ctrl+alt+space";
/// The panel's width and inner padding, in points.
pub const PANEL_W: f32 = 720.0;
const INNER: f32 = 12.0;
/// The field row's height.
const ROW_H: f32 = 44.0;
/// An attached capture's thumbnail, and the gap below the thumbnails.
pub const THUMB: (f32, f32) = (132.0, 92.0);
const THUMB_GAP: f32 = 10.0;
/// The panel's height with thumbnails: the tallest it gets.
pub const PANEL_MAX_H: f32 = INNER + THUMB.1 + THUMB_GAP + ROW_H + INNER;
/// How far below its rest the panel waits while closed; it rises this far as it opens.
pub const SLIDE: f32 = 56.0;
/// Room around the panel for its shadow.
pub const PAD: f32 = 24.0;
/// The panel's gap to the bottom of the screen. The window itself reaches the bottom edge, so a
/// tucked panel slides out of the screen rather than out of the window above it.
pub const MARGIN: f32 = 40.0;
/// How far a tucked panel drops: below the screen's bottom edge, shadow included.
pub const TUCK: f32 = PANEL_MAX_H + MARGIN + PAD;
/// The overlay's own size: the tallest panel, its shadow room above, the gap below.
pub const WINDOW: (f32, f32) = (PANEL_W + 2.0 * PAD, PAD + PANEL_MAX_H + MARGIN);
/// How long the window takes to fade out once closed: long enough for the slide down to play.
pub const EXIT: Duration = Duration::from_millis(240);
/// The slide's spring: smooth, no overshoot.
const MOTION: Spring = tok::motion::GLIDE;

const ACCENT: Color = Color { r: 0.85, g: 0.47, b: 0.34, a: 1.0 };

/// A capture attached to the input: its thumbnail.
pub struct Attached {
    pixels: Pixels,
}

pub struct QuickInput {
    query: TextEdit,
    open: bool,
    /// Out of the screen while a capture area is dragged.
    tucked: bool,
    shots: Vec<Attached>,
    /// Where closing goes, so the host hides the window after the slide (None in tests).
    host: Option<AppHandle<Host>>,
}

impl QuickInput {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        QuickInput { query: TextEdit::new(""), open: false, tucked: false, shots: Vec::new(), host }
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

    /// Slides back down; the text and attachments stay for next time. True if it was open.
    pub fn close(&mut self) -> bool {
        self.tucked = false;
        std::mem::replace(&mut self.open, false)
    }

    /// Slides out of the screen (true) or back into place (false).
    pub fn set_tucked(&mut self, tucked: bool) {
        self.tucked = tucked;
    }

    /// Adds a capture's thumbnail (RGBA, `width` x `height` pixels).
    pub fn attach(&mut self, width: u32, height: u32, rgba: Vec<u8>) {
        let pixels = Pixels::new();
        pixels.set(width, height, rgba);
        self.shots.push(Attached { pixels });
    }

    #[cfg(test)]
    pub fn attached(&self) -> usize {
        self.shots.len()
    }

    /// The typed text, trimmed, and how many captures go with it; the field and the attachments
    /// clear. Nothing typed and nothing attached submits nothing.
    pub fn take_submission(&mut self) -> Option<(String, usize)> {
        let text = self.query.text.trim().to_owned();
        if text.is_empty() && self.shots.is_empty() {
            return None;
        }
        self.query.clear();
        let shots = std::mem::take(&mut self.shots).len();
        Some((text, shots))
    }

    /// Asks the host to close the panel (which calls `close` and fades the window out).
    fn dismiss(&mut self) {
        if let Some(host) = &self.host {
            host.update(|host, _| host.hide_quick_input());
        }
    }

    fn submit(&mut self) {
        if let Some((text, shots)) = self.take_submission() {
            eprintln!("Capibarapp: quick input: {text:?} with {shots} capture(s)");
            self.dismiss();
        }
    }
}

/// The panel's vertical offset from its rest and its opacity: closed it waits a little below,
/// transparent; tucked it's out of the screen, still opaque, so it reads as sliding away.
pub fn pose(open: bool, tucked: bool) -> (f32, f32) {
    match (open, tucked) {
        (false, _) => (SLIDE, 0.0),
        (true, true) => (TUCK, 1.0),
        (true, false) => (0.0, 1.0),
    }
}

/// Shrinks `rgba` (`width` x `height`) to fit within `max` pixels, keeping its aspect, by
/// averaging the source pixels each target pixel covers. Never enlarges.
pub fn thumbnail(rgba: &[u8], width: u32, height: u32, max: (u32, u32)) -> (u32, u32, Vec<u8>) {
    let k = (max.0 as f32 / width.max(1) as f32).min(max.1 as f32 / height.max(1) as f32).min(1.0);
    let (tw, th) = (((width as f32 * k).round() as u32).max(1), ((height as f32 * k).round() as u32).max(1));
    if (tw, th) == (width, height) {
        return (width, height, rgba.to_vec());
    }
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        let (y0, y1) = (ty * height / th, ((ty + 1) * height / th).max(ty * height / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * width / tw, ((tx + 1) * width / tw).max(tx * width / tw + 1));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * width + x) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += u32::from(rgba[i + c]);
                    }
                }
            }
            let n = (x1 - x0) * (y1 - y0);
            out.extend(sum.iter().map(|s| (s / n) as u8));
        }
    }
    (tw, th, out)
}

pub fn view(s: &QuickInput, _: &mut Cx<QuickInput>) -> El {
    let t = tok::theme(true);
    let (dy, alpha) = pose(s.open, s.tucked);
    let field = text_input(&s.query, type_::BODY.lh(24.0))
        .key("query")
        .single_line()
        .grow(1.0)
        .color(t.text)
        .placeholder("What can I help you with?", t.text_secondary)
        .on_edit(|s: &mut QuickInput| &mut s.query)
        .on_input_key(|s: &mut QuickInput, ev: &KeyEvent, _: &mut Tx| {
            if ev.key == Key::Escape {
                s.dismiss();
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
    let field_row = row()
        .key("field-row")
        .w_full()
        .h(ROW_H)
        .px(6.0)
        .gap(14.0)
        .center_y()
        .child(text("✳").font(type_::TITLE.lh(24.0)).color(ACCENT))
        .child(field)
        .child(send);
    let mut panel = col()
        .key("panel")
        .hit()
        .w_full()
        .padding(INNER)
        .radius(16.0)
        .fill(t.card)
        .border(1.0, Color::WHITE.alpha(0.08))
        .shadow(10.0, 28.0, Color::BLACK.alpha(0.45))
        .offset(0.0, dy)
        .offset_motion(Anim::from(MOTION))
        .opacity(alpha)
        .opacity_motion(Anim::from(MOTION))
        .motion(MOTION);
    if !s.shots.is_empty() {
        let mut thumbs = row().key("thumbs").w_full().h(THUMB.1).gap(10.0).mb(THUMB_GAP);
        for (i, shot) in s.shots.iter().enumerate() {
            thumbs = thumbs.child(
                el().key(format!("thumb-{i}"))
                    .size(THUMB.0, THUMB.1)
                    .radius(10.0)
                    .clip_rounded(10.0)
                    .fill(Color::BLACK.alpha(0.35))
                    .border(1.0, Color::WHITE.alpha(0.12))
                    .pixels_fit(&shot.pixels, SurfaceFit::Fit),
            );
        }
        panel = panel.child(thumbs);
    }
    panel = panel.child(field_row);
    // Bottom-anchored: thumbnails grow the panel upwards.
    col().w_full().h_full().px(PAD).pt(PAD).child(spacer()).child(panel).child(el().h(MARGIN))
}

pub fn on_key(s: &mut QuickInput, event: &KeyEvent, _: &mut Tx) -> bool {
    if event.key == Key::Escape {
        s.dismiss();
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
    fn starts_closed_below_its_rest_and_transparent() {
        let s = QuickInput::new(None);
        assert!(!s.is_open());
        assert_eq!(pose(false, false), (SLIDE, 0.0));
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
        assert_eq!(s.take_submission(), Some(("hello there".into(), 0)));
        assert_eq!(s.query.text, "");
    }

    #[test]
    fn submitting_nothing_does_nothing() {
        let mut s = QuickInput::new(None);
        s.query = TextEdit::new("   ");
        assert_eq!(s.take_submission(), None);
        assert_eq!(s.query.text, "   ");
    }

    #[test]
    fn captures_attach_and_go_with_the_submission() {
        let mut s = QuickInput::new(None);
        s.open();
        s.attach(2, 1, vec![255; 8]);
        s.attach(1, 1, vec![0; 4]);
        assert_eq!(s.attached(), 2);
        s.query = TextEdit::new("what is this?");
        assert_eq!(s.take_submission(), Some(("what is this?".into(), 2)));
        assert_eq!(s.attached(), 0, "attachments clear on submit");
    }

    #[test]
    fn a_capture_alone_can_be_submitted() {
        let mut s = QuickInput::new(None);
        s.attach(1, 1, vec![0; 4]);
        assert_eq!(s.take_submission(), Some((String::new(), 1)));
    }

    #[test]
    fn tucking_slides_it_out_of_the_screen_and_back() {
        let mut s = QuickInput::new(None);
        s.open();
        s.set_tucked(true);
        assert_eq!(pose(s.open, s.tucked), (TUCK, 1.0), "below the screen, still opaque");
        assert!(TUCK >= PANEL_MAX_H + MARGIN, "clears the bottom edge");
        s.set_tucked(false);
        assert_eq!(pose(s.open, s.tucked), (0.0, 1.0));
    }

    #[test]
    fn a_closed_input_stays_closed_when_tucked_and_reopens_untucked() {
        let mut s = QuickInput::new(None);
        s.set_tucked(true);
        assert_eq!(pose(s.open, s.tucked), (SLIDE, 0.0));
        s.open();
        assert!(!s.tucked);
    }

    #[test]
    fn thumbnails_fit_the_box_keeping_aspect_and_average() {
        // 4x2: left half white, right half black.
        let mut rgba = Vec::new();
        for _ in 0..2 {
            for x in 0..4 {
                let v = if x < 2 { 255 } else { 0 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let (w, h, out) = thumbnail(&rgba, 4, 2, (2, 2));
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, vec![255, 255, 255, 255, 0, 0, 0, 255]);
        let (w, h, _) = thumbnail(&rgba, 4, 2, (100, 100));
        assert_eq!((w, h), (4, 2), "never enlarges");
    }

    #[test]
    fn the_overlay_has_room_for_the_tallest_panel() {
        assert!(WINDOW.1 >= PAD + PANEL_MAX_H + MARGIN);
        assert!(WINDOW.0 > PANEL_W);
    }

    #[test]
    fn the_shortcut_parses() {
        assert!(SHORTCUT.parse::<rune::Shortcut>().is_ok());
    }
}
