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

use std::time::Instant;

use crate::Host;
use crate::icons;
use crate::mock_model;
use crate::reply::{self, Event, Reply};
use crate::reply_view;
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

/// The panel's top and right edges' distances from the window's (points): pinned by its right
/// edge, it rests and hides relative to the window whatever screen size it was last told.
pub fn side_pins(insets: Insets) -> [f32; 2] {
    [insets.top + GAP, GAP + insets.right]
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

/// One image a paste brings: the clipboard's, or an image file copied in Finder (read later,
/// off the main thread).
#[derive(Debug, PartialEq)]
pub enum Pasted {
    Png(Vec<u8>),
    File(std::path::PathBuf),
}

/// The images a paste brings: copied files, those that are PNG or JPEG images (Finder puts
/// their icon on the clipboard too, so other files bring none); else the clipboard's image.
pub fn pasted_images(content: &rune::ClipboardContent) -> Vec<Pasted> {
    if !content.files().is_empty() {
        let image = |p: &&std::path::PathBuf| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["png", "jpg", "jpeg"].contains(&e.to_ascii_lowercase().as_str()))
        };
        return content.files().iter().filter(image).cloned().map(Pasted::File).collect();
    }
    content.image().map(|image| vec![Pasted::Png(image.png().to_vec())]).unwrap_or_default()
}

/// Each pasted image with the next of the field's ids (`next` is the last taken).
pub fn take_ids(next: &mut u64, pasted: Vec<Pasted>) -> Vec<(u64, Pasted)> {
    pasted
        .into_iter()
        .map(|p| {
            *next += 1;
            (*next, p)
        })
        .collect()
}

/// Pasted files larger than this, or images with more pixels, aren't read: decoding one takes
/// 4 bytes a pixel, twice over.
const MAX_FILE_BYTES: u64 = 64 << 20;
const MAX_PIXELS: u64 = 100_000_000;

/// The image in the file at `path` as a PNG: a PNG as it is, a JPEG converted. Else None.
pub fn image_file_png(path: &std::path::Path) -> Option<Vec<u8>> {
    image_file_png_within(path, MAX_FILE_BYTES, MAX_PIXELS)
}

/// `image_file_png`, for a file of at most `max_bytes` and an image of at most `max_pixels`.
fn image_file_png_within(path: &std::path::Path, max_bytes: u64, max_pixels: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(max_bytes + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > max_bytes {
        return None;
    }
    let fits = |(w, h): (u32, u32)| u64::from(w) * u64::from(h) <= max_pixels;
    if let Some(size) = png_size(&bytes) {
        return (fits(size) && decode_png(&bytes).is_some()).then_some(bytes);
    }
    let (rgba, w, h) = crate::capture::jpeg_rgba(&bytes, &|w, h| fits((w, h)))?;
    crate::export::encode_png(w, h, 1.0, &rgba).ok()
}

/// A PNG's width and height, from its header alone.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().ok()?;
    let info = reader.info();
    Some((info.width, info.height))
}

/// Decodes pasted image `id` off the main thread (reading it first, if a file), then hands it to
/// `field` through the host.
pub fn decode_pasted(host: &Option<AppHandle<Host>>, field: Field, id: u64, pasted: Pasted) {
    let Some(host) = host.clone() else { return };
    std::thread::spawn(move || {
        let png = match pasted {
            Pasted::Png(png) => Some(png),
            Pasted::File(path) => image_file_png(&path),
        };
        match png.and_then(|png| Image::from_png(&png).map(|image| (png, image))) {
            Some((png, image)) => host.update(move |host, _| host.pasted(field, id, png, image)),
            None => eprintln!("Ailfred: the pasted image couldn't be read"),
        }
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
/// The chat's back in the side panel from its window: it slides in, or, emptied there, a
/// fresh input rises as Cmd+N's does.
pub fn on_dock(has_messages: bool) -> Toggle {
    if has_messages { Toggle::ShowChat } else { Toggle::ShowInput }
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
}

impl Chat {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        Chat { messages: Vec::new(), draft: TextEdit::new(""), shots: Vec::new(), preview: None, open: false, insets: Insets::default(), screen: [1440.0, 900.0], landed: true, over: false, host, next_shot: 0, replies: Vec::new(), next_token: 0, scratch: TextEdit::new(""), away: false, windowed: false }
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
        // It arrives at the latest (`Host::into_window`, `QuickInput::put_chat`).
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

    /// Message `id` gets a reply from the (fake) model, which the host streams in; the reply's
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
        let script = if answer { mock_model::answer_script(&prompt) } else { mock_model::script(&prompt) };
        let mut reply = Reply::new(id, prompt);
        (reply.token, reply.answer) = (token, answer);
        self.replies.retain(|r| r.id != id);
        self.replies.push(reply);
        if let Some(host) = &self.host {
            host.update(move |host, _| host.stream_reply(token, script));
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

    /// Stop: the reply streaming keeps what came.
    pub fn stop(&mut self, now: Instant) {
        for reply in &mut self.replies {
            reply.stop(now);
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
        self.push(Message { id, text: text.clone(), images: Vec::new() });
        Some(self.start_reply(id, text, true))
    }

    fn next_id(&self) -> u64 {
        // Past every id so far: the input's (small) ones arrive after typed ones too.
        self.messages.iter().map(|m| m.id + 1).max().unwrap_or(0).max(LOCAL_IDS)
    }

    /// Adds a message at the end of the conversation, the field focused for the next.
    pub fn push(&mut self, message: Message) {
        if std::mem::replace(&mut self.over, false) {
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
        let pngs = std::mem::take(&mut self.shots).into_iter().map(|a| a.png).collect();
        let id = self.next_id();
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

fn send_button<S: 'static>(ready: bool, streaming: bool, t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
    if streaming && !ready {
        return reply_view::stop_button(lens);
    }
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
/// Over the thread's bottom while the user is scrolled back: back to the latest.
fn to_latest<S: 'static>(t: &tok::Theme) -> El {
    let button = el()
        .key("to-latest")
        .size(32.0, 32.0)
        .radius(16.0)
        .center()
        .fill(Color { r: 0.16, g: 0.16, b: 0.16, a: 0.96 })
        .border(1.0, Color::WHITE.alpha(0.14))
        .shadow(4.0, 12.0, Color::BLACK.alpha(0.35))
        .hover(Color::WHITE.alpha(0.08))
        .press(Color::BLACK.alpha(0.12))
        .press_scale(0.94)
        .cursor(Cursor::Pointer)
        .label("Jump to latest")
        .child(icon(&glyphs::CHEVRON_DOWN, 14.0).color(t.text))
        .on_press_tx(move |_: &mut S, tx: &mut Tx| tx.scroll_to_bottom("messages"));
    row()
        .key("to-latest-row")
        .pin(Some(0.0), None, Some(0.0), Some(10.0))
        .justify_center()
        .transition(Transition::opacity().combined(Transition::scale(0.85)))
        .child(button)
}

fn send(s: &mut Chat, tx: &mut Tx) {
    let Some((id, pngs)) = s.send() else { return };
    s.ask(id);
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
    let header = header(14.0, &t)
        .child(header_button("pop-out", &icons::POP_OUT, "Open in a window", &t, move |s: &mut S| lens(s).pop_out()))
        .child(close_button(&t, lens));
    let [.., w, h] = side_rect(s.screen, s.insets);
    let [top, right] = side_pins(s.insets);
    // At its landed size from the start, pinned to the panel's top left: the panel reveals it as
    // it grows. Hidden while the input's panel reshapes, then faded in.
    let content = body(s, header, lens)
        .key("chat-layer")
        .size(w, h)
        .pin(Some(0.0), Some(0.0), None, None)
        .opacity(if s.landed { 1.0 } else { 0.0 })
        .opacity_motion(Anim::new(Curve::ease_in_out(CONTENT_FADE)));
    let layers = match input_row {
        Some(row) => vec![content, row.opacity(0.0).opacity_motion(Anim::new(Curve::ease_in_out(ROW_FADE)))],
        None => vec![content],
    };
    quick_input::surface(key, s.open, layers)
        .size(w, h)
        // By its right edge: told the wrong screen (none yet at launch, or another display's),
        // closed it still waits past the window's edge, never on screen.
        .pin(None, Some(top), Some(right), None)
        .offset(offset(s.open, s.insets), 0.0)
        .offset_motion(Anim::from(GLIDE))
        // The input's panel arrives pressed in (sending) and springs back to full size.
        .scale(1.0)
        .scale_motion(Anim::from(GLIDE))
        .motion(GLIDE)
}

/// The header's row: the sparkle and "Chat", `lead` points in; the caller adds its buttons.
pub fn header(lead: f32, t: &tok::Theme) -> El {
    row()
        .key("header")
        .w_full()
        .h(HEADER_H)
        .pl(lead)
        .pr(9.0)
        .gap(8.0)
        .center_y()
        .child(icon(&glyphs::SPARKLE, 16.0).color(ACCENT))
        .child(text("Chat").font(TextStyle::new(13.5, 600.0)).color(t.text).grow(1.0))
}

pub const HEADER_HEIGHT: f32 = HEADER_H;

/// A header button: a glyph, its `label` spoken and shown on hover.
fn header_button<S: 'static>(key: &str, glyph: &'static rune::Icon, label: &str, t: &tok::Theme, f: impl Fn(&mut S) + 'static) -> El {
    el().key(key.to_owned())
        .size(26.0, 26.0)
        .radius(7.0)
        .continuous_corners()
        .center()
        .hover(Color::WHITE.alpha(0.10))
        .press(Color::WHITE.alpha(0.06))
        .cursor(Cursor::Pointer)
        .label(label.to_owned())
        .child(icon(glyph, 14.0).color(t.text_secondary))
        .on_press(f)
}

/// Its window's dock button: back into the side panel.
pub fn dock_button<S: 'static>(t: &tok::Theme, lens: fn(&mut S) -> &mut Chat) -> El {
    header_button("dock-side", &icons::DOCK_SIDE, "Back to the side panel", t, move |s: &mut S| lens(s).dock_back())
}

/// The conversation under `header`: the thread, scrolling, then the composer.
pub fn body<S: 'static>(s: &Chat, header: El, lens: fn(&mut S) -> &mut Chat) -> El {
    let t = tok::theme(true);
    let divider = || el().w_full().h(1.0).fill(Color::WHITE.alpha(0.06));
    let mut thread = col().key("thread").w_full().gap(14.0).px(14.0).py(14.0).growth(GrowthSpec::default());
    for (i, m) in s.messages.iter().enumerate() {
        thread = thread.child(message(i, m, &t));
        if let Some(r) = s.reply_to(m.id) {
            thread = thread.child(reply_view::view(r, lens));
        }
    }
    let messages = col()
        .key("messages")
        .w_full()
        .grow(1.0)
        .shrink()
        .min_h(0.0)
        // Follows what comes while at the bottom; scrolled back, it stays put.
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_edge_fade(16.0)
        .on_at_bottom(move |s: &mut S, at: bool| lens(s).away = !at)
        .child(thread);
    let mut messages = col().key("thread-frame").w_full().grow(1.0).shrink().min_h(0.0).child(messages);
    if s.away {
        messages = messages.child(to_latest::<S>(&t));
    }
    let field = text_input(&s.draft, TextStyle::new(14.0, 400.0).lh(20.0))
        .key("draft")
        .label("Message")
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
        .child(send_button(s.send_ready(), s.streaming(), &t, lens));
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
        let compact = quick_input::compact(s.shots.len(), quick_input::CHAT_FULL_CHIPS);
        let mut chips = row().key("chips").gap(6.0).pt(2.0).center_y();
        for shot in &s.shots {
            let id = shot.id;
            let (remove, open) = (move |s: &mut S| _ = lens(s).remove(id), move |s: &mut S| lens(s).open_preview(id));
            chips = chips.child(quick_input::chip(shot, compact, remove, open));
        }
        composer = composer.child(chips);
    }
    let composer = composer.child(typing);
    col().child(header).child(divider()).child(messages).child(el().w_full().padding(10.0).child(composer))
}

/// Keys in the window with the chat's field focused and nothing else taking them.
pub fn on_key(s: &mut Chat, event: &KeyEvent) -> bool {
    s.open && s.field_key(event)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn closed_it_hides_past_the_windows_edge_whatever_screen_it_was_told() {
        // Never placed yet (launch): told 1440 × 900, shown on a wider display.
        for insets in [Insets::default(), Insets { top: 33.0, bottom: 70.0, right: 72.0 }] {
            let [top, right] = side_pins(insets);
            for window in [1440.0, 1512.0, 1920.0, 2560.0] {
                let left = window - right - CHAT_W;
                assert!(left + offset(false, insets) >= window + PAD, "closed: past a {window}-wide window");
                let [x, y, ..] = side_rect([window, 900.0], insets);
                assert_eq!([left, top], [x, y], "open: at its rect");
            }
        }
    }

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
    fn landing_gives_its_field_the_keys_though_it_lost_them_while_hidden() {
        let mut c = Chat::new(None);
        c.morph(Message { id: 0, text: "hi".into(), images: Vec::new() });
        // Its content is transparent while the panel reshapes, so the window's focus passes it
        // by and the field's model is told it's not focused.
        c.draft.focused = false;
        c.land();
        assert!(c.draft_focused(), "type the next message at once");
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

    pub(crate) fn sent(c: &mut Chat, text: &str) -> u64 {
        c.draft = TextEdit::new(text);
        let (id, _) = c.send().expect("sent");
        id
    }

    fn texts(r: &Reply) -> String {
        r.parts.iter().filter_map(|p| if let reply::Part::Text(t) = p { Some(t.as_str()) } else { None }).collect()
    }

    #[test]
    fn in_its_window_it_stays_open_esc_leaves_it_and_cmd_n_clears_it_at_once() {
        let mut c = Chat::new(None);
        let id = sent(&mut c, "hi");
        let token = c.ask(id).unwrap();
        c.set_windowed(true);
        assert!(c.windowed() && c.is_open() && c.landed() && c.draft_focused());
        let esc = KeyEvent { key: Key::Escape, mods: rune::Mods::default() };
        c.stop(Instant::now());
        assert!(!c.field_key(&esc), "Esc doesn't close a window");
        c.attach(1, vec![1], tiny());
        c.draft = TextEdit::new("draft");
        c.clear();
        assert!(c.messages.is_empty() && c.replies.is_empty() && c.shots.is_empty() && c.draft.text.is_empty());
        c.reply_event(token, Event::Text("stale".into()), Instant::now());
        assert!(c.replies.is_empty());
        c.set_windowed(false);
        assert!(!c.windowed() && !c.is_open(), "back in the side panel, closed: it slides in");
        let id = sent(&mut c, "side");
        c.ask(id);
        c.stop(Instant::now());
        assert!(c.field_key(&esc), "Esc closes the side panel");
    }

    #[test]
    fn a_message_typed_after_one_from_the_input_gets_an_id_of_its_own() {
        let mut c = Chat::new(None);
        c.push(Message { id: 0, text: "from the input".into(), images: Vec::new() });
        let typed = sent(&mut c, "typed");
        c.push(Message { id: 1, text: "from the input again".into(), images: Vec::new() });
        let again = sent(&mut c, "typed again");
        let ids: Vec<u64> = c.messages.iter().map(|m| m.id).collect();
        assert!(again != typed && again != 0 && again != 1, "{ids:?}");
    }

    #[test]
    fn moved_between_the_panel_and_its_window_it_starts_at_the_latest() {
        let mut c = Chat::new(None);
        c.away = true;
        c.set_windowed(true);
        assert!(!c.away, "no stuck jump-to-latest button in the window");
        c.away = true;
        c.set_windowed(false);
        assert!(!c.away, "nor back in the side panel");
    }

    #[test]
    fn docked_back_empty_it_offers_the_input_not_an_empty_panel() {
        assert_eq!(on_dock(true), Toggle::ShowChat);
        assert_eq!(on_dock(false), Toggle::ShowInput, "cleared with Cmd+N in its window");
    }

    #[test]
    fn esc_while_a_reply_streams_stops_it_and_only_the_next_closes_the_side_panel() {
        let esc = KeyEvent { key: Key::Escape, mods: rune::Mods::default() };
        let mut c = Chat::new(None);
        c.open();
        let id = sent(&mut c, "hi");
        c.ask(id);
        assert!(c.streaming());
        assert!(c.field_key(&esc));
        assert!(!c.streaming() && c.is_open(), "stopped, still open");
        assert!(c.field_key(&esc), "the next asks the host to slide it out");
        let mut w = Chat::new(None);
        let id = sent(&mut w, "hi");
        w.ask(id);
        w.set_windowed(true);
        assert!(w.field_key(&esc) && !w.streaming() && w.is_open(), "stopped in the window");
        assert!(!w.field_key(&esc) && w.is_open(), "the window stays");
    }

    #[test]
    fn windowed_the_inputs_shortcut_opens_and_closes_the_input_only() {
        assert_eq!(toggle(false, true, false, true), Toggle::ShowInput);
        assert_eq!(toggle(true, true, false, true), Toggle::HideInput);
        assert_eq!(toggle(false, true, false, false), Toggle::ShowChat, "in the side panel, as before");
    }

    #[test]
    fn a_sent_message_gets_a_reply_fed_by_its_token() {
        let mut c = Chat::new(None);
        let id = sent(&mut c, "hi");
        let token = c.ask(id).expect("a reply");
        assert!(c.streaming());
        c.reply_event(token, Event::Text("Hello".into()), Instant::now());
        c.reply_event(token + 7, Event::Text(" stray".into()), Instant::now());
        let r = c.reply_to(id).expect("its reply");
        assert_eq!((r.prompt.as_str(), texts(r).as_str()), ("hi", "Hello"));
        c.reply_event(token, Event::Done, Instant::now());
        assert!(!c.streaming());
        assert_eq!(c.ask(99), None, "no such message");
    }

    #[test]
    fn sending_again_or_retrying_stops_the_reply_streaming_and_its_old_events_go_nowhere() {
        let mut c = Chat::new(None);
        let first = sent(&mut c, "one");
        let old = c.ask(first).unwrap();
        let second = sent(&mut c, "two");
        let token = c.ask(second).unwrap();
        assert_eq!(c.reply_to(first).map(|r| r.state), Some(reply::State::Stopped));
        c.stop(Instant::now());
        assert_eq!(c.reply_to(second).map(|r| r.state), Some(reply::State::Stopped));
        let again = c.retry(token).expect("retried");
        c.reply_event(token, Event::Text("stale".into()), Instant::now());
        c.reply_event(old, Event::Text("stale".into()), Instant::now());
        c.reply_event(again, Event::Text("fresh".into()), Instant::now());
        let r = c.reply_to(second).unwrap();
        assert_eq!((r.state, texts(r).as_str()), (reply::State::Streaming, "fresh"), "one reply per message, started over");
    }

    #[test]
    fn starting_over_drops_the_replies_and_stale_streams_stay_out() {
        let mut c = Chat::new(None);
        let id = sent(&mut c, "one");
        let old = c.ask(id).unwrap();
        c.start_over();
        let id = sent(&mut c, "new");
        c.reply_event(old, Event::Text("stale".into()), Instant::now());
        assert!(c.reply_to(id).is_none(), "the old stream feeds nothing");
        assert_eq!(c.replies.len(), 0);
    }

    #[test]
    fn answering_a_question_sends_the_answer_and_gets_its_reply() {
        let mut c = Chat::new(None);
        let id = sent(&mut c, "ask");
        let token = c.ask(id).unwrap();
        let ask = Event::Ask { prompt: "Which?".into(), options: vec!["Tabs".into(), "Page".into()], multi: false };
        c.reply_event(token, ask, Instant::now());
        c.reply_event(token, Event::Done, Instant::now());
        assert_eq!(c.answer(token, 0), None, "nothing picked");
        c.pick(token, 0, 1);
        let next = c.answer(token, 0).expect("answered");
        assert_eq!(c.messages.last().map(|m| m.text.as_str()), Some("Page"));
        let r = c.replies.iter().find(|r| r.token == next).expect("its reply");
        assert!(r.answer && r.streaming());
        assert_eq!(c.answer(token, 0), None, "once");
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
        assert_eq!(c.take_paste(&clip("just text", None)), vec![]);
        assert_eq!(c.take_paste(&clip("", Some(a_png()))), vec![(1, Pasted::Png(a_png()))]);
        assert_eq!(c.take_paste(&clip("", Some(a_png()))).first().map(|(id, _)| *id), Some(2));
    }

    #[test]
    fn image_files_copied_in_finder_paste_as_images_and_other_files_as_none() {
        let files = |names: &[&str]| clip("names", Some(a_png())).with_files(names.iter().map(std::path::PathBuf::from));
        let file = |n: &str| Pasted::File(n.into());
        assert_eq!(pasted_images(&files(&["/a/one.PNG", "/a/notes.txt", "/a/two.jpeg", "/a/3.jpg"])), vec![file("/a/one.PNG"), file("/a/two.jpeg"), file("/a/3.jpg")], "the files, not Finder's icon");
        assert_eq!(pasted_images(&files(&["/a/notes.txt"])), vec![], "not the icon of a file that isn't an image");
        assert_eq!(pasted_images(&clip("", Some(a_png()))), vec![Pasted::Png(a_png())]);
        assert_eq!(pasted_images(&clip("text", None)), vec![]);
        let mut next = 0;
        assert_eq!(take_ids(&mut next, vec![file("/a"), file("/b")]), vec![(1, file("/a")), (2, file("/b"))], "one chip each");
    }

    #[test]
    fn too_large_an_image_file_is_not_read() {
        let dir = std::env::temp_dir().join(format!("ailfred-large-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, a_png()).unwrap();
        let len = a_png().len() as u64;
        assert_eq!(image_file_png_within(&png, len - 1, 100), None, "too many bytes");
        assert_eq!(image_file_png_within(&png, len, 1), None, "too many pixels (2)");
        assert!(image_file_png_within(&png, len, 2).is_some());
        let jpeg = dir.join("a.jpg");
        std::fs::write(&jpeg, include_bytes!("testdata/tiny.jpg")).unwrap();
        assert_eq!(image_file_png_within(&jpeg, 1 << 20, 11), None, "too many pixels (4x3)");
        assert!(image_file_png_within(&jpeg, 1 << 20, 12).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_image_file_is_read_as_a_png() {
        let dir = std::env::temp_dir().join(format!("ailfred-paste-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, a_png()).unwrap();
        assert_eq!(image_file_png(&png), Some(a_png()), "a PNG as it is");
        let jpeg = dir.join("a.jpg");
        std::fs::write(&jpeg, include_bytes!("testdata/tiny.jpg")).unwrap();
        let converted = image_file_png(&jpeg).expect("a JPEG converted");
        let (w, h, _) = decode_png(&converted).unwrap();
        assert_eq!((w, h), (4, 3));
        let text = dir.join("fake.png");
        std::fs::write(&text, "not an image").unwrap();
        assert_eq!(image_file_png(&text), None);
        assert_eq!(image_file_png(&dir.join("missing.png")), None);
        std::fs::remove_dir_all(&dir).unwrap();
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
        assert_eq!(toggle(false, false, false, false), Toggle::ShowInput, "no chat yet");
        assert_eq!(toggle(true, false, false, false), Toggle::HideInput);
        assert_eq!(toggle(true, true, true, false), Toggle::HideInput, "an open input closes first");
        assert_eq!(toggle(false, true, false, false), Toggle::ShowChat, "the chat instead of the input");
        assert_eq!(toggle(false, true, true, false), Toggle::HideChat);
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
