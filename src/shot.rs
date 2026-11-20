//! The overlay: the frozen display, a drag to select an area, then annotation with a toolbar.
//! Enter or ⌘C copies and saves (export.rs); Esc cancels.

use std::sync::Arc;

use rune::{AppHandle, Color, Cursor, Cx, Drag, El, Key, KeyEvent, LineCap, Phase, Pixels, Shadow, Shape, TextStyle, Tx, canvas, col, el, icon, row, text};

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
/// The edit button's size and its gap to the selection.
const EDIT_BUTTON: f32 = 28.0;
const EDIT_GAP: f32 = 6.0;
const EDGE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.9 };
const ACTIVE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.18 };
/// The rune colour of an ink.
fn color_of(ink: Ink) -> Color {
    let [r, g, b] = ink.rgb();
    Color { r, g, b, a: 1.0 }
}

/// The label beside the pointer when the quick input starts a capture, its size (points, about,
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
    /// Which capture area this is (the quick input's chip for it carries the same id).
    id: u64,
    /// The toolbar is out. For the quick input it waits behind an edit button.
    tools: bool,
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
    fn new(id: u64, sel: [f32; 4], tools: bool) -> Edit {
        Edit { id, tools, sel, tool: Tool::Arrow, style: Style::default(), picking: false, marks: Vec::new(), previews: Vec::new(), draft: None, typing: None }
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
    /// For the quick input (open as the capture started): a label follows the pointer, the keys
    /// stay with the input, and each picked area attaches to it at once, tools tucked away.
    for_input: bool,
    /// The last capture area's id.
    last_id: u64,
    /// The frozen display (its CGDirectDisplayID), where the quick input goes beside an area.
    display: u32,
    /// The picked area a new drag started over, back if that drag comes to nothing.
    kept: Option<Edit>,
    /// Where the quick input's panel is over this display (points), while it's open: this
    /// overlay leaves that rect to it, pointer and cursor alike.
    input_at: Option<[f32; 4]>,
}

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
            last_id: 0,
            display: 0,
            kept: None,
            input_at: None,
        }
    }

    /// The quick input opened this capture: its panel waits at the bottom, over a Dock `dock`
    /// points tall.
    pub fn input_at_bottom(&mut self, dock: f32) {
        self.input_at = self.screen().map(|screen| crate::quick_input::bottom_panel(screen, dock));
    }

    /// Shows a new capture in `mode`; any previous selection and marks are dropped. `for_input`:
    /// the quick input is open.
    pub fn freeze(&mut self, shot: Shot, mode: Mode, for_input: bool) {
        self.for_input = for_input;
        self.input_at = None;
        self.pixels.set(shot.width, shot.height, shot.rgba.clone());
        let frozen = Frozen { width: shot.width, height: shot.height, scale: shot.scale, rgba: shot.rgba };
        let full = [0.0, 0.0, frozen.width as f32 / frozen.scale, frozen.height as f32 / frozen.scale];
        self.frozen = Some(Arc::new(frozen));
        self.windows = shot.windows;
        self.display = shot.display_id;
        self.pointer = shot.pointer;
        self.closing = false;
        match mode {
            Mode::Area => self.stage = Stage::Selecting(Pick::Idle),
            Mode::Window => self.stage = Stage::Selecting(Pick::Window(window_at(&self.windows, self.pointer))),
            Mode::FullScreen => self.pick(full),
        }
    }

    /// An area (or window, or the display) is chosen: annotating starts. For the quick input it
    /// attaches at once, the input moves beside it, and the tools wait behind a button; otherwise
    /// the toolbar is out and the capture takes the keys.
    fn pick(&mut self, sel: [f32; 4]) {
        self.last_id += 1;
        self.kept = None;
        self.stage = Stage::Editing(Edit::new(self.last_id, sel, !self.for_input));
        if !self.for_input {
            self.host.update(|host, _| host.capture_keys());
            return;
        }
        self.attach();
        let Some(screen) = self.screen() else { return };
        self.input_at = Some(crate::quick_input::beside(sel, screen));
        let (id, display) = (self.last_id, self.display);
        self.host.update(move |host, _| host.place_quick_input_beside(id, display, sel, screen));
    }

    /// Sends the current area, with its marks, to the quick input (replacing its earlier image).
    fn attach(&self) {
        let (Stage::Editing(edit), Some(frozen)) = (&self.stage, self.frozen.clone()) else { return };
        let (id, sel, all, host) = (edit.id, edit.sel, edit.all_marks(), self.host.clone());
        std::thread::spawn(move || export::attach(frozen, sel, all, id, host));
    }

    /// The frozen display's size in points.
    fn screen(&self) -> Option<[f32; 2]> {
        let f = self.frozen.as_ref()?;
        Some([f.width as f32 / f.scale, f.height as f32 / f.scale])
    }

    /// The quick input: shows the tools for the area, giving the capture the keys.
    fn open_tools(&mut self) {
        if let Stage::Editing(edit) = &mut self.stage {
            edit.tools = true;
            self.host.update(|host, _| {
                host.capture_keys();
                host.quick_editing(true);
            });
        }
    }

    /// ⌘E in the quick input: the tools open, or close as Done does.
    pub(crate) fn toggle_tools(&mut self) {
        match &self.stage {
            Stage::Editing(edit) if edit.tools => self.close_tools(),
            Stage::Editing(_) => self.open_tools(),
            Stage::Selecting(_) => {}
        }
    }

    /// The keys go back to the quick input, which stops showing the tools have them.
    fn give_back_keys(&self) {
        self.host.update(|host, _| {
            host.quick_editing(false);
            host.input_keys();
        });
    }

    /// The quick input: done annotating. The marks go into its image, the tools away, and the
    /// keys back to the input; the area stays on screen.
    fn close_tools(&mut self) {
        let Stage::Editing(edit) = &mut self.stage else { return };
        edit.finish_typing();
        edit.draft = None;
        edit.picking = false;
        edit.tools = false;
        self.attach();
        self.give_back_keys();
    }

    /// The quick input dropped area `id`'s image (its X, or Esc): if that area is on screen it
    /// goes too, and a new one can be dragged.
    pub fn forget(&mut self, id: u64) {
        if let Stage::Editing(edit) = &self.stage
            && edit.id == id
        {
            if edit.tools {
                self.give_back_keys();
            }
            self.stage = Stage::Selecting(Pick::Idle);
        }
    }

    /// The whole display as a selection.
    fn full_display(&self) -> Option<[f32; 4]> {
        let f = self.frozen.as_ref()?;
        Some([0.0, 0.0, f.width as f32 / f.scale, f.height as f32 / f.scale])
    }

    /// Frees the capture and hides the overlay, in one host update on the main thread. The overlay
    /// takes all pending requests in one `sync`, which applies updates before visibility, and a
    /// hidden overlay draws nothing: the cleared state is never shown. The clear is queued first
    /// so a capture thread pushing `freeze` + `show` in between always wins: [clear, freeze] then
    /// show the new shot, never a shown overlay with nothing to draw.
    fn dismiss(&mut self) {
        self.closing = true;
        self.host.update(|host, _| {
            if let Some(overlay) = &host.overlay {
                overlay.update(|s, _| s.clear());
            }
            host.hide_overlay();
        });
    }

    /// Drops the frozen capture (the export thread keeps its own reference while it runs).
    pub(crate) fn clear(&mut self) {
        self.frozen = None;
        self.stage = Stage::Selecting(Pick::Idle);
        self.windows = Vec::new();
        self.closing = false;
        self.for_input = false;
        self.kept = None;
        self.input_at = None;
        self.pixels.set(1, 1, vec![0; 4]);
    }

    /// Copies and saves the selection with its marks (off the UI thread), then closes.
    fn confirm(&mut self) {
        if self.closing {
            return;
        }
        if self.for_input {
            return self.close_tools();
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
    let Some(f) = &s.frozen else { return el() };
    // The overlay covers the display's frame, so its size in points is the capture over its scale.
    let (sw, sh) = (f.width as f32 / f.scale, f.height as f32 / f.scale);
    let mut root = el().w_full().h_full().pixels(&s.pixels).cursor(Cursor::Crosshair).on_drag(on_drag).on_pointer_move_local(on_pointer_move);
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
        // Started by the quick input: one label beside the pointer, following it.
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
            let panel = input_panel(s, edit.sel);
            if edit.tools {
                let top = bar_top(edit.sel, sh, panel);
                root.child(toolbar(edit, s.for_input, top, sw, sh, cx.layout_size("toolbar").map(|[w, _]| w)))
            } else {
                root.child(edit_button(edit_anchor(edit.sel, panel, [sw, sh])))
            }
        }
    }
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
fn toolbar(edit: &Edit, for_input: bool, top: f32, sw: f32, sh: f32, laid_out: Option<f32>) -> El {
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
    // For the quick input there's nothing to cancel or copy: Done puts the marks in its image.
    let bar = if for_input {
        bar.child(button("Done ↩".into(), true).on_press(|s: &mut Overlay| s.close_tools()))
    } else {
        bar.child(button("Cancel Esc".into(), false).on_press(|s: &mut Overlay| s.dismiss()))
            .child(button("Copy ↩".into(), true).on_press(|s: &mut Overlay| s.confirm()))
    };
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
/// (the quick input's panel, beside the selection) is.
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

/// Where the quick input's panel is while this capture is for it.
fn input_panel(s: &Overlay, sel: [f32; 4]) -> Option<[f32; 4]> {
    if !s.for_input {
        return None;
    }
    s.screen().map(|screen| crate::quick_input::beside(sel, screen))
}

/// Where the edit button goes: just outside the selection's corner nearest the quick input's
/// `panel`, clear of it, on `screen`; inside that corner when there's no room outside.
fn edit_anchor(sel: [f32; 4], panel: Option<[f32; 4]>, screen: [f32; 2]) -> [f32; 2] {
    let ([x, y, w, h], [sw, sh], b, g) = (sel, screen, EDIT_BUTTON, EDIT_GAP);
    let overlaps = |[bx, by]: [f32; 2], [px, py, pw, ph]: [f32; 4]| bx < px + pw && bx + b > px && by < py + ph && by + b > py;
    let fits = |at: [f32; 2]| at[0] >= 0.0 && at[1] >= 0.0 && at[0] + b <= sw && at[1] + b <= sh && panel.is_none_or(|p| !overlaps(at, p));
    let candidates: &[[f32; 2]] = match panel {
        // Right of it: under the selection, at its right end (above, if no room).
        Some([px, ..]) if px >= x + w => &[[x + w - b, y + h + g], [x + w - b, y - g - b]],
        // Left of it: under the selection, at its left end.
        Some([px, _, pw, _]) if px + pw <= x => &[[x, y + h + g], [x, y - g - b]],
        // Below it: beside the selection's bottom corner, right then left.
        Some([_, py, ..]) if py >= y + h => &[[x + w + g, y + h - b], [x - g - b, y + h - b], [x + w - b, y - g - b]],
        // Above it, or no input: beside the top-right corner, else under the bottom-right one.
        _ => &[[x + w + g, y], [x + w - b, y + h + g]],
    };
    let inside = [(x + w - b - g).clamp(0.0, sw - b), (y + h - b - g).clamp(0.0, sh - b)];
    candidates.iter().copied().find(|&at| fits(at)).unwrap_or(inside)
}

/// For the quick input: one button where the toolbar would be, opening it.
fn edit_button([x, y]: [f32; 2]) -> El {
    el().key("edit-tools")
        .size(EDIT_BUTTON, EDIT_BUTTON)
        .center()
        .radius(8.0)
        .continuous_corners()
        .fill(PANEL)
        .border(1.0, Color::WHITE.alpha(0.14))
        .box_shadow(Shadow::core_animation(0.0, 2.0, 8.0, Color::BLACK.alpha(0.3)))
        .hover(ACTIVE)
        .cursor(Cursor::Pointer)
        .label("Edit screenshot")
        .shortcut_hint("⌘E")
        .child(icon(&crate::icons::PENCIL, 15.0).color(EDGE))
        .on_press(|s: &mut Overlay| s.open_tools())
        .pin(Some(x), Some(y), None, None)
}

fn on_drag(s: &mut Overlay, drag: &mut Drag, tx: &mut Tx) {
    tx.instant();
    if s.closing {
        return;
    }
    match &mut s.stage {
        Stage::Selecting(Pick::Window(_)) => window_click(s, drag),
        Stage::Selecting(_) => select_drag(s, drag),
        // For the quick input, with the tools away a drag takes another area.
        Stage::Editing(edit) if !edit.tools => select_drag(s, drag),
        Stage::Editing(_) => edit_drag(s, drag),
    }
}

fn select_drag(s: &mut Overlay, drag: &Drag) {
    let was_dragging = matches!(s.stage, Stage::Selecting(Pick::Dragging(_)));
    // A drag over an area already picked (the quick input's): kept until this one is an area.
    if let Stage::Editing(_) = &s.stage
        && let Stage::Editing(edit) = std::mem::replace(&mut s.stage, Stage::Selecting(Pick::Idle))
    {
        s.kept = Some(edit);
    }
    let sel = marks::rect([drag.start_x, drag.start_y], [drag.x, drag.y]);
    let nothing = drag.cancelled() || (drag.phase == Phase::Up && (sel[2] < MIN_SIDE || sel[3] < MIN_SIDE));
    if nothing {
        // A click or a cancel, not an area: the area picked before comes back.
        s.stage = s.kept.take().map_or(Stage::Selecting(Pick::Idle), Stage::Editing);
    } else if drag.phase != Phase::Up {
        s.stage = Stage::Selecting(Pick::Dragging(sel));
    } else {
        s.pick(sel);
    }
    // A picked area for the quick input brings it back itself, beside the area.
    let picked = s.for_input && matches!(s.stage, Stage::Editing(_));
    if let Some(tucked) = tuck_change(was_dragging, matches!(s.stage, Stage::Selecting(Pick::Dragging(_))))
        && !(picked && !tucked)
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
        Key::Escape if s.for_input => s.close_tools(),
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
    fn the_edit_button_sits_at_the_corner_nearest_the_input() {
        let screen = [1920.0, 1080.0];
        let (b, g) = (EDIT_BUTTON, EDIT_GAP);
        // Input below: the button right of the selection's bottom-right corner.
        let sel = [500.0, 100.0, 1000.0, 500.0];
        let panel = crate::quick_input::beside(sel, screen);
        assert!(panel[1] > 600.0, "the input is below");
        assert_eq!(edit_anchor(sel, Some(panel), screen), [1500.0 + g, 600.0 - b]);
        // Input right: under the selection, right-aligned (clear of the input).
        let sel = [100.0, 100.0, 400.0, 300.0];
        assert_eq!(edit_anchor(sel, Some(crate::quick_input::beside(sel, screen)), screen), [500.0 - b, 400.0 + g]);
        // Input left: under the selection, left-aligned.
        let sel = [1300.0, 200.0, 600.0, 500.0];
        assert_eq!(edit_anchor(sel, Some(crate::quick_input::beside(sel, screen)), screen), [1300.0, 700.0 + g]);
        // No room outside (the whole screen): inside the corner.
        let sel = [0.0, 0.0, 1920.0, 1080.0];
        let [x, y] = edit_anchor(sel, Some(crate::quick_input::beside(sel, screen)), screen);
        assert!(x + b <= 1920.0 && y + b <= 1080.0 && x >= 0.0 && y >= 0.0, "on screen: {x},{y}");
    }

    #[test]
    fn the_toolbar_keeps_clear_of_the_quick_input() {
        let (sel, screen) = ([500.0, 100.0, 900.0, 500.0], [1920.0, 1080.0]);
        assert_eq!(bar_top(sel, screen[1], None), 608.0, "under the selection");
        let panel = crate::quick_input::beside(sel, screen);
        assert!(panel[1] < 648.0, "the input sits under it too");
        assert_eq!(bar_top(sel, screen[1], Some(panel)), 100.0 - TOOLBAR_H - 8.0, "so the bar goes above");
        // The input beside it (right): under it is free.
        let sel = [100.0, 100.0, 400.0, 300.0];
        assert_eq!(bar_top(sel, screen[1], Some(crate::quick_input::beside(sel, screen))), 408.0);
    }

    #[test]
    fn the_quick_input_tucks_for_an_area_drag_only() {
        assert_eq!(tuck_change(false, true), Some(true), "drag starts: out of the screen");
        assert_eq!(tuck_change(true, true), None, "drag moves: nothing to do");
        assert_eq!(tuck_change(true, false), Some(false), "drag ends or is cancelled: back");
        assert_eq!(tuck_change(false, false), None);
    }
}
