//! What a client sends the core. AG-UI starts runs with a [`RunAgentInput`] and has no way to
//! stop one, so commands are Ailfred's own envelope around it, tagged by `type`. Retrying a
//! reply, a new conversation and picking a model come with the server (T20).
//!
//! A run's `messages` are the complete history, as AG-UI requires, built as a snapshot builds
//! it ([`history`]): the same ids and shapes a `MESSAGES_SNAPSHOT` of those turns holds.
//! [`send`] adds the new user message `{run}-user`; [`answer`] adds none, its answer going in
//! `resume`. The core takes an answer as a new user message (the app's chat pushes the
//! answer's text as the next message), so a later snapshot shows it as the `user` message of
//! the answering run, `{run}-user`: that's how an answer command and a snapshot line up. Which
//! messages win when a client's history and the core's differ, a partial resume, and
//! `status: cancelled` are the server's to decide (T20).

use ailfred_core::turn::base64;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agui::{Content, ContentPart, Message, PROTOCOL_VERSION, PartSource, Resume, ResumeStatus, RunAgentInput};
use crate::snapshot::{Turn, history};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Command {
    /// The user sends something: a new message (a steer, if a run goes: the core decides), or
    /// the answers to the last run's questions in `resume`. Its `messages` are the complete
    /// history, as AG-UI requires, under the ids a snapshot of it uses (`send` and `answer`
    /// build it).
    Run(Box<RunAgentInput>),
    /// The user stops the run going.
    Stop { thread_id: String, run_id: String },
}

/// Run `run_id` of thread `thread_id`: the user says `text` with captures `pngs` (PNG bytes)
/// after the conversation `before`. Its messages are `before`'s history, then `{run_id}-user`.
pub fn send(thread_id: &str, run_id: &str, before: &[Turn], text: &str, pngs: &[&[u8]]) -> Command {
    let mut messages = history(before);
    messages.push(user_message(&format!("{run_id}-user"), text, pngs));
    run(thread_id, run_id, messages, None)
}

/// Run `run_id` of thread `thread_id`: the user answers question `interrupt_id`, asked in the
/// reply of `before`'s last turn. Its messages are `before`'s history; the answer is its
/// `resume` ([`answer_resume`]).
pub fn answer(thread_id: &str, run_id: &str, before: &[Turn], interrupt_id: &str, answer: &str) -> Command {
    run(thread_id, run_id, history(before), Some(vec![answer_resume(interrupt_id, answer)]))
}

fn run(thread_id: &str, run_id: &str, messages: Vec<Message>, resume: Option<Vec<Resume>>) -> Command {
    Command::Run(Box::new(RunAgentInput {
        thread_id: thread_id.into(),
        run_id: run_id.into(),
        protocol_version: Some(PROTOCOL_VERSION.into()),
        parent_run_id: None,
        state: None,
        messages,
        tools: None,
        context: None,
        forwarded_props: None,
        resume,
    }))
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
