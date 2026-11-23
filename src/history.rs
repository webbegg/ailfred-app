//! Conversations on disk, so they outlive the app: one JSON file per conversation in
//! `~/Library/Application Support/<app id>/conversations/` (folder 0700, files 0600, each written
//! to `<id>.json.tmp` and renamed), its captures as `<id>/<n>.png` beside it, referenced by
//! relative path. A damaged field falls back to its default, not the whole file; a file that
//! won't parse is skipped (and logged), never deleted.
//!
//! A message holds its text, its captures and its reply. A reply holds how it ended, its
//! thinking, text and error (what every version reads) and, since they were added, its run's
//! AG-UI events with the answers to its questions beside them (`convert`): loading prefers
//! those, so steps, diffs and questions come back. The fields were added, not changed: the
//! version stays 1, a build from before them reads a new file's text (serde passes over keys it
//! doesn't know), and a file without them loads as it always did. But an older build that
//! rewrites the file (any save, or a title patch) keeps the text and drops `events` and
//! `answers`. A question loaded without an answer is closed: the model's process that asked it
//! is gone. Files are written as compact JSON; any layout reads.
//!
//! Saving never happens on the main thread: [`Writer`] is a thread fed by a channel. Everything
//! here takes its folder as an argument, so tests stay in a scratch one.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ailfred_protocol::{agui, replay};
use serde::{Deserialize, Deserializer, Serialize};

mod convert;
mod store;
#[cfg(test)]
mod tests;
mod writer;

pub use convert::{reply_from, run_id, saved_reply};
pub use store::{Loaded, load, scan};
// What the tests reach for; the app writes through the `Writer`.
#[cfg(test)]
pub use store::{LoadedMessage, patch_title, write_snapshot};
pub use writer::Writer;

/// The file format's version: a file from a newer one is left alone.
pub const VERSION: u32 = 1;
/// How many conversations the menu lists.
pub const RECENT: usize = 10;
/// A conversation without a title shows the start of its first message, this many characters.
const LABEL_CHARS: usize = 40;
/// What a conversation without text (only a capture) is called.
const LABEL_CAPTURE: &str = "Screenshot";
/// What earlier saves put under a reply that was streaming when Ailfred quit (it's
/// `interrupted` now): one with this note loads as interrupted.
pub const INTERRUPTED: &str = "Interrupted: Ailfred quit before the reply finished.";

/// How a saved reply ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyState {
    Done,
    /// What a damaged state reads as: it shows what's there, and can be asked again.
    #[default]
    Stopped,
    Failed,
    /// It was still streaming when Ailfred quit.
    Interrupted,
}

/// A reply as saved: how it ended and its text, which any version reads, and the whole of it as
/// its run's AG-UI events, which loading prefers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedReply {
    #[serde(default, deserialize_with = "lenient")]
    pub state: ReplyState,
    #[serde(default, deserialize_with = "lenient")]
    pub thinking: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub text: String,
    #[serde(default, deserialize_with = "lenient")]
    pub error: Option<String>,
    /// Its run, from start to end (`ailfred_protocol::saved`): thinking, text, steps, diffs,
    /// questions. None in a file from before them. One of a type this build doesn't know (a
    /// newer one's) is skipped; one damaged drops them all.
    #[serde(default, deserialize_with = "lenient_events", skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<agui::Event>,
    /// What the user answered to its questions, in order (None: not answered).
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<Option<String>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedMessage {
    #[serde(default, deserialize_with = "lenient")]
    pub text: String,
    /// Its captures, as paths relative to the conversations folder.
    #[serde(default, deserialize_with = "lenient_list")]
    pub pngs: Vec<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub reply: Option<SavedReply>,
}

/// Claude Code's session of the conversation, and the message (an index in `messages`) whose
/// reply it ends at.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    #[serde(default, deserialize_with = "lenient")]
    pub id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub after_message: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    #[serde(default, deserialize_with = "lenient")]
    pub version: u32,
    #[serde(default, deserialize_with = "lenient")]
    pub id: String,
    /// Unix seconds.
    #[serde(default, deserialize_with = "lenient")]
    pub created: u64,
    #[serde(default, deserialize_with = "lenient")]
    pub updated: u64,
    #[serde(default, deserialize_with = "lenient")]
    pub title: Option<String>,
    /// The user wrote the title: an automatic one never replaces it.
    #[serde(default, deserialize_with = "lenient")]
    pub title_by_user: bool,
    #[serde(default, deserialize_with = "lenient")]
    pub session: Option<SavedSession>,
    #[serde(default, deserialize_with = "lenient_list")]
    pub messages: Vec<SavedMessage>,
}

/// What to write: the conversation, and the PNG bytes of its captures by relative path (a file
/// that's already there stays as it is).
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub saved: Saved,
    pub pngs: Vec<(String, Arc<Vec<u8>>)>,
}

/// What a chat knows of its conversation besides its messages.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub id: String,
    pub created: u64,
    pub title: Option<String>,
    pub title_by_user: bool,
    /// The number the next capture's file takes (`<id>/<n>.png`).
    pub next_png: usize,
    /// A title was asked for in this run: it's asked once.
    pub title_asked: bool,
}

impl Meta {
    /// A new conversation: a fresh id, no title.
    pub fn fresh() -> Meta {
        Meta { id: new_id(), created: now_secs(), title: None, title_by_user: false, next_png: 0, title_asked: false }
    }
}

/// A conversation in the menu's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub updated: u64,
}

/// The entry for a saved conversation; None if it has no message (nothing to show or restore).
pub fn entry_of(id: &str, saved: &Saved) -> Option<Entry> {
    let first = saved.messages.first()?;
    Some(Entry { id: id.to_owned(), label: label(saved.title.as_deref(), &first.text), updated: saved.updated })
}

/// What the menu shows: the title, else the start of the first message (cut with "…").
pub fn label(title: Option<&str>, first_message: &str) -> String {
    if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
        return title.to_owned();
    }
    let flat = first_message.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return LABEL_CAPTURE.into();
    }
    match flat.char_indices().nth(LABEL_CHARS) {
        Some((cut, _)) => format!("{}…", flat[..cut].trim_end()),
        None => flat,
    }
}

/// The `limit` most recently updated, newest first (ties: the later id).
pub fn recent(mut entries: Vec<Entry>, limit: usize) -> Vec<Entry> {
    entries.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
    entries.truncate(limit);
    entries
}

/// `~/Library/Application Support/<app id>/conversations`.
pub fn dir(home: &Path, app_id: &str) -> PathBuf {
    home.join("Library/Application Support").join(app_id).join("conversations")
}

/// Where this build keeps its conversations.
pub fn dir_now() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    dir(&home, crate::settings::app_id(crate::app_env::in_bundle()))
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A new conversation's id, `YYYYMMDD-HHMMSS-<4 hex>` (UTC): sorts by age, and two made in the
/// same second differ.
pub fn new_id() -> String {
    static SEED: OnceLock<u32> = OnceLock::new();
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let seed = *SEED.get_or_init(|| {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        nanos ^ std::process::id().rotate_left(16)
    });
    format_id(now_secs(), seed.wrapping_add(COUNT.fetch_add(1, Ordering::Relaxed)) as u16)
}

/// `secs` since the epoch as `YYYYMMDD-HHMMSS-<salt in hex>`.
pub fn format_id(secs: u64, salt: u16) -> String {
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{salt:04x}", rest / 3600, rest % 3600 / 60, rest % 60)
}

/// An id that's safe as a file name: nothing but letters, digits and dashes.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A path from a file, only if it stays inside the folder it's joined to: no root, no `..`.
pub fn safe_relative(path: &str) -> Option<PathBuf> {
    let path = PathBuf::from(path);
    let inside = path.components().count() > 0 && path.components().all(|c| matches!(c, Component::Normal(_)));
    inside.then_some(path)
}

/// A field that falls back to its default when it has the wrong shape (the rest of the file
/// still reads).
fn lenient<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// Saved events, read as a client reads a stream (`replay::parse_value`): one of a type this
/// build doesn't know is skipped, a damaged one leaves none (the reply shows its text).
fn lenient_events<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<agui::Event>, D::Error> {
    let serde_json::Value::Array(items) = serde_json::Value::deserialize(d)? else { return Ok(Vec::new()) };
    let events: Result<Vec<Option<agui::Event>>, _> = items.iter().map(replay::parse_value).collect();
    Ok(events.map(|events| events.into_iter().flatten().collect()).unwrap_or_default())
}

/// A list that keeps the elements with the right shape.
fn lenient_list<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
    let serde_json::Value::Array(items) = serde_json::Value::deserialize(d)? else { return Ok(Vec::new()) };
    Ok(items.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}
