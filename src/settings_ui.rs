//! The Settings window: the capture's and the quick input's shortcuts (rune-kit's recorders,
//! with what happened to a saved one that was taken at startup), the save folder, the model
//! (Claude's version and the Anthropic API key, which lives in the Keychain), whether Screen
//! Recording is on (with the way to System Settings), and whether animations play under the
//! system's Reduce Motion.

use std::path::PathBuf;
use std::sync::OnceLock;

use rune::{AppHandle, Cx, El, FileDialogOptions, Shortcut, ShortcutError, TextEdit, Tx, col, row, text};
use rune_kit::{Button, Checkbox, Kit, SegmentedControl, ShortcutRecorder, ShortcutRecorderState, TextInput};

use crate::Host;
use crate::keychain;
use crate::model;
use crate::settings::{HotKey, Settings, ShortcutFor};

/// The window's size in points.
pub const SIZE: (f32, f32) = (480.0, 620.0);

pub struct SettingsUi {
    settings: Settings,
    /// The capture's shortcut, and the quick input's.
    recorder: ShortcutRecorderState,
    quick_recorder: ShortcutRecorderState,
    /// What happened to each saved shortcut at startup, when it was taken.
    capture_notice: Option<String>,
    quick_notice: Option<String>,
    folder: PathBuf,
    ignore_reduce_motion: bool,
    /// The model picked (its id), and the API key being typed (a secure field).
    model: String,
    key: TextEdit,
    /// Where replies come from; empty until Settings opens (`refresh_status`): asking costs a
    /// Keychain call, which waits for the user to look.
    status: String,
    /// None only in the placeholder `init` Rune requires; the real state is supplied.
    host: Option<AppHandle<Host>>,
    capture: HotKey,
    quick: HotKey,
    error: Option<String>,
    /// Whether Screen Recording is on, as last checked (None: not yet).
    screen_recording: Option<bool>,
    /// How it's checked (the system's; a fake in tests).
    check_screen_recording: fn() -> bool,
}

/// Where replies come from with pi picked.
fn pi_status() -> String {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    match crate::pi_setup::pi_binary(&home) {
        Some(_) => "Replies come from pi, on your pi setup's model and sign-ins".into(),
        None => "pi isn't installed: install it, or set AILFRED_PI".into(),
    }
}

impl SettingsUi {
    pub fn new(settings: Settings, host: AppHandle<Host>, capture: HotKey, quick: HotKey) -> SettingsUi {
        let recorder = ShortcutRecorderState::new(settings.shortcut());
        let quick_recorder = ShortcutRecorderState::new(settings.quick_shortcut());
        let folder = settings.folder();
        let ignore_reduce_motion = settings.ignore_reduce_motion();
        let model = settings.model();
        SettingsUi {
            settings,
            recorder,
            quick_recorder,
            capture_notice: None,
            quick_notice: None,
            folder,
            ignore_reduce_motion,
            model,
            key: TextEdit::new(""),
            status: String::new(),
            host: Some(host),
            capture,
            quick,
            error: None,
            screen_recording: None,
            check_screen_recording: crate::app_env::screen_recording_allowed_quiet,
        }
    }

    /// Settings opened: says where replies come from now, and whether Screen Recording is on.
    pub fn refresh_status(&mut self) {
        self.status = if self.model == model::PI { pi_status() } else { model::status_now() };
        self.check_permissions();
    }

    /// Asks whether Screen Recording is on (cheap, no prompt); a change shows in the menu too.
    fn check_permissions(&mut self) {
        let allowed = (self.check_screen_recording)();
        if self.screen_recording.replace(allowed) == Some(allowed) {
            return;
        }
        if let Some(host) = &self.host {
            host.update(move |host, _| host.set_permission(allowed));
        }
    }

    /// A model picked in the chat's composer: kept, and shown here. The status line is looked
    /// up again only if Settings showed it (it asks the Keychain).
    pub fn choose_model(&mut self, id: &str) {
        if !model::MODELS.iter().any(|(m, _)| *m == id) {
            return;
        }
        self.settings.set_model(id);
        self.model = self.settings.model();
        if !self.status.is_empty() {
            self.refresh_status();
        }
    }

    /// A model's segment clicked: it's kept, and the status says where replies come from.
    fn pick_model(&mut self, index: usize) {
        let Some((id, _)) = model::MODELS.get(index) else { return };
        self.settings.set_model(id);
        self.model = self.settings.model();
        self.refresh_status();
    }

    /// Save: the typed key goes to the Keychain and the field empties.
    fn save_key(&mut self) {
        let key = self.key.text.trim().to_owned();
        if key.is_empty() {
            self.error = Some("Paste your Anthropic API key first.".into());
            return;
        }
        match keychain::set(&key) {
            Ok(()) => {
                self.key.clear();
                self.error = None;
                self.refresh_status();
            }
            Err(e) => self.error = Some(format!("Couldn't save the key in the Keychain: {e}.")),
        }
    }

    /// Remove: forgets the key in the Keychain.
    fn remove_key(&mut self) {
        match keychain::remove() {
            Ok(()) => {
                self.error = None;
                self.refresh_status();
            }
            Err(e) => self.error = Some(format!("Couldn't remove the key from the Keychain: {e}.")),
        }
    }

    /// A saved shortcut was taken at startup: `active` is the one that works instead (its
    /// default), if any; else the saved one stays shown, and recording it again retries it.
    pub fn shortcut_notice(&mut self, which: ShortcutFor, active: Option<Shortcut>, notice: String) {
        let (recorder, slot) = match which {
            ShortcutFor::Capture => (&mut self.recorder, &mut self.capture_notice),
            ShortcutFor::QuickInput => (&mut self.quick_recorder, &mut self.quick_notice),
        };
        if let Some(active) = active {
            *recorder = ShortcutRecorderState::new(Some(active));
        }
        // Recording the chord shown keeps it (saved, the notice gone), even the one that works.
        recorder.mark_unregistered();
        *slot = Some(notice);
    }

    fn toggle_reduce_motion(&mut self) {
        self.ignore_reduce_motion = !self.ignore_reduce_motion;
        self.settings.set_ignore_reduce_motion(self.ignore_reduce_motion);
    }

    /// The window is built with its state supplied (`WindowSpec::state`); `AppSpec` still wants an
    /// `init`, which gets a detached copy that changes nothing.
    pub fn placeholder() -> SettingsUi {
        let settings = Settings::with(rune::prefs::Prefs::memory());
        let recorder = ShortcutRecorderState::new(settings.shortcut());
        let quick_recorder = ShortcutRecorderState::new(settings.quick_shortcut());
        SettingsUi {
            quick_recorder,
            capture_notice: None,
            quick_notice: None,
            quick: HotKey::quick_input(),
            folder: settings.folder(),
            ignore_reduce_motion: settings.ignore_reduce_motion(),
            model: settings.model(),
            key: TextEdit::new(""),
            status: String::new(),
            settings,
            recorder,
            host: None,
            capture: HotKey::capture(),
            error: None,
            screen_recording: None,
            check_screen_recording: crate::app_env::screen_recording_allowed_quiet,
        }
    }
}

fn kit(dark: bool) -> &'static Kit {
    static DARK: OnceLock<Kit> = OnceLock::new();
    static LIGHT: OnceLock<Kit> = OnceLock::new();
    if dark { DARK.get_or_init(|| Kit::neutral(true)) } else { LIGHT.get_or_init(|| Kit::neutral(false)) }
}

pub fn view(s: &SettingsUi, cx: &mut Cx<SettingsUi>) -> El {
    crate::settings::apply_motion(cx);
    let k = kit(cx.system_dark());
    let label = |t: &str| text(t).font(k.ty.ui_medium).color(k.c.text_strong);
    let notice = |key: &str, notice: &Option<String>| notice.as_ref().map(|n| text(n.clone()).key(key).wrap().font(k.ty.small).color(k.c.text_weak));
    let mut shortcuts = col()
        .gap(8.0)
        .child(label("Shortcuts"))
        .child(ShortcutRecorder::new("capture-shortcut", &s.recorder, |s: &mut SettingsUi| &mut s.recorder).label("Capture area").on_change(change_shortcut).build(k));
    if let Some(n) = notice("capture-notice", &s.capture_notice) {
        shortcuts = shortcuts.child(n);
    }
    shortcuts = shortcuts.child(
        ShortcutRecorder::new("quick-shortcut", &s.quick_recorder, |s: &mut SettingsUi| &mut s.quick_recorder)
            .label("Quick input")
            .on_change(change_quick_shortcut)
            .build(k),
    );
    if let Some(n) = notice("quick-notice", &s.quick_notice) {
        shortcuts = shortcuts.child(n);
    }
    let folder = row()
        .gap(10.0)
        .child(text(short(&s.folder)).font(k.ty.ui).color(k.c.text_weak))
        .child(Button::new("Choose…").on_press_tx(choose_folder).build(k));
    let labels: Vec<&str> = model::MODELS.iter().map(|(_, label)| *label).collect();
    let selected = model::MODELS.iter().position(|(id, _)| *id == s.model);
    let models = SegmentedControl::new(&labels, selected).label("Model").on_pick(|s: &mut SettingsUi, i, _| s.pick_model(i)).build(k);
    let key = TextInput::new(&s.key)
        .secure()
        .placeholder("sk-ant-…")
        .label("Anthropic API key")
        .on_edit(|s: &mut SettingsUi| &mut s.key)
        .on_submit(|s: &mut SettingsUi, _| s.save_key())
        .build(k);
    let key_row = row()
        .gap(8.0)
        .items_center()
        .child(key)
        .child(Button::new("Save").on_press(|s: &mut SettingsUi| s.save_key()).build(k))
        .child(Button::new("Remove").on_press(|s: &mut SettingsUi| s.remove_key()).build(k));
    let mut model_section = col().key("model-section").gap(8.0).child(label("Model")).child(models).child(col().key("api-key").gap(8.0).child(key_row));
    if !s.status.is_empty() {
        model_section = model_section.child(text(s.status.clone()).key("status").font(k.ty.small).color(k.c.text_weak));
    }
    let mut page = col()
        .w_full()
        .h_full()
        .padding(24.0)
        .gap(18.0)
        .fill(k.c.page)
        // Back from System Settings: Screen Recording may have changed.
        .on_window_focus(|s: &mut SettingsUi, focused, _| {
            if focused {
                s.check_permissions();
            }
        })
        .child(shortcuts)
        .child(col().gap(8.0).child(label("Save screenshots to")).child(folder))
        .child(model_section);
    if let Some(allowed) = s.screen_recording {
        page = page.child(permissions(allowed, k, label("Permissions")));
    }
    page = page
        .child(
            Checkbox::new(s.ignore_reduce_motion)
                .label("Show animations even with Reduce Motion on")
                .description("Ignores Reduce Motion in System Settings › Accessibility › Display.")
                .on_toggle(|s: &mut SettingsUi| s.toggle_reduce_motion())
                .build(k),
        );
    if let Some(error) = &s.error {
        page = page.child(text(error.clone()).wrap().font(k.ty.small).color(k.c.text_weak));
    }
    page
}

/// Screen Recording, on or off; off, the way to turn it on.
fn permissions(allowed: bool, k: &Kit, title: El) -> El {
    let line = if allowed {
        "Screen Recording: On, so Ailfred can capture the screen."
    } else {
        "Screen Recording: Off. Turn on Ailfred there, then quit and reopen it."
    };
    let mut section = col().key("permissions").gap(8.0).child(title).child(text(line).key("screen-recording").wrap().font(k.ty.ui).color(k.c.text_weak));
    if !allowed {
        let open = Button::new("Open System Settings…").on_press(|_: &mut SettingsUi| crate::open(crate::app_env::SCREEN_RECORDING_SETTINGS)).build(k);
        section = section.child(row().child(open));
    }
    section
}

/// `~/Desktop` rather than `/Users/name/Desktop`.
fn short(path: &std::path::Path) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let full = path.to_string_lossy();
    match full.strip_prefix(&home) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => full.into_owned(),
    }
}

/// The recorder's proposal: register it (refusing keeps the old one), then save it and show it in
/// the menu.
fn change_shortcut(s: &mut SettingsUi, shortcut: Option<&Shortcut>, _: &mut Tx) -> Result<(), ShortcutError> {
    let Some(host) = s.host.clone() else { return Ok(()) };
    s.capture.replace(&host, shortcut)?;
    s.settings.set_shortcut(shortcut);
    s.capture_notice = None;
    let shown = shortcut.cloned();
    host.update(move |host, _| host.show_shortcut(ShortcutFor::Capture, shown.as_ref()));
    Ok(())
}

/// The same for the quick input's shortcut.
fn change_quick_shortcut(s: &mut SettingsUi, shortcut: Option<&Shortcut>, _: &mut Tx) -> Result<(), ShortcutError> {
    let Some(host) = s.host.clone() else { return Ok(()) };
    s.quick.replace(&host, shortcut)?;
    s.settings.set_quick_shortcut(shortcut);
    s.quick_notice = None;
    let shown = shortcut.cloned();
    host.update(move |host, _| host.show_shortcut(ShortcutFor::QuickInput, shown.as_ref()));
    Ok(())
}

fn choose_folder(s: &mut SettingsUi, tx: &mut Tx) {
    let options = FileDialogOptions { title: "Save screenshots to".into(), directory: Some(s.folder.clone()), ..Default::default() };
    tx.open_folder(options, |s: &mut SettingsUi, answer, _| match answer {
        Ok(Some(folder)) => {
            s.settings.set_folder(&folder);
            s.folder = folder;
            s.error = None;
        }
        Ok(None) => {}
        Err(e) => s.error = Some(format!("Couldn't open the folder picker: {e}")),
    });
}

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};

    use super::*;
    use crate::settings::{MODEL_TEST, ShortcutFor};

    fn ui() -> SettingsUi {
        SettingsUi::placeholder()
    }

    #[test]
    fn picking_a_model_selects_it_and_keeps_it_for_the_next_message() {
        let _turn = MODEL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = ui();
        assert_eq!(s.model, "claude-sonnet-5-5");
        s.pick_model(1);
        assert_eq!((s.model.as_str(), s.settings.model().as_str()), ("claude-opus-5-5", "claude-opus-5-5"));
        assert_eq!(crate::settings::model_now(), "claude-opus-5-5");
        s.pick_model(7);
        assert_eq!(s.model, "claude-opus-5-5", "no such segment");
        s.pick_model(0);
        assert_eq!(crate::settings::model_now(), "claude-sonnet-5-5");
    }

    #[test]
    fn picking_pi_says_replies_come_from_it_and_claude_says_its_own() {
        let _turn = MODEL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = ui();
        s.pick_model(2);
        assert_eq!(s.model, "pi");
        assert!(s.status.contains("pi"), "{}", s.status);
        s.pick_model(0);
        assert_eq!(s.status, model::status_now());
    }

    #[test]
    fn a_model_chosen_in_the_chat_is_kept_and_shown_here() {
        let _turn = MODEL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = ui();
        s.choose_model("claude-opus-5-5");
        assert_eq!((s.model.as_str(), s.settings.model().as_str()), ("claude-opus-5-5", "claude-opus-5-5"));
        s.choose_model("nope");
        assert_eq!(s.model, "claude-opus-5-5");
        assert!(s.status.is_empty(), "Settings not opened: no Keychain lookup");
        s.choose_model(crate::model::DEFAULT_MODEL);
        assert_eq!(crate::settings::model_now(), crate::model::DEFAULT_MODEL);
    }

    // The Keychain here is the test build's, in memory: no test touches the real one.
    #[test]
    fn saving_a_key_keeps_it_in_the_keychain_and_empties_the_field_and_removing_forgets_it() {
        // The Keychain changes where replies come from, which other tests read.
        let _turn = MODEL_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut s = ui();
        s.key = TextEdit::new("  sk-ant-abc \n");
        s.save_key();
        assert_eq!(keychain::get().as_deref(), Some("sk-ant-abc"), "trimmed");
        assert_eq!(s.key.text, "", "the key doesn't stay on screen");
        assert_eq!(s.status, "Replies come from the Anthropic API (key in Keychain)");
        assert!(s.error.is_none());
        s.remove_key();
        assert_eq!(keychain::get(), None);
        assert!(!s.status.contains("Keychain"), "{}", s.status);
        assert!(!s.status.is_empty());
    }

    #[test]
    fn saving_nothing_says_so_and_leaves_the_keychain_alone() {
        let mut s = ui();
        s.key = TextEdit::new("   ");
        s.save_key();
        assert_eq!(s.error.as_deref(), Some("Paste your Anthropic API key first."));
    }

    fn frames(s: SettingsUi) -> Runtime<SettingsUi> {
        let mut rt = Runtime::new(s, view, SIZE, Env::new(true, 2.0));
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        rt
    }

    fn shown(rt: &Runtime<SettingsUi>) -> Vec<rune::ElementInfo> {
        rt.elements().into_iter().filter(|e| !e.exiting).collect()
    }

    /// Everything drawn lies inside the window, clear of its padding on the right.
    fn inside(rt: &Runtime<SettingsUi>) {
        for e in shown(rt).into_iter().filter(|e| e.rect[2] < SIZE.0 - 1.0) {
            let right = e.rect[0] + e.rect[2];
            assert!(right <= SIZE.0 - 24.0 + 0.5, "{:?} {:?} ends at x {right}", e.key, e.text.as_deref().or(e.label.as_deref()));
        }
    }

    #[test]
    fn the_worst_case_fits_screen_recording_off_and_both_shortcuts_taken() {
        let mut s = ui();
        s.check_screen_recording = || false;
        s.refresh_status();
        let taken = |s: &str| -> Shortcut { s.parse().unwrap() };
        let started = crate::settings::start_shortcuts(
            &[(Some(taken("ctrl+alt+shift+cmd+j")), crate::settings::DEFAULT_SHORTCUT), (Some(taken("ctrl+alt+shift+cmd+k")), crate::settings::DEFAULT_QUICK_SHORTCUT)],
            |_, s| if s.is_some_and(|s| s.to_string().contains('j') || s.to_string().contains('k')) { Err(rune::ShortcutError::Conflict) } else { Ok(()) },
        );
        for (which, started) in [ShortcutFor::Capture, ShortcutFor::QuickInput].into_iter().zip(started) {
            s.shortcut_notice(which, started.active, started.notice.expect("taken"));
        }
        s.error = Some("Couldn't save the key in the Keychain: The user name or passphrase you entered is not correct.".into());
        let rt = frames(s);
        inside(&rt);
        let low = shown(&rt).iter().filter(|e| e.key.is_some() || !e.role.is_empty()).filter(|e| e.rect[3] < SIZE.1 - 1.0).map(|e| e.rect[1] + e.rect[3]).fold(0.0, f32::max);
        assert!(low + 24.0 <= SIZE.1, "the lowest ends at {low}");
    }

    #[test]
    fn both_shortcuts_can_be_recorded() {
        let mut rt = frames(ui());
        let els = shown(&rt);
        for label in ["Capture area", "Quick input"] {
            assert!(els.iter().any(|e| e.label.as_deref() == Some(label)), "{label}");
        }
        let default: Shortcut = "ctrl+alt+space".parse().unwrap();
        rt.update(|s, _| assert_eq!(s.quick_recorder.shortcut(), Some(&default)));
    }

    #[test]
    fn a_taken_quick_input_shortcut_shows_the_one_that_works_and_says_why() {
        let mut s = ui();
        let fallback: Shortcut = "ctrl+alt+space".parse().unwrap();
        s.shortcut_notice(ShortcutFor::QuickInput, Some(fallback.clone()), "⌃⌥J isn't available: ⌃⌥Space works instead.".into());
        assert_eq!(s.quick_recorder.shortcut(), Some(&fallback));
        assert!(s.capture_notice.is_none());
        let rt = frames(s);
        let els = shown(&rt);
        let notice = els.iter().find(|e| e.key.as_deref() == Some("quick-notice")).and_then(|e| e.text.clone());
        assert!(notice.is_some_and(|n| n.contains("⌃⌥J")));
        let low = els.iter().filter(|e| e.role == "button" || e.role == "checkbox").map(|e| e.rect[1] + e.rect[3]).fold(0.0, f32::max);
        assert!(low + 24.0 + 20.0 <= SIZE.1, "inside the window: {low}");
    }

    #[test]
    fn a_shortcut_that_cant_be_had_at_all_stays_shown_and_recording_it_again_retries() {
        let mut s = ui();
        let saved = s.recorder.shortcut().cloned();
        s.shortcut_notice(ShortcutFor::Capture, None, "⇧⌘Space isn't available.".into());
        assert_eq!(s.recorder.shortcut(), saved.as_ref(), "the saved one, unregistered");
        assert!(s.capture_notice.is_some() && s.quick_notice.is_none());
    }

    #[test]
    fn screen_recording_off_says_so_and_offers_system_settings() {
        let mut s = ui();
        s.check_screen_recording = || false;
        s.refresh_status();
        let rt = frames(s);
        let els = shown(&rt);
        assert!(els.iter().any(|e| e.key.as_deref() == Some("permissions")));
        let says = |key: &str| els.iter().find(|e| e.key.as_deref() == Some(key)).and_then(|e| e.text.clone()).unwrap_or_default();
        assert!(says("screen-recording").contains("Off"), "{:?}", says("screen-recording"));
        assert!(els.iter().any(|e| e.role == "button" && e.label.as_deref() == Some("Open System Settings…")), "a way there");
        // The lowest control (the page itself fills the window); an error line (about 20) fits under it.
        let low = els.iter().filter(|e| e.role == "button" || e.role == "checkbox").map(|e| e.rect[1] + e.rect[3]).fold(0.0, f32::max);
        assert!(low + 24.0 + 20.0 <= SIZE.1, "inside the window: {low}");
    }

    #[test]
    fn screen_recording_on_says_so_without_the_button() {
        let mut s = ui();
        s.check_screen_recording = || true;
        s.refresh_status();
        let rt = frames(s);
        let els = shown(&rt);
        let line = els.iter().find(|e| e.key.as_deref() == Some("screen-recording")).and_then(|e| e.text.clone()).unwrap_or_default();
        assert!(line.contains("On"), "{line:?}");
        assert!(!els.iter().any(|e| e.label.as_deref() == Some("Open System Settings…")));
    }

    #[test]
    fn coming_back_to_the_window_checks_again() {
        // Turned on in System Settings while Settings stayed open.
        static ALLOWED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let mut s = ui();
        s.check_screen_recording = || ALLOWED.load(std::sync::atomic::Ordering::Relaxed);
        s.refresh_status();
        let mut rt = frames(s);
        rt.update(|s, _| assert_eq!(s.screen_recording, Some(false)));
        ALLOWED.store(true, std::sync::atomic::Ordering::Relaxed);
        rt.window_focus(false);
        rt.window_focus(true);
        rt.frame(1.0 / 60.0);
        rt.update(|s, _| assert_eq!(s.screen_recording, Some(true)));
    }

    #[test]
    fn the_window_shows_the_model_the_key_field_and_where_replies_come_from_all_inside_it() {
        let mut s = ui();
        s.refresh_status();
        let mut rt = Runtime::new(s, view, SIZE, Env::new(true, 2.0));
        for _ in 0..10 {
            rt.frame(1.0 / 60.0);
        }
        let els: Vec<_> = rt.elements().into_iter().filter(|e| !e.exiting).collect();
        for key in ["model-section", "api-key", "status"] {
            assert!(els.iter().any(|e| e.key.as_deref() == Some(key)), "{key}");
        }
        assert!(els.iter().any(|e| e.label.as_deref() == Some("Anthropic API key")));
        let fields: Vec<_> = els.iter().filter(|e| e.label.as_deref() == Some("Anthropic API key")).map(|e| (e.role, e.path.clone(), rt.element_is_secure(&e.path))).collect();
        assert!(fields.iter().any(|(_, _, secure)| *secure), "the key is hidden: {fields:?}");
        // The content ends with the Reduce Motion checkbox, under the model's section; the
        // padding (24) and an error line (about 20) fit under it.
        let low = els.iter().filter(|e| e.role == "checkbox").map(|e| e.rect[1] + e.rect[3]).fold(0.0, f32::max);
        assert!(low + 24.0 + 20.0 <= SIZE.1, "the checkbox ends at {low}, the window at {}", SIZE.1);
    }
}
