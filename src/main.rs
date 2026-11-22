//! Capibarapp: an area screenshot tool. The hotkey captures the display under the pointer, then
//! shows it frozen in a frameless, transparent overlay above every other window. Drag to select;
//! on release the area goes to the clipboard and to ~/Desktop as a PNG.
//!
//! On macOS Rune hosts the overlay in a non-activating `NSPanel` at AppKit level 26 (above the
//! menu bar and Dock), on every Space and over full-screen apps.
//!
//! Cmd+Shift+Space captures. Escape cancels. Cmd+Q quits.

mod app_env;
mod capture;
mod chat;
mod chat_place;
mod chat_window;
mod export;
mod login;
mod marks;
mod mock_model;
mod output;
mod icons;
mod quick_input;
mod reply;
mod reply_view;
mod screens;
mod settings;
mod settings_ui;
mod shot;
mod tray;

use std::sync::Arc;
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};

use rune::{
    App, AppSpec, Cx, DisplayTarget, El, KeyPolicy, NativeDisplayId, OverlayHandle, OverlayInput, OverlayLevel,
    AppHandle, OverlayOptions, WindowHandle, WindowOptions, OverlayPlacement, OverlaySpec, RunOptions, Tx,
    WindowSpec, el,
};

use crate::quick_input::QuickInput;

use crate::shot::{Mode, Overlay};
use crate::settings::{CaptureShortcut, Settings};
use crate::settings_ui::SettingsUi;
use crate::chat::Chat;
use crate::chat_place::{ChatPlace, Route, Where};
use crate::chat_window::ChatWindow;
use crate::tray::{Action, Tray};

/// The windowless host app: owns the overlay and the menu-bar icon; the global shortcut and the
/// menu act on it.
pub struct Host {
    pub(crate) overlay: Option<OverlayHandle<Overlay>>,
    /// For work that finishes on another thread and reports back.
    handle: Option<AppHandle<Host>>,
    tray: Option<Tray>,
    /// Running from ~/Applications/Capibarapp.app (login item and log file only there).
    installed: bool,
    /// A capture is running on its thread; a second press waits for it.
    capturing: Arc<AtomicBool>,
    /// The capture shortcut's registration (shared with the settings window).
    capture_shortcut: CaptureShortcut,
    /// The saved shortcut, registered at the loop's first turn.
    initial_shortcut: Option<rune::Shortcut>,
    settings_window: Option<WindowHandle>,
    settings_ui: Option<AppHandle<SettingsUi>>,
    /// The quick input's and the chat's window (it covers the display), and whether the input is
    /// open (or opening).
    quick: Option<OverlayHandle<QuickInput>>,
    quick_open: bool,
    /// The window shows (or is about to hide) over this display; counts its showings, so a
    /// pending hide doesn't hide a newer one.
    shown: Option<screens::Screen>,
    shown_count: u64,
    /// The Dock's height under the open input at the bottom (0 when there's none).
    quick_dock: f32,
    /// How many images the input holds (it tells us), for the capture to leave its strip alone.
    quick_chips: usize,
    /// Counts the captures shown (main thread), so an earlier one's fade out doesn't hide a newer.
    overlay_shown: u64,
    /// The chat side panel (in the input's window) is open (or opening).
    chat_open: bool,
    /// A conversation is going (sent to, not ended by Cmd+N): the input's shortcut shows it.
    chat_started: bool,
    /// The capture overlay is on screen (set by the capture thread as it shows it, cleared on
    /// hide): it takes the keys, which mustn't close the quick input.
    capture_shown: Arc<AtomicBool>,
    /// The chat's own window, and its state.
    chat_window: Option<WindowHandle>,
    chat_ui: Option<AppHandle<ChatWindow>>,
    /// Where the chat is (the side panel, or its window, open or closed, until docked back),
    /// and what waits for it while it moves.
    place: ChatPlace<ChatUpdate>,
}

/// Something for the chat, wherever it is.
type ChatUpdate = Box<dyn FnOnce(&mut Chat, &mut Tx) + Send>;

impl Host {
    /// The input's shortcut: a started chat comes back (or goes) instead of a new input.
    pub fn toggle_quick_input(&mut self) {
        match chat::toggle(self.quick_open, self.chat_started, self.chat_open, self.place.windowed()) {
            chat::Toggle::ShowInput => self.show_quick_input(),
            chat::Toggle::HideInput => self.hide_quick_input(),
            chat::Toggle::ShowChat => self.show_chat(),
            chat::Toggle::HideChat => self.hide_chat(),
        }
    }

    /// The menu's item: the started chat, else the input.
    fn show_input_or_chat(&mut self) {
        if self.place.windowed() {
            self.show_chat_window();
        } else if self.chat_started {
            self.show_chat()
        } else {
            self.show_quick_input()
        }
    }

    /// `f` on the chat: in its window, or in the side panel's (the input's) window, or, while
    /// it moves between them, once it's arrived.
    fn with_chat(&mut self, f: impl FnOnce(&mut Chat, &mut Tx) + Send + 'static) {
        match self.place.route(Box::new(f)) {
            Route::Held => {}
            Route::Window(f) => {
                if let Some(ui) = &self.chat_ui {
                    ui.update(move |w, tx| f(&mut w.chat, tx));
                }
            }
            Route::Side(f) => {
                if let Some(quick) = &self.quick {
                    quick.update(move |s, tx| f(&mut s.chat, tx));
                }
            }
        }
    }

    /// The side panel's pop-out button: the chat moves into its own window, which opens where
    /// the panel was; the panel goes at once.
    pub fn pop_out(&mut self) {
        let (Some(quick), Some(handle)) = (self.quick.clone(), self.handle.clone()) else { return };
        if self.chat_ui.is_none() || !self.chat_open || !self.place.start(Where::Window) {
            return;
        }
        let screen = self.window_screen();
        let [x, y, width, height] = chat::side_rect(screen.size(), chat::insets(screen.frame, screen.visible));
        if let Some(window) = &self.chat_window {
            let display = match screen.id {
                0 => DisplayTarget::Primary,
                id => DisplayTarget::Native(NativeDisplayId::MacOS(id)),
            };
            window.place(display, OverlayPlacement::Rect { x, y, width, height });
        }
        self.chat_open = false;
        quick.update(move |s, tx| {
            let chat = s.take_chat();
            tx.instant();
            handle.update(move |host, _| host.into_window(chat));
        });
        if self.capture_shown.load(Ordering::Acquire) {
            self.dismiss_capture();
        }
        self.release();
    }

    /// The chat, out of the side panel, goes into its window, which opens.
    fn into_window(&mut self, mut chat: Chat) {
        let Some(ui) = self.chat_ui.clone() else {
            // No window to take it: it stays in the side panel, with what came meanwhile.
            let waiting = self.place.stayed();
            if let Some(quick) = &self.quick {
                quick.update(move |s, tx| {
                    s.put_chat(chat);
                    for f in waiting {
                        f(&mut s.chat, tx);
                    }
                });
            }
            self.show_chat();
            return;
        };
        let waiting = self.place.arrived();
        chat.set_windowed(true);
        ui.update(move |w, tx| {
            w.chat = chat;
            for f in waiting {
                f(&mut w.chat, tx);
            }
            tx.scroll_to_bottom_instant("messages");
        });
        self.show_chat_window();
        // Opened at the panel's rect, it grows to its own size, centred.
        ui.update(chat_window::popped_out);
    }

    /// Opens the chat's window in front, with a Dock icon (an accessory app's window is hard
    /// to get back to without one).
    fn show_chat_window(&self) {
        if let Some(handle) = &self.handle {
            handle.set_dock_icon(true);
        }
        if let Some(window) = &self.chat_window {
            window.open();
            window.focus();
        }
    }

    /// Its window closed: the conversation stays in it for next time; the Dock icon goes.
    pub fn chat_window_closed(&mut self) {
        if let Some(handle) = &self.handle {
            handle.set_dock_icon(false);
        }
    }

    /// The window's dock button: the chat goes back into the side panel, which slides in.
    pub fn dock_back(&mut self) {
        let (Some(ui), Some(handle)) = (self.chat_ui.clone(), self.handle.clone()) else { return };
        if self.quick.is_none() || !self.place.start(Where::Side) {
            return;
        }
        ui.update(move |w, _| {
            let chat = w.take_chat();
            handle.update(move |host, _| host.into_side(chat));
        });
        if let Some(window) = &self.chat_window {
            window.hide();
        }
        self.chat_window_closed();
    }

    /// The chat, out of its window, goes back into the side panel, which slides in.
    fn into_side(&mut self, chat: Chat) {
        let (Some(quick), Some(handle)) = (self.quick.clone(), self.handle.clone()) else {
            // No side panel to take it: it stays in its window, with what came meanwhile.
            let waiting = self.place.stayed();
            if let Some(ui) = &self.chat_ui {
                ui.update(move |w, tx| {
                    w.chat = chat;
                    w.chat.set_windowed(true);
                    for f in waiting {
                        f(&mut w.chat, tx);
                    }
                });
            }
            self.show_chat_window();
            return;
        };
        let waiting = self.place.arrived();
        quick.update(move |s, tx| {
            s.put_chat(chat);
            for f in waiting {
                f(&mut s.chat, tx);
            }
            tx.scroll_to_bottom_instant("messages");
            let has_messages = s.chat.has_messages();
            handle.update(move |host, _| host.docked(has_messages));
        });
    }

    /// Back in the side panel: it slides in, or, emptied in its window, a fresh input rises.
    fn docked(&mut self, has_messages: bool) {
        self.chat_started = has_messages;
        match chat::on_dock(has_messages) {
            chat::Toggle::ShowChat => self.show_chat(),
            _ => self.show_quick_input(),
        }
    }

    /// Sent from the input with the chat in its window (or on its way to or from it): the
    /// message goes to the chat, gets its reply, and the chat comes to the front.
    pub fn sent_to_window(&mut self, id: u64, text: String, pngs: Vec<Vec<u8>>) {
        self.quick_open = false;
        self.release();
        self.with_chat(move |c, tx| {
            c.push(chat::Message { id, text, images: Vec::new() });
            c.ask(id);
            tx.scroll_to_bottom("messages");
            tx.animated();
        });
        if !pngs.is_empty() {
            self.decode_into_chat(id, pngs);
        }
        // Mid-move, its arrival shows it.
        if self.place.moving() {
            return;
        }
        if self.place.windowed() {
            self.show_chat_window();
        } else {
            self.chat_started = true;
            self.show_chat();
        }
    }

    /// Cmd+N in the chat: it slides out past the right edge, over, and a new input rises. In its
    /// window, the conversation clears there.
    pub fn new_chat(&mut self) {
        self.chat_started = false;
        if self.place.windowed() {
            self.with_chat(|c, tx| {
                c.clear();
                tx.animated();
            });
            return;
        }
        if let (Some(quick), true) = (&self.quick, std::mem::replace(&mut self.chat_open, false)) {
            quick.update(|s, tx| {
                s.chat.start_over();
                tx.animated();
            });
        }
        if !self.quick_open {
            self.show_quick_input();
        }
    }

    /// The display the window covers: the one it shows on, else the one under the pointer.
    fn window_screen(&self) -> screens::Screen {
        self.shown.or_else(screens::under_pointer).unwrap_or(screens::Screen::FALLBACK)
    }

    /// Shows the window over `screen` (moving it there if it shows elsewhere); a pending hide
    /// is off.
    fn reveal(&mut self, screen: screens::Screen) {
        let Some(quick) = &self.quick else { return };
        self.shown_count += 1;
        if self.shown.map(|s| s.id) != Some(screen.id) {
            quick.set_display(match screen.id {
                0 => DisplayTarget::UnderPointer,
                id => DisplayTarget::Native(NativeDisplayId::MacOS(id)),
            });
        }
        self.shown = Some(screen);
        quick.show();
    }

    /// With the input and the chat both gone, the window hides once their slides are over.
    fn release(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        if self.quick_open || self.chat_open {
            return;
        }
        let count = self.shown_count;
        std::thread::spawn(move || {
            std::thread::sleep(quick_input::EXIT + Duration::from_millis(60));
            handle.update(move |host, _| {
                if host.shown_count == count && !host.quick_open && !host.chat_open {
                    host.shown = None;
                    if let Some(quick) = &host.quick {
                        quick.hide();
                    }
                }
            });
        });
    }

    /// Places the closed chat on `screen` (where it slides in from, or the input reshapes into).
    fn place_chat(quick: &OverlayHandle<QuickInput>, screen: screens::Screen) {
        let (insets, size) = (chat::insets(screen.frame, screen.visible), screen.size());
        quick.update(move |s, tx| {
            if !s.chat.is_open() {
                s.chat.place(insets, size);
                tx.scroll_to_bottom_instant("messages");
            }
            tx.instant();
        });
    }

    /// Slides the quick input up on the display under the pointer and gives it the keys, and
    /// starts an area capture there: dragging takes a screenshot that lands in the input.
    pub fn show_quick_input(&mut self) {
        let Some(quick) = self.quick.clone() else { return };
        self.quick_open = true;
        // Back at the bottom, wherever a capture area took it last time: the panel waits below
        // the screen's bottom edge (past a Dock there), out of view.
        let screen = self.window_screen();
        let dock = quick_input::dock_inset(screen.frame, screen.visible);
        self.quick_dock = dock;
        let rect = quick_input::bottom_panel(screen.size(), dock);
        quick.update(move |s, tx| {
            s.at_bottom(dock);
            s.set_rect(rect);
            tx.instant();
        });
        Host::place_chat(&quick, screen);
        self.reveal(screen);
        quick.request_keys();
        // Once the window is on screen, the panel rises from the bottom edge to its place (a
        // rise started while hidden would play out before the first visible frame).
        if let Some(handle) = self.handle.clone() {
            std::thread::spawn(move || {
                std::thread::sleep(quick_input::ARRIVAL_DELAY);
                handle.update(|host, _| {
                    if let (true, Some(quick)) = (host.quick_open, &host.quick) {
                        quick.update(|s, tx| {
                            s.open();
                            tx.animated();
                        });
                    }
                });
            });
        }
    }

    /// Ends a capture on screen without exporting.
    fn dismiss_capture(&self) {
        self.hide_overlay();
    }

    /// Slides the quick input down; the window fades out over the slide, then hides.
    pub fn hide_quick_input(&mut self) {
        let Some(quick) = &self.quick else { return };
        if !self.quick_open {
            return;
        }
        self.quick_open = false;
        quick.update(|s, tx| {
            s.close();
            tx.animated();
        });
        // Closing the input ends the capture it started (or one still on screen).
        if self.capture_shown.load(Ordering::Acquire) {
            self.dismiss_capture();
        }
        self.release();
    }

    /// The quick input lost the keys: a click elsewhere closes it, a capture taking them doesn't.
    pub fn quick_input_blurred(&mut self) {
        if !self.capture_shown.load(Ordering::Acquire) {
            self.hide_quick_input();
        }
    }

    /// A capture area drag started (true) or ended (false): the open input slides out of the
    /// screen and back.
    pub fn tuck_quick_input(&self, tucked: bool) {
        let Some(quick) = &self.quick else { return };
        if self.quick_open {
            quick.update(move |s, tx| {
                s.set_tucked(tucked);
                tx.animated();
            });
            if !tucked {
                quick.request_keys();
            }
        }
    }

    /// Enter or the send button pressed the input's panel in: once it's down, what's in it goes.
    pub fn send_after_press(&self, press: u64) {
        let Some(quick) = self.quick.clone() else { return };
        std::thread::spawn(move || {
            std::thread::sleep(quick_input::PRESS);
            quick.update(move |s, tx| {
                s.release(press);
                tx.animated();
            });
        });
    }

    /// Enter in the quick input sent message `id` (`QuickInput::send`): the input's panel is
    /// the chat's now, springing to the side; its content fades in after `chat::LANDING`. The
    /// message's captures (`pngs`) are decoded off the main thread and join it when ready.
    pub fn sent_from_quick_input(&mut self, id: u64, pngs: Vec<Vec<u8>>) {
        let Some(quick) = self.quick.clone() else { return };
        self.quick_open = false;
        // Sent as the chat left for its window: the message went with it, and the window shows.
        if self.place.windowed() {
            self.release();
            if !pngs.is_empty() {
                self.decode_into_chat(id, pngs);
            }
            return;
        }
        self.chat_open = true;
        self.chat_started = true;
        quick.request_keys();
        let lander = quick.clone();
        std::thread::spawn(move || {
            std::thread::sleep(chat::LANDING);
            lander.update(|s, tx| {
                s.land();
                tx.animated();
            });
        });
        if !pngs.is_empty() {
            self.decode_into_chat(id, pngs);
        }
    }

    /// Brings the started chat back, its field given the keys: it waits past the right edge of
    /// the display, then slides in.
    fn show_chat(&mut self) {
        let Some(quick) = self.quick.clone() else { return };
        if self.chat_open {
            quick.request_keys();
            return;
        }
        self.chat_open = true;
        let screen = self.window_screen();
        Host::place_chat(&quick, screen);
        self.reveal(screen);
        quick.request_keys();
        // On screen first, then the slide (one started while hidden would play out unseen).
        let Some(handle) = self.handle.clone() else { return };
        std::thread::spawn(move || {
            std::thread::sleep(quick_input::ARRIVAL_DELAY);
            handle.update(|host, _| {
                if let (true, Some(quick)) = (host.chat_open, &host.quick) {
                    quick.update(|s, tx| {
                        s.open_chat();
                        tx.animated();
                    });
                }
            });
        });
    }

    /// Esc or the close button: the chat slides out past the right edge (the window hides after,
    /// unless the input is up). The conversation stays for next time.
    pub fn hide_chat(&mut self) {
        let Some(quick) = &self.quick else { return };
        if !std::mem::replace(&mut self.chat_open, false) {
            return;
        }
        quick.update(|s, tx| {
            s.chat.close();
            tx.animated();
        });
        // Its screenshot goes with it.
        if !self.quick_open && self.capture_shown.load(Ordering::Acquire) {
            self.dismiss_capture();
        }
        self.release();
    }

    /// Image `id`, pasted into `field` (PNG, and `preview` decoded to show), joins it if it's
    /// still open.
    pub fn pasted(&mut self, field: chat::Field, id: u64, png: Vec<u8>, preview: chat::Image) {
        let Some(quick) = &self.quick else { return };
        if !chat::paste_lands(field, self.quick_open, self.chat_open || self.place.windowed()) {
            return;
        }
        match field {
            chat::Field::Input => quick.update(move |s, tx| {
                s.pasted(id, png, preview);
                tx.animated();
            }),
            chat::Field::Chat => self.with_chat(move |c, tx| {
                c.attach(id, png, preview);
                tx.animated();
            }),
        }
    }

    /// The input holds `n` images now.
    pub fn set_quick_chips(&mut self, n: usize) {
        self.quick_chips = n;
        // A capture for it on screen leaves the input's panel, now this tall, alone.
        if let Some(overlay) = &self.overlay {
            overlay.update_if(move |o, _| o.input_chips(n));
        }
    }

    /// Who a capture is for: the open input, else the open chat, else itself.
    fn capture_target(&self) -> shot::For {
        if self.quick_open {
            shot::For::Input { dock: self.quick_dock, chips: self.quick_chips }
        } else if self.chat_open {
            let screen = self.window_screen();
            shot::For::Chat { panel: chat::side_rect(screen.size(), chat::insets(screen.frame, screen.visible)) }
        } else {
            shot::For::Nothing
        }
    }

    /// Streams a reply (`token`) into the chat off the main thread, each event after its wait.
    /// A stopped or replaced reply ignores what still comes.
    pub fn stream_reply(&self, token: u64, script: mock_model::Script) {
        let Some(handle) = self.handle.clone() else { return };
        std::thread::spawn(move || {
            for (wait, event) in script {
                std::thread::sleep(wait);
                handle.update(move |host, _| {
                    host.with_chat(move |c, tx| {
                        c.reply_event(token, event, std::time::Instant::now());
                        tx.animated();
                    })
                });
            }
        });
    }

    /// Message `id`'s captures, decoded off the main thread, join it in the chat.
    pub fn decode_into_chat(&self, id: u64, pngs: Vec<Vec<u8>>) {
        let Some(handle) = self.handle.clone() else { return };
        std::thread::spawn(move || {
            let images: Vec<chat::Image> = pngs.iter().filter_map(|png| chat::Image::from_png(png)).collect();
            handle.update(move |host, _| {
                host.with_chat(move |c, tx| {
                    c.set_images(id, images);
                    tx.animated();
                })
            });
        });
    }

    /// Registers the quick input's fixed chord. Main thread, loop running.
    fn register_quick_shortcut(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        let Ok(shortcut) = quick_input::SHORTCUT.parse::<rune::Shortcut>() else { return };
        if let Err(e) = handle.register_shortcut(shortcut, |host: &mut Host, _| host.toggle_quick_input()) {
            eprintln!("Capibarapp: the quick input shortcut {} couldn't be registered: {e}", quick_input::SHORTCUT);
        }
    }

    pub fn hide_overlay(&self) {
        self.capture_shown.store(false, Ordering::Release);
        // A capture cancelled mid-drag leaves the input tucked; it comes back and takes the keys.
        self.tuck_quick_input(false);
        if let (Some(quick), true) = (&self.quick, self.quick_open || self.chat_open) {
            quick.request_keys();
        }
        // Its content fades to transparent with the window still up, then the window hides and
        // the capture is freed, unless another capture showed meanwhile. The window's last frame
        // is then an empty one: the next reveal can't flash the old capture.
        let Some(overlay) = self.overlay.clone() else { return };
        overlay.set_input(OverlayInput::None);
        overlay.update(|s, tx| {
            s.leave();
            tx.animated();
        });
        let (shown, handle) = (self.overlay_shown, self.handle.clone());
        std::thread::spawn(move || {
            std::thread::sleep(shot::FADE_OUT + Duration::from_millis(60));
            let Some(handle) = handle else { return };
            handle.update(move |host, _| {
                if host.overlay_shown == shown && !host.capture_shown.load(Ordering::Acquire) {
                    overlay.update(|s, _| s.clear());
                    overlay.hide();
                }
            });
        });
    }

    /// Creates the menu-bar icon. Main thread, event loop running: called from a queued update.
    fn add_tray(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        // The checkmark starts off; the startup thread reads the real state off the UI thread.
        match Tray::new(handle, self.installed, false) {
            Ok(tray) => self.tray = Some(tray),
            Err(e) => eprintln!("Capibarapp: menu-bar icon failed: {e}"),
        }
    }

    /// Registers the saved capture shortcut (main thread, loop running) and shows it in the menu.
    fn register_saved_shortcut(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        let shortcut = self.initial_shortcut.take();
        let label = shortcut.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "none (menu only)".into());
        match self.capture_shortcut.replace(&handle, shortcut.as_ref()) {
            Ok(()) => {
                eprintln!("Capibarapp ready: capture shortcut {label}");
                self.show_capture_shortcut(shortcut.as_ref());
            }
            Err(e) => {
                eprintln!("Capibarapp: the capture shortcut {label} couldn't be registered: {e}");
                if let Some(ui) = &self.settings_ui {
                    let message = format!("The shortcut isn't active: {e}. Record it again or pick another.");
                    ui.update(move |ui, _| ui.shortcut_failed(message));
                }
            }
        }
    }

    /// Shows the active capture shortcut (None: unset) in the menu and the input's hint.
    pub fn show_capture_shortcut(&self, shortcut: Option<&rune::Shortcut>) {
        if let Some(tray) = &self.tray {
            tray.show_capture_shortcut(shortcut);
        }
        if let Some(quick) = &self.quick {
            let chord = shortcut.map(|s| quick_input::chord_symbols(&s.keycaps()));
            quick.update(move |s, _| s.set_capture_chord(chord));
        }
    }

    fn set_permission(&mut self, allowed: bool) {
        if let Some(tray) = &mut self.tray {
            tray.set_permission(allowed);
        }
    }

    pub fn on_menu(&mut self, action: Action, tx: &mut Tx) {
        match action {
            // Our windows (the menu, the highlight) are excluded from the capture; the pause lets
            // the menu bar settle and the app under the menu redraw.
            Action::Capture => start_capture(self, Duration::from_millis(200), Mode::Area),
            Action::CaptureWindow => start_capture(self, Duration::from_millis(200), Mode::Window),
            Action::CaptureFullScreen => start_capture(self, Duration::from_millis(200), Mode::FullScreen),
            Action::QuickInput => self.show_input_or_chat(),
            Action::OpenFolder => open(&app_env::screenshots_dir()),
            Action::OpenPermissionSettings => open("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"),
            Action::ToggleLogin => {
                let (Some(handle), Some(tray)) = (self.handle.clone(), &self.tray) else { return };
                // muda already flipped the checkmark: its new state is the request. The service
                // manager can block, so ask off the UI thread; the item stays disabled meanwhile.
                let on = tray.login_checked();
                tray.set_login_busy(true);
                std::thread::spawn(move || {
                    let now = login::set(on);
                    handle.update(move |host, _| {
                        if let Some(tray) = &host.tray {
                            tray.set_login(now);
                            tray.set_login_busy(false);
                        }
                    });
                });
            }
            Action::Settings => {
                if let Some(window) = &self.settings_window {
                    window.open();
                }
            }
            Action::Quit => tx.quit(),
        }
    }
}

/// Opens a path or URL with its default app (`open`), without waiting for it.
fn open(target: &str) {
    if let Err(e) = std::process::Command::new("open").arg(target).spawn() {
        eprintln!("Capibarapp: open {target} failed: {e}");
    }
}

fn host_init() -> Host {
    Host {
        overlay: None,
        handle: None,
        tray: None,
        installed: false,
        capturing: Arc::default(),
        capture_shortcut: CaptureShortcut::default(),
        initial_shortcut: None,
        settings_window: None,
        settings_ui: None,
        quick: None,
        quick_open: false,
        quick_dock: 0.0,
        quick_chips: 0,
        shown: None,
        shown_count: 0,
        overlay_shown: 0,
        chat_open: false,
        chat_started: false,
        capture_shown: Arc::default(),
        chat_window: None,
        chat_ui: None,
        place: ChatPlace::default(),
    }
}

fn host_view(_: &Host, _: &mut Cx<Host>) -> El {
    el()
}

pub(crate) fn on_shortcut(host: &mut Host, _: &mut Tx) {
    start_capture(host, Duration::ZERO, Mode::Area);
}

/// Captures off the UI thread after `delay`, then shows the frozen display (starting in `mode`) on the display it came
/// from. A missing Screen Recording permission shows in the menu. With the quick input or the chat
/// open, a label follows the pointer and their panel is left alone; the capture still copies
/// and saves, and Cmd+V puts it in their field.
fn start_capture(host: &mut Host, delay: Duration, mode: Mode) {
    let target = host.capture_target();
    let Some(overlay) = host.overlay.clone() else { return };
    if host.capturing.swap(true, Ordering::AcqRel) {
        return;
    }
    let capturing = Busy(host.capturing.clone());
    let capture_shown = host.capture_shown.clone();
    let handle = host.handle.clone();
    std::thread::spawn(move || {
        let _capturing = capturing; // cleared on drop, even if the capture panics
        std::thread::sleep(delay);
        let result = capture::display_under_pointer();
        let allowed = result.is_ok() || app_env::screen_recording_allowed_quiet();
        if let Some(handle) = &handle {
            handle.update(move |host, _| host.set_permission(allowed));
        }
        match result {
            // Shown from the main thread, so it's ordered with a fade out's hide (`hide_overlay`).
            Ok(shot) => match &handle {
                Some(handle) => handle.update(move |host, _| {
                    host.overlay_shown += 1;
                    show_capture(&overlay, &capture_shown, shot, mode, target);
                }),
                None => show_capture(&overlay, &capture_shown, shot, mode, target),
            },
            Err(e) => eprintln!("Capibarapp: {e}"),
        }
    });
}

/// Puts `shot` on screen in `mode`, fading in.
fn show_capture(overlay: &OverlayHandle<Overlay>, shown: &AtomicBool, shot: capture::Shot, mode: Mode, target: shot::For) {
    let display = shot.display_id;
    let from_input = target != shot::For::Nothing;
    // Queued in this order: state, placement, reveal. They may be applied over more than one
    // event-loop turn; the update is queued before the reveal, and a reveal refreshes first.
    overlay.update(move |s, tx| {
        s.freeze(shot, mode, target);
        // Back from a fade out cut short, the content fades in again rather than jump.
        tx.animated();
    });
    // For the input or the chat, the overlay leaves its panel's rect to it (`Overlay::input_at`).
    overlay.set_input(if from_input { OverlayInput::Regions } else { OverlayInput::All });
    overlay.set_display(DisplayTarget::Native(NativeDisplayId::MacOS(display)));
    shown.store(true, Ordering::Release);
    overlay.fade_in(shot::FADE_IN);
    // The capture's keys (Esc, Space, F, the tools, Enter), for the input or chat too; they go
    // back to its field as it closes (`hide_overlay`).
    overlay.request_keys();
}

/// Clears the "capture running" flag when dropped.
struct Busy(Arc<AtomicBool>);

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn main() {
    let installed = app_env::in_bundle();
    if installed {
        app_env::log_to_file();
    }
    // accessory: no Dock icon and no activation at launch.
    rune::run_windows_with(RunOptions { accessory: true }, move |_displays| {
        let mut app = App::new(AppSpec::new("Capibarapp", (1.0, 1.0), host_init, host_view)).windowless();
        let handle = app.handle();
        let overlay = app.overlay(
            // The size is replaced by the display's size: FullScreen placement.
            OverlaySpec::new("Capibarapp overlay", (0.0, 0.0), shot::view)
                .options(OverlayOptions {
                    display: DisplayTarget::UnderPointer,
                    placement: Some(OverlayPlacement::FullScreen),
                    level: OverlayLevel::AboveStatus,
                    input: OverlayInput::All,
                    keys: KeyPolicy::OnDemand,
                    // Belt and braces: the capture filter also leaves out all of our windows.
                    exclude_from_capture: true,
                    prewarm: true,
                    ..Default::default()
                })
                .on_key(shot::on_key),
            Overlay::new(handle.clone()),
        );
        let quick = app.overlay(
            // The input and the chat in one window over the whole display (FullScreen replaces
            // the size): the input's panel turns into the chat's wherever it is on it.
            OverlaySpec::new("Capibarapp quick input", (0.0, 0.0), quick_input::view)
                .options(OverlayOptions {
                    display: DisplayTarget::UnderPointer,
                    placement: Some(OverlayPlacement::FullScreen),
                    // Above the capture overlay, so it stays in view over a frozen screen.
                    level: OverlayLevel::AboveOverlays,
                    // Only the open panels take clicks; the rest passes them through.
                    input: OverlayInput::Regions,
                    keys: KeyPolicy::OnDemand,
                    exclude_from_capture: true,
                    prewarm: true,
                    ..Default::default()
                })
                .on_key(quick_input::on_key)
                .on_focus(quick_input::on_focus)
                .animate_reveal(),
            QuickInput::new(Some(handle.clone())),
        );
        let settings = Settings::open();
        let initial_shortcut = settings.shortcut();
        let capture_shortcut = CaptureShortcut::default();
        let ui = SettingsUi::new(settings, handle.clone(), capture_shortcut.clone());
        let hidden = WindowOptions { visible: false, resizable: false, min_size: Some((0.0, 0.0)), ..Default::default() };
        let (settings_ui, settings_window) = app.window(
            WindowSpec::new("settings", AppSpec::new("Capibarapp Settings", (460.0, 260.0), SettingsUi::placeholder, settings_ui::view))
                .state(ui)
                .options(hidden),
        );
        // The chat's own window, hidden until the chat pops out into it; closing it keeps it
        // (and the conversation) for next time.
        let chat_options = WindowOptions {
            visible: false,
            resizable: true,
            min_size: Some(chat_window::MIN_SIZE),
            ..Default::default()
        };
        let (chat_ui, chat_window) = app.window(
            WindowSpec::from_app(
                "chat",
                App::new(AppSpec::new("Chat", chat_window::SIZE, ChatWindow::placeholder, chat_window::view))
                    .titlebar(chat_window::titlebar())
                    .on_close(chat_window::on_close)
                    .keep_running(),
            )
            .state(ChatWindow::new(Some(handle.clone())))
            .options(chat_options),
        );
        let own = handle.clone();
        let app = app.init(move || Host {
            overlay: Some(overlay),
            handle: Some(own),
            tray: None,
            installed,
            capturing: Arc::default(),
            capture_shortcut,
            initial_shortcut,
            settings_window: Some(settings_window),
            settings_ui: Some(settings_ui),
            quick: Some(quick),
            quick_open: false,
            quick_dock: 0.0,
            quick_chips: 0,
            shown: None,
            shown_count: 0,
            overlay_shown: 0,
            chat_open: false,
            chat_started: false,
            capture_shown: Arc::default(),
            chat_window: Some(chat_window),
            chat_ui: Some(chat_ui),
            place: ChatPlace::default(),
        });
        // Runs on the main thread at the loop's first turn: the status item needs a running NSApp,
        // and runtime shortcut registration needs the host started.
        handle.update(|host, _| {
            host.add_tray();
            host.register_saved_shortcut();
            host.register_quick_shortcut();
        });
        // Off the UI thread: both talk to system services.
        std::thread::spawn(move || {
            if installed {
                login::register_if_needed();
            }
            let allowed = app_env::screen_recording_allowed();
            let login = installed && login::enabled();
            handle.update(move |host, _| {
                host.set_permission(allowed);
                if let Some(tray) = &host.tray {
                    tray.set_login(login);
                }
            });
        });
        vec![WindowSpec::from_app("host", app)]
    });
}
