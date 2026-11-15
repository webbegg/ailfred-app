//! The overlay: the frozen display, a drag to select an area, and the crop on release
//! (PNG to the clipboard and to ~/Desktop).

use std::sync::Arc;

use rune::{AppHandle, ClipboardContent, ClipboardImage, Color, Cursor, Cx, Drag, El, Key, KeyEvent, Phase, Pixels, Tx, el, text};

use crate::Host;
use crate::capture::Shot;

const DIM: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.35 };
const EDGE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.9 };
/// A drag smaller than this (points) is a click, not a selection.
const MIN_SIDE: f32 = 3.0;

/// The display as captured, kept for cropping at native resolution.
struct Frozen {
    width: u32,
    height: u32,
    scale: f32,
    rgba: Vec<u8>,
}

/// The overlay's state. An overlay drops `Tx::close_window`/`Tx::quit` from its handlers (it has
/// no window to ask), so hiding and quitting go through the host.
pub struct Overlay {
    host: AppHandle<Host>,
    /// What the overlay draws: the frozen display.
    pixels: Pixels,
    /// Shared with the export thread, which crops it after the overlay hides.
    frozen: Option<Arc<Frozen>>,
    /// x, y, width, height in points.
    selection: Option<[f32; 4]>,
}

impl Overlay {
    pub fn new(host: AppHandle<Host>) -> Self {
        Overlay { host, pixels: Pixels::new(), frozen: None, selection: None }
    }

    /// Shows a new capture; any previous selection is dropped.
    pub fn freeze(&mut self, shot: Shot) {
        self.pixels.set(shot.width, shot.height, shot.rgba.clone());
        self.frozen = Some(Arc::new(Frozen { width: shot.width, height: shot.height, scale: shot.scale, rgba: shot.rgba }));
        self.selection = None;
    }

    /// Hides the overlay. The capture stays until the next `freeze` replaces it: clearing it here
    /// would draw one empty frame before the hide lands.
    fn dismiss(&mut self) {
        self.host.update(|host, _| host.hide_overlay());
    }
}

pub fn view(s: &Overlay, _: &mut Cx<Overlay>) -> El {
    let Some(f) = &s.frozen else { return el() };
    // The overlay covers the display's frame, so its size in points is the capture over its scale.
    let (sw, sh) = (f.width as f32 / f.scale, f.height as f32 / f.scale);
    let mut root = el().w_full().h_full().pixels(&s.pixels).cursor(Cursor::Crosshair).on_drag(on_drag);
    let Some([x, y, w, h]) = s.selection else {
        return root.child(el().size(sw, sh).fill(DIM).pin(Some(0.0), Some(0.0), None, None)).child(
            el().padding(10.0).radius(8.0).fill(DIM).pin(Some(24.0), Some(24.0), None, None)
                .child(text("Drag to capture · Esc to cancel").color(EDGE)),
        );
    };
    // Dim everything outside the selection: above, below, left and right of it.
    let shade = |px: f32, py: f32, pw: f32, ph: f32| el().size(pw.max(0.0), ph.max(0.0)).fill(DIM).pin(Some(px), Some(py), None, None);
    root = root
        .child(shade(0.0, 0.0, sw, y))
        .child(shade(0.0, y + h, sw, sh - y - h))
        .child(shade(0.0, y, x, h))
        .child(shade(x + w, y, sw - x - w, h))
        .child(el().key("selection").size(w, h).border(1.0, EDGE).pin(Some(x), Some(y), None, None))
        .child(
            text(format!("{} × {}", (w * f.scale).round(), (h * f.scale).round()))
                .color(EDGE)
                .pin(Some(x), Some((y + h + 6.0).min(sh - 20.0)), None, None),
        );
    root
}

fn on_drag(s: &mut Overlay, drag: &mut Drag, tx: &mut Tx) {
    tx.instant();
    if drag.cancelled() {
        s.selection = None;
        return;
    }
    let sel = [drag.x.min(drag.start_x), drag.y.min(drag.start_y), (drag.x - drag.start_x).abs(), (drag.y - drag.start_y).abs()];
    s.selection = Some(sel);
    if drag.phase != Phase::Up {
        return;
    }
    if sel[2] < MIN_SIDE || sel[3] < MIN_SIDE {
        s.selection = None;
        return;
    }
    if let Some(frozen) = s.frozen.clone() {
        let host = s.host.clone();
        // Cropping and encoding a large area takes long enough to stall a frame: do it off the UI thread.
        std::thread::spawn(move || export(&frozen, sel, &host));
    }
    s.dismiss();
}

/// Crops, encodes, copies to the clipboard (through a queued overlay update, which flushes the
/// clipboard even while hidden) and saves to ~/Desktop.
fn export(frozen: &Frozen, sel: [f32; 4], host: &AppHandle<Host>) {
    let (png, width, height) = match crop_png(frozen, sel) {
        Ok(out) => out,
        Err(e) => return eprintln!("ailfred: crop failed: {e}"),
    };
    match ClipboardImage::from_png(png.clone()) {
        Some(image) => {
            let content = ClipboardContent::new("").with_image(image);
            host.update(move |host, _| {
                if let Some(overlay) = &host.overlay {
                    overlay.update(move |_, tx| tx.copy_rich(content));
                }
            });
        }
        None => eprintln!("ailfred: image too large for the clipboard; saved to disk only"),
    }
    save(&png, width, height);
}

/// Crops the selection (points) out of the frozen capture at native pixels and encodes a PNG.
fn crop_png(f: &Frozen, [x, y, w, h]: [f32; 4]) -> Result<(Vec<u8>, u32, u32), String> {
    let px = |v: f32, max: u32| ((v * f.scale).round().max(0.0) as u32).min(max);
    let (x0, y0, x1, y1) = (px(x, f.width), px(y, f.height), px(x + w, f.width), px(y + h, f.height));
    let (cw, ch) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    if cw == 0 || ch == 0 {
        return Err("empty selection".into());
    }
    let row = cw as usize * 4;
    let mut out = Vec::with_capacity(row * ch as usize);
    for r in y0..y1 {
        let start = (r as usize * f.width as usize + x0 as usize) * 4;
        out.extend_from_slice(&f.rgba[start..start + row]);
    }
    Ok((encode_png(cw, ch, f.scale, &out)?, cw, ch))
}

/// RGBA to PNG, tagged with 72 DPI per point so a Retina capture pastes at its point size.
fn encode_png(width: u32, height: u32, scale: f32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut encoder = png::Encoder::new(&mut buf, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let ppm = (72.0 * f64::from(scale) / 0.0254).round() as u32;
    encoder.set_pixel_dims(Some(png::PixelDimensions { xppu: ppm, yppu: ppm, unit: png::Unit::Meter }));
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Writes the PNG to ~/Desktop. Called on the export thread.
fn save(png: &[u8], width: u32, height: u32) {
    let Some(home) = std::env::var_os("HOME") else { return eprintln!("ailfred: HOME is unset; not saved") };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = std::path::Path::new(&home).join("Desktop").join(format!("ailfred-{stamp}.png"));
    match std::fs::write(&path, png) {
        Ok(()) => eprintln!("ailfred saved {} ({width}x{height})", path.display()),
        Err(e) => eprintln!("ailfred: saving {} failed: {e}", path.display()),
    }
}

pub fn on_key(s: &mut Overlay, event: &KeyEvent, _: &mut Tx) -> bool {
    if event.key == Key::Escape {
        s.dismiss();
        return true;
    }
    if event.is_command('q') {
        s.host.quit();
        return true;
    }
    false
}
