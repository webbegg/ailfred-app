//! The menu-bar icon and its menu (`tray-icon` + `muda`; Rune's tray has no macOS backend).
//! Created on the main thread once the event loop runs; menu picks are forwarded to the host app.

use rune::AppHandle;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::Host;

/// What a menu pick asks the host to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Capture,
    OpenFolder,
    ToggleLogin,
    OpenPermissionSettings,
    Quit,
}

impl Action {
    const ALL: [(Action, &'static str); 5] = [
        (Action::Capture, "capture"),
        (Action::OpenFolder, "open-folder"),
        (Action::ToggleLogin, "login"),
        (Action::OpenPermissionSettings, "permission"),
        (Action::Quit, "quit"),
    ];
    fn id(self) -> &'static str {
        Action::ALL.iter().find(|(a, _)| *a == self).map(|(_, id)| *id).unwrap_or("")
    }
    fn from_id(id: &str) -> Option<Action> {
        Action::ALL.iter().find(|(_, i)| *i == id).map(|(a, _)| *a)
    }
}

pub struct Tray {
    /// Dropping it removes the icon.
    _icon: TrayIcon,
    menu: Menu,
    login: CheckMenuItem,
    permission: MenuItem,
    permission_shown: bool,
    can_login: bool,
}

impl Tray {
    /// Main thread only, after the event loop started. `can_login` is false outside the installed
    /// bundle, where a login item would mean nothing.
    pub fn new(host: AppHandle<Host>, can_login: bool, login_enabled: bool) -> Result<Tray, String> {
        let capture = MenuItem::with_id(
            Action::Capture.id(),
            "Capture Area",
            true,
            Some(Accelerator::new(Modifiers::META | Modifiers::SHIFT, Code::Space)),
        );
        let folder = MenuItem::with_id(Action::OpenFolder.id(), "Open Screenshots Folder", true, None);
        let login = CheckMenuItem::with_id(Action::ToggleLogin.id(), "Launch at Login", can_login, login_enabled, None);
        let permission = MenuItem::with_id(
            Action::OpenPermissionSettings.id(),
            "Screen Recording Is Off: Open Settings, Then Relaunch…",
            true,
            None,
        );
        let quit = MenuItem::with_id(Action::Quit.id(), "Quit ailfred", true, Some(Accelerator::new(Modifiers::META, Code::KeyQ)));
        let menu = Menu::new();
        menu.append_items(&[
            &capture,
            &folder,
            &PredefinedMenuItem::separator(),
            &login,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;

        let icon = TrayIconBuilder::new()
            .with_icon_templated(icon()?)
            .with_tooltip("ailfred")
            .with_menu(Box::new(menu.clone()))
            .build()
            .map_err(|e| e.to_string())?;

        // muda calls this on the main thread from the menu's action; the host handles it next turn.
        // muda keeps the first handler set in the process: build one Tray per process.
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = Action::from_id(&event.id.0) {
                host.update(move |host, tx| host.on_menu(action, tx));
            }
        }));
        Ok(Tray { _icon: icon, menu, login, permission, permission_shown: false, can_login })
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
            Err(e) => eprintln!("ailfred: updating the menu failed: {e}"),
        }
    }
}

/// A 36x36 px template icon: four crop corners. tray-icon treats pixels as points (scaling down
/// only past 22 pt), so it shows at 22 pt. Black with alpha; macOS tints it.
fn icon() -> Result<Icon, String> {
    const SIZE: usize = 36;
    const MARGIN: usize = 5;
    const ARM: usize = 10;
    const THICK: usize = 3;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let far = SIZE - MARGIN;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let near = |v: usize| (MARGIN..MARGIN + ARM).contains(&v);
            let far_side = |v: usize| (far - ARM..far).contains(&v);
            let edge = |v: usize| (MARGIN..MARGIN + THICK).contains(&v) || (far - THICK..far).contains(&v);
            let in_corner = (near(x) || far_side(x)) && (near(y) || far_side(y));
            if in_corner && (edge(x) || edge(y)) {
                rgba[(y * SIZE + x) * 4 + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32).map_err(|e| e.to_string())
}
