//! The computer-control session: idle until the user allows it, then granted until `ends_at`,
//! ended by Stop, the tray item, the panic shortcut, or the timeout. Every end bumps `epoch`,
//! so actions queued under an earlier epoch are dropped.
//!
//! Gentle Dot's "yolo mode" (risky actions without a confirmation) is left out: every risky
//! action asks.

pub const SESSION_MS: u64 = 30 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Granted,
    Stopped,
    Panic,
    Timeout,
}

/// What the session is after a transition, for what shows it (the tray, the panel). Times are
/// Unix epoch milliseconds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateEvent {
    pub active: bool,
    pub ends_at: Option<u64>,
    pub reason: Option<Reason>,
}

#[derive(Debug, Default)]
pub struct Session {
    ends_at: Option<u64>,
    epoch: u64,
}

impl Session {
    pub fn is_active(&self, now_ms: u64) -> bool {
        self.ends_at.is_some_and(|end| now_ms < end)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    #[cfg(test)]
    pub fn status(&self, now_ms: u64) -> StateEvent {
        let active = self.is_active(now_ms);
        StateEvent { active, ends_at: self.ends_at.filter(|_| active), reason: None }
    }

    /// Starts (or restarts) a session of `SESSION_MS`.
    pub fn grant(&mut self, now_ms: u64) -> StateEvent {
        self.ends_at = Some(now_ms + SESSION_MS);
        StateEvent { active: true, ends_at: self.ends_at, reason: Some(Reason::Granted) }
    }

    /// Ends the session for `reason`. `None` when there was none to end; the epoch moves on
    /// either way, so a call that waits for the grant meanwhile drops too.
    pub fn end(&mut self, reason: Reason) -> Option<StateEvent> {
        self.epoch += 1;
        self.ends_at.take()?;
        Some(StateEvent { active: false, ends_at: None, reason: Some(reason) })
    }

    /// Ends the session with `Timeout` once `now_ms` reaches its end.
    pub fn expire(&mut self, now_ms: u64) -> Option<StateEvent> {
        if self.ends_at.is_some_and(|end| now_ms >= end) { self.end(Reason::Timeout) } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000;

    #[test]
    fn a_new_session_is_idle() {
        let session = Session::default();
        assert!(!session.is_active(NOW));
        assert_eq!(session.status(NOW), StateEvent::default());
    }

    #[test]
    fn granting_starts_thirty_minutes() {
        let mut session = Session::default();
        let event = session.grant(NOW);
        assert_eq!(event, StateEvent { active: true, ends_at: Some(NOW + SESSION_MS), reason: Some(Reason::Granted) });
        assert!(session.is_active(NOW + SESSION_MS - 1));
        assert_eq!(session.status(NOW + 5).ends_at, Some(NOW + SESSION_MS));
    }

    #[test]
    fn stop_and_panic_end_the_session_and_bump_the_epoch() {
        for reason in [Reason::Stopped, Reason::Panic] {
            let mut session = Session::default();
            session.grant(NOW);
            let epoch = session.epoch();
            assert_eq!(session.end(reason), Some(StateEvent { active: false, ends_at: None, reason: Some(reason) }));
            assert!(!session.is_active(NOW));
            assert_eq!(session.epoch(), epoch + 1);
        }
    }

    #[test]
    fn ending_an_idle_session_reports_nothing_but_drops_what_waits() {
        let mut session = Session::default();
        assert_eq!(session.end(Reason::Stopped), None);
        assert_eq!(session.epoch(), 1, "a call waiting for the grant is dropped");
    }

    #[test]
    fn the_session_times_out_at_its_end() {
        let mut session = Session::default();
        session.grant(NOW);
        assert_eq!(session.expire(NOW + SESSION_MS - 1), None);
        assert!(!session.is_active(NOW + SESSION_MS));
        assert_eq!(session.expire(NOW + SESSION_MS), Some(StateEvent { active: false, ends_at: None, reason: Some(Reason::Timeout) }));
        assert_eq!(session.epoch(), 1);
        assert_eq!(session.expire(NOW + SESSION_MS + 1), None);
    }
}
