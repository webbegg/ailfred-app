//! The quick input: a floating text field that slides up from the bottom of the display under
//! the pointer, above every other window. Ctrl+Option+Space or the menu opens it; Esc, a click
//! elsewhere or Enter closes it, sliding back down. Submitting has no action yet: it logs.

use std::time::Duration;

use rune::tok::{self, type_};
use rune::{Anim, AppHandle, Color, Cx, El, Key, KeyEvent, Spring, TextEdit, Tx, el, row, text, text_input};

use crate::Host;

/// The fixed global chord that toggles the panel (Option+Space alone is Claude's quick entry).
pub const SHORTCUT: &str = "ctrl+alt+space";
/// The panel's resting size, in points.
pub const PANEL_W: f32 = 720.0;
pub const PANEL_H: f32 = 64.0;
/// How far below its rest the panel waits while closed; it rises this far as it opens.
pub const SLIDE: f32 = 56.0;
/// Room around the panel for its shadow.
pub const PAD: f32 = 24.0;
/// The overlay's own size: the panel, its shadow room, and the path it slides along.
pub const WINDOW: (f32, f32) = (PANEL_W + 2.0 * PAD, PANEL_H + 2.0 * PAD + SLIDE);
/// The overlay's gap to the bottom of the display's visible area.
pub const MARGIN: f32 = 40.0;
/// How long the window takes to fade out once closed: long enough for the slide down to play.
pub const EXIT: Duration = Duration::from_millis(240);
/// The slide's spring: smooth, no overshoot.
const MOTION: Spring = tok::motion::GLIDE;

const ACCENT: Color = Color { r: 0.85, g: 0.47, b: 0.34, a: 1.0 };

pub struct QuickInput {
    query: TextEdit,
    open: bool,
    /// Where closing goes, so the host hides the window after the slide (None in tests).
    host: Option<AppHandle<Host>>,
}

impl QuickInput {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        QuickInput { query: TextEdit::new(""), open: false, host }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Rises into place with the field focused.
    pub fn open(&mut self) {
        self.open = true;
        self.query.focused = true;
    }

    /// Slides back down; the text stays for next time. True if it was open.
    pub fn close(&mut self) -> bool {
        std::mem::replace(&mut self.open, false)
    }

    /// The typed text, trimmed, if there is any; the field clears. Empty submits nothing.
    pub fn take_submission(&mut self) -> Option<String> {
        let text = self.query.text.trim();
        if text.is_empty() {
            return None;
        }
        let text = text.to_owned();
        self.query.clear();
        Some(text)
    }

    /// Asks the host to close the panel (which calls `close` and fades the window out).
    fn dismiss(&mut self) {
        if let Some(host) = &self.host {
            host.update(|host, _| host.hide_quick_input());
        }
    }

    fn submit(&mut self) {
        if let Some(text) = self.take_submission() {
            eprintln!("Capibarapp: quick input: {text}");
            self.dismiss();
        }
    }
}

/// The panel's vertical offset from its rest and its opacity.
pub fn pose(open: bool) -> (f32, f32) {
    if open { (0.0, 1.0) } else { (SLIDE, 0.0) }
}

pub fn view(s: &QuickInput, _: &mut Cx<QuickInput>) -> El {
    let t = tok::theme(true);
    let (dy, alpha) = pose(s.open);
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
    let has_text = !s.query.text.trim().is_empty();
    let send = el()
        .key("send")
        .size(32.0, 32.0)
        .radius(8.0)
        .center()
        .fill(if has_text { ACCENT } else { ACCENT.alpha(0.45) })
        .child(text("↑").font(type_::TITLE).color(Color::WHITE))
        .on_press(|s: &mut QuickInput| s.submit());
    let panel = row()
        .key("panel")
        .hit()
        .w_full()
        .h(PANEL_H)
        .px(18.0)
        .gap(14.0)
        .center_y()
        .radius(16.0)
        .fill(t.card)
        .border(1.0, Color::WHITE.alpha(0.08))
        .shadow(10.0, 28.0, Color::BLACK.alpha(0.45))
        .offset(0.0, dy)
        .offset_motion(Anim::from(MOTION))
        .opacity(alpha)
        .opacity_motion(Anim::from(MOTION))
        .child(text("✳").font(type_::TITLE.lh(24.0)).color(ACCENT))
        .child(field)
        .child(send);
    el().w_full().h_full().padding(PAD).child(panel)
}

pub fn on_key(s: &mut QuickInput, event: &KeyEvent, _: &mut Tx) -> bool {
    if event.key == Key::Escape {
        s.dismiss();
        return true;
    }
    false
}

/// Clicking elsewhere closes it.
pub fn on_focus(s: &mut QuickInput, focused: bool, _: &mut Tx) {
    if !focused && s.is_open() {
        s.dismiss();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed_below_its_rest_and_transparent() {
        let s = QuickInput::new(None);
        assert!(!s.is_open());
        assert_eq!(pose(false), (SLIDE, 0.0));
    }

    #[test]
    fn opening_rises_to_rest_opaque_with_the_field_focused() {
        let mut s = QuickInput::new(None);
        s.open();
        assert!(s.is_open());
        assert!(s.query.focused, "the field takes the keys");
        assert_eq!(pose(true), (0.0, 1.0));
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
        assert_eq!(s.take_submission().as_deref(), Some("hello there"));
        assert_eq!(s.query.text, "");
    }

    #[test]
    fn submitting_blank_text_does_nothing() {
        let mut s = QuickInput::new(None);
        s.query = TextEdit::new("   ");
        assert_eq!(s.take_submission(), None);
        assert_eq!(s.query.text, "   ");
    }

    #[test]
    fn the_overlay_has_room_for_the_slide_and_shadow() {
        assert!(WINDOW.1 >= PANEL_H + SLIDE);
        assert!(WINDOW.0 > PANEL_W);
    }

    #[test]
    fn the_shortcut_parses() {
        assert!(SHORTCUT.parse::<rune::Shortcut>().is_ok());
    }
}
