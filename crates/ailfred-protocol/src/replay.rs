//! The other way, what every client does: AG-UI events back into the core's reply [`Event`]s,
//! to apply to a [`Reply`](ailfred_core::reply::Reply) as they come.

use ailfred_core::reply::{Event, Tool};
use serde::Deserialize;
use serde_json::Value;

use crate::agui::{self, Content, ContentPart, Interrupt, RunOutcome};
use crate::encode::{ASK, DIFF, LOST, STEP_OUTPUT};

/// What a client does with a reply on one AG-UI event.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    /// When it happened (the event's timestamp, ms since the epoch).
    pub at: Option<u64>,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Apply this event (`Reply::apply`).
    Event(Event),
    /// The run was stopped (`Reply::stop`).
    Stop,
}

/// Why an event can't be replayed: it breaks the run's lifecycle (`RUN_STARTED` first, nothing
/// after `RUN_FINISHED` or `RUN_ERROR`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayError {
    /// An event came before the run's `RUN_STARTED`.
    NotStarted,
    /// An event came after the run ended.
    AfterEnd,
    /// A `RUN_ERROR`, with its message, came after the `RUN_FINISHED`: the run failed after
    /// all: the client marks it failed (a `Reply` ignores events once it's over).
    FailedAfterFinish(String),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::NotStarted => write!(f, "an event came before RUN_STARTED"),
            ReplayError::AfterEnd => write!(f, "an event came after the run ended"),
            ReplayError::FailedAfterFinish(message) => write!(f, "the run failed after it finished: {message}"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Turns one run's events into updates; it keeps where the run is, the tool calls and the
/// questions it has seen.
#[derive(Default)]
pub struct Replayer {
    run: Run,
    calls: Vec<Call>,
    /// The interrupt ids of the questions asked so far.
    asked: Vec<String>,
}

/// Where a run is, by the events seen.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Run {
    #[default]
    NotStarted,
    Going,
    Finished,
    Failed,
}

/// A tool call: a step, once its arguments are in.
struct Call {
    call_id: String,
    /// The step's id in the replayed reply: calls in the order they came, from 1.
    id: u32,
    name: String,
    args: String,
    stepped: bool,
    /// Its output so far.
    output: String,
}

/// A custom `ailfred:step_output` event's value.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepOutput {
    tool_call_id: String,
    delta: String,
}

/// A custom `ailfred:ask` event's value.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ask {
    interrupt_id: String,
    message: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    multi: bool,
}

impl Replayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The updates `event` makes; none for what a reply doesn't show, or doesn't know. An error
    /// if it comes before the run starts or after it ended.
    pub fn replay(&mut self, event: &agui::Event) -> Result<Vec<Update>, ReplayError> {
        self.run = match (self.run, event) {
            (Run::NotStarted, agui::Event::RunStarted { .. }) => Run::Going,
            (Run::NotStarted, _) => return Err(ReplayError::NotStarted),
            (Run::Finished, agui::Event::RunError { message, .. }) => return Err(ReplayError::FailedAfterFinish(message.clone())),
            (Run::Finished | Run::Failed, _) => return Err(ReplayError::AfterEnd),
            (Run::Going, agui::Event::RunFinished { .. }) => Run::Finished,
            (Run::Going, agui::Event::RunError { .. }) => Run::Failed,
            (Run::Going, _) => Run::Going,
        };
        let at = event.base().timestamp;
        Ok(self.kinds(event).into_iter().map(|kind| Update { at, kind }).collect())
    }

    fn kinds(&mut self, event: &agui::Event) -> Vec<Kind> {
        use agui::Event as A;
        let ev = Kind::Event;
        match event {
            A::RunStarted { .. } => vec![ev(Event::Taken)],
            A::ReasoningStart { .. } => vec![ev(Event::Think(String::new()))],
            // Any event but a `Think` ends the thinking open, at its time; `Taken` does nothing else.
            A::ReasoningEnd { .. } => vec![ev(Event::Taken)],
            A::ReasoningMessageContent { delta, .. } => vec![ev(Event::Think(delta.clone()))],
            A::TextMessageStart { .. } => vec![ev(Event::Text(String::new()))],
            A::TextMessageContent { delta, .. } => vec![ev(Event::Text(delta.clone()))],
            A::ToolCallStart { tool_call_id, tool_call_name, .. } => {
                let id = self.calls.len() as u32 + 1;
                self.calls.push(Call { call_id: tool_call_id.clone(), id, name: tool_call_name.clone(), args: String::new(), stepped: false, output: String::new() });
                Vec::new()
            }
            A::ToolCallArgs { tool_call_id, delta, .. } => {
                if let Some(call) = self.call(tool_call_id) {
                    call.args.push_str(delta);
                }
                Vec::new()
            }
            A::ToolCallEnd { tool_call_id, .. } => self.step(tool_call_id).into_iter().collect(),
            A::Custom { name, value, .. } if name == STEP_OUTPUT => {
                let Ok(StepOutput { tool_call_id, delta }) = StepOutput::deserialize(value) else { return Vec::new() };
                let mut out: Vec<Kind> = self.step(&tool_call_id).into_iter().collect();
                if let Some(call) = self.call(&tool_call_id) {
                    call.output.push_str(&delta);
                    out.push(ev(Event::StepOutput { id: call.id, text: delta }));
                }
                out
            }
            A::ToolCallResult { tool_call_id, content, base, .. } => {
                let mut out: Vec<Kind> = self.step(tool_call_id).into_iter().collect();
                let Some(call) = self.call(tool_call_id) else { return out };
                let content = text_of(content);
                // What it returned past what came streaming.
                if let Some(rest) = content.strip_prefix(call.output.as_str())
                    && !rest.is_empty()
                {
                    call.output.push_str(rest);
                    out.push(ev(Event::StepOutput { id: call.id, text: rest.to_owned() }));
                }
                let ok = base.metadata.as_ref().and_then(|m| m.get("ailfred")?.get("ok")?.as_bool()).unwrap_or(true);
                out.push(ev(Event::StepEnd { id: call.id, ok }));
                out
            }
            A::ActivitySnapshot { activity_type, content, .. } if activity_type == DIFF => {
                let field = |k: &str| content.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
                vec![ev(Event::Diff { path: field("path"), patch: field("patch") })]
            }
            A::Custom { name, value, .. } if name == ASK => {
                let Ok(Ask { interrupt_id, message, options, multi }) = Ask::deserialize(value) else { return Vec::new() };
                self.asked.push(interrupt_id);
                vec![ev(Event::Ask { prompt: message, options, multi })]
            }
            A::RunFinished { outcome, .. } => match outcome {
                Some(RunOutcome::Interrupt { interrupts }) => {
                    // Ours were asked where they came; another producer's only come here.
                    let unseen = interrupts.iter().filter(|i| !self.asked.contains(&i.id));
                    unseen.map(|i| ev(ask(i))).chain([ev(Event::Done)]).collect()
                }
                Some(RunOutcome::Cancelled) => vec![Kind::Stop],
                // Success, and an outcome this version doesn't know.
                _ => vec![ev(Event::Done)],
            },
            A::RunError { message, code, .. } if code.as_deref() == Some(LOST) => vec![ev(Event::Lost(message.clone()))],
            A::RunError { message, .. } => vec![ev(Event::Fail(message.clone()))],
            _ => Vec::new(),
        }
    }

    fn call(&mut self, call_id: &str) -> Option<&mut Call> {
        self.calls.iter_mut().find(|c| c.call_id == call_id)
    }

    /// The step tool call `call_id` is, the first time it's asked for.
    fn step(&mut self, call_id: &str) -> Option<Kind> {
        let call = self.call(call_id).filter(|c| !c.stepped)?;
        call.stepped = true;
        let (tool, target) = step_of(&call.name, &call.args);
        Some(Kind::Event(Event::Step { id: call.id, tool, target }))
    }
}

/// The tool and target of a call to `name` with arguments `args` (`{"target":…}`).
pub(crate) fn step_of(name: &str, args: &str) -> (Tool, String) {
    let args: Value = serde_json::from_str(args).unwrap_or_default();
    let target = args.get("target").and_then(Value::as_str).unwrap_or_default();
    let (tool, target) = match name {
        "read" => (Tool::Read, target),
        "search" => (Tool::Search, target),
        "run" => (Tool::Run, target),
        "edit" => (Tool::Edit, target),
        "other" => (Tool::Other, target),
        // Another producer's tool: its target is its name.
        name => (Tool::Other, if target.is_empty() { name } else { target }),
    };
    (tool, target.to_owned())
}

/// A tool result's text: its text parts', joined, when it comes in parts.
pub(crate) fn text_of(content: &Content) -> String {
    match content {
        Content::Text(text) => text.clone(),
        Content::Parts(parts) => parts.iter().filter_map(|p| if let ContentPart::Text { text } = p { Some(text.as_str()) } else { None }).collect(),
    }
}

/// The question an interrupt asks.
fn ask(interrupt: &Interrupt) -> Event {
    let ours = interrupt.metadata.as_ref().and_then(|m| m.get("ailfred"));
    let options = ours.and_then(|a| a.get("options")?.as_array().cloned()).unwrap_or_default();
    Event::Ask {
        prompt: interrupt.message.clone().unwrap_or_default(),
        options: options.iter().filter_map(|o| o.as_str().map(str::to_owned)).collect(),
        multi: ours.and_then(|a| a.get("multi")?.as_bool()).unwrap_or(false),
    }
}

/// One line of a JSONL stream as an event; `Ok(None)` for an event type this version doesn't
/// know (a newer core's), so a client skips it instead of breaking.
pub fn parse_line(line: &str) -> Result<Option<agui::Event>, serde_json::Error> {
    let value: Value = serde_json::from_str(line.trim())?;
    parse_value(&value)
}

/// The same, for an event already read as JSON.
pub fn parse_value(value: &Value) -> Result<Option<agui::Event>, serde_json::Error> {
    match agui::Event::deserialize(value) {
        Ok(event) => Ok(Some(event)),
        Err(_) if value.get("type").and_then(Value::as_str).is_some_and(|t| !agui::Event::TYPES.contains(&t)) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_event_type_is_skipped_but_a_broken_known_one_is_an_error() {
        assert_eq!(parse_line(r#"{"type":"STATE_DELTA","delta":[]}"#).unwrap(), None);
        assert!(parse_line(r#"{"type":"TEXT_MESSAGE_END"}"#).is_err(), "messageId missing");
        assert!(parse_line(r#"{"messageId":"m"}"#).is_err(), "no type");
        assert!(parse_line("not json").is_err());
        let e = parse_line(r#"{"type":"TEXT_MESSAGE_END","messageId":"m","rawEvent":{},"subagentRunId":"s"}"#).unwrap();
        assert!(matches!(e, Some(agui::Event::TextMessageEnd { .. })), "fields it doesn't know are ignored");
    }

    #[test]
    fn another_producers_tool_is_an_other_step_named_after_it_and_args_may_come_in_pieces() {
        let base = agui::Base::default;
        let mut r = Replayer::new();
        let mut kinds = Vec::new();
        for e in [
            agui::Event::RunStarted { thread_id: "t".into(), run_id: "r".into(), protocol_version: None, base: base() },
            agui::Event::ToolCallStart { tool_call_id: "c".into(), tool_call_name: "weather".into(), parent_message_id: None, base: base() },
            agui::Event::ToolCallArgs { tool_call_id: "c".into(), delta: "{\"city\":".into(), base: base() },
            agui::Event::ToolCallArgs { tool_call_id: "c".into(), delta: "\"Paris\"}".into(), base: base() },
            agui::Event::ToolCallResult { message_id: "m".into(), tool_call_id: "c".into(), content: Content::Text("sunny".into()), role: None, base: base() },
        ] {
            kinds.extend(r.replay(&e).unwrap().into_iter().map(|u| u.kind));
        }
        assert_eq!(
            kinds,
            [
                Kind::Event(Event::Taken),
                Kind::Event(Event::Step { id: 1, tool: Tool::Other, target: "weather".into() }),
                Kind::Event(Event::StepOutput { id: 1, text: "sunny".into() }),
                Kind::Event(Event::StepEnd { id: 1, ok: true }),
            ]
        );
    }

    /// What `lines` replay to, or the first error.
    fn replay_lines(lines: &[&str]) -> Result<Vec<Kind>, ReplayError> {
        let mut r = Replayer::new();
        let mut kinds = Vec::new();
        for line in lines {
            let event = parse_line(line).unwrap_or_else(|e| panic!("{e}: {line}")).expect("a known event");
            kinds.extend(r.replay(&event)?.into_iter().map(|u| u.kind));
        }
        Ok(kinds)
    }

    const STARTED: &str = r#"{"type":"RUN_STARTED","threadId":"t","runId":"r"}"#;
    const FINISHED: &str = r#"{"type":"RUN_FINISHED","threadId":"t","runId":"r"}"#;
    const ERROR: &str = r#"{"type":"RUN_ERROR","message":"overloaded"}"#;
    const TEXT: &str = r#"{"type":"TEXT_MESSAGE_START","messageId":"m"}"#;

    #[test]
    fn an_event_before_the_run_starts_is_an_error() {
        assert_eq!(replay_lines(&[TEXT]), Err(ReplayError::NotStarted));
        assert_eq!(replay_lines(&[FINISHED]), Err(ReplayError::NotStarted));
    }

    #[test]
    fn an_event_after_the_run_ended_is_an_error() {
        assert_eq!(replay_lines(&[STARTED, FINISHED, TEXT]), Err(ReplayError::AfterEnd));
        assert_eq!(replay_lines(&[STARTED, FINISHED, STARTED]), Err(ReplayError::AfterEnd));
        assert_eq!(replay_lines(&[STARTED, ERROR, TEXT]), Err(ReplayError::AfterEnd));
        assert_eq!(replay_lines(&[STARTED, ERROR, ERROR]), Err(ReplayError::AfterEnd));
    }

    #[test]
    fn a_run_error_after_it_finished_says_the_run_failed_after_all() {
        assert_eq!(replay_lines(&[STARTED, FINISHED, ERROR]), Err(ReplayError::FailedAfterFinish("overloaded".into())));
    }

    #[test]
    fn a_tool_result_in_parts_and_an_outcome_this_version_doesnt_know_still_replay() {
        let kinds = replay_lines(&[
            STARTED,
            r#"{"type":"TOOL_CALL_START","toolCallId":"c","toolCallName":"read"}"#,
            r#"{"type":"TOOL_CALL_ARGS","toolCallId":"c","delta":"{\"target\":\"a.png\"}"}"#,
            r#"{"type":"TOOL_CALL_RESULT","messageId":"m","toolCallId":"c","content":[{"type":"text","text":"one "},{"type":"image","source":{"type":"url","value":"https://x/a.png"}},{"type":"text","text":"two"}]}"#,
            r#"{"type":"RUN_FINISHED","threadId":"t","runId":"r","outcome":{"type":"suspended","until":"later"}}"#,
        ]);
        assert_eq!(
            kinds.unwrap(),
            [
                Kind::Event(Event::Taken),
                Kind::Event(Event::Step { id: 1, tool: Tool::Read, target: "a.png".into() }),
                Kind::Event(Event::StepOutput { id: 1, text: "one two".into() }),
                Kind::Event(Event::StepEnd { id: 1, ok: true }),
                Kind::Event(Event::Done),
            ]
        );
    }

    #[test]
    fn a_question_another_producer_asks_only_at_the_end_still_replays_once() {
        let interrupt = r#"{"id":"i","reason":"input_required","message":"Which?"}"#;
        let finished = format!(r#"{{"type":"RUN_FINISHED","threadId":"t","runId":"r","outcome":{{"type":"interrupt","interrupts":[{interrupt}]}}}}"#);
        let ask = Kind::Event(Event::Ask { prompt: "Which?".into(), options: Vec::new(), multi: false });
        assert_eq!(replay_lines(&[STARTED, &finished]).unwrap(), [Kind::Event(Event::Taken), ask.clone(), Kind::Event(Event::Done)]);
        let ours = r#"{"type":"CUSTOM","name":"ailfred:ask","value":{"interruptId":"i","message":"Which?","multi":false,"options":[]}}"#;
        assert_eq!(replay_lines(&[STARTED, ours, &finished]).unwrap(), [Kind::Event(Event::Taken), ask, Kind::Event(Event::Done)]);
    }
}
