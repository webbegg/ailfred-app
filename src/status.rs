//! What Ailfred is doing, app-wide, in one place: derived from the reply streaming (its events
//! as the host passes them on) and whether the chat shows. The menu-bar icon and its tooltip
//! show it (`tray.rs`).

use crate::reply::Event;

/// What the menu-bar icon says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing going.
    Idle,
    /// A reply waits for its model to start (pi's first run).
    Starting,
    /// A reply is being thought out or written.
    Thinking,
    /// The model is using a tool: what it does.
    Working(String),
    /// The model asked something and waits for the answer.
    NeedsYou(String),
    /// A reply failed: why. Until the chat is seen.
    Error(String),
    /// A reply finished while the chat was hidden. Until the chat is seen.
    Replied,
}

impl Status {
    /// The tooltip: "Ailfred", then what it's doing.
    pub fn tooltip(&self) -> String {
        match self {
            Status::Idle => "Ailfred".into(),
            Status::Starting => "Ailfred: starting…".into(),
            Status::Thinking => "Ailfred: thinking…".into(),
            Status::Working(what) => format!("Ailfred: {what}"),
            Status::NeedsYou(question) => format!("Ailfred needs you: {question}"),
            Status::Error(why) => format!("Ailfred: {why}"),
            Status::Replied => "Ailfred replied".into(),
        }
    }
}

/// Follows the reply streaming. Events of a stopped or replaced reply change nothing.
#[derive(Default)]
pub struct Tracker {
    /// The reply streaming (its token), if any.
    current: Option<u64>,
    /// The steps running, by id, with what each does.
    steps: Vec<(u32, String)>,
    /// What shows when nothing streams.
    after: Option<Status>,
    /// The model hasn't said anything yet and is starting.
    starting: bool,
    /// The chat shows: a reply ending there needs no mark.
    shown: bool,
    /// Replies steered into the run going: each takes over when its events come.
    steered: Vec<u64>,
}

impl Tracker {
    /// Reply `token` started (`starting`: its model isn't up yet).
    pub fn start(&mut self, token: u64, starting: bool) {
        self.current = Some(token);
        self.steps.clear();
        self.after = None;
        self.starting = starting;
    }

    /// The next event of reply `token`.
    pub fn event(&mut self, token: u64, event: &Event) {
        if let Some(at) = self.steered.iter().position(|&t| t == token) {
            self.steered.remove(at);
            self.start(token, false);
        }
        if self.current != Some(token) {
            return;
        }
        self.starting = false;
        match event {
            Event::Step { id, tool, target } => {
                let what = match (target.trim(), tool) {
                    ("", crate::reply::Tool::Other) => "Using a tool".to_owned(),
                    ("", tool) => tool.doing().to_owned(),
                    (target, tool) => format!("{} {target}", tool.doing()),
                };
                self.steps.push((*id, what));
            }
            Event::StepEnd { id, .. } => self.steps.retain(|(s, _)| s != id),
            Event::Ask { prompt, .. } => self.after = Some(Status::NeedsYou(prompt.clone())),
            Event::Fail(why) => self.ended((!self.shown).then(|| Status::Error(why.clone()))),
            Event::Done => self.ended((!self.shown).then_some(Status::Replied)),
            _ => {}
        }
    }

    /// Reply over: a question asked stays; else `mark`, if any.
    fn ended(&mut self, mark: Option<Status>) {
        self.current = None;
        self.steps.clear();
        if !matches!(self.after, Some(Status::NeedsYou(_))) {
            self.after = mark;
        }
    }

    /// Reply `token` was steered into the run going: it's followed once it starts.
    pub fn steered(&mut self, token: u64) {
        self.steered.push(token);
    }

    /// Stop, Retry or a new conversation: nothing streams, and what was left shown goes.
    pub fn stop(&mut self) {
        self.steered.clear();
        self.current = None;
        self.steps.clear();
        self.after = None;
        self.starting = false;
    }

    /// The chat shows (true) or hides. Shown, a failure or a reply is seen.
    pub fn shown(&mut self, shown: bool) {
        self.shown = shown;
        if shown && matches!(self.after, Some(Status::Error(_) | Status::Replied)) {
            self.after = None;
        }
    }

    pub fn status(&self) -> Status {
        if let Some(asked @ Status::NeedsYou(_)) = &self.after {
            return asked.clone();
        }
        if self.current.is_none() {
            return self.after.clone().unwrap_or(Status::Idle);
        }
        match self.steps.last() {
            _ if self.starting => Status::Starting,
            Some((_, what)) => Status::Working(what.clone()),
            None => Status::Thinking,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reply::Tool;

    fn step(id: u32, tool: Tool, target: &str) -> Event {
        Event::Step { id, tool, target: target.into() }
    }

    #[test]
    fn a_reply_thinks_works_on_each_tool_and_ends_idle_with_the_chat_showing() {
        let mut t = Tracker::default();
        t.shown(true);
        assert_eq!(t.status(), Status::Idle);
        t.start(1, false);
        assert_eq!(t.status(), Status::Thinking);
        t.event(1, &Event::Think("hm".into()));
        assert_eq!(t.status(), Status::Thinking);
        t.event(1, &step(7, Tool::Read, "src/main.rs"));
        assert_eq!(t.status(), Status::Working("Reading src/main.rs".into()));
        t.event(1, &step(8, Tool::Run, "cargo test"));
        assert_eq!(t.status(), Status::Working("Running cargo test".into()), "the latest step");
        t.event(1, &Event::StepEnd { id: 8, ok: true });
        assert_eq!(t.status(), Status::Working("Reading src/main.rs".into()), "the one still running");
        t.event(1, &step(9, Tool::Other, ""));
        assert_eq!(t.status(), Status::Working("Using a tool".into()), "no target");
        t.event(1, &Event::StepEnd { id: 9, ok: true });
        t.event(1, &Event::StepEnd { id: 7, ok: false });
        assert_eq!(t.status(), Status::Thinking);
        t.event(1, &Event::Text("Done.".into()));
        t.event(1, &Event::Done);
        assert_eq!(t.status(), Status::Idle);
    }

    #[test]
    fn a_reply_ending_while_the_chat_is_hidden_leaves_a_mark_until_its_seen() {
        let mut t = Tracker::default();
        t.start(1, false);
        t.event(1, &Event::Done);
        assert_eq!(t.status(), Status::Replied);
        t.shown(true);
        assert_eq!(t.status(), Status::Idle);
        // Hidden again, nothing new: no mark.
        t.shown(false);
        assert_eq!(t.status(), Status::Idle);
    }

    #[test]
    fn a_failure_shows_until_the_chat_is_seen_or_the_next_reply() {
        let mut t = Tracker::default();
        t.start(1, false);
        t.event(1, &Event::Fail("The model is overloaded.".into()));
        assert_eq!(t.status(), Status::Error("The model is overloaded.".into()));
        t.start(2, false);
        assert_eq!(t.status(), Status::Thinking);
        t.event(2, &Event::Fail("Again.".into()));
        t.shown(true);
        assert_eq!(t.status(), Status::Idle);
        // Seen as it happens: nothing stays.
        t.start(3, false);
        t.event(3, &Event::Fail("Shown.".into()));
        assert_eq!(t.status(), Status::Idle);
    }

    #[test]
    fn a_question_needs_the_user_until_they_answer_or_start_over_even_with_the_chat_showing() {
        let mut t = Tracker::default();
        t.shown(true);
        t.start(1, false);
        t.event(1, &Event::Ask { prompt: "Which layout?".into(), options: vec!["A".into()], multi: false });
        t.event(1, &Event::Done);
        assert_eq!(t.status(), Status::NeedsYou("Which layout?".into()));
        t.shown(false);
        t.shown(true);
        assert_eq!(t.status(), Status::NeedsYou("Which layout?".into()), "seeing it doesn't answer it");
        t.start(2, false);
        assert_eq!(t.status(), Status::Thinking, "the answer is the next message");
        t.event(2, &Event::Ask { prompt: "Sure?".into(), options: vec![], multi: false });
        t.stop();
        assert_eq!(t.status(), Status::Idle, "Cmd+N");
    }

    #[test]
    fn a_model_starting_shows_until_it_says_something() {
        let mut t = Tracker::default();
        t.start(1, true);
        assert_eq!(t.status(), Status::Starting);
        t.event(1, &Event::Text("Hi".into()));
        assert_eq!(t.status(), Status::Thinking);
    }

    #[test]
    fn a_stopped_or_replaced_reply_changes_nothing() {
        let mut t = Tracker::default();
        t.shown(true);
        t.start(1, false);
        t.stop();
        assert_eq!(t.status(), Status::Idle);
        t.event(1, &step(1, Tool::Read, "a"));
        t.event(1, &Event::Fail("late".into()));
        assert_eq!(t.status(), Status::Idle);
        t.start(2, false);
        t.start(3, false);
        t.event(2, &Event::Done);
        assert_eq!(t.status(), Status::Thinking, "2 was replaced by 3");
    }

    #[test]
    fn a_reply_steered_into_the_run_is_followed_once_it_starts() {
        let mut t = Tracker::default();
        t.shown(true);
        t.start(1, false);
        t.event(1, &step(1, Tool::Read, "a"));
        t.steered(2);
        assert_eq!(t.status(), Status::Working("Reading a".into()), "the run goes on");
        t.event(1, &Event::Done);
        t.event(2, &step(1, Tool::Run, "ls"));
        assert_eq!(t.status(), Status::Working("Running ls".into()));
        t.steered(3);
        t.stop();
        t.event(3, &Event::Text("late".into()));
        assert_eq!(t.status(), Status::Idle);
    }

    #[test]
    fn the_tooltip_says_what_it_does() {
        assert_eq!(Status::Idle.tooltip(), "Ailfred");
        assert_eq!(Status::Working("Running cargo test".into()).tooltip(), "Ailfred: Running cargo test");
        assert_eq!(Status::NeedsYou("Which?".into()).tooltip(), "Ailfred needs you: Which?");
        assert_eq!(Status::Replied.tooltip(), "Ailfred replied");
    }
}
