//! What AppKit knows about the displays that Rune doesn't expose: the Dock's room.

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_foundation::NSString;


fn rect(r: CGRect) -> [f32; 4] {
    [r.origin.x as f32, r.origin.y as f32, r.size.width as f32, r.size.height as f32]
}

/// A display as AppKit has it.
#[derive(Clone, Copy, Debug)]
pub struct Screen {
    /// Its frame, and its visible frame (without the menu bar and the Dock): AppKit rects in
    /// points with y up.
    pub frame: [f32; 4],
    pub visible: [f32; 4],
    /// Its `CGDirectDisplayID` (0 if AppKit didn't say).
    pub id: u32,
}

impl Screen {
    /// When AppKit can't say: a common laptop display, without a menu bar or Dock.
    pub const FALLBACK: Screen = Screen { frame: [0.0, 0.0, 1440.0, 900.0], visible: [0.0, 0.0, 1440.0, 900.0], id: 0 };

    /// Its size in points.
    pub fn size(&self) -> [f32; 2] {
        [self.frame[2], self.frame[3]]
    }
}

/// The display under the pointer. Main thread.
pub fn under_pointer() -> Option<Screen> {
    // SAFETY: a plain AppKit class message with its documented signature, on the main thread.
    let mouse: CGPoint = unsafe { msg_send![class!(NSEvent), mouseLocation] };
    let (mx, my) = (mouse.x as f32, mouse.y as f32);
    all().into_iter().find(|s| {
        let [x, y, w, h] = s.frame;
        mx >= x && mx < x + w && my >= y && my < y + h
    })
}

/// Every display. Main thread.
fn all() -> Vec<Screen> {
    let mut out = Vec::new();
    // SAFETY: plain AppKit class and instance messages with their documented signatures, on the
    // main thread (the host's loop); `screens` is an autoreleased NSArray kept alive by the pool.
    unsafe {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return out;
        }
        let key = NSString::from_str("NSScreenNumber");
        let count: usize = msg_send![screens, count];
        for i in 0..count {
            let screen: *mut AnyObject = msg_send![screens, objectAtIndex: i];
            let frame: CGRect = msg_send![screen, frame];
            let visible: CGRect = msg_send![screen, visibleFrame];
            let description: *mut AnyObject = msg_send![screen, deviceDescription];
            let number: *mut AnyObject =
                if description.is_null() { std::ptr::null_mut() } else { msg_send![description, objectForKey: &*key] };
            let id: u32 = if number.is_null() { 0 } else { msg_send![number, unsignedIntValue] };
            out.push(Screen { frame: rect(frame), visible: rect(visible), id });
        }
    }
    out
}
