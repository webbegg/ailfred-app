//! The session shared by the MCP handler and the app (Stop, the tray item, the panic shortcut,
//! the timeout watcher). Every transition is reported to `notify`, for what shows it.

use super::session::{Reason, Session, StateEvent};
use std::sync::Mutex;

type Notify = Box<dyn Fn(&StateEvent) + Send + Sync>;

pub struct Control {
    session: Mutex<Session>,
    notify: Notify,
}

impl Control {
    pub fn new(notify: impl Fn(&StateEvent) + Send + Sync + 'static) -> Self {
        Control { session: Mutex::new(Session::default()), notify: Box::new(notify) }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Session) -> T) -> T {
        f(&mut self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    fn report(&self, event: Option<StateEvent>) {
        if let Some(event) = event {
            (self.notify)(&event);
        }
    }

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
        let event = self.with(|s| s.grant(now_ms));
        self.report(Some(event));
    }

    pub fn end(&self, reason: Reason) {
        let event = self.with(|s| s.end(reason));
        self.report(event);
    }

    pub fn expire(&self, now_ms: u64) {
        let event = self.with(|s| s.expire(now_ms));
        self.report(event);
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
        // Without a session there is nothing to end, and nothing is reported.
        control.end(Reason::Panic);
        assert_eq!(states.lock().unwrap().len(), 2);
        assert_eq!(control.epoch(), epoch + 1);
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
