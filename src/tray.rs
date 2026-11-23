//! The menu-bar icon and its menu (`tray-icon` + `muda`; Rune's tray has no macOS backend).
//! Created on the main thread once the event loop runs; menu picks are forwarded to the host app.

use rune::AppHandle;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::Host;
use crate::history::Entry;
use crate::settings::ShortcutFor;
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
    /// Ends computer control's session (shown only while one runs).
    StopComputer,
}

impl Action {
    const ALL: [(Action, &'static str); 10] = [
        (Action::Capture, "capture"),
        (Action::CaptureWindow, "capture-window"),
        (Action::CaptureFullScreen, "capture-full-screen"),
        (Action::QuickInput, "quick-input"),
        (Action::OpenFolder, "open-folder"),
        (Action::ToggleLogin, "login"),
        (Action::OpenPermissionSettings, "permission"),
        (Action::Settings, "settings"),
        (Action::Quit, "quit"),
        (Action::StopComputer, "stop-computer"),
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
    quick: MenuItem,
    menu: Menu,
    /// The most recent conversations, after "Quick Input"; in the menu only while there are any.
    recent: Submenu,
    recent_shown: bool,
    login: CheckMenuItem,
    permission: MenuItem,
    permission_shown: bool,
    can_login: bool,
    /// "Stop Controlling the Mac", at the top while the model controls it.
    stop_computer: MenuItem,
    controlling: bool,
    /// What the icon shows now, and the status it was last given.
    glyph: Glyph,
    tooltip: String,
    status: Status,
}

/// The tooltip for `status`; while the model controls the Mac, that, and how to stop it.
fn tooltip(status: &Status, controlling: bool) -> String {
    if controlling { "Ailfred is controlling your Mac (⌥⇧Esc stops it)".into() } else { status.tooltip() }
}

impl Tray {
    /// Main thread only, after the event loop started. `can_login` is false outside the installed
    /// bundle, where a login item would mean nothing.
    pub fn new(host: AppHandle<Host>, can_login: bool, login_enabled: bool) -> Result<Tray, String> {
        // The chord is set by `show_capture_shortcut` once the saved one is registered.
        let capture = MenuItem::with_id(Action::Capture.id(), "Capture Area", true, None);
        let window = MenuItem::with_id(Action::CaptureWindow.id(), "Capture Window", true, None);
        let full = MenuItem::with_id(Action::CaptureFullScreen.id(), "Capture Full Screen", true, None);
        // Its chord too, once registered.
        let quick = MenuItem::with_id(Action::QuickInput.id(), "Quick Input", true, None);
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
        let panic = crate::computer::PANIC_SHORTCUT.parse::<Accelerator>().ok();
        let stop_computer = MenuItem::with_id(Action::StopComputer.id(), "Stop Controlling the Mac", true, panic);
        Ok(Tray {
            icon,
            capture,
            quick,
            menu,
            recent,
            recent_shown: false,
            login,
            permission,
            permission_shown: false,
            can_login,
            stop_computer,
            controlling: false,
            glyph: Glyph::Plain,
            tooltip: "Ailfred".into(),
            status: Status::Idle,
        })
    }

    /// The model controls the Mac (a session runs) or not: the icon and tooltip say so over
    /// anything else, and "Stop Controlling the Mac" heads the menu meanwhile.
    pub fn set_controlling(&mut self, controlling: bool) {
        if controlling != self.controlling {
            let done = if controlling { self.menu.insert(&self.stop_computer, 0) } else { self.menu.remove(&self.stop_computer) };
            if let Err(e) = done {
                eprintln!("Ailfred: updating the menu failed: {e}");
            }
            // The icon says so even if the menu couldn't.
            self.controlling = controlling;
        }
        let status = self.status.clone();
        self.set_status(&status);
    }

    /// The icon and its tooltip show `status` (each set only when it changes).
    pub fn set_status(&mut self, status: &Status) {
        self.status = status.clone();
        let glyph = Glyph::shown(status, self.controlling);
        if glyph != self.glyph {
            match glyph_icon(glyph).and_then(|icon| self.icon.set_icon_templated(Some(icon)).map_err(|e| e.to_string())) {
                Ok(()) => self.glyph = glyph,
                Err(e) => eprintln!("Ailfred: updating the menu-bar icon failed: {e}"),
            }
        }
        let tooltip = tooltip(status, self.controlling);
        if tooltip != self.tooltip {
            if let Err(e) = self.icon.set_tooltip(Some(&tooltip)) {
                eprintln!("Ailfred: updating the menu-bar tooltip failed: {e}");
            }
            self.tooltip = tooltip;
        }
    }

    /// Shows a chord next to "Capture Area" or "Quick Input" (none when it has none). Rune writes
    /// chords as `shift+super+space`, which muda's accelerator parser reads.
    pub fn show_shortcut(&self, which: ShortcutFor, shortcut: Option<&rune::Shortcut>) {
        let accelerator = shortcut.and_then(|s| s.to_string().parse::<Accelerator>().ok());
        let item = match which {
            ShortcutFor::Capture => &self.capture,
            ShortcutFor::QuickInput => &self.quick,
        };
        if let Err(e) = item.set_accelerator(accelerator) {
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

/// How tall tray-icon shows the icon, in points: it scales any taller icon down to that.
const MENU_BAR_PT: usize = 22;
/// The menu-bar icon's side in pixels: twice its points, so each lands on one of a Retina
/// screen's pixels, sharp, rather than stretched.
const SIZE: usize = 2 * MENU_BAR_PT;
/// Pixels per point.
const PX: f32 = (SIZE / MENU_BAR_PT) as f32;

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

/// How the icon shows a [`Status`], one look each: the mark, faded while the model starts, else
/// with a badge at its top right, cut out of it: a ring while it thinks, a ring round a dot while
/// it works on a tool, a diamond when it asks the user, a dot when a reply ended unseen, a cross
/// after a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Plain,
    Faded,
    Thinking,
    Working,
    Asking,
    Replied,
    Failed,
    /// The model controls the Mac: a square badge.
    Controlling,
}

impl Glyph {
    /// What the icon shows: `status`'s look, or, while the model controls the Mac, that.
    pub fn shown(status: &Status, controlling: bool) -> Glyph {
        if controlling { Glyph::Controlling } else { Glyph::of(status) }
    }

    pub fn of(status: &Status) -> Glyph {
        match status {
            Status::Idle => Glyph::Plain,
            Status::Starting => Glyph::Faded,
            Status::Thinking => Glyph::Thinking,
            Status::Working(_) => Glyph::Working,
            Status::NeedsYou(_) => Glyph::Asking,
            Status::Replied => Glyph::Replied,
            Status::Error(_) => Glyph::Failed,
        }
    }
}

/// The badge (the dot of a reply waiting to be seen): its centre and radius, in pixels, at the
/// top right.
const DOT: [f32; 3] = [SIZE as f32 - 4.25 * PX, 4.25 * PX, 3.0 * PX];
/// The clear ring between the badge and the mark, in pixels.
const DOT_GAP: f32 = PX;
/// Half the ring's and the cross's strokes; the working dot's radius.
const RING_HALF: f32 = 0.5 * PX;
const CROSS_HALF: f32 = 0.6 * PX;
const CENTRE_DOT_R: f32 = PX;

fn glyph_rgba(glyph: Glyph) -> Vec<u8> {
    let mut alpha = rune::text::icon_coverage(&crate::app_icon::MENU_BAR_MARK, SIZE as u32);
    // How much of a pixel lies inside an edge `d` px away (positive: inside).
    let cover = |d: f32| (d + 0.5).clamp(0.0, 1.0);
    let [cx, cy, r] = DOT;
    for (i, a) in alpha.iter_mut().enumerate() {
        let (x, y) = ((i % SIZE) as f32 + 0.5, (i / SIZE) as f32 + 0.5);
        let (dx, dy) = (x - cx, y - cy);
        let d = (dx * dx + dy * dy).sqrt();
        let ring = cover(RING_HALF - (d - (r - RING_HALF)).abs());
        let badge = match glyph {
            Glyph::Plain => continue,
            Glyph::Faded => {
                *a /= 2;
                continue;
            }
            Glyph::Thinking => ring,
            Glyph::Working => ring.max(cover(CENTRE_DOT_R - d)),
            // Corners on the axes, within the badge's circle.
            Glyph::Asking => cover((r - dx.abs() - dy.abs()) / std::f32::consts::SQRT_2),
            Glyph::Replied => cover(r - d),
            Glyph::Failed => {
                // The two diagonals, within the badge's circle.
                let off = (dx - dy).abs().min((dx + dy).abs()) / std::f32::consts::SQRT_2;
                cover(CROSS_HALF - off).min(cover(r - d))
            }
            // A square about as big as the circle it stands for.
            Glyph::Controlling => cover(0.95 * r - dx.abs().max(dy.abs())),
        };
        let cut = cover(d - r - DOT_GAP);
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
        assert_eq!(Pick::from_id("stop-computer"), Some(Pick::Action(Action::StopComputer)));
    }

    #[test]
    fn the_menu_bar_icon_is_the_apps_mark() {
        let rgba = icon_rgba();
        let want = rune::text::icon_coverage(&crate::app_icon::MENU_BAR_MARK, SIZE as u32);
        let whole = rune::text::icon_coverage(&crate::app_icon::MARK, SIZE as u32);
        let ink = |c: &[u8]| c.iter().map(|&a| u32::from(a)).sum::<u32>();
        let ratio = ink(&want) as f32 / ink(&whole) as f32;
        assert!((0.75..0.87).contains(&ratio), "the mark at about 90% (81% of its ink): {ratio}");
        assert_eq!(SIZE, 2 * MENU_BAR_PT, "one pixel to each of a Retina screen's: tray-icon shows it {MENU_BAR_PT} pt tall");
        assert_eq!(rgba.len(), SIZE * SIZE * 4);
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[..3] == [0, 0, 0]), "black: a template icon");
        let alpha: Vec<u8> = rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        assert_eq!(alpha, want);
        assert!(want.iter().map(|&a| u32::from(a)).sum::<u32>() > 2000, "it has ink");
    }

    const GLYPHS: [Glyph; 8] =
        [Glyph::Plain, Glyph::Faded, Glyph::Thinking, Glyph::Working, Glyph::Asking, Glyph::Replied, Glyph::Failed, Glyph::Controlling];

    #[test]
    fn while_the_model_controls_the_mac_the_icon_and_tooltip_say_so_over_anything_else() {
        let statuses = [Status::Idle, Status::Thinking, Status::Working("Clicking".into()), Status::NeedsYou("?".into()), Status::Error("no".into())];
        for status in &statuses {
            assert_eq!(Glyph::shown(status, true), Glyph::Controlling, "{status:?}");
            assert_eq!(Glyph::shown(status, false), Glyph::of(status), "{status:?}");
            assert_eq!(tooltip(status, true), "Ailfred is controlling your Mac (⌥⇧Esc stops it)");
            assert_eq!(tooltip(status, false), status.tooltip());
        }
        // A square badge: its corner is inked, unlike the dot's and the diamond's.
        let alpha = |g: Glyph| -> Vec<u8> { glyph_rgba(g).as_chunks::<4>().0.iter().map(|p| p[3]).collect() };
        let [cx, cy, _] = DOT;
        let corner = |a: &[u8]| a[(cy + 2.5 * PX) as usize * SIZE + (cx + 2.5 * PX) as usize];
        let (square, dot) = (corner(&alpha(Glyph::Controlling)), corner(&alpha(Glyph::Replied)));
        assert!(square > 200 && dot < 60, "square {square}, dot {dot}");
    }

    #[test]
    fn the_menu_bar_mark_keeps_a_margin_like_the_other_menu_bar_icons() {
        // The badge, like a notification's, may reach the corner.
        for glyph in [Glyph::Plain, Glyph::Faded] {
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
        assert_eq!((glyph(Status::Thinking), glyph(Status::Working("Reading a".into()))), (Glyph::Thinking, Glyph::Working));
        assert_eq!((glyph(Status::NeedsYou("?".into())), glyph(Status::Replied)), (Glyph::Asking, Glyph::Replied));
        assert_eq!(glyph(Status::Error("no".into())), Glyph::Failed);
        let looks: Vec<Vec<u8>> = GLYPHS.iter().map(|&g| glyph_rgba(g)).collect();
        for (i, a) in looks.iter().enumerate() {
            assert!(looks[i + 1..].iter().all(|b| a != b), "{:?} looks like another", GLYPHS[i]);
        }

        let alpha = |g: Glyph| -> Vec<u8> { glyph_rgba(g).as_chunks::<4>().0.iter().map(|p| p[3]).collect() };
        let ink = |a: &[u8]| a.iter().map(|&a| u32::from(a)).sum::<u32>();
        let mark = alpha(Glyph::Plain);
        let faded = ink(&alpha(Glyph::Faded)) as f32 / ink(&mark) as f32;
        assert!((0.45..0.55).contains(&faded), "half the ink: {faded}");
        // Each badge alone: centre filled for a dot, empty for a ring.
        let at = |a: &[u8], x: f32, y: f32| a[y as usize * SIZE + x as usize];
        let [cx, cy, r] = DOT;
        let [thinking, working, asking, replied, failed] = [Glyph::Thinking, Glyph::Working, Glyph::Asking, Glyph::Replied, Glyph::Failed].map(alpha);
        assert_eq!([&thinking, &working, &asking, &replied, &failed].map(|a| at(a, cx, cy)), [0, 255, 255, 255, 255], "the centre");
        assert_eq!([&thinking, &working, &replied].map(|a| at(a, cx + r - 1.0, cy) > 200), [true, true, true], "the rim");
        // Offsets in points.
        let (x, y) = (|pt: f32| cx + pt * PX, |pt: f32| cy + pt * PX);
        assert!(at(&working, x(1.5), cy) < 60, "a ring round a dot");
        assert!(at(&asking, x(1.75), y(1.25)) < 60 && at(&replied, x(1.75), y(1.25)) > 200, "a diamond, not a dot");
        assert!(at(&failed, x(1.25), cy) < 60 && at(&failed, x(1.0), y(1.0)) > 200, "a cross, not a dot");
        for a in [&thinking, &working, &asking, &replied, &failed] {
            // The pixel r + gap/2 left of the centre.
            assert_eq!(at(a, cx - r - DOT_GAP / 2.0, cy), 0, "the gap around the badge");
        }
        let bottom = SIZE * SIZE / 2;
        assert_eq!(&thinking[bottom..], &mark[bottom..], "the mark's bottom half stays");
    }

    /// Writes every glyph, 8x, black on white, to look at: `cargo test --offline menu_bar_icons_to_look_at -- --ignored`.
    #[test]
    #[ignore]
    fn menu_bar_icons_to_look_at() {
        const ZOOM: usize = 8;
        let side = SIZE * ZOOM;
        let n = GLYPHS.len();
        let mut out = vec![255u8; side * n * side * 4];
        for (i, glyph) in GLYPHS.iter().enumerate() {
            let rgba = glyph_rgba(*glyph);
            for y in 0..side {
                for x in 0..side {
                    let v = 255 - rgba[((y / ZOOM) * SIZE + x / ZOOM) * 4 + 3];
                    let at = (y * side * n + i * side + x) * 4;
                    out[at..at + 3].copy_from_slice(&[v, v, v]);
                }
            }
        }
        let png = crate::export::encode_png((side * n) as u32, side as u32, 1.0, &out).unwrap();
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
        let (plain, marked) = (glyph_rgba(Glyph::Plain), glyph_rgba(Glyph::Replied));
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
