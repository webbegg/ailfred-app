//! The user's settings (the capture shortcut and the save folder), stored with Rune's prefs in
//! `~/Library/Application Support/com.ailfred/prefs.json` (`com.ailfred.dev` for a dev run), and the capture shortcut's registration.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use rune::prefs::Prefs;
use rune::{AppHandle, Shortcut, ShortcutError, ShortcutId};

use crate::Host;

pub const APP_ID: &str = "com.ailfred";
/// A run from the terminal (`cargo run`): its own settings, apart from the installed app's.
pub const DEV_APP_ID: &str = "com.ailfred.dev";

/// The id the settings are kept under: the installed app's, or a dev run's.
pub fn app_id(installed: bool) -> &'static str {
    if installed { APP_ID } else { DEV_APP_ID }
}
pub const DEFAULT_SHORTCUT: &str = "cmd+shift+space";
const SHORTCUT: &str = "shortcut";
const FOLDER: &str = "folder";
const IGNORE_REDUCE_MOTION: &str = "ignore_reduce_motion";
const MODEL: &str = "model";

/// Whether the views animate even under the system's Reduce Motion, as last set (any thread).
static IGNORE_REDUCE_MOTION_NOW: AtomicBool = AtomicBool::new(true);

/// The model as last set, read when a message is sent (any thread).
static MODEL_NOW: RwLock<Option<String>> = RwLock::new(None);

/// Tests that pick a model publish it for the whole process: they take turns.
#[cfg(test)]
pub(crate) static MODEL_TEST: Mutex<()> = Mutex::new(());

/// The save folder as last set, for the export thread.
static FOLDER_NOW: RwLock<Option<PathBuf>> = RwLock::new(None);

pub struct Settings {
    prefs: Prefs,
}

impl Settings {
    /// The real settings: also publishes the save folder for the export thread.
    pub fn open(installed: bool) -> Settings {
        let settings = Settings::with(Prefs::open(app_id(installed)));
        *FOLDER_NOW.write().unwrap_or_else(|e| e.into_inner()) = Some(settings.folder());
        IGNORE_REDUCE_MOTION_NOW.store(settings.ignore_reduce_motion(), Ordering::Relaxed);
        *MODEL_NOW.write().unwrap_or_else(|e| e.into_inner()) = Some(settings.model());
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
            eprintln!("Ailfred: saving the shortcut failed: {e}");
        }
    }

    /// The save folder; the default if unset or no longer a folder.
    pub fn folder(&self) -> PathBuf {
        self.prefs.get::<String>(FOLDER).map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(default_folder)
    }

    pub fn set_folder(&mut self, folder: &Path) {
        if let Err(e) = self.prefs.set(FOLDER, &folder.to_string_lossy()) {
            eprintln!("Ailfred: saving the folder failed: {e}");
        }
        *FOLDER_NOW.write().unwrap_or_else(|e| e.into_inner()) = Some(folder.to_path_buf());
    }
}

impl Settings {
    /// The model that writes the replies: Sonnet until Opus is picked; the pick is kept.
    pub fn model(&self) -> String {
        self.prefs.get::<String>(MODEL).filter(|m| is_model(m)).unwrap_or_else(|| crate::model::DEFAULT_MODEL.into())
    }

    /// Picks `model` (one of `model::MODELS`; anything else is ignored) for the next messages.
    pub fn set_model(&mut self, model: &str) {
        if !is_model(model) {
            return;
        }
        if let Err(e) = self.prefs.set(MODEL, &model) {
            eprintln!("Ailfred: saving the model failed: {e}");
        }
        *MODEL_NOW.write().unwrap_or_else(|e| e.into_inner()) = Some(model.to_owned());
    }
}

fn is_model(id: &str) -> bool {
    crate::model::MODELS.iter().any(|(m, _)| *m == id)
}

/// The model to ask now (any thread): the setting's, as last set.
pub fn model_now() -> String {
    MODEL_NOW.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| crate::model::DEFAULT_MODEL.into())
}

impl Settings {
    /// Show every animation even with Reduce Motion on in the system: on until turned off.
    pub fn ignore_reduce_motion(&self) -> bool {
        self.prefs.get::<bool>(IGNORE_REDUCE_MOTION).unwrap_or(true)
    }

    pub fn set_ignore_reduce_motion(&mut self, ignore: bool) {
        if let Err(e) = self.prefs.set(IGNORE_REDUCE_MOTION, &ignore) {
            eprintln!("Ailfred: saving the motion setting failed: {e}");
        }
        IGNORE_REDUCE_MOTION_NOW.store(ignore, Ordering::Relaxed);
    }
}

/// Whether views should animate under the system's Reduce Motion now (any thread).
pub fn ignore_reduce_motion_now() -> bool {
    IGNORE_REDUCE_MOTION_NOW.load(Ordering::Relaxed)
}

/// At the top of a view: animate in full when the setting says to ignore Reduce Motion.
pub fn apply_motion<S>(cx: &mut rune::Cx<S>) {
    if ignore_reduce_motion_now() {
        cx.reduced_motion(false);
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
        if let Some(old) = *current
            && let Err(e) = host.unregister_shortcut(old)
        {
            // Keep tracking the old one rather than lose its id; release the new one instead.
            eprintln!("Ailfred: releasing the old shortcut failed: {e}");
            if let Some(new) = new {
                let _ = host.unregister_shortcut(new);
            }
            return Err(e);
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
    fn the_model_is_sonnet_until_opus_is_picked_and_the_pick_is_kept() {
        let _turn = MODEL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = Settings::with(Prefs::memory());
        assert_eq!(s.model(), "claude-sonnet-5-5");
        s.set_model("claude-opus-5-5");
        assert_eq!(s.model(), "claude-opus-5-5");
        assert_eq!(model_now(), "claude-opus-5-5", "the next message asks the one picked");
        s.set_model("gpt-4");
        assert_eq!(s.model(), "claude-opus-5-5", "only the listed models can be picked");
        s.set_model("claude-sonnet-5-5");
        assert_eq!(s.model(), "claude-sonnet-5-5");
        assert_eq!(model_now(), "claude-sonnet-5-5");
    }

    #[test]
    fn animations_ignore_reduce_motion_until_turned_off() {
        let mut s = Settings::with(Prefs::memory());
        assert!(s.ignore_reduce_motion(), "on by default");
        s.set_ignore_reduce_motion(false);
        assert!(!s.ignore_reduce_motion());
        assert!(!ignore_reduce_motion_now(), "the views see it at once");
        s.set_ignore_reduce_motion(true);
        assert!(s.ignore_reduce_motion());
        assert!(ignore_reduce_motion_now());
    }

    #[test]
    fn the_settings_live_under_ailfreds_id() {
        assert_eq!(APP_ID, "com.ailfred");
        assert_eq!(crate::app_env::BUNDLE_NAME, "Ailfred");
    }

    #[test]
    fn a_run_from_the_terminal_keeps_its_settings_apart_from_the_installed_apps() {
        assert_eq!(app_id(true), "com.ailfred");
        assert_eq!(app_id(false), "com.ailfred.dev");
    }

    #[test]
    fn folder_falls_back_when_missing() {
        let mut s = Settings::with(Prefs::memory());
        assert_eq!(s.folder(), default_folder());
        s.set_folder(Path::new("/no/such/folder/ailfred"));
        assert_eq!(s.folder(), default_folder(), "a folder that's gone falls back");
        let tmp = std::env::temp_dir();
        s.set_folder(&tmp);
        assert_eq!(s.folder(), tmp);
    }
}
