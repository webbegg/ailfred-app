//! The overlay: the frozen display, a drag to select an area, then annotation with a toolbar.
//! Enter or ⌘C copies and saves (export.rs); Esc cancels.

use std::sync::Arc;

use rune::{AppHandle, Color, Cursor, Cx, Drag, El, Key, KeyEvent, LineCap, Phase, Pixels, Shape, TextStyle, Tx, canvas, el, row, text};

use crate::Host;
use crate::capture::Shot;
use crate::export::{self, Frozen, INK};
use crate::marks::{self, Mark, Tool};

const DIM: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.35 };
/// The hint's and toolbar's own background: they sit on the screen, so they must read on white.
const PANEL: Color = Color { r: 0.08, g: 0.08, b: 0.09, a: 0.92 };
const EDGE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.9 };
const ACTIVE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.18 };
const INK_COLOR: Color = Color { r: INK[0] as f32, g: INK[1] as f32, b: INK[2] as f32, a: INK[3] as f32 };
/// A drag smaller than this (points) is a click, not a selection.
const MIN_SIDE: f32 = 3.0;
const TOOLBAR_H: f32 = 40.0;

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
        Edit { sel, tool: Tool::Arrow, marks: Vec::new(), previews: Vec::new(), draft: None, typing: None }
    }

    fn push(&mut self, mark: Mark, preview: Option<Pixels>) {
        self.marks.push(mark);
        self.previews.push(preview);
    }

    /// Ends typing, keeping the text if there is any.
    fn finish_typing(&mut self) {
        if let Some((at, text)) = self.typing.take() {
            let mark = Mark::Text { at, text };
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
            let mark = Mark::Text { at: *at, text: text.clone() };
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
}

impl Overlay {
    pub fn new(host: AppHandle<Host>) -> Self {
        Overlay { host, pixels: Pixels::new(), frozen: None, stage: Stage::Selecting(Pick::Idle), windows: Vec::new(), pointer: [0.0, 0.0], closing: false }
    }

    /// Shows a new capture in `mode`; any previous selection and marks are dropped.
    pub fn freeze(&mut self, shot: Shot, mode: Mode) {
        self.pixels.set(shot.width, shot.height, shot.rgba.clone());
        let frozen = Frozen { width: shot.width, height: shot.height, scale: shot.scale, rgba: shot.rgba };
        let full = [0.0, 0.0, frozen.width as f32 / frozen.scale, frozen.height as f32 / frozen.scale];
        self.frozen = Some(Arc::new(frozen));
        self.windows = shot.windows;
        self.pointer = shot.pointer;
        self.closing = false;
        self.stage = match mode {
            Mode::Area => Stage::Selecting(Pick::Idle),
            Mode::Window => Stage::Selecting(Pick::Window(window_at(&self.windows, self.pointer))),
            Mode::FullScreen => Stage::Editing(Edit::new(full)),
        };
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
    fn clear(&mut self) {
        self.frozen = None;
        self.stage = Stage::Selecting(Pick::Idle);
        self.windows = Vec::new();
        self.closing = false;
        self.pixels.set(1, 1, vec![0; 4]);
    }

    /// Copies and saves the selection with its marks (off the UI thread), then closes.
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

pub fn view(s: &Overlay, _: &mut Cx<Overlay>) -> El {
    let Some(f) = &s.frozen else { return el() };
    // The overlay covers the display's frame, so its size in points is the capture over its scale.
    let (sw, sh) = (f.width as f32 / f.scale, f.height as f32 / f.scale);
    let root = el().w_full().h_full().pixels(&s.pixels).cursor(Cursor::Crosshair).on_drag(on_drag).on_pointer_move_local(on_pointer_move);
    let hint = |t: &str| {
        el().padding(10.0).radius(8.0).fill(PANEL).pin(Some(24.0), Some(24.0), None, None).child(text(t.to_string()).color(EDGE))
    };
    match &s.stage {
        // Before a drag the frozen screen shows as is, only the hint on top; the dim starts with the
        // drag (its first event creates the selection).
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
            shades(root, edit.sel, sw, sh)
                .child(el().key("selection").size(w, h).border(1.0, EDGE).pin(Some(x), Some(y), None, None))
                .child(marks_layer(edit).pin(Some(x), Some(y), None, None))
                .child(toolbar(edit, sw, sh))
        }
    }
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
            Mark::Rect([rx, ry, rw, rh]) => {
                layer = layer.child(el().size(*rw, *rh).border(marks::STROKE, INK_COLOR).pin(Some(*rx), Some(*ry), None, None));
            }
            Mark::Arrow { from, to } => {
                let (base, head) = marks::arrow_geometry(*from, *to, marks::STROKE);
                shape = shape.line(*from, base, marks::STROKE, LineCap::Round, INK_COLOR).fill(head.to_vec(), INK_COLOR);
            }
            Mark::Text { at, text: t } => layer = layer.child(text_mark(*at, t)),
        }
    }
    if let Some((at, t)) = &edit.typing {
        layer = layer.child(text_mark(*at, &format!("{t}|")));
    }
    layer.child(canvas(shape).size(w, h).pin(Some(0.0), Some(0.0), None, None))
}

fn text_mark(at: [f32; 2], t: &str) -> El {
    text(t.to_string()).font(TextStyle::new(marks::TEXT_SIZE, 700.0)).color(INK_COLOR).pin(Some(at[0]), Some(at[1]), None, None)
}

/// The tools and actions, under the selection (above it, or inside, when there's no room),
/// right-aligned with it.
fn toolbar(edit: &Edit, sw: f32, sh: f32) -> El {
    let [x, y, w, h] = edit.sel;
    let below = y + h + 8.0;
    let top = if below + TOOLBAR_H <= sh { below } else if y - TOOLBAR_H - 8.0 >= 0.0 { y - TOOLBAR_H - 8.0 } else { y + 8.0 };
    // Right-aligned with the selection, but never pushed off the left edge (the bar is ~560 pt).
    let right = (sw - (x + w)).max(8.0).min((sw - 560.0).max(8.0));
    let button = |label: String, active: bool| {
        el().key(format!("tool-{label}")).px(10.0).py(6.0).radius(6.0).fill(if active { ACTIVE } else { Color { a: 0.0, ..PANEL } })
            .child(text(label).color(EDGE))
    };
    let mut bar = row().key("toolbar").gap(2.0).padding(4.0).radius(10.0).fill(PANEL);
    for tool in Tool::ALL {
        let label = format!("{} {}", tool.label(), tool.key().to_uppercase());
        bar = bar.child(button(label, edit.tool == tool).on_press(move |s: &mut Overlay| s.set_tool(tool)));
    }
    bar.child(button("Undo ⌘Z".into(), false).on_press(|s: &mut Overlay| s.undo()))
        .child(button("Cancel Esc".into(), false).on_press(|s: &mut Overlay| s.dismiss()))
        .child(button("Copy ↩".into(), true).on_press(|s: &mut Overlay| s.confirm()))
        .pin(None, Some(top), Some(right), None)
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
    if drag.cancelled() {
        s.stage = Stage::Selecting(Pick::Idle);
        return;
    }
    let sel = marks::rect([drag.start_x, drag.start_y], [drag.x, drag.y]);
    s.stage = if drag.phase != Phase::Up {
        Stage::Selecting(Pick::Dragging(sel))
    } else if sel[2] < MIN_SIDE || sel[3] < MIN_SIDE {
        Stage::Selecting(Pick::Idle)
    } else {
        Stage::Editing(Edit::new(sel))
    };
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
        _ => tx.live_only(),
    }
}

/// Window mode: a click (released over a window) selects it.
fn window_click(s: &mut Overlay, drag: &Drag) {
    if drag.phase != Phase::Up || drag.cancelled() {
        return;
    }
    if let Some(win) = window_at(&s.windows, [drag.x, drag.y]) {
        s.stage = Stage::Editing(Edit::new(win));
    }
}

fn edit_drag(s: &mut Overlay, drag: &Drag) {
    let Stage::Editing(edit) = &mut s.stage else { return };
    let [sx, sy, sw, sh] = edit.sel;
    let local = |x: f32, y: f32| marks::clamp([x - sx, y - sy], sw, sh);
    // Only gestures that start inside the selection draw; a press outside just ends typing.
    let inside = |x: f32, y: f32| x >= sx && x <= sx + sw && y >= sy && y <= sy + sh;
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
    let mark = Mark::dragged(edit.tool, local(drag.start_x, drag.start_y), local(drag.x, drag.y));
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
                Stage::Selecting(Pick::Window(Some(win))) => s.stage = Stage::Editing(Edit::new(win)),
                _ => return false,
            },
            Key::Text(t) if t.eq_ignore_ascii_case("f") && !chord => {
                if let Some(full) = s.full_display() {
                    s.stage = Stage::Editing(Edit::new(full));
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
        Key::Escape => s.dismiss(),
        Key::Enter => s.confirm(),
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
}
