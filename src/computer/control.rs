//! The session shared by the MCP handler and the app (Stop, the tray item, the panic shortcut,
//! the timeout watcher). Every transition is reported to `notify`, for what shows it.
//!
//! It also holds Settings' switch: off (the default, until the app reads the setting), every
//! tool is refused before anything else happens.

use super::session::{Reason, Session, StateEvent};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

type Notify = Box<dyn Fn(&StateEvent) + Send + Sync>;

pub struct Control {
    session: Mutex<Session>,
    notify: Notify,
    enabled: AtomicBool,
}

impl Control {
    pub fn new(notify: impl Fn(&StateEvent) + Send + Sync + 'static) -> Self {
        Control { session: Mutex::new(Session::default()), notify: Box::new(notify), enabled: AtomicBool::new(false) }
    }

    /// Settings' switch is on.
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Turns computer control on or off; off also ends the session, so what was queued drops.
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::SeqCst);
        if !on {
            self.end(Reason::Stopped);
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Session) -> T) -> T {
        f(&mut self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Applies a transition and reports it while still holding the session, so two threads'
    /// reports arrive in the order their changes happened. `notify` must not call back in.
    fn change(&self, f: impl FnOnce(&mut Session) -> Option<StateEvent>) {
        let mut session = self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(event) = f(&mut session) {
            (self.notify)(&event);
        }
    }

    #[cfg(test)]
    pub fn status(&self, now_ms: u64) -> StateEvent {
        self.with(|s| s.status(now_ms))
    }

    pub fn is_active(&self, now_ms: u64) -> bool {
        self.with(|s| s.is_active(now_ms))
    }

    pub fn epoch(&self) -> u64 {
        self.with(|s| s.epoch())
    }

    pub fn grant(&self, now_ms: u64) {
        self.change(|s| Some(s.grant(now_ms)));
    }

    pub fn end(&self, reason: Reason) {
        self.change(|s| s.end(reason));
    }

    pub fn expire(&self, now_ms: u64) {
        self.change(|s| s.expire(now_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::super::session::SESSION_MS;
    use super::*;
    use std::sync::Arc;

    const NOW: u64 = 1_700_000_000_000;

    fn control() -> (Control, Arc<Mutex<Vec<StateEvent>>>) {
        let states = Arc::new(Mutex::new(Vec::new()));
        let sink = states.clone();
        (Control::new(move |event| sink.lock().unwrap().push(event.clone())), states)
    }

    #[test]
    fn a_new_control_is_off_until_the_user_turns_it_on() {
        let (control, states) = control();
        assert!(!control.enabled());
        control.set_enabled(true);
        assert!(control.enabled());
        assert!(states.lock().unwrap().is_empty(), "the switch alone starts no session");
    }

    #[test]
    fn turning_it_off_ends_the_session_and_drops_what_was_queued() {
        let (control, states) = control();
        control.set_enabled(true);
        control.grant(NOW);
        let epoch = control.epoch();
        control.set_enabled(false);
        assert!(!control.enabled() && !control.is_active(NOW));
        assert_eq!(control.epoch(), epoch + 1);
        assert_eq!(states.lock().unwrap().last(), Some(&StateEvent { active: false, ends_at: None, reason: Some(Reason::Stopped) }));
    }

    #[test]
    fn granting_starts_a_session_and_reports_it() {
        let (control, states) = control();
        control.grant(NOW);
        assert!(control.is_active(NOW));
        let granted = StateEvent { active: true, ends_at: Some(NOW + SESSION_MS), reason: Some(Reason::Granted) };
        assert_eq!(*states.lock().unwrap(), vec![granted]);
        assert_eq!(control.status(NOW), StateEvent { active: true, ends_at: Some(NOW + SESSION_MS), reason: None });
    }

    #[test]
    fn panic_ends_the_session_bumps_the_epoch_and_reports_it_once() {
        let (control, states) = control();
        control.grant(NOW);
        let epoch = control.epoch();
        control.end(Reason::Panic);
        assert!(!control.is_active(NOW));
        assert_eq!(control.epoch(), epoch + 1);
        let ended = StateEvent { active: false, ends_at: None, reason: Some(Reason::Panic) };
        assert_eq!(states.lock().unwrap().last(), Some(&ended));
        // Without a session there is nothing to end, and nothing is reported; what waits drops.
        control.end(Reason::Panic);
        assert_eq!(states.lock().unwrap().len(), 2);
        assert_eq!(control.epoch(), epoch + 2);
    }

    #[test]
    fn expiry_ends_the_session_at_its_end_and_reports_it_once() {
        let (control, states) = control();
        control.grant(NOW);
        control.expire(NOW + SESSION_MS - 1);
        assert_eq!(states.lock().unwrap().len(), 1);
        control.expire(NOW + SESSION_MS);
        assert!(!control.is_active(NOW + SESSION_MS));
        let timed_out = StateEvent { active: false, ends_at: None, reason: Some(Reason::Timeout) };
        assert_eq!(states.lock().unwrap().last(), Some(&timed_out));
        control.expire(NOW + SESSION_MS + 1);
        assert_eq!(states.lock().unwrap().len(), 2);
    }
}
