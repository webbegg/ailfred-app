//! The images in the chat: decoding PNGs and pasted files, where a paste lands, and a capture
//! drawn, small in a message or enlarged over the panel.

use super::*;

/// An enlarged chip's image takes at most this share of the screen's width and height.
const LIGHTBOX: f32 = 0.8;

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
        png::ColorType::Rgb => buf.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}

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
pub(super) fn image_file_png_within(path: &std::path::Path, max_bytes: u64, max_pixels: u64) -> Option<Vec<u8>> {
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

/// A capture, ready to draw (a handle: clones share the pixels).
#[derive(Clone)]
pub struct Image {
    pub(super) pixels: Pixels,
    pub(super) width: u32,
    pub(super) height: u32,
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
