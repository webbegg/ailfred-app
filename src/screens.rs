//! What AppKit knows about the displays that Rune doesn't expose: the Dock's room.

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect};

use crate::quick_input;

fn rect(r: CGRect) -> [f32; 4] {
    [r.origin.x as f32, r.origin.y as f32, r.size.width as f32, r.size.height as f32]
}

/// The bottom gap the Dock takes on the display under the pointer (points; 0 when it's hidden
/// or on a side). Main thread.
pub fn dock_inset_under_pointer() -> f32 {
    // SAFETY: plain AppKit class and instance messages with their documented signatures, on the
    // main thread (the host's loop); `screens` is an autoreleased NSArray kept alive by the pool.
    unsafe {
        let mouse: CGPoint = msg_send![class!(NSEvent), mouseLocation];
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return 0.0;
        }
        let count: usize = msg_send![screens, count];
        for i in 0..count {
            let screen: *mut AnyObject = msg_send![screens, objectAtIndex: i];
            let frame: CGRect = msg_send![screen, frame];
            let [x, y, w, h] = rect(frame);
            let (mx, my) = (mouse.x as f32, mouse.y as f32);
            if mx >= x && mx < x + w && my >= y && my < y + h {
                let visible: CGRect = msg_send![screen, visibleFrame];
                return quick_input::dock_inset(rect(frame), rect(visible));
            }
        }
    }
    0.0
}
