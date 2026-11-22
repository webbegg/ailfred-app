//! The Settings window: the capture shortcut (rune-kit's recorder), the save folder, the model
//! (Claude's version and the Anthropic API key, which lives in the Keychain), and whether
//! animations play under the system's Reduce Motion.

use std::path::PathBuf;
use std::sync::OnceLock;

use rune::{AppHandle, Cx, El, FileDialogOptions, Shortcut, ShortcutError, TextEdit, Tx, col, row, text};
use rune_kit::{Button, Checkbox, Kit, SegmentedControl, ShortcutRecorder, ShortcutRecorderState, TextInput};

use crate::Host;
use crate::keychain;
use crate::model;
use crate::settings::{CaptureShortcut, Settings};

/// The window's size in points.
pub const SIZE: (f32, f32) = (480.0, 400.0);

pub struct SettingsUi {
    settings: Settings,
    recorder: ShortcutRecorderState,
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
    capture: CaptureShortcut,
    error: Option<String>,
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
    pub fn new(settings: Settings, host: AppHandle<Host>, capture: CaptureShortcut) -> SettingsUi {
        let recorder = ShortcutRecorderState::new(settings.shortcut());
        let folder = settings.folder();
        let ignore_reduce_motion = settings.ignore_reduce_motion();
        let model = settings.model();
        SettingsUi { settings, recorder, folder, ignore_reduce_motion, model, key: TextEdit::new(""), status: String::new(), host: Some(host), capture, error: None }
    }

    /// Settings opened: says where replies come from now.
    pub fn refresh_status(&mut self) {
        self.status = if self.model == model::PI { pi_status() } else { model::status_now() };
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

    /// The saved shortcut couldn't be registered at startup: say so, and let recording the same
    /// chord again retry it.
    pub fn shortcut_failed(&mut self, error: String) {
        self.recorder.mark_unregistered();
        self.error = Some(error);
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
        SettingsUi {
            folder: settings.folder(),
            ignore_reduce_motion: settings.ignore_reduce_motion(),
            model: settings.model(),
            key: TextEdit::new(""),
            status: String::new(),
            settings,
            recorder,
            host: None,
            capture: CaptureShortcut::default(),
            error: None,
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
    let recorder = ShortcutRecorder::new("capture-shortcut", &s.recorder, |s: &mut SettingsUi| &mut s.recorder)
        .label("Capture area")
        .on_change(change_shortcut)
        .build(k);
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
        .child(col().gap(8.0).child(label("Capture shortcut")).child(recorder))
        .child(col().gap(8.0).child(label("Save screenshots to")).child(folder))
        .child(model_section)
        .child(
            Checkbox::new(s.ignore_reduce_motion)
                .label("Show animations even with Reduce Motion on")
                .description("Ignores the Reduce Motion setting in System Settings › Accessibility › Display.")
                .on_toggle(|s: &mut SettingsUi| s.toggle_reduce_motion())
                .build(k),
        );
    if let Some(error) = &s.error {
        page = page.child(text(error.clone()).font(k.ty.small).color(k.c.text_weak));
    }
    page
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
    let shown = shortcut.cloned();
    host.update(move |host, _| host.show_capture_shortcut(shown.as_ref()));
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
    use crate::settings::MODEL_TEST;

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
