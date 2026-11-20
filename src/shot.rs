//! The overlay: the frozen display, a drag to select an area, then annotation with a toolbar.
//! Enter or ⌘C copies and saves (export.rs); Esc cancels.

use std::sync::Arc;
use std::time::Duration;

use rune::{Anim, AppHandle, Color, Curve, Cursor, Cx, Drag, El, Key, KeyEvent, LineCap, Phase, Pixels, Shadow, Shape, TextStyle, Tx, canvas, col, el, row, text};

use crate::Host;
use crate::capture::Shot;
use crate::export::{self, Frozen};
use crate::marks::{self, Ink, Mark, Style, Tool};

/// Outside the selection; and over the whole frozen screen while the quick input waits for a
/// drag, so the frozen screen reads as a mode.
const DIM: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.5 };
const IDLE_DIM: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.2 };
/// The hint's and toolbar's own background, #161618 (Rune colours are linear): they sit on the
/// screen, so they must read on white.
const PANEL: Color = Color { r: 0.008, g: 0.008, b: 0.009, a: 0.92 };
const EDGE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.9 };
const ACTIVE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.18 };
/// The rune colour of an ink.
fn color_of(ink: Ink) -> Color {
    let [r, g, b] = ink.rgb();
    Color { r, g, b, a: 1.0 }
}

/// The label beside the pointer in a capture for the quick input or the chat, its size (points, about,
/// until it's laid out), and its gap to the pointer.
const CURSOR_HINT: &str = "Drag to take a screenshot";
const LABEL: (f32, f32) = (160.0, 22.0);
const LABEL_GAP: f32 = 16.0;

/// A drag smaller than this (points) is a click, not a selection.
const MIN_SIDE: f32 = 3.0;
const TOOLBAR_H: f32 = 40.0;
/// The colour palette's height and its gap to the toolbar.
const PALETTE_H: f32 = 36.0;
const PALETTE_GAP: f32 = 6.0;

/// How a capture starts: drag an area, pick a window, or take the whole display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Area,
    Window,
    FullScreen,
}

/// Choosing the selection, before annotating.
enum Pick {
    /// Nothing yet.
    Idle,
    /// The area being dragged (x, y, width, height in points).
    Dragging([f32; 4]),
    /// Window mode: the window under the pointer, if any.
    Window(Option<[f32; 4]>),
}

/// Where the overlay is in a capture.
enum Stage {
    Selecting(Pick),
    /// A selection being annotated.
    Editing(Edit),
}

struct Edit {
    sel: [f32; 4],
    tool: Tool,
    /// Colour and size for the next marks (and text being typed).
    style: Style,
    /// The colour palette is open.
    picking: bool,
    marks: Vec<Mark>,
    /// The pixelated preview of each mark (Some for Pixelate marks), index-aligned with `marks`.
    previews: Vec<Option<Pixels>>,
    /// The mark being dragged.
    draft: Option<Mark>,
    /// Text being typed: where, and what so far.
    typing: Option<([f32; 2], String)>,
}

impl Edit {
    fn new(sel: [f32; 4]) -> Edit {
        Edit { sel, tool: Tool::Arrow, style: Style::default(), picking: false, marks: Vec::new(), previews: Vec::new(), draft: None, typing: None }
    }

    fn push(&mut self, mark: Mark, preview: Option<Pixels>) {
        self.marks.push(mark);
        self.previews.push(preview);
    }

    /// Ends typing, keeping the text if there is any.
    fn finish_typing(&mut self) {
        if let Some((at, text)) = self.typing.take() {
            let mark = Mark::Text { at, text, style: self.style };
            if mark.is_meaningful() {
                self.push(mark, None);
            }
        }
    }

    fn undo(&mut self) {
        // Undo drops text being typed; an empty caret just goes, and the undo reaches the last mark.
        if let Some((_, typed)) = self.typing.take() {
            if !typed.trim().is_empty() {
                return;
            }
        }
        self.marks.pop();
        self.previews.pop();
    }

    /// Everything to burn in, including text still being typed.
    fn all_marks(&self) -> Vec<Mark> {
        let mut all = self.marks.clone();
        if let Some((at, text)) = &self.typing {
            let mark = Mark::Text { at: *at, text: text.clone(), style: self.style };
            if mark.is_meaningful() {
                all.push(mark);
            }
        }
        all
    }
}

/// The overlay's state. An overlay drops `Tx::close_window`/`Tx::quit` from its handlers (it has
/// no window to ask), so hiding and quitting go through the host.
pub struct Overlay {
    host: AppHandle<Host>,
    /// What the overlay draws: the frozen display.
    pixels: Pixels,
    /// Shared with the export thread, which crops it after the overlay hides.
    frozen: Option<Arc<Frozen>>,
    stage: Stage,
    /// Other apps' windows on the frozen display, front to back (display-local points).
    windows: Vec<[f32; 4]>,
    /// The pointer's last known place (display-local points).
    pointer: [f32; 2],
    /// Set by confirm/dismiss until the queued clear lands: input in between is ignored, so a held
    /// Enter (or Enter plus a Copy click) can't export twice.
    closing: bool,
    /// The quick input or the chat was open as the capture started: a label follows the pointer
    /// and their panel is left alone. Enter still copies and saves; pasting is up to the user.
    for_input: bool,
    /// For the chat (`input_at` is its panel, which stays put): the input doesn't tuck.
    for_chat: bool,
    /// Where the quick input's or the chat's panel is over this display (points), while it's on
    /// screen: this overlay leaves that rect to it, pointer and cursor alike.
    input_at: Option<[f32; 4]>,
    /// Fading out (`leave`): everything drawn goes to transparent, so the window's last frame
    /// before it hides is empty and never shows again on the next reveal.
    leaving: bool,
}

/// Who a capture is for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum For {
    /// Itself: Enter copies and saves.
    Nothing,
    /// The quick input, waiting at the bottom over a Dock `dock` points tall, with `chips`
    /// images in its strip.
    Input { dock: f32, chips: usize },
    /// The chat, its panel at `panel` (points on the display).
    Chat { panel: [f32; 4] },
}

/// The capture fades in over this as it shows.
pub const FADE_IN: Duration = Duration::from_millis(180);
/// And out over this as it goes (its content's opacity, then the window hides): the quick
/// input's slide out, as they often leave together.
pub const FADE_OUT: Duration = crate::quick_input::EXIT;

impl Overlay {
    pub fn new(host: AppHandle<Host>) -> Self {
        Overlay {
            host,
            pixels: Pixels::new(),
            frozen: None,
            stage: Stage::Selecting(Pick::Idle),
            windows: Vec::new(),
            pointer: [0.0, 0.0],
            closing: false,
            for_input: false,
            for_chat: false,
            input_at: None,
            leaving: false,
        }
    }

    /// The overlay fades out (`Host::hide_overlay` hides and clears it once it's transparent):
    /// input is ignored now.
    pub fn leave(&mut self) {
        self.closing = true;
        self.leaving = true;
    }

    /// Shows a new capture in `mode`, for `target`; any previous selection and marks are dropped.
    pub fn freeze(&mut self, shot: Shot, mode: Mode, target: For) {
        self.leaving = false;
        self.pixels.set(shot.width, shot.height, shot.rgba.clone());
        let frozen = Frozen { width: shot.width, height: shot.height, scale: shot.scale, rgba: shot.rgba };
        let full = [0.0, 0.0, frozen.width as f32 / frozen.scale, frozen.height as f32 / frozen.scale];
        self.frozen = Some(Arc::new(frozen));
        (self.for_input, self.for_chat, self.input_at) = served(target, self.screen());
        self.windows = shot.windows;
        self.pointer = shot.pointer;
        self.closing = false;
        match mode {
            Mode::Area => self.stage = Stage::Selecting(Pick::Idle),
            Mode::Window => self.stage = Stage::Selecting(Pick::Window(window_at(&self.windows, self.pointer))),
            Mode::FullScreen => self.pick(full),
        }
    }

    /// An area (or window, or the display) is chosen: annotating starts, the toolbar out. The
    /// quick input stays out of view until the capture closes, the whole screen now taking the
    /// pointer.
    fn pick(&mut self, sel: [f32; 4]) {
        self.stage = Stage::Editing(Edit::new(sel));
        self.input_at = hole_on_pick(self.for_chat, self.input_at);
        if self.for_input && !self.for_chat {
            self.host.update(|host, _| host.tuck_quick_input(true));
        }
    }

    /// The frozen display's size in points.
    fn screen(&self) -> Option<[f32; 2]> {
        let f = self.frozen.as_ref()?;
        Some([f.width as f32 / f.scale, f.height as f32 / f.scale])
    }

    /// The whole display as a selection.
    fn full_display(&self) -> Option<[f32; 4]> {
        let f = self.frozen.as_ref()?;
        Some([0.0, 0.0, f.width as f32 / f.scale, f.height as f32 / f.scale])
    }

    /// Fades the overlay out (`Host::hide_overlay`), which then frees the capture (`leave`). A
    /// capture shown meanwhile cancels the fade and is never cleared by it.
    fn dismiss(&mut self) {
        self.closing = true;
        self.host.update(|host, _| host.hide_overlay());
    }

    /// Drops the frozen capture (the export thread keeps its own reference while it runs).
    pub(crate) fn clear(&mut self) {
        self.frozen = None;
        self.stage = Stage::Selecting(Pick::Idle);
        self.windows = Vec::new();
        self.closing = false;
        self.for_input = false;
        self.for_chat = false;
        self.input_at = None;
        self.leaving = false;
        self.pixels.set(1, 1, vec![0; 4]);
    }

    /// Enter: copies and saves the selection with its marks (off the UI thread), then closes.
    /// With the input or chat open too: it's a capture of its own, pasted there only if wanted.
    fn confirm(&mut self) {
        if self.closing {
            return;
        }
        let Stage::Editing(edit) = &self.stage else { return };
        if let Some(frozen) = self.frozen.clone() {
            let (sel, all, host) = (edit.sel, edit.all_marks(), self.host.clone());
            std::thread::spawn(move || export::export(frozen, sel, all, host));
        }
        self.dismiss();
    }

    fn set_tool(&mut self, tool: Tool) {
        if let Stage::Editing(edit) = &mut self.stage {
            edit.finish_typing();
            edit.draft = None;
            edit.tool = tool;
        }
    }

    /// Picks a colour from the palette, which then closes.
    fn set_ink(&mut self, ink: Ink) {
        if let Stage::Editing(edit) = &mut self.stage {
            edit.style.ink = ink;
            edit.picking = false;
        }
    }

    fn toggle_palette(&mut self) {
        if let Stage::Editing(edit) = &mut self.stage {
            edit.picking = !edit.picking;
        }
    }

    fn next_size(&mut self) {
        if let Stage::Editing(edit) = &mut self.stage {
            edit.style.size = edit.style.size.next();
        }
    }

    fn undo(&mut self) {
        if let Stage::Editing(edit) = &mut self.stage {
            edit.undo();
        }
    }

    /// A pixelated preview of `rect` (points, relative to the selection), as the export makes it.
    /// Returned empty at once and filled from a background thread (`Pixels::set` works from any
    /// thread), so a large area doesn't stall the UI.
    fn pixelated(&self, sel: [f32; 4], rect: [f32; 4]) -> Option<Pixels> {
        let frozen = self.frozen.clone()?;
        let pixels = Pixels::new();
        let target = pixels.clone();
        std::thread::spawn(move || {
            if let Some((w, h, region)) = pixelate_region(&frozen, sel, rect) {
                target.set(w, h, region);
            }
        });
        Some(pixels)
    }
}

/// The pixels under `rect` (points, relative to `sel`), pixelated with the export's block size.
fn pixelate_region(f: &Frozen, sel: [f32; 4], rect: [f32; 4]) -> Option<(u32, u32, Vec<u8>)> {
    let px = |v: f32| (v * f.scale).round().max(0.0) as usize;
    let (x0, y0) = (px(sel[0] + rect[0]).min(f.width as usize), px(sel[1] + rect[1]).min(f.height as usize));
    let (x1, y1) = (px(sel[0] + rect[0] + rect[2]).min(f.width as usize), px(sel[1] + rect[1] + rect[3]).min(f.height as usize));
    let (w, h) = (x1.checked_sub(x0)?, y1.checked_sub(y0)?);
    if w == 0 || h == 0 {
        return None;
    }
    let mut region = Vec::with_capacity(w * h * 4);
    for y in y0..y1 {
        let start = (y * f.width as usize + x0) * 4;
        region.extend_from_slice(&f.rgba[start..start + w * 4]);
    }
    marks::pixelate(&mut region, w, h, [0, 0, w, h], px(marks::BLOCK).max(1));
    Some((w as u32, h as u32, region))
}

pub fn view(s: &Overlay, cx: &mut Cx<Overlay>) -> El {
    crate::settings::apply_motion(cx);
    let Some(f) = &s.frozen else { return el() };
    // The overlay covers the display's frame, so its size in points is the capture over its scale.
    let (sw, sh) = (f.width as f32 / f.scale, f.height as f32 / f.scale);
    let mut root = el()
        .w_full()
        .h_full()
        .opacity(content_alpha(s.leaving))
        .opacity_motion(Anim::new(Curve::ease_in_out(FADE_OUT.as_secs_f32())))
        .pixels(&s.pixels)
        .cursor(Cursor::Crosshair)
        .on_drag(on_drag)
        .on_pointer_move_local(on_pointer_move);
    // What takes the pointer (and shows the crosshair): all of it but the quick input's panel,
    // whose own cursor would otherwise flicker against this one. Behind everything else.
    for (i, [x, y, w, h]) in hit_regions(s.input_at, [sw, sh]).into_iter().enumerate() {
        root = root.child(el().key(format!("hit-{i}")).size(w, h).hit().pin(Some(x), Some(y), None, None));
    }
    let hint = |t: &str| {
        el().padding(10.0).radius(8.0).fill(PANEL).pin(Some(24.0), Some(24.0), None, None).child(text(t.to_string()).color(EDGE))
    };
    match &s.stage {
        // Before a drag the frozen screen shows as is, only the hint on top; the dim starts with the
        // drag (its first event creates the selection).
        // For the quick input or the chat: one label beside the pointer, following it.
        Stage::Selecting(Pick::Idle) if s.for_input => {
            let size = cx.layout_size("cursor-hint").map_or(LABEL, |[w, h]| (w, h));
            let [x, y] = label_at(s.pointer, [sw, sh], size);
            root.child(el().key("idle-dim").w_full().h_full().fill(IDLE_DIM).pin(Some(0.0), Some(0.0), None, None)).child(
                el().key("cursor-hint")
                    .px(8.0)
                    .h(LABEL.1)
                    .center_y()
                    .radius(6.0)
                    .continuous_corners()
                    .fill(PANEL)
                    .border(1.0, Color::WHITE.alpha(0.14))
                    .box_shadow(Shadow::core_animation(0.0, 2.0, 8.0, Color::BLACK.alpha(0.3)))
                    .pin(Some(x), Some(y), None, None)
                    .child(text(CURSOR_HINT).font(TextStyle::new(11.5, 500.0)).color(EDGE)),
            )
        }
        Stage::Selecting(Pick::Idle) => root.child(hint("Drag to select · Space: window · F: full screen · Esc: cancel")),
        Stage::Selecting(Pick::Window(None)) => root.child(hint("Click a window · Space: area · F: full screen · Esc: cancel")),
        Stage::Selecting(Pick::Window(Some(win))) => {
            let [x, y, w, h] = *win;
            shades(root, *win, sw, sh)
                .child(el().key("window").size(w, h).border(2.0, EDGE).fill(Color { a: 0.08, ..EDGE }).pin(Some(x), Some(y), None, None))
                .child(hint("Click or Enter to capture this window · Space: area · F: full screen · Esc: cancel"))
        }
        Stage::Selecting(Pick::Dragging(sel)) => {
            let mut root = shades(root, *sel, sw, sh);
            let [x, y, w, h] = *sel;
            // The border and size only once there's an area: not a 0 × 0 box on press.
            if w >= MIN_SIDE && h >= MIN_SIDE {
                root = root
                    .child(el().key("selection").size(w, h).border(1.0, EDGE).pin(Some(x), Some(y), None, None))
                    .child(
                        text(format!("{} × {}", (w * f.scale).round(), (h * f.scale).round()))
                            .color(EDGE)
                            .pin(Some(x), Some((y + h + 6.0).min(sh - 20.0)), None, None),
                    );
            }
            root
        }
        Stage::Editing(edit) => {
            let [x, y, w, h] = edit.sel;
            let root = shades(root, edit.sel, sw, sh)
                .child(el().key("selection").size(w, h).border(1.0, EDGE).pin(Some(x), Some(y), None, None))
                .child(marks_layer(edit).pin(Some(x), Some(y), None, None));
            let top = bar_top(edit.sel, sh, s.input_at);
            root.child(toolbar(edit, top, sw, sh, cx.layout_size("toolbar").map(|[w, _]| w)))
        }
    }
}

/// The capture's opacity: all of it, or none while it fades out.
fn content_alpha(leaving: bool) -> f32 {
    if leaving { 0.0 } else { 1.0 }
}

/// The overlay's input regions on a `screen`: all of it, or (`hole`, the quick input's panel)
/// the bands above, below, left and right of the hole.
fn hit_regions(hole: Option<[f32; 4]>, screen: [f32; 2]) -> Vec<[f32; 4]> {
    let [sw, sh] = screen;
    let Some([x, y, w, h]) = hole else { return vec![[0.0, 0.0, sw, sh]] };
    let (top, bottom) = (y.clamp(0.0, sh), (y + h).clamp(0.0, sh));
    let (left, right) = (x.clamp(0.0, sw), (x + w).clamp(0.0, sw));
    vec![[0.0, 0.0, sw, top], [0.0, bottom, sw, sh - bottom], [0.0, top, left, bottom - top], [right, top, sw - right, bottom - top]]
}

/// Dims everything outside `sel`: above, below, left and right of it.
fn shades(root: El, [x, y, w, h]: [f32; 4], sw: f32, sh: f32) -> El {
    let shade = |px: f32, py: f32, pw: f32, ph: f32| el().size(pw.max(0.0), ph.max(0.0)).fill(DIM).pin(Some(px), Some(py), None, None);
    root.child(shade(0.0, 0.0, sw, y))
        .child(shade(0.0, y + h, sw, sh - y - h))
        .child(shade(0.0, y, x, h))
        .child(shade(x + w, y, sw - x - w, h))
}

/// The marks, in the selection's own points.
fn marks_layer(edit: &Edit) -> El {
    let [_, _, w, h] = edit.sel;
    // Clipped to the selection, as the export crops: nothing shows outside what gets saved.
    let mut layer = el().size(w, h).clip_rounded(0.0);
    let mut shape = Shape::new();
    let draft = edit.draft.iter().map(|m| (m, None));
    let all: Vec<(&Mark, Option<&Pixels>)> = edit.marks.iter().zip(edit.previews.iter().map(Option::as_ref)).chain(draft).collect();
    // The export's order: pixelation first, then boxes and text as drawn, arrows on top.
    let pixelate_first = all.iter().filter(|(m, _)| matches!(m, Mark::Pixelate(_))).chain(all.iter().filter(|(m, _)| !matches!(m, Mark::Pixelate(_))));
    for (mark, preview) in pixelate_first {
        match mark {
            Mark::Pixelate([px, py, pw, ph]) => {
                let area = el().size(*pw, *ph).pin(Some(*px), Some(*py), None, None);
                layer = layer.child(match preview {
                    Some(pixels) => area.pixels(pixels),
                    None => area.border(1.0, EDGE), // still being dragged
                });
            }
            Mark::Rect([rx, ry, rw, rh], style) => {
                let border = el().size(*rw, *rh).border(style.size.stroke(), color_of(style.ink));
                layer = layer.child(border.pin(Some(*rx), Some(*ry), None, None));
            }
            Mark::Arrow { from, to, style } => {
                let (stroke, ink) = (style.size.stroke(), color_of(style.ink));
                let (base, head) = marks::arrow_geometry(*from, *to, stroke);
                shape = shape.line(*from, base, stroke, LineCap::Round, ink).fill(head.to_vec(), ink);
            }
            Mark::Text { at, text: t, style } => layer = layer.child(text_mark(*at, t, *style)),
        }
    }
    if let Some((at, t)) = &edit.typing {
        layer = layer.child(text_mark(*at, &format!("{t}|"), edit.style));
    }
    layer.child(canvas(shape).size(w, h).pin(Some(0.0), Some(0.0), None, None))
}

fn text_mark(at: [f32; 2], t: &str, style: Style) -> El {
    text(t.to_string()).font(TextStyle::new(style.size.text(), 700.0)).color(color_of(style.ink)).pin(Some(at[0]), Some(at[1]), None, None)
}

/// The tools and actions, under the selection (above it, or inside, when there's no room),
/// right-aligned with it.
/// `laid_out`: the bar's width from the last layout (None on its first frame).
fn toolbar(edit: &Edit, top: f32, sw: f32, sh: f32, laid_out: Option<f32>) -> El {
    let [x, y, w, _] = edit.sel;
    // Right-aligned with the selection, but never pushed off the left edge: the bar's real width
    // once laid out, a generous estimate before.
    let bar_w = laid_out.unwrap_or(720.0);
    let right = (sw - (x + w)).max(8.0).min((sw - bar_w - 8.0).max(8.0));
    let button = |label: String, active: bool| {
        el().key(format!("tool-{label}")).px(10.0).py(6.0).radius(6.0).fill(if active { ACTIVE } else { Color { a: 0.0, ..PANEL } })
            .child(text(label).color(EDGE))
    };
    let mut bar = row().key("toolbar").gap(2.0).padding(4.0).radius(10.0).fill(PANEL);
    for tool in Tool::ALL {
        let label = format!("{} {}", tool.label(), tool.key().to_uppercase());
        bar = bar.child(button(label, edit.tool == tool).on_press(move |s: &mut Overlay| s.set_tool(tool)));
    }
    // One circle in the current colour; clicking it opens the palette. Then the size (W cycles it).
    let ring = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
    let swatch = |ink: Ink| el().size(16.0, 16.0).radius(8.0).fill(color_of(ink)).border(1.5, ring);
    let current = el().key("ink").padding(6.0).radius(6.0).fill(if edit.picking { ACTIVE } else { Color { a: 0.0, ..PANEL } });
    bar = bar.child(current.child(swatch(edit.style.ink)).on_press(|s: &mut Overlay| s.toggle_palette()));
    bar = bar.child(button(format!("Size {} W", edit.style.size.label()), false).on_press(|s: &mut Overlay| s.next_size()));
    let bar = bar.child(button("Undo ⌘Z".into(), false).on_press(|s: &mut Overlay| s.undo()));
    let bar = bar
        .child(button("Cancel Esc".into(), false).on_press(|s: &mut Overlay| s.dismiss()))
        .child(button("Copy ↩".into(), true).on_press(|s: &mut Overlay| s.confirm()));
    if !edit.picking {
        return bar.pin(None, Some(top), Some(right), None);
    }
    // The palette sits on the side of the toolbar away from the selection; above it, the column
    // starts higher so the toolbar itself doesn't move.
    let mut palette = row().key("palette").gap(2.0).padding(4.0).radius(10.0).fill(PANEL);
    for ink in Ink::ALL {
        let cell = el().key(format!("ink-{ink:?}")).padding(6.0).radius(6.0).fill(if edit.style.ink == ink { ACTIVE } else { Color { a: 0.0, ..PANEL } });
        palette = palette.child(cell.child(swatch(ink)).on_press(move |s: &mut Overlay| s.set_ink(ink)));
    }
    // Away from the selection when it fits there, else the other side; the toolbar never moves.
    let extra = PALETTE_H + PALETTE_GAP;
    let (fits_above, fits_below) = (top - extra >= 0.0, top + TOOLBAR_H + extra <= sh);
    let palette_above = if top < y { fits_above || !fits_below } else { !fits_below && fits_above };
    // items_end: the palette keeps its own width, right-aligned with the toolbar.
    let column = col().gap(PALETTE_GAP).items_end();
    if palette_above {
        column.child(palette).child(bar).pin(None, Some(top - extra), Some(right), None)
    } else {
        column.child(bar).child(palette).pin(None, Some(top), Some(right), None)
    }
}

/// The toolbar's top: under the selection, else above it, else inside it; never where `avoid`
/// (the chat's panel) is.
fn bar_top([_, y, _, h]: [f32; 4], sh: f32, avoid: Option<[f32; 4]>) -> f32 {
    let clear = |top: f32| avoid.is_none_or(|[_, ay, _, ah]| top + TOOLBAR_H <= ay || top >= ay + ah);
    let (below, above) = (y + h + 8.0, y - TOOLBAR_H - 8.0);
    if below + TOOLBAR_H <= sh && clear(below) {
        below
    } else if above >= 0.0 && clear(above) {
        above
    } else {
        y + 8.0
    }
}

/// For `target`, on a `screen` of that size: whether it's for a field (the input's or the
/// chat's), whether for the chat, and the panel's rect it leaves alone.
fn served(target: For, screen: Option<[f32; 2]>) -> (bool, bool, Option<[f32; 4]>) {
    match target {
        For::Nothing => (false, false, None),
        For::Input { dock, chips } => (true, false, screen.map(|screen| crate::quick_input::input_panel(screen, dock, chips))),
        For::Chat { panel } => (true, true, Some(panel)),
    }
}

/// A picked area's toolbar and pointer: the chat keeps its panel's rect (`at`), the input tucks
/// away and leaves it to the capture.
fn hole_on_pick(for_chat: bool, at: Option<[f32; 4]>) -> Option<[f32; 4]> {
    at.filter(|_| for_chat)
}

fn on_drag(s: &mut Overlay, drag: &mut Drag, tx: &mut Tx) {
    tx.instant();
    if s.closing {
        return;
    }
    match &mut s.stage {
        Stage::Selecting(Pick::Window(_)) => window_click(s, drag),
        Stage::Selecting(_) => select_drag(s, drag),
        Stage::Editing(_) => edit_drag(s, drag),
    }
}

fn select_drag(s: &mut Overlay, drag: &Drag) {
    let was_dragging = matches!(s.stage, Stage::Selecting(Pick::Dragging(_)));
    let sel = marks::rect([drag.start_x, drag.start_y], [drag.x, drag.y]);
    let nothing = drag.cancelled() || (drag.phase == Phase::Up && (sel[2] < MIN_SIDE || sel[3] < MIN_SIDE));
    if nothing {
        // A click or a cancel, not an area.
        s.stage = Stage::Selecting(Pick::Idle);
    } else if drag.phase != Phase::Up {
        s.stage = Stage::Selecting(Pick::Dragging(sel));
    } else {
        s.pick(sel);
    }
    // A picked area keeps the quick input away until the capture closes (`pick`). The chat's
    // panel stays.
    let picked = s.for_input && matches!(s.stage, Stage::Editing(_));
    if let Some(tucked) = tuck_change(was_dragging, matches!(s.stage, Stage::Selecting(Pick::Dragging(_))))
        && !(picked && !tucked)
        && !s.for_chat
    {
        s.host.update(move |host, _| host.tuck_quick_input(tucked));
    }
}

/// The quick input leaves the screen as an area drag starts (Some(true)) and comes back as it
/// ends (Some(false)); None while nothing changes.
fn tuck_change(was_dragging: bool, dragging: bool) -> Option<bool> {
    (was_dragging != dragging).then_some(dragging)
}

/// The front-most window containing `p`.
fn window_at(windows: &[[f32; 4]], p: [f32; 2]) -> Option<[f32; 4]> {
    windows.iter().copied().find(|[x, y, w, h]| p[0] >= *x && p[0] < x + w && p[1] >= *y && p[1] < y + h)
}

/// Remembers the pointer; in window mode it highlights the window under it. Elsewhere nothing on
/// screen changes, so the view isn't rebuilt (`live_only`).
fn on_pointer_move(s: &mut Overlay, p: rune::PointerEvent, tx: &mut Tx) {
    s.pointer = [p.window_x, p.window_y];
    match &mut s.stage {
        Stage::Selecting(Pick::Window(hover)) => {
            let under = window_at(&s.windows, s.pointer);
            if *hover != under {
                *hover = under;
            }
        }
        // The pointer label follows at once, without a spring.
        Stage::Selecting(Pick::Idle) if s.for_input => tx.instant(),
        _ => tx.live_only(),
    }
}

/// Where the pointer label goes: below and right of `pointer`, flipped to the other side near the
/// right or bottom edge of a `screen` (points), so it stays on screen.
fn label_at(pointer: [f32; 2], screen: [f32; 2], size: (f32, f32)) -> [f32; 2] {
    let place = |p: f32, size: f32, max: f32| if p + LABEL_GAP + size <= max { p + LABEL_GAP } else { p - LABEL_GAP - size };
    [place(pointer[0], size.0, screen[0]), place(pointer[1], size.1, screen[1])]
}

/// Window mode: a click (released over a window) selects it.
fn window_click(s: &mut Overlay, drag: &Drag) {
    if drag.phase != Phase::Up || drag.cancelled() {
        return;
    }
    if let Some(win) = window_at(&s.windows, [drag.x, drag.y]) {
        s.pick(win);
    }
}

fn edit_drag(s: &mut Overlay, drag: &Drag) {
    let Stage::Editing(edit) = &mut s.stage else { return };
    let [sx, sy, sw, sh] = edit.sel;
    let local = |x: f32, y: f32| marks::clamp([x - sx, y - sy], sw, sh);
    // Only gestures that start inside the selection draw; a press outside just ends typing.
    let inside = |x: f32, y: f32| x >= sx && x <= sx + sw && y >= sy && y <= sy + sh;
    // Drawing (a press inside the selection) closes the palette; presses elsewhere, such as on
    // the toolbar's own colour button, leave it to that button.
    if drag.phase == Phase::Down && inside(drag.start_x, drag.start_y) {
        edit.picking = false;
    }
    if !inside(drag.start_x, drag.start_y) {
        if drag.phase == Phase::Down {
            edit.finish_typing();
        }
        edit.draft = None;
        return;
    }
    if edit.tool == Tool::Text {
        if drag.phase == Phase::Down {
            edit.finish_typing();
            edit.typing = Some((local(drag.x, drag.y), String::new()));
        }
        return;
    }
    if drag.cancelled() {
        edit.draft = None;
        return;
    }
    let mark = Mark::dragged(edit.tool, local(drag.start_x, drag.start_y), local(drag.x, drag.y), edit.style);
    if drag.phase != Phase::Up {
        edit.draft = mark;
        return;
    }
    edit.draft = None;
    let Some(mark) = mark.filter(Mark::is_meaningful) else { return };
    let (sel, preview_rect) = (edit.sel, if let Mark::Pixelate(r) = &mark { Some(*r) } else { None });
    let preview = preview_rect.and_then(|r| s.pixelated(sel, r));
    if let Stage::Editing(edit) = &mut s.stage {
        edit.push(mark, preview);
    }
}

pub fn on_key(s: &mut Overlay, event: &KeyEvent, _: &mut Tx) -> bool {
    if s.closing {
        return true;
    }
    // A chord with ⌘ or ⌃ is never typed text or a tool key.
    let chord = event.mods.cmd || event.mods.ctrl;
    if event.is_command('q') {
        s.host.quit();
        return true;
    }
    let Stage::Editing(edit) = &mut s.stage else {
        match &event.key {
            Key::Escape => s.dismiss(),
            // Space toggles window mode (not mid-drag); F takes the whole display.
            Key::Text(t) if t == " " && !chord => match s.stage {
                Stage::Selecting(Pick::Idle) => s.stage = Stage::Selecting(Pick::Window(window_at(&s.windows, s.pointer))),
                Stage::Selecting(Pick::Window(_)) => s.stage = Stage::Selecting(Pick::Idle),
                _ => {}
            },
            // Enter takes the highlighted window.
            Key::Enter => match s.stage {
                Stage::Selecting(Pick::Window(Some(win))) => s.pick(win),
                _ => return false,
            },
            Key::Text(t) if t.eq_ignore_ascii_case("f") && !chord => {
                if let Some(full) = s.full_display() {
                    s.pick(full);
                }
            }
            _ => return false,
        }
        return true;
    };
    // Typing a text mark takes the keys first.
    if let Some((_, typed)) = &mut edit.typing {
        match &event.key {
            Key::Text(t) if !chord => typed.push_str(t),
            Key::Backspace => {
                typed.pop();
            }
            Key::Enter | Key::Escape => edit.finish_typing(),
            _ if event.is_command('z') => edit.undo(),
            _ => return false,
        }
        return true;
    }
    match &event.key {
        // Esc closes the palette first, then cancels.
        Key::Escape if edit.picking => edit.picking = false,
        // For the quick input Esc is Done: the area stays attached.
        Key::Escape => s.dismiss(),
        Key::Enter => s.confirm(),
        Key::Text(t) if !chord && t.eq_ignore_ascii_case("w") => s.next_size(),
        Key::Text(t) if !chord => match Tool::from_key(t) {
            Some(tool) => s.set_tool(tool),
            None => return false,
        },
        _ if event.is_command('c') => s.confirm(),
        _ if event.is_command('z') => s.undo(),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_at_picks_the_front_most() {
        // Front to back: a small window on top of a big one.
        let windows = [[100.0, 100.0, 200.0, 100.0], [0.0, 0.0, 800.0, 600.0]];
        assert_eq!(window_at(&windows, [150.0, 150.0]), Some(windows[0]), "inside both: the front one");
        assert_eq!(window_at(&windows, [500.0, 500.0]), Some(windows[1]));
        assert_eq!(window_at(&windows, [900.0, 10.0]), None, "outside all");
        assert_eq!(window_at(&windows, [300.0, 150.0]), Some(windows[1]), "right edge is exclusive");
    }

    #[test]
    fn pixelate_region_crops_at_scale_and_averages() {
        // 8x4 px frozen display at scale 2 (4x2 pt); left half black, right half white.
        let mut rgba = Vec::new();
        for _y in 0..4 {
            for x in 0..8 {
                rgba.extend_from_slice(if x < 4 { &[0, 0, 0, 255] } else { &[255, 255, 255, 255] });
            }
        }
        let f = Frozen { width: 8, height: 4, scale: 2.0, rgba };
        // Selection at (1,0) pt, rect (0,0,2,2) pt -> px x 2..6, y 0..4; block 20 px covers it all.
        let (w, h, out) = pixelate_region(&f, [1.0, 0.0, 3.0, 2.0], [0.0, 0.0, 2.0, 2.0]).unwrap();
        assert_eq!((w, h), (4, 4));
        assert!(out.chunks(4).all(|p| p == [127, 127, 127, 255]), "one averaged block: {:?}", &out[0..4]);
        assert!(pixelate_region(&f, [3.5, 0.0, 1.0, 1.0], [0.5, 0.0, 0.0, 1.0]).is_none(), "empty area");
    }

    #[test]
    fn the_pointer_label_sits_beside_the_pointer_and_stays_on_screen() {
        let screen = [1000.0, 800.0];
        assert_eq!(label_at([100.0, 100.0], screen, LABEL), [100.0 + LABEL_GAP, 100.0 + LABEL_GAP], "below right");
        let [x, _] = label_at([990.0, 100.0], screen, LABEL);
        assert_eq!(x, 990.0 - LABEL_GAP - LABEL.0, "near the right edge: on the left");
        let [_, y] = label_at([100.0, 790.0], screen, LABEL);
        assert_eq!(y, 790.0 - LABEL_GAP - LABEL.1, "near the bottom: above");
        let [x, _] = label_at([800.0, 100.0], screen, (250.0, 22.0));
        assert_eq!(x, 800.0 - LABEL_GAP - 250.0, "its measured width decides the flip");
    }

    #[test]
    fn serving_the_chat_leaves_its_panel_to_it_and_the_input_waits_at_the_bottom() {
        let screen = [1920.0, 1080.0];
        let chat = [1508.0, 45.0, 400.0, 1023.0];
        assert_eq!(served(For::Chat { panel: chat }, Some(screen)), (true, true, Some(chat)));
        let bottom = crate::quick_input::bottom_panel(screen, 70.0);
        assert_eq!(served(For::Input { dock: 70.0, chips: 0 }, Some(screen)), (true, false, Some(bottom)));
        let grow = crate::quick_input::strip_h(2);
        let with_chips = [bottom[0], bottom[1] - grow, bottom[2], bottom[3] + grow];
        assert_eq!(served(For::Input { dock: 70.0, chips: 2 }, Some(screen)), (true, false, Some(with_chips)), "its strip of images too");
        assert_eq!(served(For::Nothing, Some(screen)), (false, false, None));
    }

    #[test]
    fn a_picked_area_leaves_the_chats_panel_to_it_and_the_tucked_inputs_to_the_capture() {
        let chat = [1508.0, 45.0, 400.0, 1023.0];
        assert_eq!(hole_on_pick(true, Some(chat)), Some(chat));
        assert_eq!(hole_on_pick(false, Some([650.0, 988.0, 620.0, 52.0])), None, "the input is out of view");
        assert_eq!(hole_on_pick(false, None), None);
    }



    #[test]
    fn leaving_fades_everything_drawn_to_transparent() {
        assert_eq!(content_alpha(false), 1.0);
        assert_eq!(content_alpha(true), 0.0, "the frame left behind when hidden is empty");
    }

    #[test]
    fn the_quick_inputs_panel_is_left_out_of_the_input_regions() {
        let screen = [1000.0, 800.0];
        assert_eq!(hit_regions(None, screen), vec![[0.0, 0.0, 1000.0, 800.0]]);
        let hole = [200.0, 600.0, 500.0, 52.0];
        let regions = hit_regions(Some(hole), screen);
        let inside = |[x, y]: [f32; 2]| regions.iter().any(|&r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]);
        assert!(!inside([450.0, 626.0]), "the panel's middle");
        assert!(!inside([200.0, 600.0]) && !inside([699.0, 651.0]), "its corners");
        for p in [[10.0, 10.0], [199.0, 626.0], [700.0, 626.0], [450.0, 599.0], [450.0, 652.0], [999.0, 799.0]] {
            assert!(inside(p), "{p:?} around it");
        }
        assert!(regions.iter().all(|r| r[2] >= 0.0 && r[3] >= 0.0));
        // A hole off the screen's edge leaves no negative bands.
        assert!(hit_regions(Some([-20.0, 780.0, 100.0, 60.0]), screen).iter().all(|r| r[2] >= 0.0 && r[3] >= 0.0));
    }

    #[test]
    fn the_panel_colour_is_the_srgb_one_in_linear() {
        let want = rune::color::hex("#161618").unwrap();
        assert!([PANEL.r - want.r, PANEL.g - want.g, PANEL.b - want.b].iter().all(|d| d.abs() < 0.002), "{PANEL:?}");
    }

    #[test]
    fn the_toolbar_keeps_clear_of_the_panel() {
        let (sel, screen) = ([500.0, 100.0, 900.0, 500.0], [1920.0, 1080.0]);
        assert_eq!(bar_top(sel, screen[1], None), 608.0, "under the selection");
        let panel = [500.0, 616.0, 620.0, 52.0];
        assert_eq!(bar_top(sel, screen[1], Some(panel)), 100.0 - TOOLBAR_H - 8.0, "a panel under it too: the bar goes above");
        // A panel beside it (right): under it is free.
        let sel = [100.0, 100.0, 400.0, 300.0];
        assert_eq!(bar_top(sel, screen[1], Some([516.0, 224.0, 620.0, 52.0])), 408.0);
    }

    #[test]
    fn the_quick_input_tucks_for_an_area_drag_only() {
        assert_eq!(tuck_change(false, true), Some(true), "drag starts: out of the screen");
        assert_eq!(tuck_change(true, true), None, "drag moves: nothing to do");
        assert_eq!(tuck_change(true, false), Some(false), "drag ends or is cancelled: back");
        assert_eq!(tuck_change(false, false), None);
    }
}
