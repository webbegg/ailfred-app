//! The conversations folder: writing a conversation, scanning the folder, loading one back.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Entry, Saved, SavedReply, Snapshot, VERSION, entry_of, recent, safe_id, safe_relative};

/// The folder, owner-only, made if it's missing.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    // One that was there already (or made by something else) is closed up too.
    if std::fs::metadata(dir)?.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// `bytes` into `path`, all or nothing: written to `<path>.tmp` (owner-only), then renamed over
/// it. A failure leaves what was there and no `.tmp`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let written = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn json_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// Writes the conversation: its captures first (once: a file that's there stays), then its file.
pub fn write_snapshot(dir: &Path, snapshot: &Snapshot) -> std::io::Result<()> {
    let id = &snapshot.saved.id;
    if !safe_id(id) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("not a conversation id: {id:?}")));
    }
    ensure_private_dir(dir)?;
    if !snapshot.pngs.is_empty() {
        ensure_private_dir(&dir.join(id))?;
    }
    for (rel, png) in &snapshot.pngs {
        let Some(rel) = safe_relative(rel) else { continue };
        let path = dir.join(rel);
        if !path.exists() {
            write_atomic(&path, png)?;
        }
    }
    let saved = Saved { version: VERSION, ..snapshot.saved.clone() };
    write_atomic(&json_path(dir, id), &serde_json::to_vec_pretty(&saved)?)
}

/// What a file says, if it parses and isn't from a newer version; otherwise logged.
fn read_saved(path: &Path) -> Option<Saved> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("Ailfred: reading {} failed: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_slice::<Saved>(&bytes) {
        Ok(saved) if saved.version > VERSION => {
            eprintln!("Ailfred: {} is from a newer version of Ailfred; left alone", path.display());
            None
        }
        Ok(saved) => Some(saved),
        Err(e) => {
            eprintln!("Ailfred: {} isn't a conversation, skipped: {e}", path.display());
            None
        }
    }
}

/// The ids of the conversation files in `dir` (no `.tmp` leftovers).
fn ids(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    read.filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".json").map(str::to_owned))
        .filter(|id| safe_id(id))
        .collect()
}

/// Every conversation in `dir` that reads and has a message, newest first. The file's name is
/// its id; a file without `updated` takes its modification time.
pub fn scan(dir: &Path) -> Vec<Entry> {
    let entries = ids(dir)
        .into_iter()
        .filter_map(|id| {
            let path = json_path(dir, &id);
            let mut saved = read_saved(&path)?;
            if saved.updated == 0 {
                saved.updated = modified(&path);
            }
            entry_of(&id, &saved)
        })
        .collect();
    recent(entries, usize::MAX)
}

fn modified(path: &Path) -> u64 {
    let time = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    time.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// One message as it was loaded: its captures read back (a missing file drops that capture).
pub struct LoadedMessage {
    pub text: String,
    /// The relative paths of the captures that were read, parallel to `pngs`.
    pub files: Vec<String>,
    pub pngs: Vec<Arc<Vec<u8>>>,
    pub reply: Option<SavedReply>,
}

/// A conversation read back.
pub struct Loaded {
    pub id: String,
    pub created: u64,
    pub title: Option<String>,
    pub title_by_user: bool,
    /// Claude Code's session id and the index of the message whose reply it ends at.
    pub session: Option<(String, usize)>,
    pub messages: Vec<LoadedMessage>,
    /// The number the next capture's file takes (past every one the file mentions).
    pub next_png: usize,
}

/// Reads conversation `id` from `dir`; None if there's no such readable file, or no message in it.
pub fn load(dir: &Path, id: &str) -> Option<Loaded> {
    if !safe_id(id) {
        return None;
    }
    let saved = read_saved(&json_path(dir, id))?;
    if saved.messages.is_empty() {
        return None;
    }
    let mut next_png = 0;
    let messages = saved
        .messages
        .into_iter()
        .map(|m| {
            let (mut files, mut pngs) = (Vec::new(), Vec::new());
            for rel in m.pngs {
                next_png = next_png.max(number_of(&rel).map_or(0, |n| n + 1));
                let Some(path) = safe_relative(&rel) else { continue };
                match std::fs::read(dir.join(path)) {
                    Ok(bytes) => {
                        files.push(rel);
                        pngs.push(Arc::new(bytes));
                    }
                    Err(e) => eprintln!("Ailfred: capture {rel} of {id} is missing, dropped: {e}"),
                }
            }
            LoadedMessage { text: m.text, files, pngs, reply: m.reply }
        })
        .collect();
    let session = saved.session.filter(|s| !s.id.is_empty()).map(|s| (s.id, s.after_message));
    Some(Loaded { id: id.to_owned(), created: saved.created, title: saved.title, title_by_user: saved.title_by_user, session, messages, next_png })
}

/// `<id>/<n>.png`'s `n`.
fn number_of(rel: &str) -> Option<usize> {
    Path::new(rel).file_stem()?.to_str()?.parse().ok()
}

/// Gives conversation `id` an automatic `title` in its file, unless it has one or the user
/// wrote it. The entry as it is now, if the file changed.
pub fn patch_title(dir: &Path, id: &str, title: &str) -> std::io::Result<Option<Entry>> {
    let path = json_path(dir, id);
    let Some(mut saved) = safe_id(id).then(|| read_saved(&path)).flatten() else { return Ok(None) };
    if saved.title.is_some() || saved.title_by_user {
        return Ok(None);
    }
    saved.title = Some(title.to_owned());
    saved.version = VERSION;
    write_atomic(&path, &serde_json::to_vec_pretty(&saved)?)?;
    Ok(entry_of(id, &saved))
}
