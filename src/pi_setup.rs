//! Ailfred's own pi: its config folder, home, working folder and sessions under
//! `~/Library/Application Support/com.ailfred`, so none of the user's own pi extensions,
//! skills, hooks or settings load. It shares the user's sign-ins (`auth.json` is a link to
//! theirs: pi writes and locks it through the link) and starts on their default model.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

/// Where Ailfred's pi keeps its things.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// pi's config folder (`PI_CODING_AGENT_DIR`).
    pub agent: PathBuf,
    /// Its `HOME`, so what pi writes on a first run stays out of the user's.
    pub home: PathBuf,
    /// Where it works (its current folder).
    pub workspace: PathBuf,
    pub sessions: PathBuf,
}

impl Layout {
    pub fn under(data: &Path) -> Self {
        Layout { agent: data.join("agent"), home: data.join("home"), workspace: data.join("workspace"), sessions: data.join("sessions") }
    }
}

/// The settings it takes from the user's pi: which model, and how hard it thinks.
const SEEDED: [&str; 3] = ["defaultProvider", "defaultModel", "defaultThinkingLevel"];

/// Makes `layout`'s folders, links the user's sign-ins from `user_agent` (their `~/.pi/agent`),
/// and, the first time, seeds its settings with their default model.
pub fn prepare(layout: &Layout, user_agent: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for dir in [&layout.agent, &layout.home, &layout.workspace, &layout.sessions] {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::set_permissions(&layout.home, std::fs::Permissions::from_mode(0o700))?;
    let auth = user_agent.join("auth.json");
    let link = layout.agent.join("auth.json");
    if auth.is_file() && std::fs::symlink_metadata(&link).is_err() {
        std::os::unix::fs::symlink(&auth, &link)?;
    }
    let own = layout.agent.join("settings.json");
    if !own.exists()
        && let Ok(text) = std::fs::read_to_string(user_agent.join("settings.json"))
        && let Ok(serde_json::Value::Object(theirs)) = serde_json::from_str(&text)
    {
        let seeded: serde_json::Map<String, serde_json::Value> = theirs.into_iter().filter(|(k, _)| SEEDED.contains(&k.as_str())).collect();
        if !seeded.is_empty() {
            std::fs::write(&own, serde_json::Value::Object(seeded).to_string())?;
        }
    }
    Ok(())
}

/// The command that starts pi (`pi`) in RPC mode with `layout`, its environment the app's
/// (`inherited`) without pi's own variables, `path` as its `PATH`, and the user's real home
/// (`real_home`) named to it.
pub fn command(pi: &Path, layout: &Layout, real_home: &Path, path: &OsString, inherited: impl IntoIterator<Item = (OsString, OsString)>) -> Command {
    let note = format!(
        "You are Ailfred, the user's personal assistant on their Mac. The user's home folder is {}: their own files are there. Use absolute paths for them.",
        real_home.display()
    );
    let mut command = Command::new(pi);
    command.arg("--mode").arg("rpc").arg("--session-dir").arg(&layout.sessions).arg("--append-system-prompt").arg(note).current_dir(&layout.workspace);
    let mut data_home = real_home.join(".local/share");
    for (key, value) in inherited {
        if key == "XDG_DATA_HOME" && !value.is_empty() {
            data_home = value.into();
        } else if key.to_string_lossy().starts_with("PI_") {
            command.env_remove(key);
        }
    }
    // pi's installer puts its node here and on no shell's PATH; its launcher finds it through
    // HOME or XDG_DATA_HOME, both Ailfred's for pi.
    let node = data_home.join("pi-node/current/bin");
    let path = if node.is_dir() {
        let mut joined = node.into_os_string();
        if !path.is_empty() {
            joined.push(":");
            joined.push(path);
        }
        joined
    } else {
        path.clone()
    };
    let home = &layout.home;
    command
        .env("PI_CODING_AGENT_DIR", &layout.agent)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("PATH", path);
    let git = real_home.join(".gitconfig");
    if git.is_file() {
        command.env("GIT_CONFIG_GLOBAL", git);
    }
    command
}

/// The MCP server computer control's tools are served as, for pi and Claude Code.
pub const COMPUTER_SERVER: &str = "computer";
/// The variable pi gets computer control's token in (its `mcp.json` names it, not the token).
const COMPUTER_TOKEN: &str = "AILFRED_COMPUTER_TOKEN";

/// Hands pi computer control's tools while `endpoint` serves them (Settings' switch is on), or
/// takes them away: the `computer` server in its `mcp.json` (its other servers stay), and the
/// token in `command`'s environment. Read when pi starts.
pub fn computer_tools(command: &mut Command, layout: &Layout, endpoint: Option<&crate::computer::server::Endpoint>) -> io::Result<()> {
    use serde_json::{Value, json};
    let path = layout.agent.join("mcp.json");
    let saved = std::fs::read_to_string(&path).ok();
    if saved.is_none() && endpoint.is_none() {
        // Off, with no file to take the server out of.
        command.env_remove(COMPUTER_TOKEN);
        return Ok(());
    }
    let mut config = saved.and_then(|text| serde_json::from_str::<Value>(&text).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    if !config["mcpServers"].is_object() {
        config["mcpServers"] = json!({});
    }
    let servers = config["mcpServers"].as_object_mut().expect("an object");
    match endpoint {
        Some(endpoint) => {
            servers.insert(
                COMPUTER_SERVER.into(),
                json!({
                    "url": endpoint.url,
                    "headers": {"Authorization": format!("Bearer ${{{COMPUTER_TOKEN}}}")},
                    "exposure": "direct",
                    "description": "See this Mac's screen and click, type and open apps in it, behind the user's approval.",
                }),
            );
            command.env(COMPUTER_TOKEN, &endpoint.token);
        }
        None => {
            servers.remove(COMPUTER_SERVER);
            command.env_remove(COMPUTER_TOKEN);
        }
    }
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap_or_default())
}

/// The text after `marker` in a shell's output, up to the next `marker` (an interactive shell
/// may print around it).
pub fn marked(output: &str, marker: &str) -> Option<String> {
    let (_, after) = output.split_once(marker)?;
    let (inside, _) = after.split_once(marker)?;
    (!inside.is_empty()).then(|| inside.to_owned())
}

/// pi itself: `$AILFRED_PI`, else the managed install's `~/.pi/agent/bin/pi` when it's there.
pub fn pi_binary(real_home: &Path) -> Option<PathBuf> {
    if let Some(pi) = std::env::var_os("AILFRED_PI") {
        return Some(pi.into());
    }
    let managed = real_home.join(".pi/agent/bin/pi");
    managed.is_file().then_some(managed)
}

/// Ailfred's pi, ready to talk to, kept running: its folders under `data`, the user's sign-ins
/// and default model from `real_home`'s `~/.pi/agent`. Runs their login shell once (for
/// `PATH`), so not on the main thread.
pub fn start(pi: &Path, data: &Path, real_home: &Path) -> io::Result<crate::pi::PiEngine> {
    let layout = Layout::under(data);
    prepare(&layout, &real_home.join(".pi/agent"))?;
    let path = login_path(&std::env::var_os("PATH").unwrap_or_default());
    let (pi, real_home) = (pi.to_owned(), real_home.to_owned());
    let make = move || {
        let mut command = command(&pi, &layout, &real_home, &path, std::env::vars_os());
        // As computer control is now: on, its tools; off, none.
        if let Err(e) = computer_tools(&mut command, &layout, crate::computer::endpoint_now().as_ref()) {
            eprintln!("Ailfred: pi's computer tools couldn't be set up: {e}");
        }
        command
    };
    crate::pi::PiEngine::supervised(make, crate::pi::SHUTDOWN_GRACE)
}

/// Ailfred's pi as a [`Model`](crate::model::Model): started off the main thread; a message
/// sent meanwhile waits for it.
pub struct Pi {
    state: Arc<Mutex<PiState>>,
}

enum PiState {
    /// With the messages waiting for it, in order (the first one's run takes the rest as
    /// steers).
    Starting(Vec<(crate::pi::Prompt, crate::pi::Sink)>),
    Up(crate::pi::PiEngine),
    Failed(String),
}

impl PiState {
    /// Up, or failed to start: the messages waiting go to pi, or fail.
    fn became(&mut self, started: Result<crate::pi::PiEngine, String>) {
        let waiting = match self {
            PiState::Starting(waiting) => std::mem::take(waiting),
            _ => Vec::new(),
        };
        *self = match started {
            Ok(engine) => PiState::Up(engine),
            Err(e) => PiState::Failed(format!("pi couldn't start: {e}")),
        };
        for (prompt, sink) in waiting {
            match self {
                PiState::Up(engine) => engine.send(prompt, sink),
                PiState::Failed(why) => sink(crate::reply::Event::Fail(why.clone())),
                PiState::Starting(_) => {}
            }
        }
    }
}

impl Pi {
    /// Starts pi (`pi_binary`) with its folders under `data` and the user's setup in `real_home`.
    pub fn start(pi: Option<PathBuf>, data: PathBuf, real_home: PathBuf) -> Pi {
        let Some(pi) = pi else {
            return Pi::with(PiState::Failed("pi isn't installed (~/.pi/agent/bin/pi, or set AILFRED_PI).".into()));
        };
        let pi_ = Pi::with(PiState::Starting(Vec::new()));
        let state = pi_.state.clone();
        std::thread::spawn(move || {
            let started = start(&pi, &data, &real_home).map_err(|e| {
                eprintln!("Ailfred: pi couldn't start: {e}");
                e.to_string()
            });
            state.lock().unwrap_or_else(|e| e.into_inner()).became(started);
        });
        pi_
    }

    fn with(state: PiState) -> Pi {
        Pi { state: Arc::new(Mutex::new(state)) }
    }

    #[cfg(test)]
    fn became(&mut self, started: Result<crate::pi::PiEngine, String>) {
        self.state.lock().unwrap().became(started);
    }

    /// It isn't up yet: a message waits for it.
    pub fn starting(&self) -> bool {
        matches!(*self.state.lock().unwrap_or_else(|e| e.into_inner()), PiState::Starting(_))
    }

    /// It couldn't start: the host starts it again for the next message.
    pub fn failed(&self) -> bool {
        matches!(*self.state.lock().unwrap_or_else(|e| e.into_inner()), PiState::Failed(_))
    }
}

impl crate::model::Model for Pi {
    fn send(&mut self, request: crate::model::Request, sink: crate::model::Sink) {
        use crate::model::Item;
        use crate::reply::Event;
        let context = crate::pi::context(&request.history);
        let prompt = crate::pi::Prompt { text: request.text, pngs: request.pngs, answer: request.answer, steer: request.steer, context };
        let sink: crate::pi::Sink = Arc::new(move |event| sink(Item::Event(event)));
        match &mut *self.state.lock().unwrap_or_else(|e| e.into_inner()) {
            PiState::Up(engine) => engine.send(prompt, sink),
            PiState::Starting(waiting) => waiting.push((prompt, sink)),
            PiState::Failed(why) => sink(Event::Fail(why.clone())),
        }
    }

    fn stop(&mut self) {
        match &mut *self.state.lock().unwrap_or_else(|e| e.into_inner()) {
            PiState::Up(engine) => engine.stop(),
            PiState::Starting(waiting) => waiting.clear(),
            PiState::Failed(_) => {}
        }
    }

    fn new_conversation(&mut self) {
        if let PiState::Up(engine) = &mut *self.state.lock().unwrap_or_else(|e| e.into_inner()) {
            engine.new_conversation();
        }
    }
}

const MARKER: &str = "__AILFRED_PATH__";

/// The `PATH` the user's login shell sets up (where nvm or Homebrew put `node`), joined before
/// the app's own: an app opened from Finder gets only the system's. Runs their shell once.
pub fn login_path(app_path: &OsString) -> OsString {
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    login_path_with(Path::new(&shell), app_path, LOGIN_SHELL_WAIT)
}

/// How long the user's login shell gets to tell its `PATH`.
const LOGIN_SHELL_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// [`login_path`] with `shell`, given up on after `wait`.
fn login_path_with(shell: &Path, app_path: &OsString, wait: std::time::Duration) -> OsString {
    use std::io::Read;
    use std::process::Stdio;
    let script = format!("printf '{MARKER}%s{MARKER}' \"$PATH\"");
    let theirs = Command::new(shell).arg("-ilc").arg(script).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok().and_then(|mut child| {
        let mut stdout = child.stdout.take()?;
        let (tx, rx) = std::sync::mpsc::channel();
        // Read on a thread of its own: a job the shell started may keep its stdout open.
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stdout.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&chunk[..n]);
                if marked(&String::from_utf8_lossy(&out), MARKER).is_some() {
                    break;
                }
            }
            let _ = tx.send(out);
        });
        let out = rx.recv_timeout(wait).ok();
        let _ = child.kill();
        let _ = child.wait();
        out.and_then(|out| marked(&String::from_utf8_lossy(&out), MARKER))
    });
    match theirs {
        Some(theirs) if app_path.is_empty() => theirs.into(),
        Some(theirs) => format!("{theirs}:{}", app_path.to_string_lossy()).into(),
        None => app_path.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// A fresh folder for one test.
    fn scratch(name: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!("ailfred-setup-{}-{name}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn settings(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn its_folders_are_made_the_sign_ins_linked_and_the_model_seeded_once() {
        let root = scratch("seed");
        let user = root.join("user-agent");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("auth.json"), "{\"x\":{}}").unwrap();
        std::fs::write(
            user.join("settings.json"),
            r#"{"defaultProvider":"anthropic","defaultModel":"m1","defaultThinkingLevel":"high","packages":["npm:x"],"extensions":["/a.ts"]}"#,
        )
        .unwrap();
        let layout = Layout::under(&root.join("data"));
        prepare(&layout, &user).unwrap();
        for dir in [&layout.agent, &layout.home, &layout.workspace, &layout.sessions] {
            assert!(dir.is_dir(), "{dir:?}");
        }
        assert_eq!(std::fs::metadata(&layout.home).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::read_link(layout.agent.join("auth.json")).unwrap(), user.join("auth.json"));
        assert_eq!(settings(&layout.agent.join("settings.json")), serde_json::json!({"defaultProvider":"anthropic","defaultModel":"m1","defaultThinkingLevel":"high"}));

        // Its own model choice later is kept.
        std::fs::write(layout.agent.join("settings.json"), r#"{"defaultModel":"m2"}"#).unwrap();
        prepare(&layout, &user).unwrap();
        assert_eq!(settings(&layout.agent.join("settings.json"))["defaultModel"], "m2");
        assert!(std::fs::symlink_metadata(layout.agent.join("auth.json")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn without_a_pi_setup_of_the_users_it_starts_empty() {
        let root = scratch("empty");
        let layout = Layout::under(&root.join("data"));
        prepare(&layout, &root.join("nowhere")).unwrap();
        assert!(layout.agent.is_dir());
        assert!(std::fs::symlink_metadata(layout.agent.join("auth.json")).is_err());
        assert!(!layout.agent.join("settings.json").exists());
    }

    fn env_of(command: &Command) -> Vec<(String, Option<String>)> {
        command.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect()
    }

    #[test]
    fn pi_runs_in_rpc_mode_with_its_own_folders_and_none_of_pis_inherited_variables() {
        let layout = Layout::under(Path::new("/data"));
        let inherited = [("PI_SESSION_ID", "s1"), ("PI_CODING_AGENT_DIR", "/Users/me/.pi/agent"), ("LANG", "en_US.UTF-8")].map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let command = command(Path::new("/bin/pi"), &layout, Path::new("/Users/me"), &OsString::from("/opt/node/bin:/usr/bin"), inherited);
        let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(&args[..4], ["--mode", "rpc", "--session-dir", "/data/sessions"]);
        let at = args.iter().position(|a| a == "--append-system-prompt").expect("a note for the model");
        assert!(args[at + 1].contains("/Users/me"), "{}", args[at + 1]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/data/workspace")));
        let env = env_of(&command);
        let set = |k: &str| env.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.clone());
        assert_eq!(set("PI_CODING_AGENT_DIR").as_deref(), Some("/data/agent"));
        assert_eq!(set("HOME").as_deref(), Some("/data/home"));
        assert_eq!(set("XDG_CONFIG_HOME").as_deref(), Some("/data/home/.config"));
        assert_eq!(set("XDG_DATA_HOME").as_deref(), Some("/data/home/.local/share"));
        assert_eq!(set("PATH").as_deref(), Some("/opt/node/bin:/usr/bin"));
        assert!(env.contains(&("PI_SESSION_ID".to_owned(), None)), "inherited PI_ variables removed: {env:?}");
        assert!(!env.iter().any(|(k, _)| k == "LANG"), "the rest is inherited as is");
    }

    #[test]
    fn the_users_git_identity_follows_when_they_have_one() {
        let root = scratch("git");
        std::fs::write(root.join(".gitconfig"), "[user]\n").unwrap();
        let layout = Layout::under(&root.join("data"));
        let with = command(Path::new("pi"), &layout, &root, &OsString::new(), []);
        let git = env_of(&with).into_iter().find(|(k, _)| k == "GIT_CONFIG_GLOBAL").and_then(|(_, v)| v);
        assert_eq!(git, Some(root.join(".gitconfig").to_string_lossy().into_owned()));
        let without = command(Path::new("pi"), &layout, &root.join("nobody"), &OsString::new(), []);
        assert!(!env_of(&without).iter().any(|(k, _)| k == "GIT_CONFIG_GLOBAL"));
    }

    #[test]
    fn pis_own_node_is_on_its_path() {
        // pi's installer puts node here and adds it to no shell profile; its launcher looks
        // for it under HOME, which is Ailfred's for pi.
        let root = scratch("node");
        std::fs::create_dir_all(root.join(".local/share/pi-node/current/bin")).unwrap();
        let layout = Layout::under(&root.join("data"));
        let with = command(Path::new("pi"), &layout, &root, &OsString::from("/usr/bin"), []);
        let path = env_of(&with).into_iter().find(|(k, _)| k == "PATH").and_then(|(_, v)| v);
        assert_eq!(path, Some(format!("{}:/usr/bin", root.join(".local/share/pi-node/current/bin").display())));
    }

    #[test]
    fn pi_gets_the_computer_tools_only_while_computer_control_is_on_and_its_other_servers_stay() {
        let root = scratch("computer");
        let layout = Layout::under(&root.join("data"));
        std::fs::create_dir_all(&layout.agent).unwrap();
        std::fs::write(layout.agent.join("mcp.json"), r#"{"mcpServers":{"mine":{"command":"x"}},"other":1}"#).unwrap();
        let endpoint = crate::computer::server::Endpoint { url: "http://127.0.0.1:4321/mcp".into(), token: "tok".into() };
        let mut on = command(Path::new("pi"), &layout, &root, &OsString::new(), []);
        computer_tools(&mut on, &layout, Some(&endpoint)).unwrap();
        let config = settings(&layout.agent.join("mcp.json"));
        assert_eq!(config["mcpServers"]["mine"], serde_json::json!({"command": "x"}), "the user's server stays");
        assert_eq!(config["other"], 1);
        let computer = &config["mcpServers"]["computer"];
        assert_eq!(computer["url"], "http://127.0.0.1:4321/mcp");
        assert_eq!(computer["headers"]["Authorization"], "Bearer ${AILFRED_COMPUTER_TOKEN}", "the token itself isn't written down");
        assert_eq!(computer["exposure"], "direct", "the model sees the tools without searching");
        let token = env_of(&on).into_iter().find(|(k, _)| k == "AILFRED_COMPUTER_TOKEN").and_then(|(_, v)| v);
        assert_eq!(token.as_deref(), Some("tok"));

        let mut off = command(Path::new("pi"), &layout, &root, &OsString::new(), [(OsString::from("AILFRED_COMPUTER_TOKEN"), OsString::from("old"))]);
        computer_tools(&mut off, &layout, None).unwrap();
        let config = settings(&layout.agent.join("mcp.json"));
        assert!(config["mcpServers"].get("computer").is_none(), "{config}");
        assert_eq!(config["mcpServers"]["mine"], serde_json::json!({"command": "x"}));
        assert!(env_of(&off).contains(&("AILFRED_COMPUTER_TOKEN".to_owned(), None)), "no token reaches pi");
    }

    /// pi itself connects to the tools as Ailfred hands them to it, and lists them (no model is
    /// asked; the Mac is a fake): `cargo test pi_lists_the_computer_tools -- --ignored --nocapture`.
    #[test]
    #[ignore = "runs the installed pi"]
    fn pi_lists_the_computer_tools() {
        use crate::computer::{control::Control, fake, mcp::Helper, server};
        use std::sync::Arc;
        let real_home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let pi = pi_binary(&real_home).expect("pi installed");
        let token = server::new_token().unwrap();
        let control = Arc::new(Control::new(|_| {}));
        control.set_enabled(true);
        let desktop = Arc::new(fake::FakeDesktop::default());
        let dialogs = Arc::new(fake::FakeDialogs::new(true, true));
        let helper = Helper::new(desktop, dialogs, Arc::new(fake::FakeClock::new(0)), control, token.clone(), 1);
        let endpoint = server::start(Arc::new(helper), token).unwrap();
        let layout = Layout::under(&scratch("pi-mcp").join("data"));
        prepare(&layout, &real_home.join(".pi/agent")).unwrap();
        let mut list = command(&pi, &layout, &real_home, &login_path(&std::env::var_os("PATH").unwrap_or_default()), std::env::vars_os());
        computer_tools(&mut list, &layout, Some(&endpoint)).unwrap();
        // `pi mcp list` instead of RPC mode: the same folders, environment and mcp.json.
        let mut cmd = Command::new(list.get_program());
        cmd.args(["mcp", "list"]).current_dir(list.get_current_dir().unwrap());
        for (k, v) in list.get_envs() {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
        let out = cmd.output().unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        eprintln!("{said}");
        for tool in ["screenshot", "click", "type", "batch"] {
            assert!(said.contains(tool), "{tool} listed");
        }
    }

    #[test]
    fn without_an_mcp_file_off_writes_none_and_on_makes_one() {
        let root = scratch("computer-fresh");
        let layout = Layout::under(&root.join("data"));
        std::fs::create_dir_all(&layout.agent).unwrap();
        let mut off = command(Path::new("pi"), &layout, &root, &OsString::new(), []);
        computer_tools(&mut off, &layout, None).unwrap();
        assert!(!layout.agent.join("mcp.json").exists());
        let endpoint = crate::computer::server::Endpoint { url: "http://127.0.0.1:1/mcp".into(), token: "t".into() };
        computer_tools(&mut off, &layout, Some(&endpoint)).unwrap();
        assert_eq!(settings(&layout.agent.join("mcp.json"))["mcpServers"]["computer"]["url"], "http://127.0.0.1:1/mcp");
    }

    #[test]
    fn a_login_shell_that_hangs_is_given_up_on() {
        let root = scratch("shell");
        let shell = root.join("slow-shell");
        std::fs::write(&shell, "#!/bin/sh\nsleep 10\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        let path = login_path_with(&shell, &OsString::from("/usr/bin"), std::time::Duration::from_millis(500));
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());
        assert_eq!(path, OsString::from("/usr/bin"));
    }

    #[test]
    fn without_pi_a_message_fails_with_why() {
        use crate::model::{Item, Model};
        let mut pi = Pi::start(None, PathBuf::new(), PathBuf::new());
        let why = "pi isn't installed (~/.pi/agent/bin/pi, or set AILFRED_PI).";
        {
            let (s, rx) = sink();
            pi.send(pi_request("hi"), s);
            assert_eq!(rx.try_recv(), Ok(Item::Event(crate::reply::Event::Fail(why.into()))));
            pi.stop();
            pi.new_conversation();
        }
    }

    fn sink() -> (crate::model::Sink, std::sync::mpsc::Receiver<crate::model::Item>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |item| drop(tx.lock().unwrap().send(item))), rx)
    }

    fn pi_request(text: &str) -> crate::model::Request {
        crate::model::Request { model: crate::model::PI.into(), history: Vec::new(), text: text.into(), pngs: Vec::new(), session: None, answer: false, steer: false }
    }

    #[test]
    fn a_message_sent_while_pi_starts_goes_to_it_once_its_up() {
        use crate::model::{Item, Model};
        use crate::reply::Event;
        let mut pi = Pi::with(PiState::Starting(Vec::new()));
        let (s, rx) = sink();
        pi.send(pi_request("hi"), s);
        assert!(rx.try_recv().is_err(), "it waits");
        let reply = r#"{"type":"response","command":"prompt","success":true,"data":{"disposition":"started"}}"#;
        let delta = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Up"}}"#;
        let mut fake = Command::new("/bin/sh");
        fake.arg("-c").arg(format!("read line; printf '%s\\n' '{reply}' '{delta}' '{{\"type\":\"agent_settled\"}}'; cat > /dev/null"));
        pi.became(crate::pi::PiEngine::spawn(fake).map_err(|e| e.to_string()));
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(Item::Event(Event::Text("Up".into()))));

        // Stopped while it starts: nothing goes; failing to start fails the one waiting.
        let mut pi = Pi::with(PiState::Starting(Vec::new()));
        let (s, rx) = sink();
        pi.send(pi_request("never"), s);
        pi.stop();
        pi.became(Err("no node".into()));
        assert!(rx.try_recv().is_err());
        let mut pi = Pi::with(PiState::Starting(Vec::new()));
        let (s, rx) = sink();
        pi.send(pi_request("hi"), s);
        pi.became(Err("no node".into()));
        assert_eq!(rx.try_recv(), Ok(Item::Event(Event::Fail("pi couldn't start: no node".into()))));
        assert!(pi.failed(), "so the host starts it again at the next message");
    }

    #[test]
    fn every_message_sent_while_pi_starts_waits_for_it() {
        use crate::model::{Item, Model};
        use crate::reply::Event;
        let mut pi = Pi::with(PiState::Starting(Vec::new()));
        let (s1, rx1) = sink();
        pi.send(pi_request("hi"), s1);
        let (s2, rx2) = sink();
        pi.send(crate::model::Request { steer: true, ..pi_request("and this") }, s2);
        pi.became(Err("no node".into()));
        let failed = Item::Event(Event::Fail("pi couldn't start: no node".into()));
        assert_eq!((rx1.try_recv(), rx2.try_recv()), (Ok(failed.clone()), Ok(failed)));
    }

    #[test]
    fn the_login_shells_path_is_read_between_markers() {
        let m = "__AILFRED__";
        assert_eq!(marked("motd\n__AILFRED__/a:/b__AILFRED__\nbye", m).as_deref(), Some("/a:/b"));
        assert_eq!(marked("no markers", m), None);
        assert_eq!(marked("__AILFRED____AILFRED__", m), None, "an empty PATH is no PATH");
    }
}
