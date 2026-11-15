//! The Settings window: the capture shortcut (rune-kit's recorder) and the save folder.

use std::path::PathBuf;
use std::sync::OnceLock;

use rune::{AppHandle, Cx, El, FileDialogOptions, Shortcut, ShortcutError, Tx, col, row, text};
use rune_kit::{Button, Kit, ShortcutRecorder, ShortcutRecorderState};

use crate::Host;
use crate::settings::{CaptureShortcut, Settings};

pub struct SettingsUi {
    settings: Settings,
    recorder: ShortcutRecorderState,
    folder: PathBuf,
    /// None only in the placeholder `init` Rune requires; the real state is supplied.
    host: Option<AppHandle<Host>>,
    capture: CaptureShortcut,
    error: Option<String>,
}

impl SettingsUi {
    pub fn new(settings: Settings, host: AppHandle<Host>, capture: CaptureShortcut) -> SettingsUi {
        let recorder = ShortcutRecorderState::new(settings.shortcut());
        let folder = settings.folder();
        SettingsUi { settings, recorder, folder, host: Some(host), capture, error: None }
    }

    /// The saved shortcut couldn't be registered at startup: say so, and let recording the same
    /// chord again retry it.
    pub fn shortcut_failed(&mut self, error: String) {
        self.recorder.mark_unregistered();
        self.error = Some(error);
    }

    /// The window is built with its state supplied (`WindowSpec::state`); `AppSpec` still wants an
    /// `init`, which gets a detached copy that changes nothing.
    pub fn placeholder() -> SettingsUi {
        let settings = Settings::with(rune::prefs::Prefs::memory());
        let recorder = ShortcutRecorderState::new(settings.shortcut());
        SettingsUi { folder: settings.folder(), settings, recorder, host: None, capture: CaptureShortcut::default(), error: None }
    }
}

fn kit(dark: bool) -> &'static Kit {
    static DARK: OnceLock<Kit> = OnceLock::new();
    static LIGHT: OnceLock<Kit> = OnceLock::new();
    if dark { DARK.get_or_init(|| Kit::neutral(true)) } else { LIGHT.get_or_init(|| Kit::neutral(false)) }
}

pub fn view(s: &SettingsUi, cx: &mut Cx<SettingsUi>) -> El {
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
    let mut page = col()
        .w_full()
        .h_full()
        .padding(24.0)
        .gap(18.0)
        .fill(k.c.page)
        .child(col().gap(8.0).child(label("Capture shortcut")).child(recorder))
        .child(col().gap(8.0).child(label("Save screenshots to")).child(folder));
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
