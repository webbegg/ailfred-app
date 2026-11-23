//! The AG-UI 1.0 wire types Ailfred sends and reads: the events of a run, the messages and the
//! request that starts one. Only the subset the core emits or a client needs; field names are
//! the official schema's (`spec/ag-ui-1.0.schema.json`), optional fields are left out when
//! absent, never `null`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The protocol version a run declares in its `RUN_STARTED`.
pub const PROTOCOL_VERSION: &str = "1.0";

/// What every event carries besides its own fields. (`rawEvent` is never sent, and ignored
/// when read.)
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Base {
    /// When it happened, in milliseconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    /// Extra information; Ailfred's own sits under the key `ailfred`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

impl Base {
    /// A base stamped at `ms`, with no metadata.
    pub fn at(ms: u64) -> Self {
        Base { timestamp: Some(ms), metadata: None }
    }
}

/// One event of a run, tagged by `type`. More may come in a later version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", rename_all_fields = "camelCase")]
pub enum Event {
    /// A run starts.
    RunStarted {
        thread_id: String,
        run_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        protocol_version: Option<String>,
        /// The run that started this one as a run of its own (Ailfred's runs have none).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_run_id: Option<String>,
        /// What the run was asked: the server sends the user message a run answers here, so a
        /// client that didn't send it sees it too.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Box<RunAgentInput>>,
        #[serde(flatten)]
        base: Base,
    },
    /// A run ends without failing: done, waiting for answers, or stopped.
    RunFinished {
        thread_id: String,
        run_id: String,
        #[serde(skip_serializing_if = "absent")]
        result: Option<Value>,
        /// Absent means success.
        #[serde(skip_serializing_if = "Option::is_none")]
        outcome: Option<RunOutcome>,
        #[serde(flatten)]
        base: Base,
    },
    /// A run ends failed.
    RunError {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(flatten)]
        base: Base,
    },
    /// A text message opens.
    TextMessageStart {
        message_id: String,
        /// Absent means assistant.
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<TextRole>,
        #[serde(flatten)]
        base: Base,
    },
    /// More of a text message.
    TextMessageContent {
        message_id: String,
        delta: String,
        #[serde(flatten)]
        base: Base,
    },
    /// A text message closes.
    TextMessageEnd {
        message_id: String,
        #[serde(flatten)]
        base: Base,
    },
    /// A span of reasoning opens.
    ReasoningStart {
        message_id: String,
        #[serde(flatten)]
        base: Base,
    },
    /// A reasoning message opens within the span.
    ReasoningMessageStart {
        message_id: String,
        role: ReasoningRole,
        #[serde(flatten)]
        base: Base,
    },
    /// More of a reasoning message.
    ReasoningMessageContent {
        message_id: String,
        delta: String,
        #[serde(flatten)]
        base: Base,
    },
    /// A reasoning message closes.
    ReasoningMessageEnd {
        message_id: String,
        #[serde(flatten)]
        base: Base,
    },
    /// A span of reasoning closes.
    ReasoningEnd {
        message_id: String,
        #[serde(flatten)]
        base: Base,
    },
    /// A tool call opens.
    ToolCallStart {
        tool_call_id: String,
        tool_call_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_message_id: Option<String>,
        #[serde(flatten)]
        base: Base,
    },
    /// More of a tool call's arguments (their pieces make a JSON document).
    ToolCallArgs {
        tool_call_id: String,
        delta: String,
        #[serde(flatten)]
        base: Base,
    },
    /// A tool call's arguments are complete.
    ToolCallEnd {
        tool_call_id: String,
        #[serde(flatten)]
        base: Base,
    },
    /// What a tool returned, as a new tool message.
    ToolCallResult {
        message_id: String,
        tool_call_id: String,
        /// Ailfred sends the step's output as plain text; other producers may send parts.
        content: Content,
        #[serde(skip_serializing_if = "Option::is_none")]
        role: Option<ToolRole>,
        #[serde(flatten)]
        base: Base,
    },
    /// Progress that isn't conversation text, shown as its own widget (Ailfred's diffs).
    ActivitySnapshot {
        message_id: String,
        activity_type: String,
        content: Map<String, Value>,
        /// Absent means it replaces what the activity showed.
        #[serde(skip_serializing_if = "Option::is_none")]
        replace: Option<bool>,
        #[serde(flatten)]
        base: Base,
    },
    /// The whole conversation, in order.
    MessagesSnapshot {
        messages: Vec<Message>,
        #[serde(flatten)]
        base: Base,
    },
    /// An application's own event (Ailfred's: `ailfred:…`).
    Custom {
        name: String,
        value: Value,
        #[serde(flatten)]
        base: Base,
    },
}

impl Event {
    /// The `type`s these events go by: a line of another type comes from a newer producer.
    pub const TYPES: &[&str] = &[
        "RUN_STARTED",
        "RUN_FINISHED",
        "RUN_ERROR",
        "TEXT_MESSAGE_START",
        "TEXT_MESSAGE_CONTENT",
        "TEXT_MESSAGE_END",
        "REASONING_START",
        "REASONING_MESSAGE_START",
        "REASONING_MESSAGE_CONTENT",
        "REASONING_MESSAGE_END",
        "REASONING_END",
        "TOOL_CALL_START",
        "TOOL_CALL_ARGS",
        "TOOL_CALL_END",
        "TOOL_CALL_RESULT",
        "ACTIVITY_SNAPSHOT",
        "MESSAGES_SNAPSHOT",
        "CUSTOM",
    ];

    pub fn base(&self) -> &Base {
        match self {
            Event::RunStarted { base, .. }
            | Event::RunFinished { base, .. }
            | Event::RunError { base, .. }
            | Event::TextMessageStart { base, .. }
            | Event::TextMessageContent { base, .. }
            | Event::TextMessageEnd { base, .. }
            | Event::ReasoningStart { base, .. }
            | Event::ReasoningMessageStart { base, .. }
            | Event::ReasoningMessageContent { base, .. }
            | Event::ReasoningMessageEnd { base, .. }
            | Event::ReasoningEnd { base, .. }
            | Event::ToolCallStart { base, .. }
            | Event::ToolCallArgs { base, .. }
            | Event::ToolCallEnd { base, .. }
            | Event::ToolCallResult { base, .. }
            | Event::ActivitySnapshot { base, .. }
            | Event::MessagesSnapshot { base, .. }
            | Event::Custom { base, .. } => base,
        }
    }
}

/// Who a streamed text message is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TextRole {
    Developer,
    System,
    Assistant,
    User,
}

/// A reasoning message's role: always `reasoning`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningRole {
    Reasoning,
}

/// A tool result's role: always `tool`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ToolRole {
    Tool,
}

/// Why a run ended, tagged by `type`. More may come in a later version.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum RunOutcome {
    Success,
    /// It waits for answers: the next run resumes it.
    Interrupt { interrupts: Vec<Interrupt> },
    /// Whoever ran it stopped it.
    Cancelled,
    /// An outcome of a type this version doesn't know (a newer producer's); never sent.
    #[serde(other)]
    #[schemars(skip)]
    Unknown,
}

/// Something a run needs from outside before it goes on (for Ailfred, an answer to a question).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Interrupt {
    /// What a [`Resume`] answers it by.
    pub id: String,
    /// Ailfred's: `input_required`.
    pub reason: String,
    /// The prompt, for whoever answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// A JSON Schema of the answer it expects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

/// A message of the conversation, tagged by `role`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "role", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum Message {
    Developer {
        id: String,
        content: String,
    },
    System {
        id: String,
        content: String,
    },
    Assistant {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    User {
        id: String,
        content: Content,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Tool {
        id: String,
        content: Content,
        tool_call_id: String,
        /// Why the tool failed, beside what it returned.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Activity {
        id: String,
        activity_type: String,
        content: Map<String, Value>,
    },
    Reasoning {
        id: String,
        content: String,
    },
}

/// A message's body: plain text, or parts (text and media) in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

/// One part of a message's body, tagged by `type`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentPart {
    Text { text: String },
    Image { source: PartSource },
    Audio { source: PartSource },
    Video { source: PartSource },
    Document { source: PartSource },
}

/// Where a media part's bytes come from, tagged by `type`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum PartSource {
    /// The bytes themselves, base64.
    Data { value: String, mime_type: String },
    Url {
        value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
    /// A handle a model provider issued.
    File {
        value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
    },
}

/// A call an assistant message made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: CallKind,
    pub function: FunctionCall,
}

/// A tool call's kind: always `function`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CallKind {
    Function,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FunctionCall {
    pub name: String,
    /// As a JSON string.
    pub arguments: String,
}

/// What starts a run: the conversation, and the answers to the last run's questions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunAgentInput {
    pub thread_id: String,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    #[serde(skip_serializing_if = "absent")]
    pub state: Option<Value>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<Context>>,
    #[serde(skip_serializing_if = "absent")]
    pub forwarded_props: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<Vec<Resume>>,
}

/// A tool the client offers the agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// A JSON Schema of its arguments.
    #[serde(skip_serializing_if = "absent")]
    pub parameters: Option<Value>,
}

/// A named piece of what's around the conversation, for the agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Context {
    pub description: String,
    pub value: String,
}

/// An answer to one [`Interrupt`] (the schema's `ResumeEntry`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Resume {
    pub interrupt_id: String,
    pub status: ResumeStatus,
    /// The answer itself (Ailfred's: `{"answer": "…"}`).
    #[serde(skip_serializing_if = "absent")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ResumeStatus {
    Resolved,
    Cancelled,
}

/// Whether an optional JSON value is left out: when there's none, and when it's `null` (which
/// the official schema refuses where it says "any JSON value").
fn absent(value: &Option<Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

/// `event` framed for Server-Sent Events: `data: <json>` and a blank line.
pub fn sse(event: &Event) -> String {
    format!("data: {}\n\n", to_line(event))
}

/// `event` as one line of compact JSON (no newline), as in the fixtures.
pub fn to_line(event: &Event) -> String {
    serde_json::to_string(event).expect("an event always serializes: its maps have string keys")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_is_tagged_by_type_with_camel_case_fields_and_no_nulls() {
        let e = Event::TextMessageStart { message_id: "m".into(), role: Some(TextRole::Assistant), base: Base::at(5) };
        assert_eq!(to_line(&e), r#"{"type":"TEXT_MESSAGE_START","messageId":"m","role":"assistant","timestamp":5}"#);
        let e = Event::TextMessageEnd { message_id: "m".into(), base: Base::default() };
        assert_eq!(to_line(&e), r#"{"type":"TEXT_MESSAGE_END","messageId":"m"}"#);
    }

    #[test]
    fn a_json_null_in_an_optional_field_is_left_out_like_none() {
        let e = Event::RunFinished { thread_id: "t".into(), run_id: "r".into(), result: Some(Value::Null), outcome: None, base: Base::default() };
        assert_eq!(to_line(&e), r#"{"type":"RUN_FINISHED","threadId":"t","runId":"r"}"#);
        let r = Resume { interrupt_id: "i".into(), status: ResumeStatus::Resolved, payload: Some(Value::Null), metadata: None };
        assert_eq!(serde_json::to_string(&r).unwrap(), r#"{"interruptId":"i","status":"resolved"}"#);
        let input = RunAgentInput {
            thread_id: "t".into(),
            run_id: "r".into(),
            protocol_version: None,
            parent_run_id: None,
            state: Some(Value::Null),
            messages: Vec::new(),
            tools: Some(vec![Tool { name: "n".into(), description: "d".into(), parameters: Some(Value::Null) }]),
            context: None,
            forwarded_props: Some(Value::Null),
            resume: None,
        };
        assert_eq!(serde_json::to_string(&input).unwrap(), r#"{"threadId":"t","runId":"r","messages":[],"tools":[{"name":"n","description":"d"}]}"#);
    }

    #[test]
    fn sse_frames_one_event_as_a_data_line_and_a_blank_one() {
        let e = Event::RunError { message: "x".into(), code: None, base: Base::at(1) };
        assert_eq!(sse(&e), "data: {\"type\":\"RUN_ERROR\",\"message\":\"x\",\"timestamp\":1}\n\n");
    }
}
