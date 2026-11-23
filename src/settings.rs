//! The user's settings (the capture shortcut and the save folder), stored with Rune's prefs in
//! `~/Library/Application Support/com.ailfred/prefs.json` (`com.ailfred.dev` for a dev run), and the capture shortcut's registration.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use rune::prefs::Prefs;
use rune::{AppHandle, Shortcut, ShortcutError, ShortcutId, Tx};

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
/// The quick input's chord, until changed in Settings (Option+Space alone is Claude's quick entry).
pub const DEFAULT_QUICK_SHORTCUT: &str = "ctrl+alt+space";

/// Which global shortcut: the capture's or the quick input's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutFor {
    Capture,
    QuickInput,
}

impl ShortcutFor {
    pub fn default_chord(self) -> &'static str {
        match self {
            ShortcutFor::Capture => DEFAULT_SHORTCUT,
            ShortcutFor::QuickInput => DEFAULT_QUICK_SHORTCUT,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ShortcutFor::Capture => "capture",
            ShortcutFor::QuickInput => "quick input",
        }
    }
}
const QUICK_SHORTCUT: &str = "quick_shortcut";
const FOLDER: &str = "folder";
const IGNORE_REDUCE_MOTION: &str = "ignore_reduce_motion";
const COMPUTER_CONTROL: &str = "computer_control";
const LOCAL_SERVER: &str = "local_server";
const MODEL: &str = "model";

/// Whether the views animate even under the system's Reduce Motion, until set (also before the
/// settings are read).
const IGNORE_REDUCE_MOTION_DEFAULT: bool = false;
/// Whether the views animate even under the system's Reduce Motion, as last set (any thread).
static IGNORE_REDUCE_MOTION_NOW: AtomicBool = AtomicBool::new(IGNORE_REDUCE_MOTION_DEFAULT);

/// The model as last set, read when a message is sent (any thread).
static MODEL_NOW: RwLock<Option<String>> = RwLock::new(None);

/// Tests that pick a model or set the (test) Keychain publish it for the whole process: they
/// take turns.
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
        self.saved_shortcut(SHORTCUT, DEFAULT_SHORTCUT)
    }

    pub fn set_shortcut(&mut self, shortcut: Option<&Shortcut>) {
        self.save_shortcut(SHORTCUT, shortcut);
    }

    /// The quick input's shortcut: the default until changed; None once the user cleared it.
    pub fn quick_shortcut(&self) -> Option<Shortcut> {
        self.saved_shortcut(QUICK_SHORTCUT, DEFAULT_QUICK_SHORTCUT)
    }

    pub fn set_quick_shortcut(&mut self, shortcut: Option<&Shortcut>) {
        self.save_shortcut(QUICK_SHORTCUT, shortcut);
    }

    fn saved_shortcut(&self, key: &str, default: &str) -> Option<Shortcut> {
        match self.prefs.get::<Option<String>>(key) {
            None => default.parse().ok(),
            Some(None) => None,
            Some(Some(saved)) => saved.parse().ok().or_else(|| default.parse().ok()),
        }
    }

    fn save_shortcut(&mut self, key: &str, shortcut: Option<&Shortcut>) {
        let value: Option<String> = shortcut.map(|s| s.to_string());
        if let Err(e) = self.prefs.set(key, &value) {
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
    /// Show every animation even with Reduce Motion on in the system: off until turned on.
    pub fn ignore_reduce_motion(&self) -> bool {
        self.prefs.get::<bool>(IGNORE_REDUCE_MOTION).unwrap_or(IGNORE_REDUCE_MOTION_DEFAULT)
    }

    pub fn set_ignore_reduce_motion(&mut self, ignore: bool) {
        if let Err(e) = self.prefs.set(IGNORE_REDUCE_MOTION, &ignore) {
            eprintln!("Ailfred: saving the motion setting failed: {e}");
        }
        IGNORE_REDUCE_MOTION_NOW.store(ignore, Ordering::Relaxed);
    }

    /// Let the model see the screen and drive the Mac (with pi or Claude Code): off until turned on.
    pub fn computer_control(&self) -> bool {
        self.prefs.get::<bool>(COMPUTER_CONTROL).unwrap_or(false)
    }

    pub fn set_computer_control(&mut self, on: bool) {
        if let Err(e) = self.prefs.set(COMPUTER_CONTROL, &on) {
            eprintln!("Ailfred: saving the computer control setting failed: {e}");
        }
    }

    /// Serve the conversation to apps on this Mac (`crate::server`): off until turned on.
    pub fn local_server(&self) -> bool {
        self.prefs.get::<bool>(LOCAL_SERVER).unwrap_or(false)
    }

    pub fn set_local_server(&mut self, on: bool) {
        if let Err(e) = self.prefs.set(LOCAL_SERVER, &on) {
            eprintln!("Ailfred: saving the local server setting failed: {e}");
        }
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

/// A saved shortcut as registered at startup.
#[derive(Debug, PartialEq, Eq)]
pub struct Started {
    /// The chord that works now: the saved one, else the default; None: neither (or cleared).
    pub active: Option<Shortcut>,
    /// What to tell the user when the saved one couldn't be had.
    pub notice: Option<String>,
}

/// Registers each shortcut's saved chord (`register(index, chord)`; None: cleared, nothing), then
/// each refused one's default instead, saying so. The saved chords go first, so a fallback can't
/// take another shortcut's own chord.
pub fn start_shortcuts(slots: &[(Option<Shortcut>, &str)], mut register: impl FnMut(usize, Option<&Shortcut>) -> Result<(), ShortcutError>) -> Vec<Started> {
    let tried: Vec<_> = slots.iter().enumerate().map(|(i, (saved, _))| register(i, saved.as_ref())).collect();
    let mut started = Vec::with_capacity(slots.len());
    for (i, ((saved, default), result)) in slots.iter().zip(tried).enumerate() {
        started.push(match (saved, result) {
            (None, _) => Started { active: None, notice: None },
            (Some(saved), Ok(())) => Started { active: Some(saved.clone()), notice: None },
            (Some(saved), Err(error)) => fall_back(saved, error, default, |s| register(i, Some(s))),
        });
    }
    started
}

/// `saved` was refused with `error`: the `default` instead, if it's another chord and it can be had.
fn fall_back(saved: &Shortcut, error: ShortcutError, default: &str, mut register: impl FnMut(&Shortcut) -> Result<(), ShortcutError>) -> Started {
    let name = |s: &Shortcut| crate::quick_input::chord_symbols(&s.keycaps());
    let gone = format!("{} isn't available ({error})", name(saved));
    if let Some(default) = default.parse::<Shortcut>().ok().filter(|d| d != saved)
        && register(&default).is_ok()
    {
        let notice = format!("{gone}: {} works instead. Record another to change it.", name(&default));
        return Started { active: Some(default), notice: Some(notice) };
    }
    Started { active: None, notice: Some(format!("{gone}. Record another; the menu still has it.")) }
}

/// A global shortcut's current registration and what it does, shared by startup and the settings
/// window. Main thread only (registration is called from the app's own updates and windows).
#[derive(Clone)]
pub struct HotKey {
    /// The registration held, and its chord.
    current: Arc<Mutex<Option<(ShortcutId, Shortcut)>>>,
    action: fn(&mut Host, &mut Tx),
}

impl HotKey {
    /// The capture's: takes a screenshot.
    pub fn capture() -> HotKey {
        HotKey { current: Arc::default(), action: crate::on_shortcut }
    }

    /// The quick input's: shows or hides it.
    pub fn quick_input() -> HotKey {
        HotKey { current: Arc::default(), action: |host, _| host.toggle_quick_input() }
    }

    /// Computer control's panic chord: ends the session (held only while computer control is on).
    pub fn computer_panic() -> HotKey {
        HotKey { current: Arc::default(), action: |host, _| host.stop_computer_control(crate::computer::session::Reason::Panic) }
    }

    /// Registers `shortcut` (None: no shortcut) and only then releases the previous one, so a
    /// refused chord leaves the old one working. The chord already held stays as it is (the
    /// system would refuse holding it twice).
    pub fn replace(&self, host: &AppHandle<Host>, shortcut: Option<&Shortcut>) -> Result<(), ShortcutError> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        swap(&mut current, shortcut, |s| host.register_shortcut(s.clone(), self.action), |id| host.unregister_shortcut(id))
    }
}

/// [`HotKey::replace`]'s steps on the registration held (`current`), with the system's
/// `register` and `unregister` passed in.
fn swap<I: Copy, E: std::fmt::Display>(
    current: &mut Option<(I, Shortcut)>,
    shortcut: Option<&Shortcut>,
    register: impl FnOnce(&Shortcut) -> Result<I, E>,
    mut unregister: impl FnMut(I) -> Result<(), E>,
) -> Result<(), E> {
    if let (Some((_, held)), Some(s)) = (&*current, shortcut)
        && held == s
    {
        return Ok(());
    }
    let new = match shortcut {
        Some(s) => Some((register(s)?, s.clone())),
        None => None,
    };
    if let Some((old, _)) = &*current
        && let Err(e) = unregister(*old)
    {
        // Keep tracking the old one rather than lose its id; release the new one instead.
        eprintln!("Ailfred: releasing the old shortcut failed: {e}");
        if let Some((new, _)) = new {
            let _ = unregister(new);
        }
        return Err(e);
    }
    *current = new;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A system shortcut registry: ids from 1, `refuse` and `stuck` say which calls fail, and
    /// every call is logged in order.
    #[derive(Default)]
    struct Registry {
        next: u32,
        held: Vec<u32>,
        log: Vec<String>,
        refuse: bool,
        stuck: Option<u32>,
    }

    fn swap_in(registry: &std::cell::RefCell<Registry>, current: &mut Option<(u32, Shortcut)>, chord: Option<&str>) -> Result<(), String> {
        let shortcut = chord.map(|c| c.parse::<Shortcut>().unwrap());
        let register = |_: &Shortcut| {
            let mut r = registry.borrow_mut();
            r.log.push("register".to_string());
            if r.refuse {
                return Err("taken".to_string());
            }
            r.next += 1;
            let id = r.next;
            r.held.push(id);
            Ok(id)
        };
        let unregister = |id: u32| {
            let mut r = registry.borrow_mut();
            r.log.push(format!("unregister {id}"));
            if r.stuck == Some(id) {
                return Err("busy".to_string());
            }
            r.held.retain(|&h| h != id);
            Ok(())
        };
        swap(current, shortcut.as_ref(), register, unregister)
    }

    #[test]
    fn a_new_chord_is_registered_before_the_old_one_is_released() {
        let registry = std::cell::RefCell::new(Registry::default());
        let mut current = None;
        swap_in(&registry, &mut current, Some("ctrl+alt+k")).unwrap();
        swap_in(&registry, &mut current, Some("ctrl+alt+j")).unwrap();
        let r = registry.borrow();
        assert_eq!(r.log[1..], ["register", "unregister 1"], "{:?}", r.log);
        assert_eq!((r.held.clone(), current.map(|(id, _)| id)), (vec![2], Some(2)));
    }

    #[test]
    fn a_refused_chord_leaves_the_old_one_working() {
        let registry = std::cell::RefCell::new(Registry::default());
        let mut current = None;
        swap_in(&registry, &mut current, Some("ctrl+alt+k")).unwrap();
        registry.borrow_mut().refuse = true;
        assert_eq!(swap_in(&registry, &mut current, Some("ctrl+alt+j")), Err("taken".into()));
        let r = registry.borrow();
        assert_eq!(r.held, [1], "the old one is still registered");
        assert_eq!(current, Some((1, "ctrl+alt+k".parse().unwrap())));
    }

    #[test]
    fn the_chord_already_held_is_kept_without_registering_it_again() {
        let registry = std::cell::RefCell::new(Registry::default());
        let mut current = None;
        swap_in(&registry, &mut current, Some("ctrl+alt+k")).unwrap();
        swap_in(&registry, &mut current, Some("ctrl+alt+k")).unwrap();
        assert_eq!(registry.borrow().log.len(), 1, "{:?}", registry.borrow().log);
        assert_eq!(current.map(|(id, _)| id), Some(1));
    }

    #[test]
    fn no_shortcut_releases_the_one_held() {
        let registry = std::cell::RefCell::new(Registry::default());
        let mut current = None;
        swap_in(&registry, &mut current, None).unwrap();
        swap_in(&registry, &mut current, Some("ctrl+alt+k")).unwrap();
        swap_in(&registry, &mut current, None).unwrap();
        assert!(registry.borrow().held.is_empty());
        assert_eq!(current, None);
    }

    #[test]
    fn an_old_chord_that_cannot_be_released_stays_tracked_and_the_new_one_is_let_go() {
        let registry = std::cell::RefCell::new(Registry::default());
        let mut current = None;
        swap_in(&registry, &mut current, Some("ctrl+alt+k")).unwrap();
        registry.borrow_mut().stuck = Some(1);
        assert_eq!(swap_in(&registry, &mut current, Some("ctrl+alt+j")), Err("busy".into()));
        let r = registry.borrow();
        assert_eq!(r.held, [1], "the new one was released: {:?}", r.log);
        assert_eq!(current.map(|(id, _)| id), Some(1), "its id isn't lost");
    }

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
    fn the_quick_input_shortcut_defaults_to_ctrl_option_space_clears_and_round_trips() {
        let mut s = Settings::with(Prefs::memory());
        assert_eq!(s.quick_shortcut(), "ctrl+alt+space".parse().ok());
        s.set_quick_shortcut(None);
        assert_eq!(s.quick_shortcut(), None, "cleared stays cleared");
        let chord: Shortcut = "ctrl+alt+j".parse().unwrap();
        s.set_quick_shortcut(Some(&chord));
        assert_eq!(s.quick_shortcut(), Some(chord));
        assert_eq!(s.shortcut(), DEFAULT_SHORTCUT.parse().ok(), "the capture shortcut is its own");
    }

    fn chord(s: &str) -> Shortcut {
        s.parse().unwrap()
    }

    /// Registers what `taken` doesn't hold, recording each try (one shortcut).
    fn registrar<'a>(taken: &'a [&'a str], tried: &'a mut Vec<Option<String>>) -> impl FnMut(Option<&Shortcut>) -> Result<(), ShortcutError> + 'a {
        move |s| {
            tried.push(s.map(|s| s.to_string()));
            match s {
                Some(s) if taken.iter().any(|t| chord(t) == *s) => Err(ShortcutError::Conflict),
                _ => Ok(()),
            }
        }
    }

    /// One shortcut started alone.
    fn start_shortcut(saved: Option<Shortcut>, default: &str, mut register: impl FnMut(Option<&Shortcut>) -> Result<(), ShortcutError>) -> Started {
        start_shortcuts(&[(saved, default)], |_, s| register(s)).remove(0)
    }

    #[test]
    fn a_fallback_never_takes_the_other_shortcuts_saved_chord() {
        // Capture's saved chord is held by another app; the quick input's saved chord is the
        // capture's default. Ailfred holds what it registers (Rune refuses a second hold).
        let mut held: Vec<(usize, Shortcut)> = Vec::new();
        let other_app = chord("ctrl+alt+shift+j");
        let started = start_shortcuts(&[(Some(other_app.clone()), DEFAULT_SHORTCUT), (Some(chord(DEFAULT_SHORTCUT)), DEFAULT_QUICK_SHORTCUT)], |i, s| {
            let Some(s) = s else { return Ok(()) };
            if *s == other_app || held.iter().any(|(_, h)| h == s) {
                return Err(ShortcutError::Conflict);
            }
            held.push((i, s.clone()));
            Ok(())
        });
        assert_eq!(started[1], Started { active: Some(chord(DEFAULT_SHORTCUT)), notice: None }, "the quick input keeps its own");
        assert_eq!(started[0].active, None, "the capture's default was the quick input's");
        assert!(started[0].notice.is_some());
        assert_eq!(held, vec![(1, chord(DEFAULT_SHORTCUT))]);
    }

    #[test]
    fn a_saved_shortcut_that_registers_is_used_as_is() {
        let mut tried = Vec::new();
        let started = start_shortcut(Some(chord("ctrl+alt+j")), "ctrl+alt+space", registrar(&[], &mut tried));
        assert_eq!(started, Started { active: Some(chord("ctrl+alt+j")), notice: None });
        assert_eq!(tried.len(), 1);
    }

    #[test]
    fn a_saved_shortcut_thats_taken_falls_back_to_the_default_and_says_so() {
        let mut tried = Vec::new();
        let started = start_shortcut(Some(chord("ctrl+alt+j")), "ctrl+alt+space", registrar(&["ctrl+alt+j"], &mut tried));
        assert_eq!(started.active, Some(chord("ctrl+alt+space")));
        let notice = started.notice.expect("a notice");
        assert!(notice.contains("⌃⌥J") && notice.contains("⌃⌥Space"), "names both: {notice}");
    }

    #[test]
    fn with_the_default_taken_too_theres_no_shortcut_and_a_notice() {
        let mut tried = Vec::new();
        let started = start_shortcut(Some(chord("ctrl+alt+j")), "ctrl+alt+space", registrar(&["ctrl+alt+j", "ctrl+alt+space"], &mut tried));
        assert_eq!(started.active, None);
        assert!(started.notice.is_some());
        assert_eq!(tried.len(), 2);
    }

    #[test]
    fn the_default_itself_taken_isnt_tried_twice() {
        let mut tried = Vec::new();
        let started = start_shortcut(Some(chord("ctrl+alt+space")), "ctrl+alt+space", registrar(&["ctrl+alt+space"], &mut tried));
        assert_eq!(started.active, None);
        assert!(started.notice.expect("a notice").contains("⌃⌥Space"));
        assert_eq!(tried.len(), 1);
    }

    #[test]
    fn a_cleared_shortcut_registers_nothing_and_says_nothing() {
        let mut tried = Vec::new();
        let started = start_shortcut(None, "ctrl+alt+space", registrar(&[], &mut tried));
        assert_eq!(started, Started { active: None, notice: None });
        assert_eq!(tried, vec![None]);
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
    fn pi_can_be_picked_to_write_the_replies() {
        let _turn = MODEL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = Settings::with(Prefs::memory());
        s.set_model(crate::model::PI);
        assert_eq!((s.model().as_str(), model_now().as_str()), ("pi", "pi"));
        s.set_model(crate::model::DEFAULT_MODEL);
        assert_eq!(model_now(), crate::model::DEFAULT_MODEL);
    }

    #[test]
    fn animations_follow_reduce_motion_until_told_to_ignore_it() {
        let mut s = Settings::with(Prefs::memory());
        assert!(!s.ignore_reduce_motion(), "off by default: the system's Reduce Motion is respected");
        s.set_ignore_reduce_motion(true);
        assert!(s.ignore_reduce_motion());
        assert!(ignore_reduce_motion_now(), "the views see it at once");
        s.set_ignore_reduce_motion(false);
        assert!(!s.ignore_reduce_motion());
        assert!(!ignore_reduce_motion_now());
    }

    #[test]
    fn computer_control_is_off_until_turned_on_and_stays_as_set() {
        let mut s = Settings::with(Prefs::memory());
        assert!(!s.computer_control(), "off by default: the model gets no tools to drive the Mac");
        s.set_computer_control(true);
        assert!(s.computer_control());
        s.set_computer_control(false);
        assert!(!s.computer_control());
    }

    #[test]
    fn the_local_server_is_off_until_turned_on_and_stays_as_set() {
        let mut s = Settings::with(Prefs::memory());
        assert!(!s.local_server(), "off by default: no app reaches the conversation");
        s.set_local_server(true);
        assert!(s.local_server());
        s.set_local_server(false);
        assert!(!s.local_server());
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
