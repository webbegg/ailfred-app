//! Captures the display under the pointer with ScreenCaptureKit (`SCScreenshotManager`, macOS 14+)
//! through objc2-screen-capture-kit, in sRGB, leaving out every window of this process (the
//! overlay, a menu still fading out, the status item's highlight). Blocking: call it off the UI
//! thread. ScreenCaptureKit answers through completion blocks; each call here waits for its answer.

use std::ptr::NonNull;
use std::sync::{Mutex, mpsc};
use std::time::Duration;

use block2::RcBlock;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_core_foundation::{CFArray, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorRenderingIntent, CGColorSpace, CGContext, CGDataProvider, CGEvent, CGImage, CGImageAlphaInfo,
    CGRectMakeWithDictionaryRepresentation,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGColorSpaceSRGB, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, kCGWindowOwnerPID,
};
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow};

/// How long a ScreenCaptureKit answer may take before we give up.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// One captured display, as straight RGBA pixels at the display's native resolution, in sRGB.
pub struct Shot {
    /// CGDirectDisplayID: the same number as Rune's `NativeDisplayId::MacOS`.
    pub display_id: u32,
    pub width: u32,
    pub height: u32,
    /// Pixels per point.
    pub scale: f32,
    pub rgba: Vec<u8>,
    /// Other apps' windows on this display, front to back, in display-local points (x, y, w, h),
    /// clipped to the display: what window mode can pick.
    pub windows: Vec<[f32; 4]>,
    /// The pointer at capture time, in display-local points: window mode highlights the window
    /// under it before the pointer moves.
    pub pointer: [f32; 2],
}

/// A ScreenCaptureKit result carried from its completion block to the waiting thread.
struct Answer<T>(T);
// SAFETY: shareable content and CGImages are immutable once delivered, and only the waiting thread
// uses them after the block hands them over. Only these two types cross.
unsafe impl Send for Answer<Retained<SCShareableContent>> {}
unsafe impl Send for Answer<CFRetained<CGImage>> {}

fn describe(error: *mut NSError) -> String {
    // SAFETY: ScreenCaptureKit passes either null or a valid NSError for the block's duration.
    match unsafe { error.as_ref() } {
        Some(error) => error.localizedDescription().to_string(),
        None => "no answer and no error".into(),
    }
}

/// Waits for the one answer a completion block sends.
fn wait<T>(rx: &mpsc::Receiver<Result<Answer<T>, String>>, what: &str) -> Result<T, String> {
    match rx.recv_timeout(ANSWER_TIMEOUT) {
        Ok(answer) => answer.map(|a| a.0).map_err(|e| format!("{what} failed: {e}")),
        Err(_) => Err(format!("{what} failed: no answer from ScreenCaptureKit")),
    }
}

/// Displays and windows on screen. Fails without Screen Recording permission.
fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: a valid object or null; retaining keeps it past the block.
        let answer = match unsafe { Retained::retain(content) } {
            Some(content) => Ok(Answer(content)),
            None => Err(describe(error)),
        };
        if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(answer);
        }
    });
    // SAFETY: the block matches the handler's signature and is retained by ScreenCaptureKit.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    wait(&rx, "listing displays").map_err(|e| format!("{e} (is Screen Recording allowed for this app?)"))
}

/// One screenshot through `filter` and `config`.
fn screenshot(filter: &SCContentFilter, config: &SCStreamConfiguration) -> Result<CFRetained<CGImage>, String> {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(Some(tx));
    let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        // SAFETY: a valid CGImage or null; retaining keeps it past the block.
        let answer = match NonNull::new(image) {
            Some(image) => Ok(Answer(unsafe { CFRetained::retain(image) })),
            None => Err(describe(error)),
        };
        if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(answer);
        }
    });
    // SAFETY: valid filter and configuration; the block matches the handler's signature.
    unsafe { SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(filter, config, Some(&block)) };
    wait(&rx, "capture")
}

/// The pointer in global CoreGraphics coordinates (points, origin at the primary display's top-left),
/// the same space as `SCDisplay::frame`.
fn pointer() -> (f64, f64) {
    let event = CGEvent::new(None);
    let p = CGEvent::location(event.as_deref());
    (p.x, p.y)
}

/// The image's pixels as straight RGBA in sRGB: drawn into an sRGB bitmap (no conversion for an
/// sRGB capture). Screen pixels are opaque, so premultiplied equals straight.
fn rgba(image: &CGImage) -> Result<(Vec<u8>, u32, u32), String> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    let mut pixels = vec![0u8; w * h * 4];
    // SAFETY: kCGColorSpaceSRGB is a valid, static colour space name.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or("no sRGB colour space")?;
    // SAFETY: `pixels` is `w * h * 4` bytes and outlives the context, dropped at the end of this function.
    let context = unsafe {
        CGBitmapContextCreate(pixels.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), CGImageAlphaInfo::PremultipliedLast.0)
    }
    .ok_or("no bitmap context")?;
    let rect = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64));
    CGContext::draw_image(Some(&context), rect, Some(image));
    drop(context);
    Ok((pixels, w as u32, h as u32))
}

/// A JPEG's pixels as straight RGBA in sRGB (opaque), its width and height.
pub fn jpeg_rgba(jpeg: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let data = CFData::from_bytes(jpeg);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    // SAFETY: a null decode array is allowed (no remapping).
    let image = unsafe { CGImage::with_jpeg_data_provider(Some(&provider), std::ptr::null(), true, CGColorRenderingIntent::RenderingIntentDefault) }?;
    rgba(&image).ok()
}

/// Windows smaller than this (points, either side) aren't offered: status items, tooltips.
const MIN_WINDOW: f64 = 40.0;

/// Other apps' normal windows (layer 0) on `display` (global points), front to back, in
/// display-local points and clipped to it. CGWindowList keeps the on-screen order;
/// ScreenCaptureKit's list doesn't promise one.
fn window_frames(display: CGRect) -> Vec<[f32; 4]> {
    let option = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(option, 0) else { return Vec::new() };
    // SAFETY: CGWindowListCopyWindowInfo returns an array of CFDictionary<CFString, CFType>.
    let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };
    let pid = i64::from(std::process::id());
    // SAFETY: CoreGraphics' static dictionary keys.
    let (layer_key, owner_key, bounds_key, alpha_key) = unsafe { (kCGWindowLayer, kCGWindowOwnerPID, kCGWindowBounds, kCGWindowAlpha) };
    let number = |info: &CFDictionary<CFString, CFType>, key: &CFString| {
        info.get(key).and_then(|v| v.downcast::<CFNumber>().ok()).and_then(|n| n.as_i64())
    };
    let (dx, dy, dw, dh) = (display.origin.x, display.origin.y, display.size.width, display.size.height);
    let mut out = Vec::new();
    for info in list.iter() {
        if number(&info, layer_key) != Some(0) || number(&info, owner_key) == Some(pid) {
            continue;
        }
        // Invisible helper windows would sit on top of the one the user sees.
        let alpha = info.get(alpha_key).and_then(|v| v.downcast::<CFNumber>().ok()).and_then(|n| n.as_f64());
        if alpha.is_some_and(|a| a <= 0.0) {
            continue;
        }
        let Some(bounds) = info.get(bounds_key).and_then(|v| v.downcast::<CFDictionary>().ok()) else { continue };
        let mut r = CGRect::default();
        // SAFETY: `bounds` is the window's bounds dictionary; `r` is a valid out-pointer.
        if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut r) } {
            continue;
        }
        let (x0, y0) = (r.origin.x.max(dx), r.origin.y.max(dy));
        let (x1, y1) = ((r.origin.x + r.size.width).min(dx + dw), (r.origin.y + r.size.height).min(dy + dh));
        if x1 - x0 < MIN_WINDOW || y1 - y0 < MIN_WINDOW {
            continue;
        }
        out.push([(x0 - dx) as f32, (y0 - dy) as f32, (x1 - x0) as f32, (y1 - y0) as f32]);
    }
    out
}

/// Captures the display under the pointer, without any of our own windows.
pub fn display_under_pointer() -> Result<Shot, String> {
    // Waiting here on the main thread would freeze the UI (and could starve a handler on the main queue).
    debug_assert!(objc2::MainThreadMarker::new().is_none(), "capture must run off the main thread");
    let (px, py) = pointer();
    let content = shareable_content()?;
    // SAFETY: plain getters on a valid SCShareableContent.
    let displays = unsafe { content.displays() };
    let windows = unsafe { content.windows() };
    let display = displays
        .iter()
        .find(|d| {
            // SAFETY: a getter on a valid SCDisplay.
            let f = unsafe { d.frame() };
            px >= f.origin.x && px < f.origin.x + f.size.width && py >= f.origin.y && py < f.origin.y + f.size.height
        })
        .or_else(|| displays.firstObject())
        .ok_or("no display found")?;

    let pid = std::process::id() as i32;
    // SAFETY: getters on valid SCWindow / SCRunningApplication objects.
    let ours: Vec<Retained<SCWindow>> = windows
        .iter()
        .filter(|w| unsafe { w.owningApplication() }.is_some_and(|app| unsafe { app.processID() } == pid))
        .collect();
    let excluded = NSArray::from_retained_slice(&ours);
    // SAFETY: a valid display and window array.
    let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &excluded) };

    // SAFETY: getters and setters on valid objects. The colour space name is CoreGraphics' static
    // constant, which outlives the configuration (it isn't retained by it).
    let (scale, frame, display_id) = unsafe { (filter.pointPixelScale(), display.frame(), display.displayID()) };
    let width = (frame.size.width * f64::from(scale)).round() as usize;
    let height = (frame.size.height * f64::from(scale)).round() as usize;
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setWidth(width);
        config.setHeight(height);
        config.setShowsCursor(false);
        config.setColorSpaceName(kCGColorSpaceSRGB);
    }

    // The window list from the same moment as the pixels, as near as we can: just before.
    let windows = window_frames(frame);
    let image = screenshot(&filter, &config)?;
    let (rgba, width, height) = rgba(&image)?;
    eprintln!(
        "Capibarapp captured display {display_id} ({width}x{height} px, frame {}x{} pt, scale {scale}, {} windows)",
        frame.size.width,
        frame.size.height,
        windows.len()
    );
    let pointer = [(px - frame.origin.x) as f32, (py - frame.origin.y) as f32];
    Ok(Shot { display_id, width, height, scale, rgba, windows, pointer })
}
