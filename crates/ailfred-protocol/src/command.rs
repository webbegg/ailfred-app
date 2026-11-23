//! What a client sends the core. AG-UI starts runs with a [`RunAgentInput`] and has no way to
//! stop one, retry one or start a new conversation, so commands are Ailfred's own envelope
//! around it, tagged by `type`. Picking a model isn't a command yet. A newer client's command of
//! a type this version doesn't know doesn't read (the server answers 400).
//!
//! A run's `messages` are the complete history, as AG-UI requires, built as a snapshot builds
//! it ([`history`]): the same ids and shapes a `MESSAGES_SNAPSHOT` of those turns holds.
//! [`send`] adds the new user message `{run}-user`; [`answer`] adds none, its answer going in
//! `resume`. The core takes an answer as a new user message (the app's chat pushes the
//! answer's text as the next message), so a later snapshot shows it as the `user` message of
//! the answering run, `{run}-user`: that's how an answer command and a snapshot line up. The
//! server (the app's `server`, `docs/local-server.md`) keeps its own history: a run's earlier
//! messages aren't replayed, a partial resume answers the questions it names, and one with
//! `status: cancelled` ([`decline`]) closes its questions: nothing is sent to the model.

use ailfred_core::turn::base64;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agui::{Content, ContentPart, Message, PROTOCOL_VERSION, PartSource, Resume, ResumeStatus, RunAgentInput};
use crate::snapshot::{Turn, history, unbase64};

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
    /// The user starts a new conversation, as Cmd+N in the chat does; `thread_id` is the one
    /// current now. Answered with the new one's `threadId`, whose empty `MESSAGES_SNAPSHOT`
    /// every client gets.
    NewConversation { thread_id: String },
    /// The user retries a reply, as the chat's Retry does, as run `run_id` (new in the thread):
    /// `reply_run_id` names the reply by its run, as the live run (`RUN_STARTED`) or a snapshot
    /// (its user message `{run}-user`) gave it, matched whole.
    Retry { thread_id: String, run_id: String, reply_run_id: String },
}

/// A new conversation in place of `thread_id`'s.
pub fn new_conversation(thread_id: &str) -> Command {
    Command::NewConversation { thread_id: thread_id.into() }
}

/// Run `run_id` of thread `thread_id` retries the reply of run `reply_run_id`.
pub fn retry(thread_id: &str, run_id: &str, reply_run_id: &str) -> Command {
    Command::Retry { thread_id: thread_id.into(), run_id: run_id.into(), reply_run_id: reply_run_id.into() }
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

/// Run `run_id` of thread `thread_id`: the user declines question `interrupt_id`, asked in the
/// reply of `before`'s last turn ([`decline_resume`]). Nothing goes to the model: the question
/// closes.
pub fn decline(thread_id: &str, run_id: &str, before: &[Turn], interrupt_id: &str) -> Command {
    run(thread_id, run_id, history(before), Some(vec![decline_resume(interrupt_id)]))
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

/// Question `interrupt_id` declined, for a run's `resume`: `cancelled`, without a payload.
pub fn decline_resume(interrupt_id: &str) -> Resume {
    Resume { interrupt_id: interrupt_id.into(), status: ResumeStatus::Cancelled, payload: None, metadata: None }
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

/// Why a user message's content can't be taken.
#[derive(Debug, PartialEq, Eq)]
pub enum Unsaid {
    /// A part that isn't text or an inline PNG image (a URL, another type, not base64, or
    /// audio, video or a document).
    NotPng,
    /// Neither text nor an image.
    Empty,
}

/// What a user message's `content` says, as [`user_message`] wrote it: its text (its text
/// parts', joined) and its images as PNG bytes.
pub fn user_said(content: &Content) -> Result<(String, Vec<Vec<u8>>), Unsaid> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let (text, pngs) = match content {
        Content::Text(text) => (text.clone(), Vec::new()),
        Content::Parts(parts) => {
            let (mut text, mut pngs) = (String::new(), Vec::new());
            for part in parts {
                match part {
                    ContentPart::Text { text: t } => text.push_str(t),
                    ContentPart::Image { source: PartSource::Data { value, mime_type } } if mime_type == "image/png" => {
                        let png = unbase64(value).filter(|b| b.starts_with(SIGNATURE)).ok_or(Unsaid::NotPng)?;
                        pngs.push(png);
                    }
                    _ => return Err(Unsaid::NotPng),
                }
            }
            (text, pngs)
        }
    };
    if text.trim().is_empty() && pngs.is_empty() {
        return Err(Unsaid::Empty);
    }
    Ok((text, pngs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_message_says_its_text_and_pngs_back_and_refuses_an_image_not_inline_png() {
        let png: &[u8] = b"\x89PNG\r\n\x1a\nrest";
        let Message::User { content, .. } = user_message("m", "look", &[png]) else { unreachable!() };
        assert_eq!(user_said(&content), Ok(("look".into(), vec![png.to_vec()])));
        assert_eq!(user_said(&Content::Text("hi".into())), Ok(("hi".into(), Vec::new())));
        assert_eq!(user_said(&Content::Text("  ".into())), Err(Unsaid::Empty));
        let url = Content::Parts(vec![ContentPart::Image { source: PartSource::Url { value: "http://x/a.png".into(), mime_type: None } }]);
        assert_eq!(user_said(&url), Err(Unsaid::NotPng));
        let jpeg = Content::Parts(vec![ContentPart::Image { source: PartSource::Data { value: base64(b"jpeg"), mime_type: "image/jpeg".into() } }]);
        assert_eq!(user_said(&jpeg), Err(Unsaid::NotPng));
        let not_png = Content::Parts(vec![ContentPart::Image { source: PartSource::Data { value: base64(b"text"), mime_type: "image/png".into() } }]);
        assert_eq!(user_said(&not_png), Err(Unsaid::NotPng), "the bytes must be a PNG's");
    }

    #[test]
    fn a_stop_names_its_thread_and_run_and_a_run_flattens_its_input() {
        let stop = Command::Stop { thread_id: "t".into(), run_id: "r".into() };
        assert_eq!(serde_json::to_string(&stop).unwrap(), r#"{"type":"stop","threadId":"t","runId":"r"}"#);
        assert_eq!(serde_json::to_string(&new_conversation("t")).unwrap(), r#"{"type":"new_conversation","threadId":"t"}"#);
        assert_eq!(serde_json::to_string(&retry("t", "r2", "r")).unwrap(), r#"{"type":"retry","threadId":"t","runId":"r2","replyRunId":"r"}"#);
        let declined = serde_json::to_value(decline_resume("r-ask-1")).unwrap();
        assert_eq!(declined, json!({ "interruptId": "r-ask-1", "status": "cancelled" }));
        let newer = serde_json::from_str::<Command>(r#"{"type":"pick_model","threadId":"t"}"#).unwrap_err().to_string();
        assert!(newer.contains("pick_model"), "a newer client's type is named: {newer}");
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
