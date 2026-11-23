//! Ailfred: an area screenshot tool. The hotkey captures the display under the pointer, then
//! shows it frozen in a frameless, transparent overlay above every other window. Drag to select;
//! on release the area goes to the clipboard and to ~/Desktop as a PNG.
//!
//! On macOS Rune hosts the overlay in a non-activating `NSPanel` at AppKit level 26 (above the
//! menu bar and Dock), on every Space and over full-screen apps.
//!
//! Cmd+Shift+Space captures. Escape cancels. Cmd+Q quits.

mod app_env;
mod capture;
mod app_icon;
mod chat;
mod chat_place;
mod chat_window;
mod computer;
mod export;
mod history;
mod login;
mod marks;
mod model;
mod output;
mod pi_setup;
mod icons;
mod keychain;
mod quick_input;
mod reply_view;
mod screens;
mod link;
mod settings;
mod settings_ui;
mod shot;
mod status;
mod tray;

// The engine and the conversation model, shared with other clients: `crate::reply` and the
// rest read as before.
use ailfred_core::{mock_model, pi, reply};

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
use crate::settings::{HotKey, Settings, ShortcutFor};
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
    /// Running from ~/Applications/Ailfred.app (login item and log file only there).
    installed: bool,
    /// A capture is running on its thread; a second press waits for it.
    capturing: Arc<AtomicBool>,
    /// The capture's and the quick input's shortcuts (shared with the settings window).
    capture_shortcut: HotKey,
    quick_shortcut: HotKey,
    /// The saved shortcuts (capture, quick input), registered at the loop's first turn.
    initial_shortcuts: Option<(Option<rune::Shortcut>, Option<rune::Shortcut>)>,
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
    /// The chat's own window is open.
    chat_window_open: bool,
    /// The capture overlay is on screen (set by the capture thread as it shows it, cleared on
    /// hide): it takes the keys, which mustn't close the quick input.
    capture_shown: Arc<AtomicBool>,
    /// The chat's own window, and its state.
    chat_window: Option<WindowHandle>,
    chat_ui: Option<AppHandle<ChatWindow>>,
    /// Where the chat is (the side panel, or its window, open or closed, until docked back),
    /// and what waits for it while it moves.
    place: ChatPlace<ChatUpdate>,
    /// Claude, and Ailfred's pi once it's been picked: whichever the setting names writes the
    /// next reply; Stop ends the one going.
    claude: model::Claude,
    pi: Option<pi_setup::Pi>,
    /// The thread that saves conversations (None only in the placeholder host).
    writer: Option<history::Writer>,
    /// The most recent conversations, as the menu lists them.
    recent: Vec<history::Entry>,
    /// What Ailfred is doing, for the menu-bar icon (its dot, too, when a reply ended while
    /// the chat was off screen).
    status: status::Tracker,
    /// Computer control's tools, served once it was first turned on in Settings, and its panic
    /// chord, held while it's on.
    computer: Option<computer::Running>,
    panic_shortcut: HotKey,
    /// pi started before computer control was last switched: its tools are out of date, so it
    /// starts again for the next message.
    pi_stale: bool,
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
            handle.update(move |host, _| host.land_in_window(chat));
        });
        if self.capture_shown.load(Ordering::Acquire) {
            self.dismiss_capture();
        }
        self.release();
    }

    /// The chat, out of the side panel, goes into its window, which opens.
    fn land_in_window(&mut self, mut chat: Chat) {
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
        let [.., width, height] = self.window_screen().visible;
        let size = chat_window::popped_size([width, height]);
        ui.update(move |w, tx| chat_window::popped_out(w, tx, size));
    }

    /// Opens the chat's window in front, with a Dock icon (an accessory app's window is hard
    /// to get back to without one). Asked on the window's own handle, so the switch to a
    /// Dock app and the window's opening apply in one turn, policy first (Rune's tested path).
    fn show_chat_window(&mut self) {
        if let Some(ui) = &self.chat_ui {
            ui.set_dock_icon(true);
        }
        if let Some(window) = &self.chat_window {
            window.open();
            window.focus();
            self.chat_window_open = true;
        }
        self.show_status();
        app_env::log_app_type_soon("chat window opened, Dock icon on");
    }

    /// Its window closed: the conversation stays in it for next time; the Dock icon goes.
    pub fn chat_window_closed(&mut self) {
        self.chat_window_open = false;
        self.show_status();
        if let Some(ui) = &self.chat_ui {
            ui.set_dock_icon(false);
        }
        app_env::log_app_type_soon("chat window closed, Dock icon off");
    }

    /// The window's dock button: the chat goes back into the side panel, which slides in.
    pub fn dock_back(&mut self) {
        let (Some(ui), Some(handle)) = (self.chat_ui.clone(), self.handle.clone()) else { return };
        if self.quick.is_none() || !self.place.start(Where::Side) {
            return;
        }
        ui.update(move |w, _| {
            let chat = w.take_chat();
            handle.update(move |host, _| host.land_in_side(chat));
        });
        if let Some(window) = &self.chat_window {
            window.hide();
        }
        self.chat_window_closed();
    }

    /// The chat, out of its window, goes back into the side panel, which slides in.
    fn land_in_side(&mut self, chat: Chat) {
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
        let message = chat::Message::text(id, text).with_pngs(&pngs);
        self.with_chat(move |c, tx| {
            c.push(message);
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
        self.show_status();
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
        self.show_status();
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
        let Some(quick) = self.quick.clone() else { return };
        if !std::mem::replace(&mut self.chat_open, false) {
            return;
        }
        self.show_status();
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
    pub fn stream_reply(&mut self, token: u64, script: mock_model::Script) {
        let Some(handle) = self.handle.clone() else { return };
        self.status.start(token, false);
        self.show_status();
        std::thread::spawn(move || {
            for (wait, event) in script {
                std::thread::sleep(wait);
                handle.update(move |host, _| host.reply_said(token, event));
            }
        });
    }

    /// The next event of reply `token`: into the chat, and the menu-bar icon.
    fn reply_said(&mut self, token: u64, event: reply::Event) {
        self.status.event(token, &event);
        self.show_status();
        self.with_chat(move |c, tx| {
            c.reply_event(token, event, std::time::Instant::now());
            tx.animated();
        });
    }

    /// The menu-bar icon and its tooltip say what Ailfred is doing.
    fn show_status(&mut self) {
        // Moving between panel and window counts as on screen: the user just acted.
        self.status.shown(self.chat_open || self.chat_window_open || self.place.moving());
        if let Some(tray) = &mut self.tray {
            tray.set_status(&self.status.status());
        }
    }

    /// Asks the model `request` for the reply `token`, off the main thread: what it streams
    /// comes into the chat as events; `Stop` ends the run (`cancel_runs`). A stopped or replaced
    /// reply ignores what still comes.
    pub fn ask_model(&mut self, token: u64, request: model::Request) {
        let Some(handle) = self.handle.clone() else { return };
        let sink: model::Sink = Arc::new(move |item| handle.update(move |host, _| host.model_said(token, item)));
        let (is_pi, steer) = (request.model == model::PI, request.steer);
        self.model(is_pi, steer).send(request, sink);
        if steer {
            self.status.steered(token);
            return;
        }
        self.status.start(token, is_pi && self.pi.as_ref().is_some_and(pi_setup::Pi::starting));
        self.show_status();
    }

    /// pi (started the first time it's asked for, and again after it failed to, or once computer
    /// control was switched since it started: it reads its tools as it starts) or Claude. A
    /// `steer` goes to the run going, so pi stays as it is for it.
    fn model(&mut self, pi: bool, steer: bool) -> &mut dyn model::Model {
        if !pi {
            return &mut self.claude;
        }
        if self.pi.as_ref().is_some_and(pi_setup::Pi::failed) || (self.pi_stale && !steer) {
            self.pi = None;
            self.pi_stale = false;
        }
        self.pi.get_or_insert_with(|| {
            let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            let data = home.join("Library/Application Support").join(settings::APP_ID).join("pi");
            pi_setup::Pi::start(pi_setup::pi_binary(&home), data, home)
        })
    }

    /// The chat's model picker picked `id`: the settings keep it (and show it); the chat shows it
    /// once kept.
    pub fn pick_model(&mut self, id: String) {
        let (Some(ui), Some(handle)) = (&self.settings_ui, self.handle.clone()) else { return };
        ui.update(move |s, tx| {
            s.choose_model(&id);
            tx.animated();
            handle.update(|host, _| host.with_chat(|_, tx| tx.animated()));
        });
    }

    /// Cmd+N: pi starts a new session (Claude is told the history each time).
    pub fn new_conversation(&mut self) {
        self.status.stop();
        self.show_status();
        if let Some(pi) = &mut self.pi {
            model::Model::new_conversation(pi);
        }
    }

    /// What the model's run for `token` reported.
    fn model_said(&mut self, token: u64, item: model::Item) {
        match item {
            model::Item::Session(session) => self.with_chat(move |c, _| c.set_session(token, session)),
            model::Item::Event(event) => {
                if matches!(event, reply::Event::Done | reply::Event::Fail(_)) {
                    self.claude.finished();
                }
                self.reply_said(token, event);
            }
        }
    }

    /// Stop, Retry, a new message or Cmd+N: the reply streaming (one at a time; the chat names
    /// it by `_tokens`) ends, with the process behind it.
    pub fn cancel_runs(&mut self, _tokens: &[u64]) {
        self.status.stop();
        self.show_status();
        model::Model::stop(&mut self.claude);
        if let Some(pi) = &mut self.pi {
            model::Model::stop(pi);
        }
    }

    /// A conversation to write: the writer thread does it, this one goes on.
    pub fn save_conversation(&mut self, snapshot: history::Snapshot) {
        if let Some(writer) = &self.writer {
            writer.save(snapshot);
        }
    }

    /// An automatic title for a conversation the chat has left: into its file.
    pub fn patch_title(&mut self, id: String, title: String) {
        if let Some(writer) = &self.writer {
            writer.set_title(id, title);
        }
    }

    /// Asks the cheap model for conversation `id`'s title, off the main thread; what comes
    /// (nothing, if it fails or takes more than `model::TITLE_TIMEOUT`) goes to the chat.
    pub fn ask_title(&mut self, id: String, request: model::Request) {
        let Some(handle) = self.handle.clone() else { return };
        model::ask_title(request, model::TITLE_TIMEOUT, move |title| {
            match title {
                Some(title) => handle.update(move |host, _| host.with_chat(move |c, _| c.title_arrived(&id, title))),
                None => eprintln!("Ailfred: no title for conversation {id}"),
            }
        });
    }

    /// The menu's list of recent conversations changed.
    pub fn set_recent(&mut self, list: Vec<history::Entry>) {
        if let Some(tray) = &mut self.tray {
            tray.set_recent(&list);
        }
        self.recent = list;
    }

    /// Launch: the conversation last updated comes back into the chat, which stays closed: the
    /// next press of the input's shortcut shows it. Not when the user has started one already.
    fn restore(&mut self, loaded: history::Loaded) {
        if self.chat_started || self.place.windowed() {
            return;
        }
        self.chat_started = true;
        self.load_into_chat(loaded);
    }

    /// A pick in the menu's Recent: that conversation is read off the main thread, and comes into
    /// the chat (what streams is stopped), which shows.
    pub fn open_recent(&mut self, id: String) {
        let Some(handle) = self.handle.clone() else { return };
        std::thread::spawn(move || match history::load(&history::dir_now(), &id) {
            Some(loaded) => handle.update(move |host, _| host.show_loaded(loaded)),
            None => eprintln!("Ailfred: conversation {id} couldn't be read"),
        });
    }

    fn show_loaded(&mut self, loaded: history::Loaded) {
        self.chat_started = true;
        self.load_into_chat(loaded);
        // Mid-move, its arrival shows it.
        if self.place.moving() {
            return;
        }
        if self.place.windowed() {
            self.show_chat_window();
        } else {
            if self.quick_open {
                self.hide_quick_input();
            }
            self.show_chat();
        }
    }

    fn load_into_chat(&mut self, loaded: history::Loaded) {
        self.with_chat(move |c, tx| {
            c.load(loaded);
            tx.scroll_to_bottom_instant("messages");
            tx.animated();
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
            Ok(mut tray) => {
                tray.set_recent(&self.recent);
                self.tray = Some(tray);
            }
            Err(e) => eprintln!("Ailfred: menu-bar icon failed: {e}"),
        }
    }

    /// Registers the saved shortcuts (main thread, loop running), each falling back to its
    /// default when taken, and shows the ones that work in the menu; Settings says what failed.
    fn register_saved_shortcuts(&mut self) {
        let Some(handle) = self.handle.clone() else { return };
        let Some((capture, quick)) = self.initial_shortcuts.take() else { return };
        let order = [ShortcutFor::Capture, ShortcutFor::QuickInput];
        let slots = [self.capture_shortcut.clone(), self.quick_shortcut.clone()];
        let started = settings::start_shortcuts(&[(capture, order[0].default_chord()), (quick, order[1].default_chord())], |i, s| slots[i].replace(&handle, s));
        for (which, started) in order.into_iter().zip(started) {
            let label = started.active.as_ref().map(|s| s.to_string()).unwrap_or_else(|| "none (menu only)".into());
            eprintln!("Ailfred ready: {} shortcut {label}", which.name());
            self.show_shortcut(which, started.active.as_ref());
            if let Some(notice) = started.notice {
                eprintln!("Ailfred: {} shortcut: {notice}", which.name());
                if let Some(ui) = &self.settings_ui {
                    let active = started.active.clone();
                    ui.update(move |ui, _| ui.shortcut_notice(which, active, notice));
                }
            }
        }
    }

    /// Shows a shortcut that works (None: none) in the menu; the capture's also in the input's hint.
    pub fn show_shortcut(&self, which: ShortcutFor, shortcut: Option<&rune::Shortcut>) {
        if let Some(tray) = &self.tray {
            tray.show_shortcut(which, shortcut);
        }
        if which == ShortcutFor::Capture
            && let Some(quick) = &self.quick
        {
            let chord = shortcut.map(|s| quick_input::chord_symbols(&s.keycaps()));
            quick.update(move |s, _| s.set_capture_chord(chord));
        }
    }

    /// Settings' computer-control switch (and its saved state at launch): on, the tools are
    /// served (started the first time); off, they refuse everything and the session ends.
    pub fn set_computer_control(&mut self, on: bool) {
        if on && self.computer.is_none() {
            let handle = self.handle.clone();
            let notify = move |state: &computer::session::StateEvent| {
                eprintln!("Ailfred: computer control {state:?}");
                let active = state.active;
                if let Some(handle) = &handle {
                    handle.update(move |host, _| host.show_computer_control(active));
                }
            };
            match computer::start(notify) {
                Ok(running) => {
                    eprintln!("Ailfred: computer control's tools are served at {}", running.endpoint.url);
                    self.computer = Some(running);
                }
                Err(e) => eprintln!("Ailfred: computer control couldn't start: {e}"),
            }
        }
        if let Some(running) = &self.computer {
            if running.control.enabled() != on && self.pi.is_some() {
                self.pi_stale = true;
            }
            computer::set_enabled(running, on);
        }
        // Off: the token Claude Code was handed doesn't stay on disk until its next run.
        if !on && let Err(e) = model::claude_code::write_computer_config(&model::claude_code::private_dir_now(), None) {
            eprintln!("Ailfred: removing Claude Code's computer tools failed: {e}");
        }
        // The panic chord is held only while computer control is on.
        if let Some(handle) = &self.handle {
            let chord = computer::PANIC_SHORTCUT.parse::<rune::Shortcut>().ok();
            match self.panic_shortcut.replace(handle, chord.as_ref().filter(|_| on)) {
                Ok(()) => eprintln!("Ailfred: computer control {}", if on { "on (⌥⇧Esc stops it)" } else { "off" }),
                Err(e) => eprintln!("Ailfred: the panic shortcut (⌥⇧Esc) isn't available: {e}; Stop is in the menu"),
            }
        }
    }

    /// Stop (the menu) or the panic chord: the session ends, and what was queued drops.
    pub fn stop_computer_control(&mut self, reason: computer::session::Reason) {
        if let Some(running) = &self.computer {
            running.control.end(reason);
        }
    }

    /// A session started or ended: the menu-bar icon says so.
    fn show_computer_control(&mut self, active: bool) {
        if let Some(tray) = &mut self.tray {
            tray.set_controlling(active);
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
            Action::OpenPermissionSettings => open(app_env::SCREEN_RECORDING_SETTINGS),
            Action::StopComputer => self.stop_computer_control(computer::session::Reason::Stopped),
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
                // The user looking: a good time to ask the Keychain what replies come from.
                if let Some(ui) = &self.settings_ui {
                    ui.update(|ui, _| ui.refresh_status());
                }
            }
            Action::Quit => {
                // What's queued to be saved is written first (a moment at most).
                if let Some(writer) = &self.writer {
                    writer.flush(Duration::from_secs(2));
                }
                tx.quit()
            }
        }
    }
}

/// A link the system opened with Ailfred (`ailfred://…`; it comes from anyone who can run `open`).
fn on_link(host: &mut Host, url: &str, _: &mut Tx) {
    match link::parse(url) {
        Some(link::Link::Toggle) => host.toggle_quick_input(),
        None => eprintln!("Ailfred: ignored a link it doesn't know ({} chars)", url.chars().count()),
    }
}

/// Opens a path or URL with its default app (`open`), without waiting for it.
pub(crate) fn open(target: &str) {
    if let Err(e) = std::process::Command::new("open").arg(target).spawn() {
        eprintln!("Ailfred: open {target} failed: {e}");
    }
}

fn host_init() -> Host {
    Host {
        overlay: None,
        handle: None,
        tray: None,
        installed: false,
        capturing: Arc::default(),
        capture_shortcut: HotKey::capture(),
        quick_shortcut: HotKey::quick_input(),
        initial_shortcuts: None,
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
        claude: model::Claude::default(),
        pi: None,
        writer: None,
        recent: Vec::new(),
        status: status::Tracker::default(),
        chat_window_open: false,
        computer: None,
        panic_shortcut: HotKey::computer_panic(),
        pi_stale: false,
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
            Err(e) => eprintln!("Ailfred: {e}"),
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
    // `--app-icon <out.png>`: writes the app's icon, 1024 px, for the bundle's `.icns`
    // (scripts/bundle-mac), and exits.
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag, out] = args.as_slice()
        && flag == "--app-icon"
    {
        if let Err(e) = std::fs::write(out, app_icon::png(1024)) {
            eprintln!("Ailfred: writing the icon to {out} failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    // The Dock's and app switcher's icon while the chat's window shows one (a dev build has no
    // bundle to take it from).
    if let Err(e) = rune::set_app_icon(app_icon::png(512)) {
        eprintln!("Ailfred: {e}");
    }
    let installed = app_env::in_bundle();
    if installed {
        app_env::log_to_file();
    }
    // accessory: no Dock icon and no activation at launch.
    rune::run_windows_with(RunOptions { accessory: true }, move |_displays| {
        let mut app = App::new(AppSpec::new("Ailfred", (1.0, 1.0), host_init, host_view)).windowless().on_open_url(on_link);
        let handle = app.handle();
        let overlay = app.overlay(
            // The size is replaced by the display's size: FullScreen placement.
            OverlaySpec::new("Ailfred overlay", (0.0, 0.0), shot::view)
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
            OverlaySpec::new("Ailfred quick input", (0.0, 0.0), quick_input::view)
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
        let settings = Settings::open(installed);
        let computer_control = settings.computer_control();
        let initial_shortcuts = Some((settings.shortcut(), settings.quick_shortcut()));
        let (capture_shortcut, quick_shortcut) = (HotKey::capture(), HotKey::quick_input());
        let ui = SettingsUi::new(settings, handle.clone(), capture_shortcut.clone(), quick_shortcut.clone());
        let hidden = WindowOptions { visible: false, resizable: false, min_size: Some((0.0, 0.0)), ..Default::default() };
        let (settings_ui, settings_window) = app.window(
            WindowSpec::new("settings", AppSpec::new("Ailfred Settings", settings_ui::SIZE, SettingsUi::placeholder, settings_ui::view))
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
        // Conversations are written by a thread of their own; the list it keeps goes to the menu.
        let folder = history::dir_now();
        let writer = {
            let handle = handle.clone();
            history::Writer::start(folder.clone(), move |list| handle.update(move |host, _| host.set_recent(list)))
        };
        // At launch, off the UI thread: what's saved gives the menu its list and the chat its
        // last conversation.
        {
            let (handle, writer) = (handle.clone(), writer.clone());
            std::thread::spawn(move || {
                let entries = history::scan(&folder);
                let latest = entries.iter().find_map(|e| history::load(&folder, &e.id));
                writer.seed(entries);
                if let Some(loaded) = latest {
                    handle.update(move |host, _| host.restore(loaded));
                }
            });
        }
        let app = app.init(move || Host {
            overlay: Some(overlay),
            handle: Some(own),
            tray: None,
            installed,
            capturing: Arc::default(),
            capture_shortcut,
            quick_shortcut,
            initial_shortcuts,
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
            claude: model::Claude::default(),
            pi: None,
            writer: Some(writer),
            recent: Vec::new(),
            status: status::Tracker::default(),
            chat_window_open: false,
            computer: None,
            panic_shortcut: HotKey::computer_panic(),
            pi_stale: false,
        });
        // Runs on the main thread at the loop's first turn: the status item needs a running NSApp,
        // and runtime shortcut registration needs the host started.
        handle.update(move |host, _| {
            host.add_tray();
            host.register_saved_shortcuts();
            if computer_control {
                host.set_computer_control(true);
            }
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
