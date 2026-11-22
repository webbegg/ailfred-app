//! The installed Claude Code as the model: `claude -p` with its stream-json in and out, run lean
//! (no settings, hooks, plugins, MCP servers or tools: a "say hi" costs hundredths of a cent
//! instead of a quarter) from a private folder so its sessions stay out of the user's projects.

use std::ffi::OsStr;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use super::stream::Kind;
use super::{Item, Request, Run, SYSTEM_PROMPT, errors, pump, user_content};
use crate::reply::Event;

/// The folders Claude Code lands in, then `PATH`'s (an app launched from Finder has a short
/// one that lacks them).
pub fn candidate_dirs(home: &Path, path_var: Option<&OsStr>) -> Vec<PathBuf> {
    let mut dirs = vec![home.join(".local/bin"), "/opt/homebrew/bin".into(), "/usr/local/bin".into(), home.join(".claude/local")];
    dirs.extend(path_var.iter().flat_map(|p| std::env::split_paths(p)));
    dirs
}

/// The first executable file named `claude` in `dirs`.
pub fn find_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join("claude")).find(|path| path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// Where `claude` is on this Mac.
pub fn find() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    find_in(&candidate_dirs(&home, std::env::var_os("PATH").as_deref()))
}

/// `claude`'s arguments: lean, streaming, the model, and the session to continue.
pub fn args(model: &str, session: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--setting-sources",
        "",
        "--strict-mcp-config",
        "--tools",
        "",
        "--system-prompt",
        SYSTEM_PROMPT,
        "--model",
        model,
    ]
    .map(String::from)
    .to_vec();
    if let Some(session) = session {
        args.extend(["--resume".into(), session.into()]);
    }
    args
}

/// The one line on its stdin: the user's message with its captures. A run without a session
/// to resume (the conversation's first, a retry, or one begun on the API) gets the earlier
/// turns first, as a transcript.
pub fn input(request: &Request) -> String {
    let mut content = user_content(&request.text, &request.pngs);
    if request.session.is_none() && !request.history.is_empty()
        && let serde_json::Value::Array(blocks) = &mut content
    {
        blocks.insert(0, json!({"type": "text", "text": transcript(&request.history)}));
    }
    let line = json!({"type": "user", "message": {"role": "user", "content": content}});
    format!("{line}\n")
}

/// The conversation so far, in words: earlier captures are only counted.
fn transcript(history: &[super::Turn]) -> String {
    let mut out = String::from("Our conversation so far, for context:\n");
    for turn in history {
        let shots = match turn.pngs.len() {
            0 => String::new(),
            1 => " [with a screenshot]".into(),
            n => format!(" [with {n} screenshots]"),
        };
        out.push_str(&format!("\nUser{shots}: {}\n\nAssistant: {}\n", turn.text, turn.reply));
    }
    out.push_str("\nThe user's new message follows.");
    out
}

/// `~/Library/Application Support/<app id>/claude-code`.
pub fn private_dir(home: &Path, app_id: &str) -> PathBuf {
    home.join("Library/Application Support").join(app_id).join("claude-code")
}

/// Where this build's Claude Code runs.
pub fn private_dir_now() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    private_dir(&home, crate::settings::app_id(crate::app_env::in_bundle()))
}

/// Creates `dir` (and its parents) owner-only.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

/// Answers `request` with Claude Code at `claude`, run from `dir`, streaming into `sink`.
pub fn run(run: &Run, claude: &Path, dir: &Path, request: &Request, sink: &mut dyn FnMut(Item)) {
    if let Err(e) = ensure_private_dir(dir) {
        return sink(Item::Event(Event::Fail(errors::not_started(Kind::Cli, &e))));
    }
    let mut cmd = Command::new(claude);
    cmd.args(args(&request.model, request.session.as_deref())).current_dir(dir);
    pump(run, Kind::Cli, cmd, input(request).into_bytes(), sink);
}
