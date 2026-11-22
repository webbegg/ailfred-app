//! The menu-bar icon and its menu (`tray-icon` + `muda`; Rune's tray has no macOS backend).
//! Created on the main thread once the event loop runs; menu picks are forwarded to the host app.

use rune::AppHandle;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::Host;
use crate::history::Entry;

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
    _icon: TrayIcon,
    capture: MenuItem,
    menu: Menu,
    /// The most recent conversations, after "Quick Input"; in the menu only while there are any.
    recent: Submenu,
    recent_shown: bool,
    login: CheckMenuItem,
    permission: MenuItem,
    permission_shown: bool,
    can_login: bool,
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
            .with_icon_templated(icon(false)?)
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
        Ok(Tray { _icon: icon, capture, menu, recent, recent_shown: false, login, permission, permission_shown: false, can_login })
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

    /// The dot on the icon on or off: a reply waits to be seen.
    pub fn set_marked(&self, marked: bool) {
        let done = icon(marked).and_then(|icon| self._icon.set_icon_templated(Some(icon)).map_err(|e| e.to_string()));
        if let Err(e) = done {
            eprintln!("Ailfred: marking the menu-bar icon failed: {e}");
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

/// The dot of a reply waiting to be seen: its centre and radius, in pixels, at the top right.
const DOT: [f32; 3] = [SIZE as f32 - 7.0, 7.0, 5.0];
/// The clear ring between the dot and the mark, in pixels.
const DOT_GAP: f32 = 2.0;

/// A template icon: the app's mark, black with alpha; macOS tints it. `marked`: with the dot.
fn icon(marked: bool) -> Result<Icon, String> {
    Icon::from_rgba(icon_rgba(marked), SIZE as u32, SIZE as u32).map_err(|e| e.to_string())
}

/// The icon's pixels, RGBA.
fn icon_rgba(marked: bool) -> Vec<u8> {
    let coverage = rune::text::icon_coverage(&crate::app_icon::MENU_BAR_MARK, SIZE as u32);
    let [cx, cy, r] = DOT;
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for (at, a) in coverage.into_iter().enumerate() {
        let a = if marked {
            let (x, y) = ((at % SIZE) as f32 + 0.5, (at / SIZE) as f32 + 0.5);
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
            // Antialiased by half a pixel each side: the dot, and the ring cut out of the mark.
            let dot = (r + 0.5 - d).clamp(0.0, 1.0);
            let kept = (d - (r + DOT_GAP) + 0.5).clamp(0.0, 1.0);
            ((f32::from(a) * kept).max(dot * 255.0)).round() as u8
        } else {
            a
        };
        rgba.extend([0, 0, 0, a]);
    }
    rgba
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
        let rgba = icon_rgba(false);
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

    #[test]
    fn the_menu_bar_mark_keeps_a_margin_like_the_other_menu_bar_icons() {
        let rgba = icon_rgba(false);
        let edge = 4;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let inside = (edge..SIZE - edge).contains(&x) && (edge..SIZE - edge).contains(&y);
                if !inside {
                    assert_eq!(rgba[(y * SIZE + x) * 4 + 3], 0, "ink at ({x}, {y}), within {edge} px of the edge");
                }
            }
        }
    }

    /// Writes both icons, 8x, black on white, to look at: `cargo test --offline menu_bar_icons_to_look_at -- --ignored`.
    #[test]
    #[ignore]
    fn menu_bar_icons_to_look_at() {
        const ZOOM: usize = 8;
        let side = SIZE * ZOOM;
        let mut out = vec![255u8; side * 2 * side * 4];
        for (i, rgba) in [icon_rgba(false), icon_rgba(true)].iter().enumerate() {
            for y in 0..side {
                for x in 0..side {
                    let v = 255 - rgba[((y / ZOOM) * SIZE + x / ZOOM) * 4 + 3];
                    let at = (y * side * 2 + i * side + x) * 4;
                    out[at..at + 3].copy_from_slice(&[v, v, v]);
                }
            }
        }
        let png = crate::export::encode_png((side * 2) as u32, side as u32, 1.0, &out).unwrap();
        let path = std::env::temp_dir().join("ailfred-menu-bar-icons.png");
        std::fs::write(&path, png).unwrap();
        eprintln!("wrote {}", path.display());
    }

    /// A pixel's distance from the dot's centre.
    fn from_dot(x: usize, y: usize) -> f32 {
        let [cx, cy, _] = DOT;
        ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt()
    }

    #[test]
    fn a_reply_waiting_puts_a_dot_at_the_top_right_with_a_clear_ring_around_it() {
        let (plain, marked) = (icon_rgba(false), icon_rgba(true));
        assert_eq!(marked.len(), plain.len());
        assert!(marked.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 0, 0]), "still a template icon");
        let alpha = |rgba: &[u8], x: usize, y: usize| rgba[(y * SIZE + x) * 4 + 3];
        let [cx, cy, r] = DOT;
        let (cx, cy) = (cx as usize, cy as usize);
        assert_eq!(alpha(&marked, cx, cy), 255, "the dot is solid");
        assert!(cx + (r as usize) < SIZE && cy >= r as usize, "the dot fits inside the icon");
        for y in 0..SIZE {
            for x in 0..SIZE {
                let d = from_dot(x, y);
                if d < r - 1.0 {
                    assert_eq!(alpha(&marked, x, y), 255, "inside the dot at ({x}, {y})");
                } else if d > r + 1.0 && d < r + DOT_GAP - 0.5 {
                    assert_eq!(alpha(&marked, x, y), 0, "the ring is clear at ({x}, {y})");
                } else if d > r + DOT_GAP + 1.0 {
                    assert_eq!(alpha(&marked, x, y), alpha(&plain, x, y), "the mark as it was at ({x}, {y})");
                }
            }
        }
        let ink = |rgba: &[u8]| rgba.as_chunks::<4>().0.iter().map(|p| u32::from(p[3])).sum::<u32>();
        assert!(ink(&marked) != ink(&plain));
    }
}
