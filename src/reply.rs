//! A model's reply in the chat: the parts it streams in (thinking, markdown, agent steps, a
//! diff, a question, an error), built up from [`Event`]s as they arrive.

use std::time::{Duration, Instant};

use rune::TextEdit;

/// One piece of a streaming reply, in the order a model sends them.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// More of its thinking.
    Think(String),
    /// More of its answer (markdown, code fences included).
    Text(String),
    /// A tool call starts: `id` names it in the events that follow.
    Step { id: u32, tool: Tool, target: String },
    /// More of a running step's output.
    StepOutput { id: u32, text: String },
    /// A step ends, done or failed.
    StepEnd { id: u32, ok: bool },
    /// A change to a file, as a unified diff.
    Diff { path: String, patch: String },
    /// It asks the user to pick (`multi`: one or more) of `options`, or write something else.
    Ask { prompt: String, options: Vec<String>, multi: bool },
    /// It broke off.
    Fail(String),
    /// The model's process ended mid-reply (why): what came stays, and the reply is
    /// interrupted, as when Ailfred quits during one.
    Lost(String),
    /// It's finished.
    Done,
    /// A queued message's reply starts: the model read it (pi, steered into its run).
    Taken,
}

/// What an agent step does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Read,
    Search,
    Run,
    Edit,
    /// Any other tool (memory, a helper, an extension's): its target is its name.
    Other,
}

impl Tool {
    pub fn verb(self) -> &'static str {
        match self {
            Tool::Read => "Read",
            Tool::Search => "Searched",
            Tool::Run => "Ran",
            Tool::Edit => "Edited",
            Tool::Other => "Used",
        }
    }

    /// What `n` of its steps act on ("Read 2 files").
    pub fn noun(self, n: usize) -> &'static str {
        let (one, many) = match self {
            Tool::Read | Tool::Edit => ("file", "files"),
            Tool::Search => ("time", "times"),
            Tool::Run => ("command", "commands"),
            Tool::Other => ("tool", "tools"),
        };
        if n == 1 { one } else { many }
    }

    /// The same, while it runs.
    pub fn doing(self) -> &'static str {
        match self {
            Tool::Read => "Reading",
            Tool::Search => "Searching",
            Tool::Run => "Running",
            Tool::Edit => "Editing",
            Tool::Other => "Using",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepStatus {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Sent while another reply streamed: it waits for that one (or for pi to read it).
    Queued,
    Streaming,
    Done,
    /// The user stopped it.
    Stopped,
    Failed,
}

pub struct Thinking {
    pub text: String,
    pub started: Instant,
    /// How long it thought, once the answer starts.
    pub took: Option<Duration>,
    pub open: bool,
}

pub struct Step {
    pub id: u32,
    pub tool: Tool,
    pub target: String,
    pub output: String,
    pub status: StepStatus,
    pub open: bool,
}

/// Steps in a row, shown as one run.
pub struct Agent {
    pub steps: Vec<Step>,
    pub open: bool,
}

impl Agent {
    /// One line: what it does now ("Running cargo test…"), or what it did, by kind in the
    /// order they came ("Read 2 files · Ran 1 command · 1 failed").
    pub fn summary(&self) -> String {
        if let Some(s) = self.steps.iter().rev().find(|s| s.status == StepStatus::Running) {
            return format!("{}…", doing(s.tool, &s.target));
        }
        let mut kinds: Vec<(Tool, usize)> = Vec::new();
        for s in &self.steps {
            match kinds.iter_mut().find(|(t, _)| *t == s.tool) {
                Some((_, n)) => *n += 1,
                None => kinds.push((s.tool, 1)),
            }
        }
        let mut parts: Vec<String> = kinds.into_iter().map(|(tool, n)| format!("{} {n} {}", tool.verb(), tool.noun(n))).collect();
        let failed = self.steps.iter().filter(|s| s.status == StepStatus::Failed).count();
        if failed > 0 {
            parts.push(format!("{failed} failed"));
        }
        parts.join(" · ")
    }
}

/// What a step running on `target` does, in words ("Reading src/main.rs").
pub fn doing(tool: Tool, target: &str) -> String {
    match (target.trim(), tool) {
        ("", Tool::Other) => "Using a tool".to_owned(),
        ("", tool) => tool.doing().to_owned(),
        (target, tool) => format!("{} {target}", tool.doing()),
    }
}

pub struct Diff {
    pub path: String,
    pub patch: String,
    pub added: usize,
    pub removed: usize,
    pub open: bool,
    pub applied: bool,
}

pub struct Question {
    pub prompt: String,
    pub options: Vec<String>,
    pub multi: bool,
    pub picked: Vec<usize>,
    /// "Something else": the user's own answer.
    pub other: TextEdit,
    /// What the user answered; then it's read only.
    pub answered: Option<String>,
}

// A reply holds a handful of parts: boxing the big ones would buy nothing.
#[allow(clippy::large_enum_variant)]
pub enum Part {
    Thinking(Thinking),
    Text(String),
    Agent(Agent),
    Diff(Diff),
    Question(Question),
    Error(String),
}

/// A diff longer than this many changed lines starts collapsed.
pub const DIFF_OPEN_LINES: usize = 12;

pub struct Reply {
    pub id: u64,
    pub parts: Vec<Part>,
    pub state: State,
    /// What it answers: Retry asks it again.
    pub prompt: String,
    /// Its stream: events carry it, so a stopped or replaced stream feeds nothing.
    pub token: u64,
    /// It answers the user's answer to a question.
    pub answer: bool,
    /// Queued, its message is with the model already: steered into pi's run.
    pub steered: bool,
    /// It ended unfinished without the user stopping it: Ailfred quit, or the model's process
    /// ended. The conversation's last one offers to go on (`Chat::go_on`).
    pub interrupted: bool,
}

impl Reply {
    pub fn new(id: u64, prompt: impl Into<String>) -> Self {
        Reply { id, parts: Vec::new(), state: State::Streaming, prompt: prompt.into(), token: 0, answer: false, steered: false, interrupted: false }
    }

    pub fn streaming(&self) -> bool {
        self.state == State::Streaming
    }

    /// Streaming, or steered into pi's run going: what the model says next is its own.
    pub fn live(&self) -> bool {
        self.streaming() || (self.state == State::Queued && self.steered)
    }

    /// It ended with a question the user hasn't answered.
    pub fn asks(&self) -> bool {
        self.parts.iter().any(|p| matches!(p, Part::Question(q) if q.answered.is_none()))
    }

    /// What the model is told it answered, in a later message's history: its text, if it
    /// finished or was stopped with some. A failed or empty reply has none.
    pub fn history_text(&self) -> Option<String> {
        if !matches!(self.state, State::Done | State::Stopped) {
            return None;
        }
        let texts: Vec<&str> = self.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect();
        let text = texts.join("\n\n");
        (!text.trim().is_empty()).then_some(text)
    }

    /// Takes in the next event (`now`: when it arrived); none once it's over.
    pub fn apply(&mut self, event: Event, now: Instant) {
        // A queued reply starts with what the model says to it.
        if self.state == State::Queued && self.steered {
            self.state = State::Streaming;
        }
        if !self.streaming() {
            return;
        }
        if !matches!(event, Event::Think(_)) {
            self.end_thinking(now);
        }
        match event {
            Event::Think(more) => match self.parts.last_mut() {
                Some(Part::Thinking(t)) if t.took.is_none() => t.text.push_str(&more),
                _ => self.parts.push(Part::Thinking(Thinking { text: more, started: now, took: None, open: false })),
            },
            Event::Text(more) => match self.parts.last_mut() {
                Some(Part::Text(text)) => text.push_str(&more),
                _ => self.parts.push(Part::Text(more)),
            },
            Event::Step { id, tool, target } => {
                let step = Step { id, tool, target, output: String::new(), status: StepStatus::Running, open: false };
                match self.parts.last_mut() {
                    Some(Part::Agent(agent)) => agent.steps.push(step),
                    _ => self.parts.push(Part::Agent(Agent { steps: vec![step], open: true })),
                }
            }
            Event::StepOutput { id, text } => {
                if let Some(step) = self.step_mut(id) {
                    step.output.push_str(&text);
                }
            }
            Event::StepEnd { id, ok } => {
                if let Some(step) = self.step_mut(id) {
                    step.status = if ok { StepStatus::Done } else { StepStatus::Failed };
                }
            }
            Event::Diff { path, patch } => {
                let (added, removed) = diff_counts(&patch);
                let open = added + removed <= DIFF_OPEN_LINES;
                self.parts.push(Part::Diff(Diff { path, patch, added, removed, open, applied: false }));
            }
            Event::Ask { prompt, options, multi } => {
                self.parts.push(Part::Question(Question { prompt, options, multi, picked: Vec::new(), other: TextEdit::new(""), answered: None }));
            }
            Event::Fail(message) => {
                self.end_steps(StepStatus::Failed);
                self.parts.push(Part::Error(message));
                self.state = State::Failed;
            }
            Event::Lost(_) => {
                self.end_steps(StepStatus::Failed);
                (self.state, self.interrupted) = (State::Stopped, true);
            }
            Event::Done => {
                self.end_steps(StepStatus::Done);
                self.state = State::Done;
            }
            Event::Taken => {}
        }
    }

    /// The user stopped it: what came stays, running steps fail.
    pub fn stop(&mut self, now: Instant) {
        if self.streaming() {
            self.end_thinking(now);
            self.end_steps(StepStatus::Failed);
            self.state = State::Stopped;
        }
    }

    fn end_thinking(&mut self, now: Instant) {
        for part in &mut self.parts {
            if let Part::Thinking(t) = part
                && t.took.is_none()
            {
                t.took = Some(now.saturating_duration_since(t.started));
            }
        }
    }

    fn end_steps(&mut self, status: StepStatus) {
        for part in &mut self.parts {
            if let Part::Agent(agent) = part {
                for step in agent.steps.iter_mut().filter(|s| s.status == StepStatus::Running) {
                    step.status = status;
                }
            }
        }
    }

    fn step_mut(&mut self, id: u32) -> Option<&mut Step> {
        self.parts.iter_mut().find_map(|p| match p {
            Part::Agent(agent) => agent.steps.iter_mut().find(|s| s.id == id),
            _ => None,
        })
    }

    /// Part `part` (thinking, an agent run, a diff), or step `step` of an agent run, clicked:
    /// it opens or closes.
    pub fn toggle(&mut self, part: usize, step: Option<usize>) {
        match (self.parts.get_mut(part), step) {
            (Some(Part::Thinking(t)), None) => t.open = !t.open,
            (Some(Part::Agent(a)), None) => a.open = !a.open,
            (Some(Part::Agent(a)), Some(k)) => {
                if let Some(s) = a.steps.get_mut(k) {
                    s.open = !s.open;
                }
            }
            (Some(Part::Diff(d)), None) => d.open = !d.open,
            _ => {}
        }
    }

    /// Apply on diff `part` (fake: it only shows as applied).
    pub fn apply_diff(&mut self, part: usize) {
        if let Some(Part::Diff(d)) = self.parts.get_mut(part) {
            d.applied = true;
        }
    }

    /// What Copy puts on the clipboard: its answer as markdown, diffs as ```diff fences;
    /// thinking, steps and questions left out.
    pub fn copy_text(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        for part in &self.parts {
            match part {
                Part::Text(text) => out.push(text.trim().to_owned()),
                Part::Diff(d) => out.push(format!("```diff\n{}\n```", d.patch.trim_end())),
                _ => {}
            }
        }
        out.retain(|s| !s.is_empty());
        out.join("\n\n")
    }

    /// The user's answer to question `part`: the options picked (in their order) or what they
    /// wrote, which then shows as answered. None if nothing is picked or written, or it was
    /// answered already.
    pub fn answer(&mut self, part: usize) -> Option<String> {
        let Some(Part::Question(q)) = self.parts.get_mut(part) else { return None };
        if q.answered.is_some() {
            return None;
        }
        let other = q.other.text.trim();
        let answer = if !other.is_empty() {
            other.to_owned()
        } else {
            let mut picked = q.picked.clone();
            picked.sort_unstable();
            picked.iter().filter_map(|&i| q.options.get(i).cloned()).collect::<Vec<_>>().join(", ")
        };
        if answer.is_empty() {
            return None;
        }
        q.answered = Some(answer.clone());
        Some(answer)
    }
}

impl Question {
    /// Option `i` clicked: picked alone (single) or toggled (multi).
    pub fn pick(&mut self, i: usize) {
        if self.answered.is_some() || i >= self.options.len() {
            return;
        }
        if self.multi {
            match self.picked.iter().position(|&p| p == i) {
                Some(at) => _ = self.picked.remove(at),
                None => self.picked.push(i),
            }
        } else {
            self.picked = vec![i];
        }
    }
}

/// A unified diff's added and removed lines (file headers aside).
pub fn diff_counts(patch: &str) -> (usize, usize) {
    let mut counts = (0, 0);
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            counts.0 += 1;
        } else if line.starts_with('-') {
            counts.1 += 1;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(list: &[(Tool, &str, StepStatus)]) -> Agent {
        let steps = list.iter().enumerate().map(|(i, (tool, target, status))| Step { id: i as u32, tool: *tool, target: (*target).into(), output: String::new(), status: *status, open: false }).collect();
        Agent { steps, open: false }
    }

    #[test]
    fn a_run_of_steps_sums_up_in_one_line_by_kind_in_the_order_they_came() {
        use StepStatus::{Done, Failed, Running};
        let a = steps(&[(Tool::Read, "a.rs", Done), (Tool::Run, "ls", Done), (Tool::Read, "b.rs", Done)]);
        assert_eq!(a.summary(), "Read 2 files · Ran 1 command");
        let a = steps(&[(Tool::Search, "fn", Done), (Tool::Edit, "a.rs", Failed), (Tool::Other, "mem", Done), (Tool::Other, "x", Done)]);
        assert_eq!(a.summary(), "Searched 1 time · Edited 1 file · Used 2 tools · 1 failed");
        let a = steps(&[(Tool::Run, "cargo test", Done), (Tool::Run, "ls", Done)]);
        assert_eq!(a.summary(), "Ran 2 commands");
        // Going: the step running now.
        let a = steps(&[(Tool::Read, "a.rs", Done), (Tool::Run, "cargo test", Running)]);
        assert_eq!(a.summary(), "Running cargo test…");
        let a = steps(&[(Tool::Other, "", Running)]);
        assert_eq!(a.summary(), "Using a tool…");
    }

    fn at(ms: u64) -> Instant {
        // One base for the whole test binary: instants only compare within it.
        static BASE: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *BASE.get_or_init(Instant::now) + Duration::from_millis(ms)
    }

    fn run(events: Vec<Event>) -> Reply {
        let mut r = Reply::new(1, "q");
        for (i, e) in events.into_iter().enumerate() {
            r.apply(e, at(i as u64 * 100));
        }
        r
    }

    #[test]
    fn text_deltas_grow_one_markdown_part() {
        let r = run(vec![Event::Text("Hello ".into()), Event::Text("**world**".into()), Event::Done]);
        assert!(matches!(r.parts.as_slice(), [Part::Text(t)] if t == "Hello **world**"));
        assert_eq!(r.state, State::Done);
    }

    #[test]
    fn thinking_ends_with_how_long_it_took_once_the_answer_starts() {
        let r = run(vec![Event::Think("Let me ".into()), Event::Think("see".into()), Event::Text("Answer".into())]);
        let [Part::Thinking(t), Part::Text(_)] = r.parts.as_slice() else { panic!("thinking, then text") };
        assert_eq!((t.text.as_str(), t.took), ("Let me see", Some(Duration::from_millis(200))));
        assert!(!t.open, "collapsed by default");
    }

    #[test]
    fn steps_in_a_row_are_one_agent_run_and_end_with_the_reply() {
        let r = run(vec![
            Event::Step { id: 1, tool: Tool::Read, target: "a.rs".into() },
            Event::StepOutput { id: 1, text: "fn a()".into() },
            Event::StepEnd { id: 1, ok: true },
            Event::Step { id: 2, tool: Tool::Run, target: "cargo test".into() },
            Event::Text("done".into()),
            Event::Step { id: 3, tool: Tool::Run, target: "x".into() },
            Event::Done,
        ]);
        let [Part::Agent(a), Part::Text(_), Part::Agent(b)] = r.parts.as_slice() else { panic!("agent, text, agent") };
        assert_eq!(a.steps.iter().map(|s| (s.id, s.status)).collect::<Vec<_>>(), [(1, StepStatus::Done), (2, StepStatus::Done)]);
        assert_eq!(a.steps[0].output, "fn a()");
        assert_eq!(b.steps[0].status, StepStatus::Done);
    }

    #[test]
    fn a_failure_shows_an_error_and_fails_running_steps() {
        let r = run(vec![Event::Step { id: 1, tool: Tool::Run, target: "x".into() }, Event::Fail("overloaded".into()), Event::Text("late".into())]);
        let [Part::Agent(a), Part::Error(e)] = r.parts.as_slice() else { panic!("agent, error") };
        assert_eq!((a.steps[0].status, e.as_str(), r.state), (StepStatus::Failed, "overloaded", State::Failed));
    }

    #[test]
    fn stopping_keeps_what_came_and_ignores_the_rest() {
        let mut r = run(vec![Event::Think("hm".into())]);
        r.stop(at(500));
        r.apply(Event::Text("late".into()), at(600));
        assert_eq!(r.state, State::Stopped);
        assert!(matches!(r.parts.as_slice(), [Part::Thinking(t)] if t.took.is_some()));
    }

    #[test]
    fn a_long_diff_starts_collapsed_and_counts_its_lines() {
        let short = "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n same\n";
        let long: String = (0..20).map(|i| format!("+line {i}\n")).collect();
        let r = run(vec![Event::Diff { path: "x".into(), patch: short.into() }, Event::Diff { path: "y".into(), patch: long }]);
        let [Part::Diff(a), Part::Diff(b)] = r.parts.as_slice() else { panic!("two diffs") };
        assert_eq!((a.added, a.removed, a.open), (1, 1, true));
        assert_eq!((b.added, b.open), (20, false));
    }

    #[test]
    fn a_question_answers_with_its_picks_in_order_or_the_users_words_once() {
        let ask = |multi| Event::Ask { prompt: "Which?".into(), options: vec!["A".into(), "B".into(), "C".into()], multi };
        let mut r = run(vec![ask(true), ask(false), ask(false)]);
        fn q(r: &mut Reply, i: usize) -> &mut Question {
            match &mut r.parts[i] {
                Part::Question(q) => q,
                _ => panic!("question"),
            }
        }
        q(&mut r, 0).pick(2);
        q(&mut r, 0).pick(0);
        q(&mut r, 1).pick(0);
        q(&mut r, 1).pick(1);
        assert_eq!(r.answer(0).as_deref(), Some("A, C"));
        assert_eq!(r.answer(0), None, "answered once");
        assert_eq!(r.answer(1).as_deref(), Some("B"), "single: the last pick");
        assert_eq!(r.answer(2), None, "nothing picked");
        q(&mut r, 2).other = TextEdit::new("  my own  ");
        assert_eq!(r.answer(2).as_deref(), Some("my own"));
    }

    #[test]
    fn clicking_opens_and_closes_thinking_runs_steps_and_diffs() {
        let mut r = run(vec![
            Event::Think("a".into()),
            Event::Step { id: 1, tool: Tool::Read, target: "a".into() },
            Event::Diff { path: "x".into(), patch: "+a\n".into() },
        ]);
        let open = |r: &Reply| match r.parts.as_slice() {
            [Part::Thinking(t), Part::Agent(a), Part::Diff(d)] => (t.open, a.open, a.steps[0].open, d.open),
            _ => panic!("thinking, agent, diff"),
        };
        assert_eq!(open(&r), (false, true, false, true));
        r.toggle(0, None);
        r.toggle(1, None);
        r.toggle(1, Some(0));
        r.toggle(2, None);
        r.toggle(2, Some(0));
        r.toggle(9, None);
        assert_eq!(open(&r), (true, false, true, false));
        r.apply_diff(2);
        assert!(matches!(&r.parts[2], Part::Diff(d) if d.applied));
    }

    #[test]
    fn copy_takes_the_answer_and_diffs_not_thinking_or_steps() {
        let r = run(vec![
            Event::Think("secret plan".into()),
            Event::Step { id: 1, tool: Tool::Read, target: "a".into() },
            Event::Text("Here:".into()),
            Event::Diff { path: "x".into(), patch: "+a\n".into() },
        ]);
        assert_eq!(r.copy_text(), "Here:\n\n```diff\n+a\n```");
    }
}
