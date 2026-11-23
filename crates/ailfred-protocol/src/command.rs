//! What a client sends the core. AG-UI starts runs with a [`RunAgentInput`] and has no way to
//! stop one, so commands are Ailfred's own envelope around it, tagged by `type`. Retrying a
//! reply, a new conversation and picking a model come with the server (T20).

use ailfred_core::turn::base64;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agui::{Content, ContentPart, Message, PartSource, Resume, ResumeStatus, RunAgentInput};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Command {
    /// The user sends something: a new message (a steer, if a run goes: the core decides), or
    /// the answers to the last run's questions in `resume`. Its `messages` are the whole
    /// conversation so far, as AG-UI requires, the replies included as assistant messages.
    Run(Box<RunAgentInput>),
    /// The user stops the run going.
    Stop { thread_id: String, run_id: String },
}

/// The answer to question `interrupt_id`, for a run's `resume`.
pub fn answer_resume(interrupt_id: &str, answer: &str) -> Resume {
    Resume { interrupt_id: interrupt_id.into(), status: ResumeStatus::Resolved, payload: Some(json!({ "answer": answer })), metadata: None }
}

/// A user message: its `text`, then `pngs` (screenshots) as inline images. Plain text when
/// there are none.
pub fn user_message(id: &str, text: &str, pngs: &[&[u8]]) -> Message {
    let content = if pngs.is_empty() {
        Content::Text(text.into())
    } else {
        let images = pngs.iter().map(|png| ContentPart::Image { source: PartSource::Data { value: base64(png), mime_type: "image/png".into() } });
        Content::Parts(std::iter::once(ContentPart::Text { text: text.into() }).chain(images).collect())
    };
    Message::User { id: id.into(), content, name: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_names_its_thread_and_run_and_a_run_flattens_its_input() {
        let stop = Command::Stop { thread_id: "t".into(), run_id: "r".into() };
        assert_eq!(serde_json::to_string(&stop).unwrap(), r#"{"type":"stop","threadId":"t","runId":"r"}"#);
        let run = Command::Run(Box::new(RunAgentInput {
            thread_id: "t".into(),
            run_id: "r".into(),
            protocol_version: None,
            parent_run_id: None,
            state: None,
            messages: vec![user_message("m", "hi", &[])],
            tools: None,
            context: None,
            forwarded_props: None,
            resume: Some(vec![answer_resume("r0-ask-1", "A")]),
        }));
        assert_eq!(
            serde_json::to_string(&run).unwrap(),
            r#"{"type":"run","threadId":"t","runId":"r","messages":[{"role":"user","id":"m","content":"hi"}],"resume":[{"interruptId":"r0-ask-1","status":"resolved","payload":{"answer":"A"}}]}"#
        );
    }
}
