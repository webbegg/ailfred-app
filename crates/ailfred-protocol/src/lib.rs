//! Ailfred's wire protocol: how the core sends a conversation to its clients (a web view, a Swift
//! app) and what they send back, over localhost. The conversation travels as AG-UI 1.0 events
//! ([`agui`], <https://docs.ag-ui.com>); the clients' side is a [`Command`](command::Command).
//!
//! ## The mapping
//!
//! [`Encoder`](encode::Encoder) turns one run's core [`Event`](ailfred_core::reply::Event)s into
//! AG-UI events, each stamped with when it came; [`Replayer`](replay::Replayer) turns them back,
//! so a client builds the same [`Reply`](ailfred_core::reply::Reply) the core has.
//!
//! | core | AG-UI |
//! |---|---|
//! | (the run's first event, whatever it is) | `RUN_STARTED` `protocolVersion: "1.0"` before it |
//! | `Taken` | nothing more |
//! | `Think` | `REASONING_START` + `REASONING_MESSAGE_START` once, then `REASONING_MESSAGE_CONTENT` (non-empty pieces); any other event first closes it: `REASONING_MESSAGE_END` + `REASONING_END` (replayed as `Taken`, which ends the thinking at that time) |
//! | `Text` | `TEXT_MESSAGE_START` (assistant) once, then `TEXT_MESSAGE_CONTENT` (non-empty pieces); any other event first closes it: `TEXT_MESSAGE_END` |
//! | `Step` | `TOOL_CALL_START` (`read`, `search`, `run`, `edit`, `other`) + `TOOL_CALL_ARGS` `{"target":…}` + `TOOL_CALL_END` |
//! | `StepOutput` | `CUSTOM` `ailfred:step_output` `{"delta","toolCallId"}` |
//! | `StepEnd` | `TOOL_CALL_RESULT`: the whole output as a string (read back from text parts too), `metadata.ailfred.ok`; steps still running when the run ends get theirs then (ok on `Done`, failed otherwise) |
//! | `Diff` | `ACTIVITY_SNAPSHOT` `ailfred:diff` `{"patch","path"}` |
//! | `Ask` | `CUSTOM` `ailfred:ask` `{"interruptId","message","multi","options"}` where it was asked, and again, at `Done`, as an `Interrupt` with the same id (`input_required`, the prompt as `message`, `metadata.ailfred` `{"multi","options"}`) |
//! | `Done` | `RUN_FINISHED` outcome `interrupt` if it asked, else `success` (an outcome of a type this version doesn't know replays as `Done`; an interrupt not already asked, another producer's, as an `Ask` first) |
//! | `Fail` | `RUN_ERROR` |
//! | `Lost` | `RUN_ERROR` code `ailfred:lost` |
//! | the user's Stop | `RUN_FINISHED` outcome `cancelled` |
//!
//! Once the run has ended, the encoder sends nothing more, as a `Reply` ignores it; a replayer
//! refuses an event before `RUN_STARTED` or after the end, a second `RUN_STARTED` while the run
//! goes, and a `RUN_FINISHED` naming another thread or run ([`ReplayError`](replay::ReplayError)):
//! one replayer per run.
//! Answers go back in the next run's `resume` ([`answer_resume`](command::answer_resume):
//! `{"answer": "…"}`). A run's `messages` are, as AG-UI requires, the complete history, built
//! as a snapshot builds it ([`snapshot::history`]), so the ids and shapes are the snapshot's:
//! [`command::send`] adds the new user message `{run}-user`, [`command::answer`] the reply that
//! asked is the last of. The core takes an answer as a new user message (the app's chat pushes
//! its text as the next message), so a later snapshot shows it as the answering run's `user`
//! message `{run}-user`: that's how the two line up. Which messages win when a client's history
//! and the core's differ, a partial resume, and `status: cancelled` are left to the server (T20).
//!
//! ## Saved replies and snapshots
//!
//! [`saved`] turns a finished reply back into one run's events, and those events into the same
//! reply: a saved conversation keeps its replies so (the app's `history`). [`snapshot`] makes a
//! conversation one `MESSAGES_SNAPSHOT` for a client that joins it, and reads one back; its
//! docs say what a snapshot doesn't carry. A question nothing waits an answer for (closed, or
//! in a reply that failed or was stopped) says `"closed": true` there, and comes back closed: a
//! joining client can't answer it.
//!
//! ## Ids
//!
//! Derived from the run's id `{run}`: text messages `{run}-text-{n}`, a reasoning span
//! (`REASONING_START`/`END`) `{run}-reasoning-{n}` and its message `{run}-thinking-{n}`, steps `{run}-step-{id}` (the core's step id) with their result message
//! `{run}-step-{id}-result`, diffs `{run}-diff-{n}`, questions `{run}-ask-{n}`; `n` counts from 1.
//! A saved reply's steps, and a snapshot's, are numbered `1, 2, …` in the order they came; a
//! snapshot's assistant message with a run of steps' calls is `{run}-steps-{n}`, a user message
//! `{run}-user`, an error `{run}-error-{n}`.
//!
//! ## Fixtures and schemas
//!
//! `fixtures/*.jsonl` hold one run each, one AG-UI event per line (`commands.jsonl`: commands),
//! encoded from scripted core events in the tests: every client replays them in its own tests.
//! `schema/event.schema.json` and `schema/command.schema.json` are the JSON Schemas of
//! [`agui::Event`] and [`command::Command`] the clients generate their types from;
//! `spec/ag-ui-1.0.schema.json` is the official schema the fixtures are checked against.
//! The tests fail when the fixtures or schemas differ from what the code makes; after a
//! deliberate change, rewrite them with `UPDATE_FIXTURES=1 cargo test -p ailfred-protocol`.
//!
//! ## Limits, for the server (T20)
//!
//! - Steered replies overlap on pi: the next reply's `Taken` comes before the previous one's
//!   `Done`. On one thread's stream runs must not interleave (a client's replayer refuses it),
//!   so the server starts the next run after the previous one finished.
//! - Run ids must be unique within a thread, since every other id derives from them: the server
//!   mints them, or refuses a `runId` already used.
//! - Retry reuses a message's saved run id (the app's `history::run_id`), so the server must
//!   mint a fresh run id per run.
//! - A second `StepEnd` for a step that ended is ignored; the same core step id twice in one run
//!   isn't supported.
//! - An answer picking several options travels as one string, joined with `", "`, as the core's
//!   `Reply::answer` makes it.

pub mod agui;
pub mod command;
pub mod encode;
pub mod replay;
pub mod saved;
pub mod snapshot;

#[cfg(test)]
mod tests;
