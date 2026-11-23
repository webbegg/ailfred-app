//! The core's reply, as AG-UI events: one [`Encoder`] per run turns the [`Event`]s an engine
//! streams (and the user's Stop) into what a client receives.

use ailfred_core::reply::{Event, Tool};
use serde_json::{Map, Value, json};

use crate::agui::{self, Base, Content, Interrupt, PROTOCOL_VERSION, ReasoningRole, RunOutcome, TextRole, ToolRole};

/// What a custom event carrying more of a step's output is called.
pub const STEP_OUTPUT: &str = "ailfred:step_output";
/// The activity type of a diff.
pub const DIFF: &str = "ailfred:diff";
/// The error code of a model process that died mid-reply.
pub const LOST: &str = "ailfred:lost";
/// What a custom event carrying a question, where it was asked, is called.
pub const ASK: &str = "ailfred:ask";

/// One run's events, in order. Ids derive from the run's: see the crate docs.
pub struct Encoder {
    thread: String,
    run: String,
    started: bool,
    over: bool,
    /// The text message open, and how many there were.
    text: Option<String>,
    texts: u32,
    /// The reasoning open (its span's id and its message's), and how many there were.
    thinking: Option<(String, String)>,
    thoughts: u32,
    steps: Vec<Step>,
    diffs: u32,
    /// The questions asked, sent again as the interrupts the run ends with.
    asks: Vec<Interrupt>,
}

/// A step of the run: its output so far is its result's content.
struct Step {
    id: u32,
    output: String,
    running: bool,
}

impl Encoder {
    pub fn new(thread_id: impl Into<String>, run_id: impl Into<String>) -> Self {
        Encoder { thread: thread_id.into(), run: run_id.into(), started: false, over: false, text: None, texts: 0, thinking: None, thoughts: 0, steps: Vec::new(), diffs: 0, asks: Vec::new() }
    }

    /// The events for `event`, which came at `at` (ms since the epoch); none once the run is over.
    pub fn encode(&mut self, event: &Event, at: u64) -> Vec<agui::Event> {
        let mut out = Vec::new();
        if self.over {
            return out;
        }
        self.start(at, &mut out);
        if !matches!(event, Event::Think(_)) {
            self.end_thinking(at, &mut out);
        }
        if !matches!(event, Event::Text(_)) {
            self.end_text(at, &mut out);
        }
        match event {
            Event::Think(delta) => {
                let id = match &self.thinking {
                    Some((_, id)) => id.clone(),
                    None => {
                        self.thoughts += 1;
                        let span = format!("{}-reasoning-{}", self.run, self.thoughts);
                        let id = format!("{}-thinking-{}", self.run, self.thoughts);
                        out.push(agui::Event::ReasoningStart { message_id: span.clone(), base: Base::at(at) });
                        out.push(agui::Event::ReasoningMessageStart { message_id: id.clone(), role: ReasoningRole::Reasoning, base: Base::at(at) });
                        self.thinking = Some((span, id.clone()));
                        id
                    }
                };
                if !delta.is_empty() {
                    out.push(agui::Event::ReasoningMessageContent { message_id: id, delta: delta.clone(), base: Base::at(at) });
                }
            }
            Event::Text(delta) => {
                let id = match &self.text {
                    Some(id) => id.clone(),
                    None => {
                        self.texts += 1;
                        let id = format!("{}-text-{}", self.run, self.texts);
                        out.push(agui::Event::TextMessageStart { message_id: id.clone(), role: Some(TextRole::Assistant), base: Base::at(at) });
                        self.text = Some(id.clone());
                        id
                    }
                };
                if !delta.is_empty() {
                    out.push(agui::Event::TextMessageContent { message_id: id, delta: delta.clone(), base: Base::at(at) });
                }
            }
            Event::Step { id, tool, target } => {
                let call = self.call_id(*id);
                out.push(agui::Event::ToolCallStart { tool_call_id: call.clone(), tool_call_name: tool_name(*tool).into(), parent_message_id: None, base: Base::at(at) });
                out.push(agui::Event::ToolCallArgs { tool_call_id: call.clone(), delta: json!({ "target": target }).to_string(), base: Base::at(at) });
                out.push(agui::Event::ToolCallEnd { tool_call_id: call, base: Base::at(at) });
                self.steps.push(Step { id: *id, output: String::new(), running: true });
            }
            Event::StepOutput { id, text } => {
                let call = self.call_id(*id);
                if let Some(step) = self.steps.iter_mut().find(|s| s.id == *id)
                    && !text.is_empty()
                {
                    step.output.push_str(text);
                    // Keys in order, so the line reads the same whatever map serde_json keeps.
                    let value = json!({ "delta": text, "toolCallId": call });
                    out.push(agui::Event::Custom { name: STEP_OUTPUT.into(), value, base: Base::at(at) });
                }
            }
            Event::StepEnd { id, ok } => {
                if let Some(i) = self.steps.iter().position(|s| s.id == *id && s.running) {
                    self.end_step(i, *ok, at, &mut out);
                }
            }
            Event::Diff { path, patch } => {
                self.diffs += 1;
                let content = object(json!({ "patch": patch, "path": path }));
                out.push(agui::Event::ActivitySnapshot { message_id: format!("{}-diff-{}", self.run, self.diffs), activity_type: DIFF.into(), content, replace: None, base: Base::at(at) });
            }
            Event::Ask { prompt, options, multi } => {
                let id = format!("{}-ask-{}", self.run, self.asks.len() + 1);
                let response_schema = object(json!({ "properties": { "answer": { "type": "string" } }, "required": ["answer"], "type": "object" }));
                let metadata = object(json!({ "ailfred": { "multi": multi, "options": options } }));
                // Keys in order, so the line reads the same whatever map serde_json keeps.
                let value = json!({ "interruptId": &id, "message": prompt, "multi": multi, "options": options });
                out.push(agui::Event::Custom { name: ASK.into(), value, base: Base::at(at) });
                self.asks.push(Interrupt { id, reason: "input_required".into(), message: Some(prompt.clone()), tool_call_id: None, response_schema: Some(response_schema), expires_at: None, metadata: Some(metadata) });
            }
            Event::Fail(message) => {
                self.end_steps(false, at, &mut out);
                out.push(agui::Event::RunError { message: message.clone(), code: None, base: Base::at(at) });
                self.over = true;
            }
            Event::Lost(why) => {
                self.end_steps(false, at, &mut out);
                out.push(agui::Event::RunError { message: why.clone(), code: Some(LOST.into()), base: Base::at(at) });
                self.over = true;
            }
            Event::Done => {
                self.end_steps(true, at, &mut out);
                let outcome = if self.asks.is_empty() { RunOutcome::Success } else { RunOutcome::Interrupt { interrupts: std::mem::take(&mut self.asks) } };
                self.finish(outcome, at, &mut out);
            }
            Event::Taken => {}
        }
        out
    }

    /// The user stopped the run at `at`: running steps fail, and it ends cancelled.
    pub fn stop(&mut self, at: u64) -> Vec<agui::Event> {
        let mut out = Vec::new();
        if self.over {
            return out;
        }
        self.start(at, &mut out);
        self.end_thinking(at, &mut out);
        self.end_text(at, &mut out);
        self.end_steps(false, at, &mut out);
        self.finish(RunOutcome::Cancelled, at, &mut out);
        out
    }

    fn call_id(&self, step: u32) -> String {
        format!("{}-step-{step}", self.run)
    }

    fn start(&mut self, at: u64, out: &mut Vec<agui::Event>) {
        if !self.started {
            self.started = true;
            out.push(agui::Event::RunStarted { thread_id: self.thread.clone(), run_id: self.run.clone(), protocol_version: Some(PROTOCOL_VERSION.into()), base: Base::at(at) });
        }
    }

    fn finish(&mut self, outcome: RunOutcome, at: u64, out: &mut Vec<agui::Event>) {
        out.push(agui::Event::RunFinished { thread_id: self.thread.clone(), run_id: self.run.clone(), result: None, outcome: Some(outcome), base: Base::at(at) });
        self.over = true;
    }

    fn end_thinking(&mut self, at: u64, out: &mut Vec<agui::Event>) {
        if let Some((span, id)) = self.thinking.take() {
            out.push(agui::Event::ReasoningMessageEnd { message_id: id, base: Base::at(at) });
            out.push(agui::Event::ReasoningEnd { message_id: span, base: Base::at(at) });
        }
    }

    fn end_text(&mut self, at: u64, out: &mut Vec<agui::Event>) {
        if let Some(id) = self.text.take() {
            out.push(agui::Event::TextMessageEnd { message_id: id, base: Base::at(at) });
        }
    }

    fn end_steps(&mut self, ok: bool, at: u64, out: &mut Vec<agui::Event>) {
        for i in 0..self.steps.len() {
            if self.steps[i].running {
                self.end_step(i, ok, at, out);
            }
        }
    }

    fn end_step(&mut self, i: usize, ok: bool, at: u64, out: &mut Vec<agui::Event>) {
        let call = self.call_id(self.steps[i].id);
        let step = &mut self.steps[i];
        step.running = false;
        let metadata = object(json!({ "ailfred": { "ok": ok } }));
        let base = Base { timestamp: Some(at), metadata: Some(metadata) };
        out.push(agui::Event::ToolCallResult { message_id: format!("{call}-result"), tool_call_id: call, content: Content::Text(step.output.clone()), role: Some(ToolRole::Tool), base });
    }
}

/// A step's tool as a tool call's name.
pub fn tool_name(tool: Tool) -> &'static str {
    match tool {
        Tool::Read => "read",
        Tool::Search => "search",
        Tool::Run => "run",
        Tool::Edit => "edit",
        Tool::Other => "other",
    }
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => unreachable!("an object literal"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agui::to_line;

    fn lines(events: Vec<agui::Event>) -> Vec<String> {
        events.iter().map(to_line).collect()
    }

    #[test]
    fn the_first_event_starts_the_run_and_once_it_ends_nothing_follows() {
        let mut e = Encoder::new("t", "r");
        assert_eq!(
            lines(e.encode(&Event::Taken, 1)),
            [r#"{"type":"RUN_STARTED","threadId":"t","runId":"r","protocolVersion":"1.0","timestamp":1}"#]
        );
        assert_eq!(lines(e.encode(&Event::Fail("x".into()), 2)), [r#"{"type":"RUN_ERROR","message":"x","timestamp":2}"#]);
        assert!(e.encode(&Event::Text("late".into()), 3).is_empty());
        assert!(e.stop(4).is_empty());
    }

    #[test]
    fn a_stop_before_anything_came_still_starts_and_cancels_the_run() {
        let mut e = Encoder::new("t", "r");
        let out = lines(e.stop(1));
        assert_eq!(out.len(), 2);
        assert!(out[1].contains(r#""outcome":{"type":"cancelled"}"#));
    }

    #[test]
    fn a_new_text_after_a_step_is_a_new_message_and_an_unknown_step_is_ignored() {
        let mut e = Encoder::new("t", "r");
        e.encode(&Event::Text("a".into()), 0);
        e.encode(&Event::Step { id: 4, tool: Tool::Edit, target: "x".into() }, 0);
        assert!(e.encode(&Event::StepOutput { id: 9, text: "?".into() }, 0).is_empty());
        assert!(e.encode(&Event::StepEnd { id: 9, ok: true }, 0).is_empty());
        let next = lines(e.encode(&Event::Text("b".into()), 1));
        assert_eq!(next[0], r#"{"type":"TEXT_MESSAGE_START","messageId":"r-text-2","role":"assistant","timestamp":1}"#);
    }

    #[test]
    fn a_reasoning_span_and_the_message_in_it_have_ids_of_their_own() {
        let mut e = Encoder::new("t", "r");
        let mut out = e.encode(&Event::Think("a".into()), 0);
        out.extend(e.encode(&Event::Done, 1));
        let ids: Vec<String> = out[1..6].iter().map(|ev| serde_json::to_value(ev).unwrap()).map(|v| format!("{} {}", v["type"].as_str().unwrap(), v["messageId"].as_str().unwrap())).collect();
        assert_eq!(
            ids,
            [
                "REASONING_START r-reasoning-1",
                "REASONING_MESSAGE_START r-thinking-1",
                "REASONING_MESSAGE_CONTENT r-thinking-1",
                "REASONING_MESSAGE_END r-thinking-1",
                "REASONING_END r-reasoning-1",
            ]
        );
    }

    #[test]
    fn a_question_goes_out_where_it_was_asked_and_again_in_the_run_finished() {
        let mut e = Encoder::new("t", "r");
        let ask = Event::Ask { prompt: "Which?".into(), options: vec!["A".into()], multi: true };
        let out = lines(e.encode(&ask, 1));
        assert_eq!(out[1], r#"{"type":"CUSTOM","name":"ailfred:ask","value":{"interruptId":"r-ask-1","message":"Which?","multi":true,"options":["A"]},"timestamp":1}"#);
        let done = lines(e.encode(&Event::Done, 2));
        assert!(done[0].contains(r#""interrupts":[{"id":"r-ask-1","#), "{}", done[0]);
    }
}
