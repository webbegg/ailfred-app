//! The chat side panel: sending from the quick input turns the input's own panel into it (same
//! window, same element: it springs to the right edge, reshaping), with what was sent as the
//! first message and its own field focused. It stays open
//! when another app is clicked; Esc or its close button slide it back out, and the input's
//! shortcut brings it back. Cmd+N slides it out for good and opens a new input. No AI: it shows
//! the conversation typed into it.

use rune::tok;
use rune::{
    Anim, AppHandle, Color, Curve, Cursor, El, GrowthSpec, Key, KeyEvent, Pixels, ScrollSpec, Shadow, Spring, TextEdit, TextStyle, Transition, Tx,
    auto_grow, col, el, icon, row, text, text_input,
};
use rune_kit::glyphs;

use crate::Host;
use crate::icons;
use crate::quick_input::{self, ACCENT, Attached, Escape};

/// The panel's width and its gap to the screen's edges, in points. Its corners are the input's
/// (`quick_input::RADIUS`): it's the same panel.
pub const CHAT_W: f32 = 400.0;
const GAP: f32 = 12.0;
/// Room past the panel for its shadow.
const PAD: f32 = 32.0;
const HEADER_H: f32 = 44.0;
/// A message's widest, and its images' largest.
const BUBBLE_MAX_W: f32 = CHAT_W * 0.82;
const THUMB_MAX: (f32, f32) = (240.0, 160.0);
/// An enlarged chip's image takes at most this share of the screen's width and height.
const LIGHTBOX: f32 = 0.8;
/// The slide in from the edge and back, and the input's panel reshaping into this one.
const GLIDE: Spring = tok::motion::GLIDE;
/// How long after the input starts reshaping the content fades in, and over how long; and how
/// fast the input's row fades as it goes.
pub const LANDING: std::time::Duration = std::time::Duration::from_millis(260);
const CONTENT_FADE: f32 = 0.2;
pub const ROW_FADE: f32 = 0.12;

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

/// The panel's rect at rest on a `screen` of that size (points, the overlay covers the display):
/// at its right edge, its full height, past the menu bar and the Dock.
pub fn side_rect(screen: [f32; 2], insets: Insets) -> [f32; 4] {
    let [sw, sh] = screen;
    [sw - GAP - insets.right - CHAT_W, insets.top + GAP, CHAT_W, (sh - insets.top - insets.bottom - 2.0 * GAP).max(0.0)]
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

/// A PNG (a capture, or whatever was pasted) to its 8-bit RGBA pixels.
pub fn decode_png(png: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

/// Messages typed in the panel take ids from here; the host's (sent from the input) are below.
const LOCAL_IDS: u64 = 1 << 32;

/// The field an image was pasted into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Field {
    Input,
    Chat,
}

/// A pasted image, decoded, still lands only if its field is still open.
pub fn paste_lands(field: Field, quick_open: bool, chat_open: bool) -> bool {
    match field {
        Field::Input => quick_open,
        Field::Chat => chat_open,
    }
}

/// The image on the clipboard, if any, as a PNG to send.
pub fn pasted_png(content: &rune::ClipboardContent) -> Option<Vec<u8>> {
    content.image().map(|image| image.png().to_vec())
}

/// Decodes pasted image `id` off the main thread, then hands it to `field` through the host.
pub fn decode_pasted(host: &Option<AppHandle<Host>>, field: Field, id: u64, png: Vec<u8>) {
    let Some(host) = host.clone() else { return };
    std::thread::spawn(move || match Image::from_png(&png) {
        Some(image) => host.update(move |host, _| host.pasted(field, id, png, image)),
        None => eprintln!("Capibarapp: the pasted image couldn't be read"),
    });
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

/// A capture, ready to draw (a handle: clones share the pixels).
#[derive(Clone)]
pub struct Image {
    pixels: Pixels,
    width: u32,
    height: u32,
}

impl Image {
    pub fn from_png(png: &[u8]) -> Option<Image> {
        let (width, height, rgba) = decode_png(png)?;
        Some(Image::from_rgba(width, height, rgba))
    }

    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Image {
        let pixels = Pixels::new();
        pixels.set(width, height, rgba);
        Image { pixels, width, height }
    }

    /// The image as large as fits in `max` (`(w, h)` in points), keeping its shape, its corners
    /// rounded `radius`.
    pub fn fitted(&self, max: (f32, f32), radius: f32) -> El {
        let (w, h) = thumb_size(self.width, self.height, max);
        el().size(w, h).radius(radius).continuous_corners().clip_rounded(radius).pixels(&self.pixels)
    }
}

/// A chip's image enlarged in the middle of a `screen` of that size, over a dim that closes it
/// (`close`) when clicked.
pub fn lightbox<S: 'static>(image: &Image, screen: [f32; 2], close: impl Fn(&mut S) + 'static) -> El {
    let max = (screen[0] * LIGHTBOX, screen[1] * LIGHTBOX);
    el().key("preview")
        .w_full()
        .h_full()
        .center()
        .fill(Color::BLACK.alpha(0.55))
        .hit()
        .label("Close preview")
        .shortcut_hint("Esc")
        .transition(Transition::opacity())
        .on_press(close)
        .child(
            image
                .fitted(max, 12.0)
                .key("preview-image")
                .border(1.0, Color::WHITE.alpha(0.14))
                .box_shadow(Shadow::core_animation(0.0, 16.0, 40.0, Color::BLACK.alpha(0.5)))
                .transition(Transition::scale(0.94)),
        )
}

/// One sent message: its text (maybe empty) and its captures.
pub struct Message {
    /// Its captures arrive once decoded (`set_images`).
    pub id: u64,
    pub text: String,
    pub images: Vec<Image>,
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
}

impl Chat {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        Chat { messages: Vec::new(), draft: TextEdit::new(""), shots: Vec::new(), preview: None, open: false, insets: Insets::default(), screen: [1440.0, 900.0], landed: true, over: false, host, next_shot: 0 }
    }

    pub fn is_open(&self) -> bool {
        self.open
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

    /// In place after the flight: its content fades in.
    pub fn land(&mut self) {
        self.landed = true;
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
        self.open = false;
        self.over = true;
        self.preview = None;
    }

    /// Adds a message at the end of the conversation, the field focused for the next.
    pub fn push(&mut self, message: Message) {
        if std::mem::replace(&mut self.over, false) {
            self.messages.clear();
            self.draft.clear();
            self.shots.clear();
        }
        self.messages.push(message);
        self.draft.focused = true;
    }

    /// Cmd+V in the field: an image on the clipboard takes the next id and, once decoded, joins
    /// the draft (`attach`). False lets the field paste the text.
    pub fn paste(&mut self, content: &rune::ClipboardContent) -> bool {
        let Some((id, png)) = self.take_paste(content) else { return false };
        decode_pasted(&self.host, Field::Chat, id, png);
        true
    }

    fn take_paste(&mut self, content: &rune::ClipboardContent) -> Option<(u64, Vec<u8>)> {
        let png = pasted_png(content)?;
        self.next_shot += 1;
        Some((self.next_shot, png))
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
        let pngs = std::mem::take(&mut self.shots).into_iter().map(|a| a.png).collect();
        let id = self.messages.last().map_or(0, |m| m.id + 1).max(LOCAL_IDS);
        self.push(Message { id, text, images: Vec::new() });
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
        if ev.key == Key::Escape {
            if self.escape() == Escape::Close {
                self.dismiss();
            }
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

fn close_button<S: 'static>(t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
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
        .on_press(move |s: &mut S| lens(s).dismiss())
}

fn send_button<S: 'static>(ready: bool, t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
    let button = el().key("send").size(28.0, 28.0).radius(8.0).continuous_corners().center().label("Send").shortcut_hint("↵");
    if ready {
        button
            .fill(ACCENT)
            .hover(Color::WHITE.alpha(0.10))
            .press(Color::BLACK.alpha(0.12))
            .press_scale(0.94)
            .cursor(Cursor::Pointer)
            .child(icon(&icons::ARROW_UP, 16.0).color(Color::WHITE))
            .on_press_tx(move |s: &mut S, tx: &mut Tx| send(lens(s), tx))
    } else {
        button.fill(Color::WHITE.alpha(0.08)).disabled(true).child(icon(&icons::ARROW_UP, 16.0).color(t.text_secondary))
    }
}

/// Enter or the send button: the draft and its captures join the conversation; the captures
/// are decoded off the main thread and show when ready.
fn send(s: &mut Chat, tx: &mut Tx) {
    let Some((id, pngs)) = s.send() else { return };
    tx.scroll_to_bottom("messages");
    if !pngs.is_empty()
        && let Some(host) = &s.host
    {
        host.update(move |host, _| host.decode_into_chat(id, pngs));
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

/// The panel, keyed `key`, in a window whose state `S` holds the chat at `lens`. The input's row
/// (`input_row`), while the input's panel is turning into this one, fades out over it.
pub fn panel<S: 'static>(s: &Chat, key: &str, lens: fn(&mut S) -> &mut Chat, input_row: Option<El>) -> El {
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
        .child(close_button(&t, lens));
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
        .on_edit(move |s: &mut S| &mut lens(s).draft)
        .on_input_key(move |s: &mut S, ev: &KeyEvent, _: &mut Tx| lens(s).field_key(ev))
        .on_input_paste(move |s: &mut S, content: &rune::ClipboardContent, _: &mut Tx| lens(s).paste(content))
        .on_submit(move |s: &mut S, tx: &mut Tx| send(lens(s), tx));
    let typing = row()
        .key("typing")
        .w_full()
        .items_end()
        .gap(8.0)
        .child(col().grow(1.0).min_w(0.0).py(4.0).child(auto_grow(field, 1, 6)))
        .child(send_button(s.send_ready(), &t, lens));
    let mut composer = col()
        .key("composer")
        .w_full()
        .gap(6.0)
        .pl(12.0)
        .pr(8.0)
        .py(8.0)
        .radius(12.0)
        .continuous_corners()
        .fill(Color::WHITE.alpha(0.05))
        .border(1.0, Color::WHITE.alpha(0.08));
    // Captures pasted in, as the input's chips.
    if !s.shots.is_empty() {
        let compact = quick_input::compact(s.shots.len());
        let mut chips = row().key("chips").gap(6.0).pt(2.0).center_y();
        for shot in &s.shots {
            let id = shot.id;
            let (remove, open) = (move |s: &mut S| _ = lens(s).remove(id), move |s: &mut S| lens(s).open_preview(id));
            chips = chips.child(quick_input::chip(shot, compact, &t, remove, open));
        }
        composer = composer.child(chips);
    }
    let composer = composer.child(typing);
    let [x, y, w, h] = side_rect(s.screen, s.insets);
    // At its landed size from the start, pinned to the panel's top left: the panel reveals it as
    // it grows. Hidden while the input's panel reshapes, then faded in.
    let content = col()
        .key("chat-layer")
        .size(w, h)
        .pin(Some(0.0), Some(0.0), None, None)
        .opacity(if s.landed { 1.0 } else { 0.0 })
        .opacity_motion(Anim::new(Curve::ease_in_out(CONTENT_FADE)))
        .child(header)
        .child(divider())
        .child(messages)
        .child(el().w_full().padding(10.0).child(composer));
    let layers = match input_row {
        Some(row) => vec![content, row.opacity(0.0).opacity_motion(Anim::new(Curve::ease_in_out(ROW_FADE)))],
        None => vec![content],
    };
    quick_input::surface(key, s.open, layers)
        .size(w, h)
        .pin(Some(x), Some(y), None, None)
        .offset(offset(s.open, s.insets), 0.0)
        .offset_motion(Anim::from(GLIDE))
        .motion(GLIDE)
}

/// Keys in the window with the chat's field focused and nothing else taking them.
pub fn on_key(s: &mut Chat, event: &KeyEvent) -> bool {
    s.open && s.field_key(event)
}

#[cfg(test)]
pub(crate) mod tests {
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
        let [x, ..] = side_rect([1512.0, 982.0], insets);
        assert!(x + offset(false, insets) >= 1512.0 + PAD, "past the screen's edge, shadow included");
    }

    #[test]
    fn at_rest_it_takes_the_right_edge_past_the_menu_bar_and_dock() {
        let insets = Insets { top: 33.0, bottom: 70.0, right: 0.0 };
        assert_eq!(side_rect([1512.0, 982.0], insets), [1512.0 - GAP - CHAT_W, 33.0 + GAP, CHAT_W, 982.0 - 33.0 - 70.0 - 2.0 * GAP]);
        let [x, _, w, _] = side_rect([1512.0, 982.0], Insets { right: 72.0, ..insets });
        assert_eq!(x + w, 1512.0 - 72.0 - GAP, "left of a Dock on the right");
    }

    fn msg(id: u64, text: &str) -> Message {
        Message { id, text: text.into(), images: Vec::new() }
    }

    #[test]
    fn the_inputs_panel_becomes_it_open_with_the_message_its_content_shown_once_landed() {
        let mut c = Chat::new(None);
        c.place(Insets::default(), [1512.0, 982.0]);
        c.morph(msg(0, "hi"));
        assert!(c.is_open(), "open at once: the input's panel springs to the side");
        assert!(!c.landed(), "its content waits while it reshapes");
        assert_eq!(c.messages.len(), 1);
        assert!(c.draft.focused, "type at once");
        c.land();
        assert!(c.landed());
        c.close();
        c.place(Insets::default(), [1512.0, 982.0]);
        assert!(c.landed(), "brought back later, it slides in whole");
    }

    #[test]
    fn a_messages_captures_arrive_once_decoded() {
        let mut c = Chat::new(None);
        c.push(msg(3, "look"));
        c.push(msg(4, "and"));
        let image = Image { pixels: Pixels::new(), width: 2, height: 1 };
        c.set_images(3, vec![image]);
        assert_eq!(c.messages.iter().map(|m| m.images.len()).collect::<Vec<_>>(), vec![1, 0]);
        c.set_images(9, Vec::new());
    }

    #[test]
    fn opening_focuses_its_field_and_closing_keeps_the_conversation() {
        let mut c = Chat::new(None);
        c.place(Insets::default(), [1512.0, 982.0]);
        c.push(msg(0, "hello"));
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
        assert!(c.send().is_none(), "nothing to send");
        assert!(c.messages.is_empty());
        c.draft = TextEdit::new("  and this?\n");
        assert!(c.send().is_some());
        assert_eq!(c.messages.last().map(|m| m.text.as_str()), Some("and this?"));
        assert_eq!(c.draft.text, "");
    }

    #[test]
    fn captures_attach_to_the_draft_replace_in_place_and_go_with_the_next_message() {
        let mut c = Chat::new(None);
        c.attach(1, vec![1], tiny());
        c.attach(2, vec![2], tiny());
        c.attach(1, vec![9], tiny());
        assert_eq!(c.shot_ids(), vec![1, 2], "the same area again replaces its image in place");
        assert!(c.remove(2));
        assert!(!c.remove(2));
        let (id, pngs) = c.send().expect("a capture alone sends");
        assert_eq!(pngs, vec![vec![9]]);
        assert!(c.shot_ids().is_empty());
        assert_eq!(c.messages.last().map(|m| (m.id, m.text.as_str())), Some((id, "")));
    }

    pub(crate) fn tiny() -> Image {
        Image::from_rgba(1, 1, vec![0; 4])
    }

    /// A 2 × 1 PNG of `color`'s kind, its pixels `data`.
    fn png_of(color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 2, 1);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(data).unwrap();
        out
    }

    /// What the clipboard holds: `text`, and an image when `png` is given.
    pub(crate) fn clip(text: &str, png: Option<Vec<u8>>) -> rune::ClipboardContent {
        let content = rune::ClipboardContent::new(text);
        match png.and_then(rune::ClipboardImage::from_png) {
            Some(image) => content.with_image(image),
            None => content,
        }
    }

    pub(crate) fn a_png() -> Vec<u8> {
        png_of(png::ColorType::Rgba, &[1, 2, 3, 255, 4, 5, 6, 255])
    }

    #[test]
    fn pasted_images_take_their_own_ids_and_text_is_left_to_the_field() {
        let mut c = Chat::new(None);
        assert_eq!(c.take_paste(&clip("just text", None)), None);
        assert_eq!(c.take_paste(&clip("", Some(a_png()))), Some((1, a_png())));
        assert_eq!(c.take_paste(&clip("", Some(a_png()))).map(|(id, _)| id), Some(2));
    }

    #[test]
    fn any_8_bit_png_decodes_to_rgba() {
        assert_eq!(decode_png(&png_of(png::ColorType::Rgb, &[1, 2, 3, 4, 5, 6])), Some((2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255])));
        assert_eq!(decode_png(&png_of(png::ColorType::Grayscale, &[7, 8])), Some((2, 1, vec![7, 7, 7, 255, 8, 8, 8, 255])));
        assert_eq!(decode_png(&png_of(png::ColorType::GrayscaleAlpha, &[7, 9, 8, 10])), Some((2, 1, vec![7, 7, 7, 9, 8, 8, 8, 10])));
        assert_eq!(decode_png(&a_png()).map(|(.., px)| px), Some(vec![1, 2, 3, 255, 4, 5, 6, 255]));
    }

    #[test]
    fn a_pasted_image_lands_only_in_a_field_still_open() {
        assert!(paste_lands(Field::Input, true, false));
        assert!(paste_lands(Field::Chat, false, true));
        assert!(paste_lands(Field::Chat, true, true), "Cmd+N right after: the chat still shows");
        assert!(!paste_lands(Field::Input, false, true), "the input closed meanwhile");
        assert!(!paste_lands(Field::Chat, true, false), "the chat closed meanwhile");
    }

    #[test]
    fn a_chips_image_opens_enlarged_and_esc_closes_it_before_the_chat() {
        let mut c = Chat::new(None);
        c.attach(1, vec![1], tiny());
        c.attach(2, vec![2], tiny());
        c.open_preview(2);
        assert!(c.preview_image().is_some());
        assert_eq!(c.escape(), Escape::Preview, "the enlarged image closes first");
        assert!(c.preview_image().is_none());
        assert_eq!(c.shot_ids(), vec![1, 2], "its capture stays attached");
        assert_eq!(c.escape(), Escape::Close, "then the chat closes");
        c.open_preview(9);
        assert!(c.preview_image().is_none(), "no such capture");
    }

    #[test]
    fn removing_or_sending_a_capture_closes_its_enlarged_image() {
        let mut c = Chat::new(None);
        c.attach(1, vec![1], tiny());
        c.open_preview(1);
        assert!(c.remove(1));
        assert!(c.preview_image().is_none());
        assert_eq!(c.escape(), Escape::Close, "nothing enlarged left to close first");
        c.attach(2, vec![2], tiny());
        c.open_preview(2);
        c.send().expect("sent");
        assert_eq!(c.escape(), Escape::Close);
    }

    #[test]
    fn backspace_in_an_empty_field_removes_the_last_capture() {
        let mut c = Chat::new(None);
        c.attach(1, vec![1], tiny());
        c.attach(2, vec![2], tiny());
        assert_eq!(c.backspace_empty(), Some(2));
        assert_eq!(c.backspace_empty(), Some(1));
        assert_eq!(c.backspace_empty(), None);
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
        c.push(msg(0, "old"));
        c.open();
        c.draft = TextEdit::new("unsent");
        c.start_over();
        assert!(!c.is_open());
        assert_eq!(c.messages.len(), 1, "still shown while it slides out");
        c.push(msg(1, "new"));
        assert_eq!(c.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), vec!["new"]);
        assert_eq!(c.draft.text, "", "the old draft went with it");
        c.push(msg(2, "more"));
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
