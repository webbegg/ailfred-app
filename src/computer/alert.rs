//! The native dialogs computer control asks through (`Dialogs`): an `NSAlert` run modally on the
//! main thread, asked from the tool's thread, which waits for the answer. Ported from Gentle
//! Dot's `alert.rs`.
//!
//! Refusing is the default. The refusing button is added first (`NSAlertFirstButtonReturn`) with
//! Return as its key equivalent; the allowing button is second with none, so no key allows. An
//! alert that can't be shown, or any answer but the allowing button's, refuses.

use super::Dialogs;

/// The `NSAlert` response of the refusing button, added first (`NSAlertFirstButtonReturn`).
pub const REFUSE_RESPONSE: isize = 1000;
/// The `NSAlert` response of the allowing button, added second (`NSAlertSecondButtonReturn`).
pub const ALLOW_RESPONSE: isize = 1001;

/// The two buttons of a yes/no alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub allow: &'static str,
    pub refuse: &'static str,
}

impl Choice {
    /// The buttons in the order they are added: the first is the default (Return).
    pub fn alert_buttons(&self) -> [&'static str; 2] {
        [self.refuse, self.allow]
    }

    /// True only for the allowing button's response; `None` is an alert that never ran.
    pub fn allows_response(&self, response: Option<isize>) -> bool {
        self.allow != self.refuse && response == Some(ALLOW_RESPONSE)
    }
}

pub const GRANT_TITLE: &str = "Let Ailfred control this Mac?";
pub const GRANT_MESSAGE: &str = "For the next 30 minutes, the model can take screenshots of your screen, click, and type in \
your apps. Actions that send, pay, buy or delete still ask you first, and some apps (System Settings, password managers) \
are always off limits. Turn computer control off any time in Ailfred's Settings.";
pub const GRANT: Choice = Choice { allow: "Allow for 30 Minutes", refuse: "Don't Allow" };
pub const CONFIRM_TITLE: &str = "Allow this action?";
pub const CONFIRM: Choice = Choice { allow: "Allow", refuse: "Don't Allow" };

/// The real dialogs.
pub struct MacDialogs;

impl Dialogs for MacDialogs {
    fn ask_grant(&self) -> bool {
        ask(GRANT_TITLE, GRANT_MESSAGE, GRANT)
    }

    fn confirm(&self, message: &str) -> bool {
        ask(CONFIRM_TITLE, message, CONFIRM)
    }
}

/// Shows the alert and waits for the answer. Blocks, so it never runs on the main thread (there
/// it refuses at once).
pub fn ask(title: &str, message: &str, choice: Choice) -> bool {
    use objc2::MainThreadMarker;
    if MainThreadMarker::new().is_some() {
        eprintln!("Ailfred: a computer-control question was asked on the main thread, so it was refused");
        return false;
    }
    let (answer, answered) = std::sync::mpsc::channel();
    let (title, message) = (title.to_owned(), message.to_owned());
    dispatch2::DispatchQueue::main().exec_async(move || {
        let response = MainThreadMarker::new().map(|mtm| run(mtm, &title, &message, choice));
        let _ = answer.send(response);
    });
    // A block dropped without running (the app is quitting) reads as no answer.
    choice.allows_response(answered.recv().ok().flatten())
}

fn run(mtm: objc2::MainThreadMarker, title: &str, message: &str, choice: Choice) -> isize {
    use objc2_app_kit::{NSAlert, NSAlertStyle, NSApplication};
    use objc2_foundation::NSString;
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(title));
    alert.setInformativeText(&NSString::from_str(message));
    let [refuse, allow] = choice.alert_buttons();
    alert.addButtonWithTitle(&NSString::from_str(refuse)).setKeyEquivalent(&NSString::from_str("\r"));
    alert.addButtonWithTitle(&NSString::from_str(allow)).setKeyEquivalent(&NSString::from_str(""));
    // Ailfred is a menu-bar app; without this the alert can open behind the front app.
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    alert.runModal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusing_button_comes_first_so_return_refuses() {
        assert_eq!(GRANT.alert_buttons(), ["Don't Allow", "Allow for 30 Minutes"]);
        assert_eq!(CONFIRM.alert_buttons(), ["Don't Allow", "Allow"]);
        assert_eq!(REFUSE_RESPONSE, objc2_app_kit::NSAlertFirstButtonReturn);
        assert_eq!(ALLOW_RESPONSE, objc2_app_kit::NSAlertSecondButtonReturn);
    }

    #[test]
    fn only_the_allowing_button_allows() {
        assert!(GRANT.allows_response(Some(ALLOW_RESPONSE)));
        for refused in [None, Some(REFUSE_RESPONSE), Some(-1000), Some(1002), Some(0)] {
            assert!(!GRANT.allows_response(refused), "{refused:?}");
        }
        let same = Choice { allow: "OK", refuse: "OK" };
        assert!(!same.allows_response(Some(ALLOW_RESPONSE)), "buttons that can't be told apart never allow");
    }

}
