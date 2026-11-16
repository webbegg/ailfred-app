//! From a frozen capture, a selection and its marks to a PNG on the clipboard and on disk.
//! Runs on the export thread, never the UI thread.

use std::sync::Arc;

use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColor, CGColorSpace, CGContext, CGImageAlphaInfo, CGLineCap, CGLineJoin, kCGColorSpaceSRGB};
use rune::{AppHandle, ClipboardContent, ClipboardImage};

use crate::Host;
use crate::marks::{self, Mark};

/// The display as captured, kept for cropping at native resolution.
pub struct Frozen {
    pub width: u32,
    pub height: u32,
    /// Pixels per point.
    pub scale: f32,
    pub rgba: Vec<u8>,
}

/// The annotation colour (sRGB red), straight alpha.
pub const INK: [f64; 4] = [0.96, 0.18, 0.2, 1.0];

/// Crops, applies the marks, encodes, copies to the clipboard (through a queued overlay update,
/// which flushes the clipboard even while hidden) and saves.
pub fn export(frozen: Arc<Frozen>, sel: [f32; 4], marks: Vec<Mark>, host: AppHandle<Host>) {
    let (mut rgba, width, height) = match crop(&frozen, sel) {
        Ok(out) => out,
        Err(e) => return eprintln!("ailfred: crop failed: {e}"),
    };
    if let Err(e) = apply(&mut rgba, width, height, frozen.scale, &marks) {
        eprintln!("ailfred: drawing the marks failed: {e}");
    }
    let png = match encode_png(width, height, frozen.scale, &rgba) {
        Ok(png) => png,
        Err(e) => return eprintln!("ailfred: encoding failed: {e}"),
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
    let dir = std::path::PathBuf::from(crate::app_env::screenshots_dir());
    match crate::output::save(&dir, &png) {
        Ok(path) => eprintln!("ailfred saved {} ({width}x{height}, {} marks)", path.display(), marks.len()),
        Err(e) => eprintln!("ailfred: {e}"),
    }
}

/// Copies the selection (points) out of the frozen capture at native pixels.
fn crop(f: &Frozen, [x, y, w, h]: [f32; 4]) -> Result<(Vec<u8>, u32, u32), String> {
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
    // Screen pixels are opaque; make sure, since the bitmap context treats them as premultiplied.
    for alpha in out.iter_mut().skip(3).step_by(4) {
        *alpha = 255;
    }
    Ok((out, cw, ch))
}

/// Burns the marks into the crop: pixelation first (on the pixels), then strokes and text on top.
fn apply(rgba: &mut [u8], width: u32, height: u32, scale: f32, all: &[Mark]) -> Result<(), String> {
    let (w, h) = (width as usize, height as usize);
    let to_px = |v: f32| (v * scale).round().max(0.0) as usize;
    for mark in all {
        if let Mark::Pixelate([x, y, rw, rh]) = mark {
            marks::pixelate(rgba, w, h, [to_px(*x), to_px(*y), to_px(*rw), to_px(*rh)], to_px(marks::BLOCK).max(1));
        }
    }
    if !all.iter().any(|m| matches!(m, Mark::Arrow { .. } | Mark::Rect(_) | Mark::Text { .. })) {
        return Ok(());
    }
    // sRGB for the context and the ink, so strokes and text are the same red.
    // SAFETY: kCGColorSpaceSRGB is a valid, static colour space name.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or("no colour space")?;
    // SAFETY: `rgba` is `width * height * 4` bytes, outlives the context (dropped at the end of
    // this function), and every pixel is opaque (`crop`), so premultiplied equals straight alpha.
    let context = unsafe {
        CGBitmapContextCreate(rgba.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), CGImageAlphaInfo::PremultipliedLast.0)
    }
    .ok_or("no bitmap context")?;
    let c = Some(&*context);
    // Top-left origin, in points: the marks' own space.
    CGContext::translate_ctm(c, 0.0, f64::from(height));
    CGContext::scale_ctm(c, f64::from(scale), -f64::from(scale));
    let [r, g, b, a] = INK;
    let ink = CGColor::new_srgb(r, g, b, a);
    CGContext::set_stroke_color_with_color(c, Some(&ink));
    CGContext::set_fill_color_with_color(c, Some(&ink));
    CGContext::set_line_width(c, f64::from(marks::STROKE));
    let pt = |p: [f32; 2]| (f64::from(p[0]), f64::from(p[1]));
    // The view's order: boxes and text as drawn, then arrows on top.
    let ordered = all.iter().filter(|m| !matches!(m, Mark::Arrow { .. })).chain(all.iter().filter(|m| matches!(m, Mark::Arrow { .. })));
    for mark in ordered {
        match mark {
            Mark::Rect([x, y, rw, rh]) => {
                // Inside the edge with square corners, as the overlay draws its border.
                let half = f64::from(marks::STROKE) / 2.0;
                let rect = CGRect::new(
                    CGPoint::new(f64::from(*x) + half, f64::from(*y) + half),
                    CGSize::new((f64::from(*rw) - 2.0 * half).max(0.0), (f64::from(*rh) - 2.0 * half).max(0.0)),
                );
                CGContext::set_line_join(c, CGLineJoin::Miter);
                CGContext::stroke_rect(c, rect);
            }
            Mark::Arrow { from, to } => {
                let (base, head) = marks::arrow_geometry(*from, *to, marks::STROKE);
                CGContext::set_line_cap(c, CGLineCap::Round);
                CGContext::set_line_join(c, CGLineJoin::Round);
                CGContext::begin_path(c);
                let (fx, fy) = pt(*from);
                let (bx, by) = pt(base);
                CGContext::move_to_point(c, fx, fy);
                CGContext::add_line_to_point(c, bx, by);
                CGContext::stroke_path(c);
                CGContext::begin_path(c);
                let (x0, y0) = pt(head[0]);
                CGContext::move_to_point(c, x0, y0);
                for p in &head[1..] {
                    let (x, y) = pt(*p);
                    CGContext::add_line_to_point(c, x, y);
                }
                CGContext::close_path(c);
                CGContext::fill_path(c);
            }
            Mark::Text { at, text } => {
                if let Err(e) = draw_text(&context, *at, text, &ink) {
                    eprintln!("ailfred: a text mark couldn't be drawn: {e}");
                }
            }
            Mark::Pixelate(_) => {}
        }
    }
    Ok(())
}

/// Draws a text mark with CoreText: the system UI font (emphasized, as the overlay's bold) at
/// `TEXT_SIZE`, `at` its top-left (as the overlay shows it). The context is flipped to a top-left
/// origin, so the text matrix flips glyphs back upright.
fn draw_text(context: &CGContext, at: [f32; 2], text: &str, ink: &CGColor) -> Result<(), String> {
    use objc2_core_foundation::{CFAttributedString, CFDictionary, CFString, CFType, CGAffineTransform};
    use objc2_core_text::{CTFont, CTFontUIFontType, CTLine, kCTFontAttributeName, kCTForegroundColorAttributeName};

    // SAFETY: a CoreText constructor with a valid type and size.
    let font = unsafe { CTFont::new_ui_font_for_language(CTFontUIFontType::EmphasizedSystem, f64::from(marks::TEXT_SIZE), None) }
        .ok_or("no system font")?;
    // SAFETY: the attribute names are CoreText's static keys.
    let keys: [&CFString; 2] = unsafe { [kCTFontAttributeName, kCTForegroundColorAttributeName] };
    let values: [&CFType; 2] = [&font, ink];
    let attributes = CFDictionary::from_slices(&keys, &values);
    let string = CFString::from_str(text);
    // SAFETY: the dictionary holds a CTFont under the font key and a CGColor under the colour key.
    let attributed = unsafe { CFAttributedString::new(None, Some(&string), Some(attributes.as_opaque())) }.ok_or("no attributed string")?;
    // SAFETY: a valid attributed string.
    let line = unsafe { CTLine::with_attributed_string(&attributed) };
    let (mut ascent, mut descent, mut leading) = (0.0, 0.0, 0.0);
    // SAFETY: the out-pointers are valid locals.
    unsafe { line.typographic_bounds(&mut ascent, &mut descent, &mut leading) };
    CGContext::set_text_matrix(Some(context), CGAffineTransform { a: 1.0, b: 0.0, c: 0.0, d: -1.0, tx: 0.0, ty: 0.0 });
    CGContext::set_text_position(Some(context), f64::from(at[0]), f64::from(at[1]) + ascent);
    // SAFETY: a valid line and context.
    unsafe { line.draw(context) };
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A white 40x20 crop at scale 1 with a box: the box's edge turns red, its middle stays white.
    #[test]
    fn rect_is_burned_in() {
        let (w, h) = (40u32, 20u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        apply(&mut rgba, w, h, 1.0, &[Mark::Rect([5.0, 5.0, 30.0, 10.0])]).unwrap();
        let at = |x: u32, y: u32| &rgba[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
        assert!(at(5, 10)[0] > 200 && at(5, 10)[1] < 120, "left edge is red: {:?}", at(5, 10));
        assert_eq!(at(20, 10), &[255, 255, 255, 255], "inside untouched");
        assert_eq!(at(1, 1), &[255, 255, 255, 255], "outside untouched");
    }

    #[test]
    fn text_is_burned_in_upright_near_its_top_left() {
        let (w, h) = (120u32, 40u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        apply(&mut rgba, w, h, 1.0, &[Mark::Text { at: [4.0, 4.0], text: "Hi".into() }]).unwrap();
        let red_rows: Vec<u32> = (0..h).filter(|y| (0..w).any(|x| rgba[((y * w + x) * 4 + 1) as usize] < 150)).collect();
        assert!(!red_rows.is_empty(), "some text drawn");
        assert!(*red_rows.first().unwrap() >= 4 && *red_rows.last().unwrap() <= 4 + 30, "within the line below `at`: {red_rows:?}");
    }

    /// Renders every kind of mark on a gradient to /tmp/ailfred-marks.png, for a look by eye:
    /// `cargo test sample_render -- --ignored`.
    #[test]
    #[ignore]
    fn sample_render() {
        let (w, h, scale) = (480u32, 240u32, 2.0);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, 160, 255]);
            }
        }
        let all = [
            Mark::Pixelate([150.0, 10.0, 80.0, 50.0]),
            Mark::Rect([10.0, 10.0, 100.0, 60.0]),
            Mark::Arrow { from: [20.0, 110.0], to: [130.0, 80.0] },
            Mark::Text { at: [140.0, 80.0], text: "Look here".into() },
        ];
        apply(&mut rgba, w, h, scale, &all).unwrap();
        std::fs::write("/tmp/ailfred-marks.png", encode_png(w, h, scale, &rgba).unwrap()).unwrap();
    }

    #[test]
    fn top_left_origin() {
        // An arrow along the top row: drawn near y=2, not flipped to the bottom.
        let (w, h) = (40u32, 20u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        apply(&mut rgba, w, h, 1.0, &[Mark::Arrow { from: [2.0, 2.0], to: [38.0, 2.0] }]).unwrap();
        let red = |x: u32, y: u32| rgba[((y * w + x) * 4 + 1) as usize] < 150;
        assert!(red(10, 2), "near the top");
        assert!(!red(10, 17), "not at the bottom");
    }
}
