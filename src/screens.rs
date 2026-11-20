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
    /// Its size in points.
    pub fn size(&self) -> [f32; 2] {
        [self.frame[2], self.frame[3]]
    }
}

/// The display under the pointer. Main thread.
pub fn under_pointer() -> Option<Screen> {
    // SAFETY: plain AppKit class and instance messages with their documented signatures, on the
    // main thread (the host's loop); `screens` is an autoreleased NSArray kept alive by the pool.
    unsafe {
        let mouse: CGPoint = msg_send![class!(NSEvent), mouseLocation];
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return None;
        }
        let count: usize = msg_send![screens, count];
        for i in 0..count {
            let screen: *mut AnyObject = msg_send![screens, objectAtIndex: i];
            let frame: CGRect = msg_send![screen, frame];
            let [x, y, w, h] = rect(frame);
            let (mx, my) = (mouse.x as f32, mouse.y as f32);
            if mx >= x && mx < x + w && my >= y && my < y + h {
                let visible: CGRect = msg_send![screen, visibleFrame];
                let description: *mut AnyObject = msg_send![screen, deviceDescription];
                let key = NSString::from_str("NSScreenNumber");
                let number: *mut AnyObject =
                    if description.is_null() { std::ptr::null_mut() } else { msg_send![description, objectForKey: &*key] };
                let id: u32 = if number.is_null() { 0 } else { msg_send![number, unsignedIntValue] };
                return Some(Screen { frame: rect(frame), visible: rect(visible), id });
            }
        }
    }
    None
}
