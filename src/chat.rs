//! The chat side panel: sending from the quick input slides it in from the right edge of the
//! display, with what was sent as the first message and its own field focused. It stays open
//! when another app is clicked; Esc or its close button slide it back out, and the input's
//! shortcut brings it back. Cmd+N slides it out for good and opens a new input. No AI: it shows
//! the conversation typed into it.

use rune::tok;
use rune::{
    Anim, AppHandle, Color, Cursor, Cx, El, GrowthSpec, Key, KeyEvent, Pixels, ScrollSpec, Shadow, Spring, SystemMaterial, TextEdit,
    TextStyle, Transition, Tx, auto_grow, col, el, icon, row, text, text_input,
};
use rune_kit::glyphs;

use crate::Host;
use crate::icons;
use crate::quick_input::ACCENT;

/// The panel's width, its gap to the screen's edges, and its corner radius, in points.
pub const CHAT_W: f32 = 400.0;
const GAP: f32 = 12.0;
const RADIUS: f32 = 16.0;
/// Room left of the panel for its shadow.
const PAD: f32 = 32.0;
const HEADER_H: f32 = 44.0;
/// A message's widest, and its images' largest.
const BUBBLE_MAX_W: f32 = CHAT_W * 0.82;
const THUMB_MAX: (f32, f32) = (240.0, 160.0);
/// The slide in from the edge and back.
const GLIDE: Spring = tok::motion::GLIDE;
/// The panel's frost over the system material, #1E1E1E (as the quick input's).
const FROST: Color = Color { r: 0.013, g: 0.013, b: 0.013, a: 0.82 };

/// What the menu bar and the Dock take at the display's edges (points).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub bottom: f32,
    pub right: f32,
}

/// The menu bar's and Dock's room on a display: `visible` within `frame` (AppKit rects,
/// `[x, y, w, h]` with y up).
pub fn insets(frame: [f32; 4], visible: [f32; 4]) -> Insets {
    let ([fx, fy, fw, fh], [vx, vy, vw, vh]) = (frame, visible);
    Insets { top: (fy + fh - (vy + vh)).max(0.0), bottom: (vy - fy).max(0.0), right: (fx + fw - (vx + vw)).max(0.0) }
}

/// The overlay's size on a display `height` points tall: the full height, the panel and its
/// shadow room wide, past a Dock on the right.
pub fn window_size(insets: Insets, height: f32) -> (f32, f32) {
    (PAD + CHAT_W + GAP + insets.right, height)
}

/// The panel's horizontal offset from its rest: open, there; closed, past the screen's right
/// edge, shadow included.
pub fn offset(open: bool, insets: Insets) -> f32 {
    if open { 0.0 } else { CHAT_W + GAP + insets.right + PAD }
}

/// An image `w` × `h` pixels shown within `max` points, keeping its shape, never enlarged.
pub fn thumb_size(w: u32, h: u32, max: (f32, f32)) -> (f32, f32) {
    if w == 0 || h == 0 {
        return (0.0, 0.0);
    }
    let (w, h) = (w as f32, h as f32);
    let k = (max.0 / w).min(max.1 / h).min(1.0);
    ((w * k).round(), (h * k).round())
}

/// An 8-bit RGBA PNG (as the captures are encoded) back to its pixels.
pub fn decode_png(png: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    rgba.truncate(info.buffer_size());
    Some((info.width, info.height, rgba))
}

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
pub fn toggle(quick_open: bool, started: bool, chat_open: bool) -> Toggle {
    match (quick_open, started, chat_open) {
        (true, _, _) => Toggle::HideInput,
        (false, true, false) => Toggle::ShowChat,
        (false, true, true) => Toggle::HideChat,
        (false, false, _) => Toggle::ShowInput,
    }
}

/// A sent capture, ready to draw.
pub struct Image {
    pixels: Pixels,
    width: u32,
    height: u32,
}

impl Image {
    pub fn from_png(png: &[u8]) -> Option<Image> {
        let (width, height, rgba) = decode_png(png)?;
        let pixels = Pixels::new();
        pixels.set(width, height, rgba);
        Some(Image { pixels, width, height })
    }
}

/// One sent message: its text (maybe empty) and its captures.
pub struct Message {
    pub text: String,
    pub images: Vec<Image>,
}

pub struct Chat {
    messages: Vec<Message>,
    draft: TextEdit,
    open: bool,
    insets: Insets,
    /// Cmd+N: the conversation is over; it still shows while sliding out, and the next message
    /// starts a new one.
    over: bool,
    /// Where Esc and the close button go (None in tests).
    host: Option<AppHandle<Host>>,
}

impl Chat {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        Chat { messages: Vec::new(), draft: TextEdit::new(""), open: false, insets: Insets::default(), over: false, host }
    }

    #[cfg(test)]
    fn is_open(&self) -> bool {
        self.open
    }

    /// Before showing, on a display with these `insets`: out of view past its right edge, so
    /// `open` slides in from there. Applied without motion.
    pub fn place(&mut self, insets: Insets) {
        self.insets = insets;
        self.open = false;
    }

    /// Slides in, its field focused.
    pub fn open(&mut self) {
        self.open = true;
        self.draft.focused = true;
    }

    /// Slides out; the conversation and the draft stay. True if it was open.
    pub fn close(&mut self) -> bool {
        std::mem::replace(&mut self.open, false)
    }

    /// Cmd+N: slides out, the conversation over (see `over`).
    pub fn start_over(&mut self) {
        self.open = false;
        self.over = true;
    }

    /// Adds a message at the end of the conversation, the field focused for the next.
    pub fn push(&mut self, message: Message) {
        if std::mem::replace(&mut self.over, false) {
            self.messages.clear();
            self.draft.clear();
        }
        self.messages.push(message);
        self.draft.focused = true;
    }

    /// Sends the draft, trimmed, as a message; nothing typed sends nothing. True if it went.
    pub fn send(&mut self) -> bool {
        let text = self.draft.text.trim().to_owned();
        if text.is_empty() {
            return false;
        }
        self.draft.clear();
        self.push(Message { text, images: Vec::new() });
        true
    }

    fn dismiss(&self) {
        if let Some(host) = &self.host {
            host.update(|host, _| host.hide_chat());
        }
    }

    /// Esc slides the panel out; Cmd+N starts a new chat (`Host::new_chat`).
    fn field_key(&mut self, ev: &KeyEvent) -> bool {
        if ev.key == Key::Escape {
            self.dismiss();
            return true;
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

fn close_button(t: &tok::Theme) -> El {
    el().key("close")
        .size(26.0, 26.0)
        .radius(7.0)
        .continuous_corners()
        .center()
        .hover(Color::WHITE.alpha(0.10))
        .press(Color::WHITE.alpha(0.06))
        .cursor(Cursor::Pointer)
        .label("Close")
        .shortcut_hint("Esc")
        .child(icon(&glyphs::CLOSE, 12.0).color(t.text_secondary))
        .on_press(|s: &mut Chat| s.dismiss())
}

fn send_button(ready: bool, t: &tok::Theme) -> El {
    let button = el().key("send").size(28.0, 28.0).radius(8.0).continuous_corners().center().label("Send").shortcut_hint("↵");
    if ready {
        button
            .fill(ACCENT)
            .hover(Color::WHITE.alpha(0.10))
            .press(Color::BLACK.alpha(0.12))
            .press_scale(0.94)
            .cursor(Cursor::Pointer)
            .child(icon(&icons::ARROW_UP, 16.0).color(Color::WHITE))
            .on_press_tx(|s: &mut Chat, tx: &mut Tx| send(s, tx))
    } else {
        button.fill(Color::WHITE.alpha(0.08)).disabled(true).child(icon(&icons::ARROW_UP, 16.0).color(t.text_secondary))
    }
}

fn send(s: &mut Chat, tx: &mut Tx) {
    if s.send() {
        tx.scroll_to_bottom("messages");
    }
}

/// A sent message, on the right: its captures, then its text in a bubble.
fn message(i: usize, m: &Message, t: &tok::Theme) -> El {
    let mut out = col().key(format!("msg-{i}")).w_full().items_end().gap(6.0).arrives_whole().transition(
        Transition::offset(0.0, 8.0).combined(Transition::opacity()),
    );
    for (j, image) in m.images.iter().enumerate() {
        let (w, h) = thumb_size(image.width, image.height, THUMB_MAX);
        out = out.child(
            el().key(format!("msg-{i}-img-{j}"))
                .size(w, h)
                .radius(10.0)
                .continuous_corners()
                .clip_rounded(10.0)
                .border(1.0, Color::WHITE.alpha(0.10))
                .pixels(&image.pixels),
        );
    }
    if !m.text.is_empty() {
        out = out.child(
            el().key(format!("msg-{i}-text"))
                .max_w(BUBBLE_MAX_W)
                .px(12.0)
                .py(8.0)
                .radius(14.0)
                .continuous_corners()
                .fill(Color::WHITE.alpha(0.08))
                .child(text(m.text.clone()).font(TextStyle::new(13.5, 400.0).lh(20.0)).color(t.text).wrap()),
        );
    }
    out
}

pub fn view(s: &Chat, cx: &mut Cx<Chat>) -> El {
    crate::settings::apply_motion(cx);
    let t = tok::theme(true);
    let header = row()
        .key("header")
        .w_full()
        .h(HEADER_H)
        .pl(14.0)
        .pr(9.0)
        .gap(8.0)
        .center_y()
        .child(icon(&glyphs::SPARKLE, 16.0).color(ACCENT))
        .child(text("Chat").font(TextStyle::new(13.5, 600.0)).color(t.text).grow(1.0))
        .child(close_button(&t));
    let divider = || el().w_full().h(1.0).fill(Color::WHITE.alpha(0.06));
    let mut thread = col().key("thread").w_full().gap(14.0).px(14.0).py(14.0).growth(GrowthSpec::default());
    for (i, m) in s.messages.iter().enumerate() {
        thread = thread.child(message(i, m, &t));
    }
    let messages = col()
        .key("messages")
        .w_full()
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(16.0)
        .child(thread);
    let field = text_input(&s.draft, TextStyle::new(14.0, 400.0).lh(20.0))
        .key("draft")
        .w_full()
        .color(t.text)
        .caret(t.text, ACCENT.alpha(0.35))
        .placeholder("Message…", t.text_secondary.alpha(0.8))
        .on_edit(|s: &mut Chat| &mut s.draft)
        .on_input_key(|s: &mut Chat, ev: &KeyEvent, _: &mut Tx| s.field_key(ev))
        .on_submit(send);
    let composer = row()
        .key("composer")
        .w_full()
        .items_end()
        .gap(8.0)
        .pl(12.0)
        .pr(8.0)
        .py(8.0)
        .radius(12.0)
        .continuous_corners()
        .fill(Color::WHITE.alpha(0.05))
        .border(1.0, Color::WHITE.alpha(0.08))
        .child(col().grow(1.0).min_w(0.0).py(4.0).child(auto_grow(field, 1, 6)))
        .child(send_button(!s.draft.text.trim().is_empty(), &t));
    let frost = col()
        .key("frost")
        .w_full()
        .h_full()
        .min_h(0.0)
        .radius(RADIUS)
        .continuous_corners()
        .clip_rounded(RADIUS)
        .fill(FROST)
        .border_gradient(1.0, Color::WHITE.alpha(0.14), Color::WHITE.alpha(0.05))
        .child(header)
        .child(divider())
        .child(messages)
        .child(el().w_full().padding(10.0).child(composer));
    let motion = Anim::from(GLIDE);
    let panel = el()
        .key("panel")
        .hit()
        .w(CHAT_W)
        .h_full()
        .radius(RADIUS)
        .continuous_corners()
        .box_shadow(Shadow::core_animation(0.0, 0.5, 1.5, Color::BLACK.alpha(0.3)))
        .box_shadow(Shadow::core_animation(0.0, 10.0, 22.0, Color::BLACK.alpha(0.4)))
        .system_material(SystemMaterial::Popover)
        .offset(offset(s.open, s.insets), 0.0)
        .offset_motion(motion)
        .motion(GLIDE)
        .child(frost);
    row()
        .w_full()
        .h_full()
        .pl(PAD)
        .pr(GAP + s.insets.right)
        .pt(GAP + s.insets.top)
        .pb(GAP + s.insets.bottom)
        .child(panel)
}

pub fn on_key(s: &mut Chat, event: &KeyEvent, _: &mut Tx) -> bool {
    s.field_key(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insets_leave_the_menu_bar_and_the_dock() {
        let frame = [0.0, 0.0, 1512.0, 982.0];
        assert_eq!(insets(frame, [0.0, 70.0, 1512.0, 879.0]), Insets { top: 33.0, bottom: 70.0, right: 0.0 }, "Dock at the bottom");
        assert_eq!(insets(frame, [0.0, 0.0, 1440.0, 949.0]), Insets { top: 33.0, bottom: 0.0, right: 72.0 }, "Dock on the right");
        // A second display above the first: AppKit y is global.
        assert_eq!(insets([0.0, 982.0, 1920.0, 1080.0], [0.0, 982.0, 1920.0, 1055.0]), Insets { top: 25.0, bottom: 0.0, right: 0.0 });
    }

    #[test]
    fn closed_it_waits_past_the_right_edge_and_opens_to_rest() {
        let insets = Insets { top: 33.0, bottom: 0.0, right: 72.0 };
        assert_eq!(offset(true, insets), 0.0);
        let (w, _) = window_size(insets, 982.0);
        // The panel sits PAD in from the window's left: past its right edge with PAD of shadow too.
        assert!(offset(false, insets) >= w, "out of the window, shadow included: {}", offset(false, insets));
    }

    #[test]
    fn opening_focuses_its_field_and_closing_keeps_the_conversation() {
        let mut c = Chat::new(None);
        c.place(Insets::default());
        c.push(Message { text: "hello".into(), images: Vec::new() });
        c.draft.focused = false;
        c.open();
        assert!(c.is_open());
        assert!(c.draft.focused, "type at once");
        c.draft = TextEdit::new("draft");
        assert!(c.close());
        assert!(!c.close(), "already closed");
        assert_eq!((c.messages.len(), c.draft.text.as_str()), (1, "draft"));
    }

    #[test]
    fn sending_appends_the_trimmed_draft_and_clears_it() {
        let mut c = Chat::new(None);
        c.draft = TextEdit::new("   ");
        assert!(!c.send(), "nothing to send");
        assert!(c.messages.is_empty());
        c.draft = TextEdit::new("  and this?\n");
        assert!(c.send());
        assert_eq!(c.messages.last().map(|m| m.text.as_str()), Some("and this?"));
        assert_eq!(c.draft.text, "");
    }

    #[test]
    fn the_input_shortcut_brings_back_a_started_chat() {
        assert_eq!(toggle(false, false, false), Toggle::ShowInput, "no chat yet");
        assert_eq!(toggle(true, false, false), Toggle::HideInput);
        assert_eq!(toggle(true, true, true), Toggle::HideInput, "an open input closes first");
        assert_eq!(toggle(false, true, false), Toggle::ShowChat, "the chat instead of the input");
        assert_eq!(toggle(false, true, true), Toggle::HideChat);
    }

    #[test]
    fn starting_over_slides_out_and_the_next_message_starts_a_new_chat() {
        let mut c = Chat::new(None);
        c.push(Message { text: "old".into(), images: Vec::new() });
        c.open();
        c.draft = TextEdit::new("unsent");
        c.start_over();
        assert!(!c.is_open());
        assert_eq!(c.messages.len(), 1, "still shown while it slides out");
        c.push(Message { text: "new".into(), images: Vec::new() });
        assert_eq!(c.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), vec!["new"]);
        assert_eq!(c.draft.text, "", "the old draft went with it");
        c.push(Message { text: "more".into(), images: Vec::new() });
        assert_eq!(c.messages.len(), 2, "then it carries on");
    }

    #[test]
    fn thumbnails_fit_keeping_their_shape() {
        assert_eq!(thumb_size(1200, 400, (240.0, 160.0)), (240.0, 80.0), "wide");
        assert_eq!(thumb_size(400, 800, (240.0, 160.0)), (80.0, 160.0), "tall");
        assert_eq!(thumb_size(100, 50, (240.0, 160.0)), (100.0, 50.0), "never enlarged");
        assert_eq!(thumb_size(0, 0, (240.0, 160.0)), (0.0, 0.0));
    }

    #[test]
    fn a_capture_png_decodes_back_to_its_pixels() {
        let rgba: Vec<u8> = (0..3 * 2 * 4).map(|i| i as u8).collect();
        let png = crate::export::encode_png(3, 2, 2.0, &rgba).unwrap();
        assert_eq!(decode_png(&png), Some((3, 2, rgba)));
        assert_eq!(decode_png(b"not a png"), None);
    }
}
