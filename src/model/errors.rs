//! What went wrong, in words for the user (Settings is where a key or a model gets fixed).

use super::stream::Kind;

pub const MISSING: &str = "No model to answer: add an Anthropic API key in Settings, or install Claude Code.";
const NOT_LOGGED_IN: &str = "Claude Code isn't logged in. Run `claude` in Terminal and log in.";

/// An error the API reported: its `type` (`authentication_error`…) and message.
pub fn api_error(kind: &str, message: &str) -> String {
    match kind {
        "authentication_error" | "permission_error" => "The Anthropic API key was refused. Check it in Settings.".into(),
        "overloaded_error" => "Claude is overloaded right now. Try again in a moment.".into(),
        "rate_limit_error" => "Rate limited by Anthropic: too many requests. Wait a moment and try again.".into(),
        "not_found_error" => format!("The model isn't available ({message}). Pick another in Settings."),
        _ => format!("The Anthropic API said: {message}"),
    }
}

/// Claude Code's `result` line with `is_error`: the HTTP status it saw (if any) and its text.
pub fn cli_error(status: Option<u64>, message: &str) -> String {
    let lower = message.to_lowercase();
    if status == Some(401) || logged_out(&lower) {
        NOT_LOGGED_IN.into()
    } else if status == Some(529) || lower.contains("overloaded") {
        api_error("overloaded_error", message)
    } else if status == Some(429) || lower.contains("rate limit") {
        api_error("rate_limit_error", message)
    } else if status == Some(404) || (lower.contains("model") && (lower.contains("not found") || lower.contains("issue with the selected model"))) {
        // Claude Code's own sentence names the model in brackets: "…selected model (claude-x)…".
        match message.split_once("model (").and_then(|(_, rest)| rest.split_once(')')) {
            Some((model, _)) => format!("The model {model} isn't available. Pick another in Settings."),
            None => "The model isn't available. Pick another in Settings.".into(),
        }
    } else {
        format!("Claude Code stopped with an error: {message}")
    }
}

fn logged_out(lower: &str) -> bool {
    lower.contains("not logged in") || lower.contains("/login") || lower.contains("invalid api key")
}

/// The process couldn't start (curl or claude).
pub fn not_started(kind: Kind, error: &std::io::Error) -> String {
    match kind {
        Kind::Api => format!("Couldn't run curl to reach the Anthropic API: {error}"),
        Kind::Cli => format!("Couldn't start Claude Code: {error}"),
    }
}

/// The stream ended without a `Done` or an error: `stray` is the non-JSON it printed, `stderr`
/// what the process wrote to its error output, `exited_ok` whether it exited cleanly.
pub fn broke_off(kind: Kind, stray: &str, stderr: &str, exited_ok: bool) -> String {
    let ended = if exited_ok { "ended" } else { "ended with an error" };
    let said: String = format!("{} {}", stray.trim(), stderr.trim()).trim().chars().take(300).collect();
    match kind {
        Kind::Cli if logged_out(&said.to_lowercase()) => NOT_LOGGED_IN.into(),
        Kind::Cli if said.is_empty() => format!("Claude Code {ended} without a reply."),
        Kind::Cli => format!("Claude Code stopped: {said}"),
        Kind::Api if said.is_empty() => format!("The connection to the Anthropic API {ended} without a reply."),
        Kind::Api => format!("Couldn't reach the Anthropic API: {said}"),
    }
}
