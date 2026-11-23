//! The local server's tests: the hub's order and frames, the guards and routes without
//! sockets, and one run over a real socket.

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use ailfred_protocol::agui::{self, Base};
use ailfred_protocol::conformance::Spec;
use serde_json::Value;

use super::hub::Frame;
use super::routes::STREAM_FRAMES;

mod handoff;
mod head;
mod hub;
mod limits;
mod routes;
mod socket;

/// A 1×1 PNG.
pub(crate) const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89,
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f, 0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A conversation with nothing in it, as a snapshot.
pub(crate) fn empty() -> agui::Event {
    agui::Event::MessagesSnapshot { messages: Vec::new(), base: Base::at(0) }
}

/// A client's channel, as the server makes it.
pub(crate) fn watcher() -> (SyncSender<Frame>, Receiver<Frame>) {
    sync_channel(STREAM_FRAMES)
}

/// The events a watcher got so far, each checked: an SSE `data:` line, keeping to the official
/// schema (a joining client's first frame holds several).
pub(crate) fn got(frames: &Receiver<Frame>) -> Vec<Value> {
    let spec = Spec::load();
    let frames: Vec<Frame> = frames.try_iter().collect();
    frames
        .iter()
        .flat_map(|frame| frame.split_inclusive("\n\n"))
        .map(|frame| {
            let json = frame.strip_prefix("data: ").and_then(|f| f.strip_suffix("\n\n")).unwrap_or_else(|| panic!("not an SSE data frame: {frame:?}"));
            let value: Value = serde_json::from_str(json).unwrap();
            let errors = spec.errors(&value, "Event");
            assert!(errors.is_empty(), "{value}: {errors:?}");
            value
        })
        .collect()
}

/// An event's type and the run it's of: its `runId`, else the run its message or call id
/// derives from (`{run}-…`, the runs here named without dashes).
pub(crate) fn tag(e: &Value) -> String {
    let kind = e["type"].as_str().unwrap_or_default();
    let id = ["runId", "messageId", "toolCallId"].iter().find_map(|k| e[k].as_str()).unwrap_or_default();
    format!("{kind} {}", id.split('-').next().unwrap_or_default())
}
