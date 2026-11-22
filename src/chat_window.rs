//! The chat in its own window: popped out of the side panel (its header's button), it's a
//! normal macOS window, resizable, with the conversation, the reply streaming and the draft
//! moved into it. Its titlebar is the chat's header (the traffic lights over its left end), with
//! a button that docks it back into the side panel. While it's open the app shows a Dock icon;
//! closing it keeps the conversation for the next time it opens.

use rune::{AppHandle, Color, Cx, El, Titlebar, Tx, tok};

use crate::Host;
use crate::chat::{self, Chat};

/// Its size before it's first popped out (`popped_size` then sizes it to the screen), and its
/// smallest.
pub const SIZE: (f32, f32) = (1024.0, 768.0);
pub const MIN_SIZE: (f32, f32) = (360.0, 420.0);
/// Its background: the panel's frost, opaque (#1E1E1E; `Color` is linear).
const BACKGROUND: Color = Color { r: 0.013, g: 0.013, b: 0.013, a: 1.0 };
/// An enlarged capture takes at most this much of it.
const PREVIEW_MAX: [f32; 2] = [chat::CHAT_W, 560.0];

pub struct ChatWindow {
    pub chat: Chat,
    host: Option<AppHandle<Host>>,
}

impl ChatWindow {
    pub fn new(host: Option<AppHandle<Host>>) -> Self {
        ChatWindow { chat: Chat::new(host.clone()), host }
    }

    /// `AppSpec` wants an `init`; the window gets its state from `WindowSpec::state`.
    pub fn placeholder() -> Self {
        ChatWindow::new(None)
    }

    /// The chat goes back to the side panel; an empty one stays.
    pub fn take_chat(&mut self) -> Chat {
        std::mem::replace(&mut self.chat, Chat::new(self.host.clone()))
    }
}

/// Its titlebar: the chat's header, the traffic lights over its left end.
pub fn titlebar() -> Titlebar {
    Titlebar::new(chat::HEADER_HEIGHT)
}

fn lens(s: &mut ChatWindow) -> &mut Chat {
    &mut s.chat
}

pub fn view(s: &ChatWindow, cx: &mut Cx<ChatWindow>) -> El {
    crate::settings::apply_motion(cx);
    let t = tok::theme(true);
    let lead = cx.chrome_insets().leading.max(8.0) + 6.0;
    let header = chat::header(lead, &t).window_drag().child(chat::dock_button(&t, lens));
    let root = chat::body(&s.chat, header, lens).key("chat-window").w_full().h_full().fill(BACKGROUND);
    match s.chat.preview_image() {
        Some(image) => rune::el().w_full().h_full().child(root.pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))).child(
            chat::lightbox(image, PREVIEW_MAX, |s: &mut ChatWindow| s.chat.close_preview()),
        ),
        None => root,
    }
}

/// Its size popped out over a screen whose visible area (without the menu bar and the Dock)
/// is `visible` points: 30% of its width and 80% of its height, never below `MIN_SIZE`.
pub fn popped_size(visible: [f32; 2]) -> (f32, f32) {
    ((visible[0] * 0.3).round().max(MIN_SIZE.0), (visible[1] * 0.8).round().max(MIN_SIZE.1))
}

/// Just popped out, at the side panel's rect: it grows to its own size, centred.
pub fn popped_out(_: &mut ChatWindow, tx: &mut Tx, size: (f32, f32)) {
    tx.resize_window(size);
    tx.center_window();
}

/// The window closes (its close button, Cmd+W): it hides, keeping the conversation, and the
/// Dock icon goes.
pub fn on_close(s: &mut ChatWindow, _: &mut Tx) {
    if let Some(host) = &s.host {
        host.update(|host, _| host.chat_window_closed());
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use rune::{Env, Runtime};

    use super::*;
    use crate::mock_model;

    fn keys(rt: &Runtime<ChatWindow>) -> Vec<String> {
        rt.elements().into_iter().filter(|e| !e.exiting).filter_map(|e| e.key).collect()
    }

    #[test]
    fn the_window_shows_the_conversation_with_a_way_back_to_the_side_panel() {
        let mut w = ChatWindow::new(None);
        let id = chat::tests::sent(&mut w.chat, "code");
        let token = w.chat.ask(id).unwrap();
        for (_, event) in mock_model::script("code") {
            w.chat.reply_event(token, event, Instant::now());
        }
        w.chat.set_windowed(true);
        let mut rt = Runtime::new(w, view, SIZE, Env::new(true, 2.0));
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let keys = keys(&rt);
        for key in ["chat-window", "header", "dock-side", "messages", "msg-0", "reply-1", "draft"] {
            assert!(keys.iter().any(|k| k == key), "{key} in {keys:?}");
        }
        assert!(!keys.iter().any(|k| k == "pop-out" || k == "close"), "no pop-out or close: it's a window");
    }

    #[test]
    fn popped_out_it_takes_30_percent_of_the_screens_width_and_80_percent_of_its_height() {
        assert_eq!(popped_size([1920.0, 1055.0]), (576.0, 844.0));
        assert_eq!(popped_size([2560.0, 1415.0]), (768.0, 1132.0));
        assert_eq!(popped_size([1000.0, 500.0]), MIN_SIZE, "never below its smallest");
    }

    #[test]
    fn its_background_is_the_side_panels_frost_made_opaque() {
        assert_eq!(BACKGROUND.hex(), "#1e1e1eff");
    }

    #[test]
    fn the_side_panel_offers_its_window() {
        struct W {
            chat: Chat,
        }
        let mut chat = Chat::new(None);
        chat.place(chat::Insets::default(), [1440.0, 900.0]);
        chat.open();
        let view = |s: &W, _: &mut Cx<W>| chat::panel(&s.chat, "panel", |s: &mut W| &mut s.chat, None);
        let mut rt = Runtime::new(W { chat }, view, (1440.0, 900.0), Env::new(true, 2.0));
        rt.frame(1.0 / 60.0);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(keys.iter().any(|k| k == "pop-out") && keys.iter().any(|k| k == "close"), "{keys:?}");
    }
}
