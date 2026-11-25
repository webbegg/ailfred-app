//! The connected apps: Composio's MCP server in the `mcp.json` of Ailfred's pi while Settings'
//! switch is on, and the guard extension that asks the user before anything there that isn't a
//! known read (`connections_guard.js`). See `docs/connected-apps.md`.

/// The pi extension that lets known reads of the connected apps run and asks before the rest.
pub(super) const GUARD: &str = include_str!("connections_guard.js");

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use super::Layout;

/// The server's name in `mcp.json`: the guard knows its tools by it (`mcp__composio__…`), and
/// pi keeps the sign-in under it and [`URL`].
pub const SERVER: &str = "composio";
pub const URL: &str = "https://connect.composio.dev/mcp";
/// The guard's file in pi's extensions folder.
const GUARD_FILE: &str = "ailfred-connections-guard.js";
/// The guard made by hand before this one: left there, it would ask a second time.
const HAND_MADE_GUARD: &str = "composio-guard.ts";
/// How long signing in may take, the browser's part included (pi's default).
const SIGN_IN_SECONDS: &str = "300";

/// Settings' switch as last set, read when pi starts (any thread).
static ON: AtomicBool = AtomicBool::new(false);

/// Settings' switch (and its state at launch): kept for pi's next start, and the server goes
/// into or out of the `mcp.json` of Ailfred's pi under `data` now, not only when pi next starts.
pub fn set(data: &Path, on: bool) -> io::Result<()> {
    ON.store(on, Ordering::Relaxed);
    apply(&Layout::under(data), on)
}

/// Whether the connected apps are on, as last set.
pub fn now() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Composio's server in pi's `mcp.json` while `on`, else taken out (the other servers stay;
/// pi keeps the sign-in, which is the server's name and URL's, either way). The guard is in pi's
/// extensions either way: it costs nothing without the server, and a `composio` server added by
/// hand is guarded too. The hand-made guard goes.
pub(super) fn apply(layout: &Layout, on: bool) -> io::Result<()> {
    let server = super::set_server(layout, SERVER, on.then(entry), "the connected apps' server");
    server.and(guard(layout))
}

fn entry() -> serde_json::Value {
    serde_json::json!({
        "url": URL,
        "exposure": "direct",
        "timeout": 120,
        "description": "The user's connected apps (Google Calendar, Gmail, GitHub, Jira, Slack, ...): search for a tool, connect an app, then execute it",
        // Code and raw requests run on Composio's side, feedback, skills: the guard blocks them
        // too, should they be called.
        "toolExposure": {
            "COMPOSIO_*_SKILL*": "hidden",
            "COMPOSIO_PROXY_EXECUTE": "hidden",
            "COMPOSIO_REMOTE_BASH_TOOL": "hidden",
            "COMPOSIO_REMOTE_WORKBENCH": "hidden",
            "COMPOSIO_SUBMIT_FEEDBACK": "hidden",
        },
    })
}

fn guard(layout: &Layout) -> io::Result<()> {
    let dir = layout.agent.join("extensions");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(GUARD_FILE), GUARD)?;
    match std::fs::remove_file(dir.join(HAND_MADE_GUARD)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// `mcp.json` of Ailfred's pi under `data` has a `composio` server (the user's own, before
/// Settings had the switch, counts).
pub fn configured(data: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(Layout::under(data).agent.join("mcp.json")) else { return false };
    super::pi_reads(&text).is_some_and(|config| config.get("mcpServers").and_then(|s| s.get(SERVER)).is_some())
}

/// pi has Composio's tokens: its `mcp-auth.json` holds a `tokens` entry under the server's name
/// and URL (or the URL alone, as older pi kept it). Only names are looked at, never a value.
pub fn signed_in(data: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(Layout::under(data).agent.join("mcp-auth.json")) else { return false };
    let Ok(serde_json::Value::Object(states)) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
    [format!("mcp__{SERVER}|{URL}"), URL.to_owned()].iter().any(|key| states.get(key).and_then(|state| state.get("tokens")).is_some())
}

/// `pi mcp login composio` with Ailfred's folders: pi opens the sign-in page in the browser and
/// waits for it.
pub(super) fn login_command(pi: &Path, layout: &Layout, real_home: &Path, path: &OsString, inherited: impl IntoIterator<Item = (OsString, OsString)>) -> Command {
    let mut command = super::with_layout(pi, layout, real_home, path, inherited);
    command.args(["mcp", "login", SERVER, "--timeout", SIGN_IN_SECONDS]);
    command
}

/// Signs pi in to Composio: its sign-in page opens in the browser, and this returns once the
/// user is done there (or gave up, or it timed out), with what pi said when it failed. Runs the
/// user's login shell once and waits for the browser, so not on the main thread.
pub fn sign_in(pi: &Path, data: &Path, real_home: &Path) -> Result<(), String> {
    let layout = Layout::under(data);
    super::prepare(&layout, &real_home.join(".pi/agent")).map_err(|e| e.to_string())?;
    let path = super::login_path(&std::env::var_os("PATH").unwrap_or_default());
    let mut command = login_command(pi, &layout, real_home, &path, std::env::vars_os());
    let out = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Err(said.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("pi mcp login failed").trim().to_owned())
}

/// What to run in Terminal to sign in by hand (`data`: Ailfred's pi).
pub fn sign_in_by_hand(data: &Path) -> String {
    format!("PI_CODING_AGENT_DIR=\"{}\" ~/.pi/agent/bin/pi mcp login {SERVER}", Layout::under(data).agent.display())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use serde_json::{Value, json};

    use super::*;
    use crate::pi_setup::Layout;
    use crate::pi_setup::tests::{scratch, settings};

    #[test]
    fn while_on_composio_is_in_pis_mcp_json_and_off_it_leaves_and_the_other_servers_stay() {
        let root = scratch("connections");
        let data = root.join("data");
        let layout = Layout::under(&data);
        std::fs::create_dir_all(&layout.agent).unwrap();
        let mcp = layout.agent.join("mcp.json");
        std::fs::write(&mcp, r#"{"mcpServers":{"mine":{"command":"x"},"computer":{"url":"http://127.0.0.1:1/mcp"}},"other":1}"#).unwrap();
        apply(&layout, true).unwrap();
        let config = settings(&mcp);
        assert_eq!(config["mcpServers"]["mine"], json!({"command": "x"}), "the user's server stays");
        assert_eq!(config["mcpServers"]["computer"]["url"], "http://127.0.0.1:1/mcp");
        assert_eq!(config["other"], 1);
        let composio = &config["mcpServers"]["composio"];
        assert_eq!(composio["url"], "https://connect.composio.dev/mcp", "the URL its sign-in is kept under");
        assert_eq!(composio["exposure"], "direct");
        assert_eq!(composio["timeout"], 120);
        assert!(composio["description"].as_str().is_some_and(|d| !d.is_empty()));
        for hidden in ["COMPOSIO_*_SKILL*", "COMPOSIO_PROXY_EXECUTE", "COMPOSIO_REMOTE_BASH_TOOL", "COMPOSIO_REMOTE_WORKBENCH", "COMPOSIO_SUBMIT_FEEDBACK"] {
            assert_eq!(composio["toolExposure"][hidden], "hidden", "{hidden}");
        }
        assert!(composio.get("headers").is_none(), "signed in through pi's OAuth, no token here");
        assert!(configured(&data));

        apply(&layout, false).unwrap();
        let config = settings(&mcp);
        assert!(config["mcpServers"].get("composio").is_none(), "{config}");
        assert_eq!(config["mcpServers"]["mine"], json!({"command": "x"}));
        assert!(config["mcpServers"].get("computer").is_some());
        assert!(!configured(&data));

        // Off with no mcp.json: none is made.
        let fresh = Layout::under(&root.join("fresh"));
        std::fs::create_dir_all(&fresh.agent).unwrap();
        apply(&fresh, false).unwrap();
        assert!(!fresh.agent.join("mcp.json").exists());
        assert!(!configured(&root.join("fresh")) && !configured(&root.join("nowhere")));
    }

    #[test]
    fn the_guard_is_in_pis_extensions_whether_on_or_off_and_the_hand_made_one_goes() {
        let root = scratch("connections-guard-file");
        let layout = Layout::under(&root.join("data"));
        let extensions = layout.agent.join("extensions");
        std::fs::create_dir_all(&extensions).unwrap();
        std::fs::write(extensions.join("composio-guard.ts"), "// spike").unwrap();
        std::fs::write(extensions.join("mine.ts"), "// theirs").unwrap();
        apply(&layout, true).unwrap();
        assert_eq!(std::fs::read_to_string(extensions.join("ailfred-connections-guard.js")).unwrap(), GUARD);
        assert!(!extensions.join("composio-guard.ts").exists(), "two guards would ask twice");
        assert!(extensions.join("mine.ts").exists(), "only that one file goes");
        apply(&layout, false).unwrap();
        assert_eq!(std::fs::read_to_string(extensions.join("ailfred-connections-guard.js")).unwrap(), GUARD, "off too: a composio server added by hand is guarded");
        // No extensions folder yet.
        let fresh = Layout::under(&root.join("fresh"));
        std::fs::create_dir_all(&fresh.agent).unwrap();
        apply(&fresh, false).unwrap();
        assert!(fresh.agent.join("extensions/ailfred-connections-guard.js").is_file());
    }

    #[test]
    fn signed_in_means_pis_mcp_auth_has_tokens_under_composios_name_and_url() {
        let root = scratch("connections-auth");
        let data = root.join("data");
        let agent = Layout::under(&data).agent;
        std::fs::create_dir_all(&agent).unwrap();
        let auth = agent.join("mcp-auth.json");
        assert!(!signed_in(&data), "no file");
        for (text, expected) in [
            (r#"{"mcp__composio|https://connect.composio.dev/mcp":{"tokens":{"access_token":"a"},"clientInformation":{}}}"#, true),
            // The key pi used before it named the server in it.
            (r#"{"https://connect.composio.dev/mcp":{"tokens":{}}}"#, true),
            // Registered as a client, never signed in.
            (r#"{"mcp__composio|https://connect.composio.dev/mcp":{"clientInformation":{}}}"#, false),
            (r#"{"mcp__other|https://connect.composio.dev/mcp":{"tokens":{}}}"#, false),
            (r#"{"mcp__composio|https://example.com/mcp":{"tokens":{}}}"#, false),
            ("{not json", false),
            ("[]", false),
        ] {
            std::fs::write(&auth, text).unwrap();
            assert_eq!(signed_in(&data), expected, "{text}");
        }
    }

    #[test]
    fn signing_in_runs_pis_mcp_login_for_composio_with_ailfreds_folders() {
        let layout = Layout::under(Path::new("/data"));
        let inherited = [("PI_SESSION_ID", "s1"), ("AILFRED_CONNECTIONS", "readonly")].map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let login = login_command(Path::new("/bin/pi"), &layout, Path::new("/Users/me"), &OsString::from("/usr/bin"), inherited);
        let args: Vec<String> = login.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["mcp", "login", "composio", "--timeout", "300"]);
        assert_eq!(login.get_program(), "/bin/pi");
        let env: Vec<(String, Option<String>)> = login.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        assert!(env.contains(&("PI_CODING_AGENT_DIR".into(), Some("/data/agent".into()))), "{env:?}");
        assert!(env.contains(&("HOME".into(), Some("/data/home".into()))), "{env:?}");
        assert!(env.contains(&("PI_SESSION_ID".into(), None)), "{env:?}");
    }

    /// What the guard makes of `cases`, run in `node` as pi runs it: each case a tool call
    /// (`tool`, `input`), the mode Ailfred set (`mode`, the `AILFRED_CONNECTIONS` variable, or
    /// null), whether pi has a UI (`hasUI`, default true) and the user's answer (`answer`).
    /// Per case: what the handler returned (null: the call runs) and the dialogs it opened.
    fn guard_with_node(cases: &Value) -> Vec<Value> {
        let dir = crate::pi_setup::tests::scratch("connections-guard");
        std::fs::write(dir.join("guard.mjs"), GUARD).unwrap();
        std::fs::write(dir.join("cases.json"), cases.to_string()).unwrap();
        let harness = r#"
import { readFileSync } from "node:fs";
import extension from "./guard.mjs";
const handlers = {};
extension({ on(name, handler) { handlers[name] = handler; } });
if (!handlers.tool_call) throw new Error("no tool_call handler");
const cases = JSON.parse(readFileSync(new URL("./cases.json", import.meta.url), "utf8"));
const out = [];
for (const c of cases) {
  if (c.mode == null) delete process.env.AILFRED_CONNECTIONS; else process.env.AILFRED_CONNECTIONS = c.mode;
  const asked = [];
  const ui = { confirm: async (title, message) => { asked.push({ title, message }); return c.answer ?? false; } };
  const result = await handlers.tool_call({ type: "tool_call", toolCallId: "t1", toolName: c.tool, input: c.input }, { hasUI: c.hasUI ?? true, ui });
  out.push({ result: result ?? null, asked });
}
process.stdout.write(JSON.stringify(out));
"#;
        std::fs::write(dir.join("harness.mjs"), harness).unwrap();
        let out = std::process::Command::new("node")
            .arg(dir.join("harness.mjs"))
            .env_remove("AILFRED_CONNECTIONS")
            .output()
            .unwrap_or_else(|e| panic!("this test needs node on PATH (pi runs on it): {e}"));
        assert!(out.status.success(), "node failed: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    const EXECUTE: &str = "mcp__composio__COMPOSIO_MULTI_EXECUTE_TOOL";

    /// A Composio batch running `slugs`, each with no arguments.
    fn batch(slugs: &[&str]) -> Value {
        json!({"tools": slugs.iter().map(|s| json!({"tool_slug": s, "arguments": {}})).collect::<Vec<_>>()})
    }

    fn call(tool: &str, input: Value) -> Value {
        json!({"tool": tool, "input": input, "answer": true})
    }

    /// Ran without a dialog.
    fn ran(got: &Value) -> bool {
        got["result"].is_null() && got["asked"].as_array().is_some_and(Vec::is_empty)
    }

    /// Asked once (and, the user saying yes, ran).
    fn asked(got: &Value) -> bool {
        got["result"].is_null() && got["asked"].as_array().is_some_and(|a| a.len() == 1)
    }

    /// Blocked without asking, saying why.
    fn blocked(got: &Value) -> bool {
        got["result"]["block"] == true && got["result"]["reason"].as_str().is_some_and(|r| !r.is_empty()) && got["asked"].as_array().is_some_and(Vec::is_empty)
    }

    #[test]
    fn a_known_read_runs_without_asking_and_every_other_slug_or_tool_asks() {
        let reads = [
            call(EXECUTE, batch(&["GMAIL_FETCH_EMAILS"])),
            call(EXECUTE, batch(&["GOOGLECALENDAR_EVENTS_LIST", "GITHUB_GET_AN_ISSUE", "JIRA_SEARCH_ISSUES", "SLACK_FETCH_CONVERSATION_HISTORY"])),
            call(EXECUTE, batch(&["gmail_fetch_emails"])),
            call("mcp__composio__COMPOSIO_SEARCH_TOOLS", json!({"queries": [{"use_case": "read my mail"}]})),
            call("mcp__composio__COMPOSIO_GET_TOOL_SCHEMAS", json!({"tool_slugs": ["GMAIL_SEND_EMAIL"]})),
            call("mcp__composio__COMPOSIO_WAIT_FOR_CONNECTIONS", json!({})),
            call("bash", json!({"command": "ls"})),
            call("mcp__computer__click", json!({})),
        ];
        let asks = [
            call(EXECUTE, batch(&["GMAIL_SEND_EMAIL"])),
            call(EXECUTE, batch(&["GMAIL_FETCH_EMAILS", "GMAIL_DELETE_MESSAGE"])),
            call(EXECUTE, batch(&["GMAIL_GET_UNHEARD_OF_THING"])),
            call(EXECUTE, batch(&[""])),
            call(EXECUTE, json!({"tools": [{"arguments": {}}]})),
            call(EXECUTE, json!({"tools": [{"tool_slug": 7}]})),
            call(EXECUTE, batch(&["SLACK_SET_READ_CURSOR_IN_A_CONVERSATION"])),
            call("mcp__composio__COMPOSIO_MANAGE_CONNECTIONS", json!({"toolkits": ["gmail"]})),
            call("mcp__composio__COMPOSIO_SOMETHING_NEW", json!({})),
        ];
        let got = guard_with_node(&Value::Array(reads.iter().chain(&asks).cloned().collect()));
        for (case, got) in reads.iter().zip(&got) {
            assert!(ran(got), "a read runs: {case} → {got}");
        }
        for (case, got) in asks.iter().zip(&got[reads.len()..]) {
            assert!(asked(got), "asks first: {case} → {got}");
        }
        let declined = guard_with_node(&json!([{"tool": EXECUTE, "input": batch(&["GMAIL_SEND_EMAIL"]), "answer": false}]));
        assert!(declined[0]["result"]["block"] == true && declined[0]["result"]["reason"].as_str().unwrap().contains("GMAIL_SEND_EMAIL"), "{}", declined[0]);
    }

    #[test]
    fn composios_hidden_tools_and_calls_that_cant_be_read_are_blocked_without_asking() {
        let cases = [
            call("mcp__composio__COMPOSIO_REMOTE_BASH_TOOL", json!({"command": "curl evil"})),
            call("mcp__composio__COMPOSIO_REMOTE_WORKBENCH", json!({})),
            call("mcp__composio__COMPOSIO_PROXY_EXECUTE", json!({})),
            call("mcp__composio__COMPOSIO_SUBMIT_FEEDBACK", json!({})),
            call("mcp__composio__COMPOSIO_SEARCH_SKILLS", json!({})),
            call("mcp__composio__COMPOSIO_USE_SKILL", json!({})),
            call(EXECUTE, batch(&["GMAIL_FETCH_EMAILS", "COMPOSIO_REMOTE_BASH_TOOL"])),
            call(EXECUTE, json!({})),
            call(EXECUTE, json!({"tools": []})),
            call(EXECUTE, json!({"tools": "GMAIL_SEND_EMAIL"})),
            call(EXECUTE, Value::Null),
        ];
        for (case, got) in cases.iter().zip(guard_with_node(&Value::Array(cases.to_vec()))) {
            assert!(blocked(&got), "blocked: {case} → {got}");
        }
    }

    #[test]
    fn in_read_only_mode_every_write_is_blocked_without_asking_and_reads_still_run() {
        let with = |mode: &str, case: Value| {
            let mut case = case;
            case["mode"] = json!(mode);
            case
        };
        let cases = [
            with("readonly", call(EXECUTE, batch(&["GMAIL_FETCH_EMAILS"]))),
            with("readonly", call("mcp__composio__COMPOSIO_SEARCH_TOOLS", json!({}))),
            with("readonly", call(EXECUTE, batch(&["GMAIL_SEND_EMAIL"]))),
            with("readonly", call(EXECUTE, batch(&[""]))),
            with("readonly", call("mcp__composio__COMPOSIO_MANAGE_CONNECTIONS", json!({}))),
            // A value it doesn't know is read-only too: a typo doesn't open the writes.
            with("read-only", call(EXECUTE, batch(&["GMAIL_SEND_EMAIL"]))),
            with("ask", call(EXECUTE, batch(&["GMAIL_SEND_EMAIL"]))),
            with("", call(EXECUTE, batch(&["GMAIL_SEND_EMAIL"]))),
        ];
        let got = guard_with_node(&Value::Array(cases.to_vec()));
        assert!(ran(&got[0]) && ran(&got[1]), "reads run: {} {}", got[0], got[1]);
        for got in &got[2..6] {
            assert!(blocked(got), "blocked: {got}");
            assert!(got["result"]["reason"].as_str().unwrap().contains("read-only"), "{got}");
        }
        assert!(asked(&got[6]) && asked(&got[7]), "ask (or unset) asks: {} {}", got[6], got[7]);
    }

    #[test]
    fn with_no_one_to_ask_a_write_is_blocked_and_a_read_runs() {
        let cases = json!([
            {"tool": EXECUTE, "input": batch(&["GMAIL_SEND_EMAIL"]), "hasUI": false, "answer": true},
            {"tool": EXECUTE, "input": batch(&["GMAIL_FETCH_EMAILS"]), "hasUI": false},
        ]);
        let got = guard_with_node(&cases);
        assert!(blocked(&got[0]) && got[0]["result"]["reason"].as_str().unwrap().contains("GMAIL_SEND_EMAIL"), "{}", got[0]);
        assert!(ran(&got[1]), "{}", got[1]);
    }

    #[test]
    fn the_dialog_shows_each_actions_slug_and_its_arguments_one_line_each_cut_short() {
        let body = format!("Hi Bob,\nGMAIL_FETCH_EMAILS\n{}", "x".repeat(2000));
        let input = json!({"tools": [
            {"tool_slug": "GMAIL_FETCH_EMAILS", "arguments": {"query": "is:unread"}},
            {"tool_slug": "GMAIL_SEND_EMAIL", "arguments": {"recipient_email": "bob@example.com", "subject": "Quarterly numbers", "body": body, "cc": ["ann@example.com"], "is_html": false}},
            {"tool_slug": "SLACK_SEND_MESSAGE", "arguments": {"channel": "#general", "markdown_text": "\u{202e}hello"}},
        ]});
        let got = guard_with_node(&json!([{"tool": EXECUTE, "input": input, "answer": true}]));
        let dialog = &got[0]["asked"][0];
        let message = dialog["message"].as_str().unwrap();
        eprintln!("{}\n{message}", dialog["title"]);
        assert!(dialog["title"].as_str().unwrap().contains("connected apps"), "{dialog}");
        let lines: Vec<&str> = message.lines().collect();
        assert!(lines.contains(&"GMAIL_SEND_EMAIL") && lines.contains(&"SLACK_SEND_MESSAGE"), "each write's slug on a line: {message}");
        assert!(!lines.contains(&"GMAIL_FETCH_EMAILS"), "the read isn't asked about, and the body can't fake a line: {message}");
        assert!(lines.contains(&"  recipient_email: bob@example.com"), "{message}");
        assert!(lines.contains(&"  subject: Quarterly numbers"), "{message}");
        assert!(lines.contains(&r#"  cc: ["ann@example.com"]"#), "{message}");
        let body_line = lines.iter().find(|l| l.starts_with("  body: Hi Bob, GMAIL_FETCH_EMAILS x")).unwrap_or_else(|| panic!("{message}"));
        assert!(body_line.ends_with('…') && body_line.chars().count() < 200, "cut short: {body_line}");
        assert!(lines.contains(&"  channel: #general") && lines.contains(&"  markdown_text: hello"), "no direction overrides: {message}");
        assert!(message.chars().count() < 1500, "{}", message.chars().count());
    }
}
