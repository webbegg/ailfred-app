//! Captures the display under the pointer with ScreenCaptureKit (`SCScreenshotManager`, macOS 14+),
//! leaving out every window of this process (the overlay, a menu still fading out, the status
//! item's highlight). Blocking: call it off the UI thread.

use objc2_core_graphics::CGEvent;
use screencapturekit::screenshot_manager::SCScreenshotManager;
use screencapturekit::shareable_content::{SCShareableContent, SCWindow};
use screencapturekit::stream::configuration::SCStreamConfiguration;
use screencapturekit::stream::content_filter::SCContentFilter;

/// One captured display, as straight RGBA pixels at the display's native resolution.
pub struct Shot {
    /// CGDirectDisplayID: the same number as Rune's `NativeDisplayId::MacOS`.
    pub display_id: u32,
    pub width: u32,
    pub height: u32,
    /// Pixels per point.
    pub scale: f32,
    pub rgba: Vec<u8>,
}

/// The pointer in global CoreGraphics coordinates (points, origin at the primary display's top-left),
/// the same space as `SCDisplay::frame`.
fn pointer() -> (f64, f64) {
    let event = CGEvent::new(None);
    let p = CGEvent::location(event.as_deref());
    (p.x, p.y)
}

/// Captures the display under the pointer, without any of our own windows.
pub fn display_under_pointer() -> Result<Shot, String> {
    let (px, py) = pointer();
    let content = SCShareableContent::get()
        .map_err(|e| format!("listing displays failed: {e} (is Screen Recording allowed for this app?)"))?;
    let displays = content.displays();
    let display = displays
        .iter()
        .find(|d| {
            let f = d.frame();
            px >= f.x && px < f.x + f.width && py >= f.y && py < f.y + f.height
        })
        .or_else(|| displays.first())
        .ok_or("no display found")?;

    let windows = content.windows();
    let pid = std::process::id() as i32;
    let ours: Vec<&SCWindow> =
        windows.iter().filter(|w| w.owning_application().is_some_and(|app| app.process_id() == pid)).collect();
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&ours)
        .build();

    let scale = filter.point_pixel_scale();
    let frame = display.frame();
    let width = (frame.width * f64::from(scale)).round() as u32;
    let height = (frame.height * f64::from(scale)).round() as u32;
    let config = SCStreamConfiguration::new().with_width(width).with_height(height).with_shows_cursor(false);
    // No `with_color_space_name`: in screencapturekit 1.5.4 the bridge hands ScreenCaptureKit a
    // temporary string it doesn't retain, and the capture crashes (SIGSEGV in
    // -[SCStreamConfiguration copyWithZone:]).

    let image = SCScreenshotManager::capture_image(&filter, &config).map_err(|e| format!("capture failed: {e}"))?;
    let (width, height) = (image.width() as u32, image.height() as u32);
    let rgba = image.rgba_data().map_err(|e| format!("reading pixels failed: {e}"))?;
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(format!("unexpected pixel buffer: {} bytes for {width}x{height}", rgba.len()));
    }
    eprintln!("ailfred captured display {} ({width}x{height} px, frame {}x{} pt, scale {scale})", display.display_id(), frame.width, frame.height);
    Ok(Shot { display_id: display.display_id(), width, height, scale, rgba })
}
