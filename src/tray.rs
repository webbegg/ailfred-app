//! The menu-bar icon and its menu (`tray-icon` + `muda`; Rune's tray has no macOS backend).
//! Created on the main thread once the event loop runs; menu picks are forwarded to the host app.

use rune::AppHandle;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::Host;
use crate::history::Entry;
use crate::status::Status;

/// What a menu pick asks the host to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Capture,
    CaptureWindow,
    CaptureFullScreen,
    QuickInput,
    OpenFolder,
    ToggleLogin,
    OpenPermissionSettings,
    Settings,
    Quit,
}

impl Action {
    const ALL: [(Action, &'static str); 9] = [
        (Action::Capture, "capture"),
        (Action::CaptureWindow, "capture-window"),
        (Action::CaptureFullScreen, "capture-full-screen"),
        (Action::QuickInput, "quick-input"),
        (Action::OpenFolder, "open-folder"),
        (Action::ToggleLogin, "login"),
        (Action::OpenPermissionSettings, "permission"),
        (Action::Settings, "settings"),
        (Action::Quit, "quit"),
    ];
    fn id(self) -> &'static str {
        Action::ALL.iter().find(|(a, _)| *a == self).map(|(_, id)| *id).unwrap_or("")
    }
    fn from_id(id: &str) -> Option<Action> {
        Action::ALL.iter().find(|(_, i)| *i == id).map(|(a, _)| *a)
    }
}

/// What a menu pick is: one of the fixed items, or a conversation from Recent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Action(Action),
    Recent(String),
}

/// The menu id of a recent conversation's item: the prefix, then its id.
const RECENT_PREFIX: &str = "recent:";

impl Pick {
    pub fn from_id(id: &str) -> Option<Pick> {
        match id.strip_prefix(RECENT_PREFIX) {
            Some(conversation) => Some(Pick::Recent(conversation.to_owned())),
            None => Action::from_id(id).map(Pick::Action),
        }
    }
}

pub struct Tray {
    /// Dropping it removes the icon.
    icon: TrayIcon,
    capture: MenuItem,
    menu: Menu,
    /// The most recent conversations, after "Quick Input"; in the menu only while there are any.
    recent: Submenu,
    recent_shown: bool,
    login: CheckMenuItem,
    permission: MenuItem,
    permission_shown: bool,
    can_login: bool,
    /// What the icon shows now.
    glyph: Glyph,
    tooltip: String,
}

impl Tray {
    /// Main thread only, after the event loop started. `can_login` is false outside the installed
    /// bundle, where a login item would mean nothing.
    pub fn new(host: AppHandle<Host>, can_login: bool, login_enabled: bool) -> Result<Tray, String> {
        // The chord is set by `show_capture_shortcut` once the saved one is registered.
        let capture = MenuItem::with_id(Action::Capture.id(), "Capture Area", true, None);
        let window = MenuItem::with_id(Action::CaptureWindow.id(), "Capture Window", true, None);
        let full = MenuItem::with_id(Action::CaptureFullScreen.id(), "Capture Full Screen", true, None);
        // Shows the fixed chord (`quick_input::SHORTCUT`, Ctrl+Option+Space).
        let quick = MenuItem::with_id(
            Action::QuickInput.id(),
            "Quick Input",
            true,
            Some(Accelerator::new(Modifiers::CONTROL | Modifiers::ALT, Code::Space)),
        );
        let folder = MenuItem::with_id(Action::OpenFolder.id(), "Open Screenshots Folder", true, None);
        let login = CheckMenuItem::with_id(Action::ToggleLogin.id(), "Launch at Login", can_login, login_enabled, None);
        let permission = MenuItem::with_id(
            Action::OpenPermissionSettings.id(),
            "Screen Recording Is Off: Open Settings, Then Relaunch…",
            true,
            None,
        );
        let settings = MenuItem::with_id(Action::Settings.id(), "Settings…", true, Some(Accelerator::new(Modifiers::META, Code::Comma)));
        let quit = MenuItem::with_id(Action::Quit.id(), "Quit Ailfred", true, Some(Accelerator::new(Modifiers::META, Code::KeyQ)));
        let menu = Menu::new();
        menu.append_items(&[
            &capture,
            &window,
            &full,
            &PredefinedMenuItem::separator(),
            &quick,
            &PredefinedMenuItem::separator(),
            &folder,
            &PredefinedMenuItem::separator(),
            &login,
            &settings,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;

        let icon = TrayIconBuilder::new()
            .with_icon_templated(icon()?)
            .with_tooltip("Ailfred")
            .with_menu(Box::new(menu.clone()))
            .build()
            .map_err(|e| e.to_string())?;

        // muda calls this on the main thread from the menu's action; the host handles it next turn.
        // muda keeps the first handler set in the process: build one Tray per process.
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            match Pick::from_id(&event.id.0) {
                Some(Pick::Action(action)) => host.update(move |host, tx| host.on_menu(action, tx)),
                Some(Pick::Recent(id)) => host.update(move |host, _| host.open_recent(id)),
                None => {}
            }
        }));
        let recent = Submenu::with_id("recent", "Recent", true);
        Ok(Tray { icon, capture, menu, recent, recent_shown: false, login, permission, permission_shown: false, can_login, glyph: Glyph::Plain, tooltip: "Ailfred".into() })
    }

    /// The icon and its tooltip show `status` (each set only when it changes).
    pub fn set_status(&mut self, status: &Status) {
        let glyph = Glyph::of(status);
        if glyph != self.glyph {
            match glyph_icon(glyph).and_then(|icon| self.icon.set_icon_templated(Some(icon)).map_err(|e| e.to_string())) {
                Ok(()) => self.glyph = glyph,
                Err(e) => eprintln!("Ailfred: updating the menu-bar icon failed: {e}"),
            }
        }
        let tooltip = status.tooltip();
        if tooltip != self.tooltip {
            if let Err(e) = self.icon.set_tooltip(Some(&tooltip)) {
                eprintln!("Ailfred: updating the menu-bar tooltip failed: {e}");
            }
            self.tooltip = tooltip;
        }
    }

    /// Shows the capture chord next to "Capture Area" (none when cleared). Rune writes chords as
    /// `shift+super+space`, which muda's accelerator parser reads.
    pub fn show_capture_shortcut(&self, shortcut: Option<&rune::Shortcut>) {
        let accelerator = shortcut.and_then(|s| s.to_string().parse::<Accelerator>().ok());
        if let Err(e) = self.capture.set_accelerator(accelerator) {
            eprintln!("Ailfred: showing the shortcut in the menu failed: {e}");
        }
    }

    /// Shows `entries` (newest first) under "Recent", which is in the menu only while there are
    /// any.
    pub fn set_recent(&mut self, entries: &[Entry]) {
        while self.recent.remove_at(0).is_some() {}
        for entry in entries {
            let item = MenuItem::with_id(format!("{RECENT_PREFIX}{}", entry.id), &entry.label, true, None);
            if let Err(e) = self.recent.append(&item) {
                eprintln!("Ailfred: listing a recent conversation failed: {e}");
            }
        }
        if entries.is_empty() == !self.recent_shown {
            return;
        }
        let done = if entries.is_empty() {
            self.menu.remove(&self.recent)
        } else {
            let after_quick_input = self.menu.items().iter().position(|item| item.id().0 == Action::QuickInput.id()).map_or(0, |at| at + 1);
            self.menu.insert(&self.recent, after_quick_input)
        };
        match done {
            Ok(()) => self.recent_shown = !entries.is_empty(),
            Err(e) => eprintln!("Ailfred: updating the menu failed: {e}"),
        }
    }

    pub fn set_login(&self, enabled: bool) {
        self.login.set_checked(enabled);
    }

    /// The checkmark as the user left it (muda toggles it before sending the click).
    pub fn login_checked(&self) -> bool {
        self.login.is_checked()
    }

    /// Greys the item out while a change is in flight, so two clicks can't race.
    pub fn set_login_busy(&self, busy: bool) {
        self.login.set_enabled(!busy && self.can_login);
    }

    /// Shows the "Screen Recording is off" row at the top while capture isn't allowed.
    pub fn set_permission(&mut self, allowed: bool) {
        if allowed == !self.permission_shown {
            return;
        }
        let done = if allowed { self.menu.remove(&self.permission) } else { self.menu.insert(&self.permission, 0) };
        match done {
            Ok(()) => self.permission_shown = !allowed,
            Err(e) => eprintln!("Ailfred: updating the menu failed: {e}"),
        }
    }
}

/// The menu-bar icon's side in pixels: tray-icon treats pixels as points (scaling down only past
/// 22 pt), so it shows at 22 pt.
const SIZE: usize = 36;

/// A template icon: the app's mark, black with alpha; macOS tints it.
fn icon() -> Result<Icon, String> {
    glyph_icon(Glyph::Plain)
}

fn glyph_icon(glyph: Glyph) -> Result<Icon, String> {
    Icon::from_rgba(glyph_rgba(glyph), SIZE as u32, SIZE as u32).map_err(|e| e.to_string())
}

/// The icon's pixels, RGBA.
#[cfg(test)]
fn icon_rgba() -> Vec<u8> {
    glyph_rgba(Glyph::Plain)
}

/// How the icon shows a [`Status`]: the mark, faded while the model starts, with a badge in its
/// lower right corner, cut out of it, while it works (a ring), when it wants a look (a dot), or
/// after a failure (a cross).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Plain,
    Faded,
    Busy,
    Look,
    Failed,
}

impl Glyph {
    pub fn of(status: &Status) -> Glyph {
        match status {
            Status::Idle => Glyph::Plain,
            Status::Starting => Glyph::Faded,
            Status::Thinking | Status::Working(_) => Glyph::Busy,
            Status::NeedsYou(_) | Status::Replied => Glyph::Look,
            Status::Error(_) => Glyph::Failed,
        }
    }
}

/// The badge's centre and radius, in pixels, and the gap cut around it.
const BADGE: (f32, f32, f32) = (26.5, 5.0, 2.0);

fn glyph_rgba(glyph: Glyph) -> Vec<u8> {
    let mut alpha = rune::text::icon_coverage(&crate::app_icon::MENU_BAR_MARK, SIZE as u32);
    let (centre, r, gap) = BADGE;
    let cover = |d: f32| (d + 0.5).clamp(0.0, 1.0);
    for (i, a) in alpha.iter_mut().enumerate() {
        let (x, y) = ((i % SIZE) as f32 + 0.5, (i / SIZE) as f32 + 0.5);
        let (dx, dy) = (x - centre, y - centre);
        let d = (dx * dx + dy * dy).sqrt();
        let badge = match glyph {
            Glyph::Plain => {
                continue;
            }
            Glyph::Faded => {
                *a /= 2;
                continue;
            }
            Glyph::Busy => cover(0.9 - (d - (r - 0.9)).abs()),
            Glyph::Look => cover(r - d),
            Glyph::Failed => {
                // The two diagonals, within the badge's circle.
                let off = (dx - dy).abs().min((dx + dy).abs()) / std::f32::consts::SQRT_2;
                cover(1.1 - off).min(cover(r - d))
            }
        };
        let cut = cover(d - r - gap);
        *a = ((f32::from(*a) * cut).max(255.0 * badge)).round() as u8;
    }
    alpha.into_iter().flat_map(|a| [0, 0, 0, a]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_id_is_a_fixed_item_or_a_recent_conversation_by_its_id() {
        for (action, id) in Action::ALL {
            assert_eq!(Pick::from_id(id), Some(Pick::Action(action)));
        }
        assert_eq!(Pick::from_id("recent:20240101-000000-abcd"), Some(Pick::Recent("20240101-000000-abcd".into())));
        assert_eq!(Pick::from_id("recent"), None, "the submenu itself picks nothing");
        assert_eq!(Pick::from_id("nope"), None);
    }

    #[test]
    fn the_menu_bar_icon_is_the_apps_mark() {
        let rgba = icon_rgba();
        let want = rune::text::icon_coverage(&crate::app_icon::MENU_BAR_MARK, SIZE as u32);
        let whole = rune::text::icon_coverage(&crate::app_icon::MARK, SIZE as u32);
        let ink = |c: &[u8]| c.iter().map(|&a| u32::from(a)).sum::<u32>();
        let ratio = ink(&want) as f32 / ink(&whole) as f32;
        assert!((0.55..0.75).contains(&ratio), "the mark at about 80% (64% of its ink): {ratio}");
        assert_eq!(rgba.len(), SIZE * SIZE * 4);
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 0, 0]), "black: a template icon");
        let alpha: Vec<u8> = rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        assert_eq!(alpha, want);
        assert!(want.iter().map(|&a| u32::from(a)).sum::<u32>() > 2000, "it has ink");
    }

    const GLYPHS: [Glyph; 5] = [Glyph::Plain, Glyph::Faded, Glyph::Busy, Glyph::Look, Glyph::Failed];

    #[test]
    fn the_menu_bar_mark_keeps_a_margin_like_the_other_menu_bar_icons() {
        for glyph in GLYPHS {
            let rgba = glyph_rgba(glyph);
            let edge = 4;
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let inside = (edge..SIZE - edge).contains(&x) && (edge..SIZE - edge).contains(&y);
                    if !inside {
                        assert_eq!(rgba[(y * SIZE + x) * 4 + 3], 0, "{glyph:?}: ink at ({x}, {y}), within {edge} px of the edge");
                    }
                }
            }
        }
    }

    #[test]
    fn each_status_has_its_glyph_and_each_glyph_looks_different() {
        let glyph = |s: Status| Glyph::of(&s);
        assert_eq!(glyph(Status::Idle), Glyph::Plain);
        assert_eq!(glyph(Status::Starting), Glyph::Faded);
        assert_eq!((glyph(Status::Thinking), glyph(Status::Working("Reading a".into()))), (Glyph::Busy, Glyph::Busy));
        assert_eq!((glyph(Status::NeedsYou("?".into())), glyph(Status::Replied)), (Glyph::Look, Glyph::Look));
        assert_eq!(glyph(Status::Error("no".into())), Glyph::Failed);

        let alpha = |g: Glyph| -> Vec<u8> { glyph_rgba(g).as_chunks::<4>().0.iter().map(|p| p[3]).collect() };
        let ink = |a: &[u8]| a.iter().map(|&a| u32::from(a)).sum::<u32>();
        let mark = alpha(Glyph::Plain);
        let faded = ink(&alpha(Glyph::Faded)) as f32 / ink(&mark) as f32;
        assert!((0.45..0.55).contains(&faded), "half the ink: {faded}");
        // Each badge in the corner, alone: centre filled for a dot, empty for a ring.
        let at = |a: &[u8], x: f32, y: f32| a[y as usize * SIZE + x as usize];
        let (c, r, gap) = BADGE;
        let (busy, look, failed) = (alpha(Glyph::Busy), alpha(Glyph::Look), alpha(Glyph::Failed));
        assert_eq!((at(&busy, c, c), at(&look, c, c), at(&failed, c, c)), (0, 255, 255));
        assert!(at(&busy, c + r - 1.0, c) > 200, "the ring");
        assert!(at(&failed, c + 2.5, c) < 60 && at(&failed, c + 2.0, c + 2.0) > 200, "a cross, not a dot");
        for a in [&busy, &look, &failed] {
            assert_eq!(at(a, c - r - gap + 0.6, c), 0, "the gap around the badge");
        }
        assert_eq!(&busy[..SIZE * 18], &mark[..SIZE * 18], "the mark's top half stays");
    }
}
