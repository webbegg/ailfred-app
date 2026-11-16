//! The user's settings (the capture shortcut and the save folder), stored with Rune's prefs in
//! `~/Library/Application Support/com.capibarapp/prefs.json`, and the capture shortcut's registration.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use rune::prefs::Prefs;
use rune::{AppHandle, Shortcut, ShortcutError, ShortcutId};

use crate::Host;

pub const APP_ID: &str = "com.capibarapp";
pub const DEFAULT_SHORTCUT: &str = "cmd+shift+space";
const SHORTCUT: &str = "shortcut";
const FOLDER: &str = "folder";

/// The save folder as last set, for the export thread.
static FOLDER_NOW: RwLock<Option<PathBuf>> = RwLock::new(None);

pub struct Settings {
    prefs: Prefs,
}

impl Settings {
    /// The real settings: also publishes the save folder for the export thread.
    pub fn open() -> Settings {
        let settings = Settings::with(Prefs::open(APP_ID));
        *FOLDER_NOW.write().unwrap_or_else(|e| e.into_inner()) = Some(settings.folder());
        settings
    }

    /// Settings over any store, publishing nothing (tests, the window's placeholder).
    pub fn with(prefs: Prefs) -> Settings {
        Settings { prefs }
    }

    /// The capture shortcut: the default until changed; None once the user cleared it.
    pub fn shortcut(&self) -> Option<Shortcut> {
        match self.prefs.get::<Option<String>>(SHORTCUT) {
            None => DEFAULT_SHORTCUT.parse().ok(),
            Some(None) => None,
            Some(Some(saved)) => saved.parse().ok().or_else(|| DEFAULT_SHORTCUT.parse().ok()),
        }
    }

    pub fn set_shortcut(&mut self, shortcut: Option<&Shortcut>) {
        let value: Option<String> = shortcut.map(|s| s.to_string());
        if let Err(e) = self.prefs.set(SHORTCUT, &value) {
            eprintln!("Capibarapp: saving the shortcut failed: {e}");
        }
    }

    /// The save folder; the default if unset or no longer a folder.
    pub fn folder(&self) -> PathBuf {
        self.prefs.get::<String>(FOLDER).map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(default_folder)
    }

    pub fn set_folder(&mut self, folder: &Path) {
        if let Err(e) = self.prefs.set(FOLDER, &folder.to_string_lossy()) {
            eprintln!("Capibarapp: saving the folder failed: {e}");
        }
        *FOLDER_NOW.write().unwrap_or_else(|e| e.into_inner()) = Some(folder.to_path_buf());
    }
}

pub fn default_folder() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Desktop")
}

/// Where exports save now (any thread): the chosen folder, or ~/Desktop if it's gone.
pub fn folder_now() -> PathBuf {
    FOLDER_NOW.read().unwrap_or_else(|e| e.into_inner()).clone().filter(|p| p.is_dir()).unwrap_or_else(default_folder)
}

/// The capture shortcut's current registration, shared by startup and the settings window.
/// Main thread only (registration is called from the app's own updates and windows).
#[derive(Clone, Default)]
pub struct CaptureShortcut(Arc<Mutex<Option<ShortcutId>>>);

impl CaptureShortcut {
    /// Registers `shortcut` (None: no shortcut) and only then releases the previous one, so a
    /// refused chord leaves the old one working.
    pub fn replace(&self, host: &AppHandle<Host>, shortcut: Option<&Shortcut>) -> Result<(), ShortcutError> {
        let new = match shortcut {
            Some(s) => Some(host.register_shortcut(s.clone(), |host: &mut Host, tx| crate::on_shortcut(host, tx))?),
            None => None,
        };
        let mut current = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = *current {
            if let Err(e) = host.unregister_shortcut(old) {
                // Keep tracking the old one rather than lose its id; release the new one instead.
                eprintln!("Capibarapp: releasing the old shortcut failed: {e}");
                if let Some(new) = new {
                    let _ = host.unregister_shortcut(new);
                }
                return Err(e);
            }
        }
        *current = new;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_defaults_clears_and_round_trips() {
        let mut s = Settings::with(Prefs::memory());
        assert_eq!(s.shortcut(), DEFAULT_SHORTCUT.parse().ok());
        s.set_shortcut(None);
        assert_eq!(s.shortcut(), None, "cleared stays cleared");
        let chord: Shortcut = "ctrl+alt+k".parse().unwrap();
        s.set_shortcut(Some(&chord));
        assert_eq!(s.shortcut(), Some(chord));
    }

    #[test]
    fn folder_falls_back_when_missing() {
        let mut s = Settings::with(Prefs::memory());
        assert_eq!(s.folder(), default_folder());
        s.set_folder(Path::new("/no/such/folder/capibarapp"));
        assert_eq!(s.folder(), default_folder(), "a folder that's gone falls back");
        let tmp = std::env::temp_dir();
        s.set_folder(&tmp);
        assert_eq!(s.folder(), tmp);
    }
}
