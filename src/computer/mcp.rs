//! The MCP endpoint: JSON-RPC 2.0 with `initialize`, `tools/list`, and `tools/call`, behind a
//! per-launch bearer token. It knows nothing of the transport: the HTTP server hands it each
//! request (`HttpRequest`) and sends back its reply (`HttpReply`). Each call passes the guards in
//! order: permissions, session grant, rate limit, blocklist, risky-action confirmation; then it
//! acts and returns a fresh screenshot. The rate limit counts a call when its turn to act comes
//! (calls run one at a time), so calls refused before it, or queued behind a slow one, don't
//! use up the window while nothing runs.
//!
//! `batch` runs up to 20 actions in one call. Each step takes the single tool's path (`perform`)
//! and counts as one action for the rate limit; a full window delays the step instead of refusing
//! it. Coordinates refer to the screenshot taken before the batch, and one screenshot follows it.
//! The first refused, declined, or failed step stops the batch.

use super::blocklist::{AppIdentity, blocked_name, blocked_reason};
use super::control::Control;
use super::coords::{Point, ScreenshotGeometry};
use super::keys::{KeyCombo, parse_combo};
use super::rate::{MAX_ACTIONS_PER_SECOND, RateLimiter};
use super::risk::{Act, assess};
use super::{Clock, Desktop, Dialogs, InputEvent, MouseButton};
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Supported MCP revisions, newest first; the newest answers an unknown request.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// Time for the screen to settle before the screenshot that follows an action.
const SETTLE_MS: u64 = 150;
/// After a confirmation, the previous app gets up to RESTORE_TRIES × RESTORE_STEP_MS to be back in front.
const RESTORE_TRIES: u32 = 10;
const RESTORE_STEP_MS: u64 = 50;
const OPEN_APP_SETTLE_MS: u64 = 1500;
const MAX_BATCH_STEPS: usize = 20;
/// The tools a batch step may use: actions only (a batch returns its own screenshot).
const BATCH_TOOLS: &[&str] = &["click", "move", "drag", "scroll", "type", "key", "open_app", "wait"];
/// Between batch steps, so each step's guards read the focus and windows the previous step left.
const STEP_PAUSE_MS: u64 = 100;
const MAX_WAIT_MS: u64 = 5000;
const WAIT_SLICE_MS: u64 = 100;
/// macOS posts at most 20 UTF-16 units of text per keyboard event.
const TEXT_CHUNK_UNITS: usize = 20;

const STOPPED: &str = "Computer control was stopped; this action was dropped.";
const NOT_ALLOWED: &str = "Computer control was not allowed by the user. Nothing was done.";
/// Settings' switch is off: no tool runs, and no session is asked for.
const OFF: &str = "Computer control is off. The user can turn it on in Ailfred's Settings. Nothing was done.";
const NEEDS_PERMISSION: &str = "Ailfred needs the";
const GRANT_IT: &str = "Ask the user to grant it in Ailfred's Settings → Permissions.";
const NEEDS_SCREENSHOT: &str = "Take a screenshot first: coordinates are pixels of the latest screenshot.";
const UNKNOWN_TARGET: &str = "Cannot tell which app is under that point, so the action was refused. \
Take a new screenshot and try again.";

const INSTRUCTIONS: &str = "Controls this Mac. Take a screenshot first; x and y are pixels of the latest \
screenshot. Every action returns a fresh screenshot. Use `batch` for a predictable sequence (click a field, \
type, press tab, type, press return) so it runs in one call; end the batch whenever you must see the screen \
before deciding the next step. The user must allow a session, risky actions ask the user to confirm, and \
some apps (System Settings, password managers) are off limits.";

/// Why computer control is unavailable when the system cannot take its screenshots.
const SCREENSHOTS_UNSUPPORTED: &str = "Screenshots need macOS 14 or later, so computer control is unavailable on this Mac.";

/// Why typing stopped when the app receiving the keys changed, once part of the text was sent and
/// before any of it was.
const KEYS_CHANGED_WHILE_TYPING: &str = "The app receiving the keys changed while typing, so the rest was not sent.";
const KEYS_CHANGED_BEFORE_TYPING: &str = "The app receiving the keys changed before typing, so nothing was sent.";
/// Why a key was not sent when the app receiving it changed after it was checked.
const KEY_RECIPIENT_CHANGED: &str = "The app receiving the keys changed before the key was sent, so it was not sent.";

const KEY_DESCRIPTION: &str = "Press a key combo in the frontmost app, for example `shift+cmd+t`, `return`, or `esc`.";
const OPEN_APP_DESCRIPTION: &str = "Open or bring forward an app by name or bundle id.";
/// What an `open_app` confirmation names as its target.
const SYSTEM: &str = "macOS";

pub struct HttpRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub authorization: Option<&'a str>,
    pub body: &'a str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpReply {
    pub status: u16,
    pub body: Option<Value>,
}

enum Tool {
    Screenshot,
    Click { x: f64, y: f64, button: MouseButton, count: u8 },
    Move { x: f64, y: f64 },
    Drag { from: (f64, f64), to: (f64, f64) },
    Scroll { x: f64, y: f64, dx: i32, dy: i32 },
    Type { text: String },
    Key { combo: KeyCombo },
    OpenApp { name: String },
    ListApps,
    Wait { ms: u64 },
    /// Steps with their tool names, checked one by one when they run.
    Batch { steps: Vec<(&'static str, Call)> },
}

struct Call {
    tool: Tool,
    intent: Option<String>,
}

/// The apps approved to receive keys: the front app, and the app owning the focused element when
/// it is a different one (a non-activating panel). Every key and text chunk goes only to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Recipient {
    front_pid: i32,
    focused_pid: Option<i32>,
}

type Content = Vec<Value>;

/// A tool error; its content is a message, or what a batch did before it stopped.
struct Failed(Content);

impl From<String> for Failed {
    fn from(message: String) -> Self {
        Failed(vec![text(message)])
    }
}

impl From<&str> for Failed {
    fn from(message: &str) -> Self {
        Failed(vec![text(message)])
    }
}

fn text(text: impl Into<String>) -> Value {
    json!({"type": "text", "text": text.into()})
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn rpc_reply(status: u16, id: Value, outcome: Result<Value, (i64, String)>) -> HttpReply {
    let body = match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    };
    HttpReply { status, body: Some(body) }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    let mut properties = properties.as_object().cloned().unwrap_or_default();
    properties.insert(
        "intent".into(),
        json!({"type": "string", "description": "What this step is for, in a few words. Shown to the user when the action needs a confirmation."}),
    );
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn coordinate(description: &str) -> Value {
    json!({"type": "number", "minimum": 0, "description": description})
}

/// The tool catalog.
fn tools() -> Value {
    let x = coordinate("Pixels from the left edge of the latest screenshot.");
    let y = coordinate("Pixels from the top edge of the latest screenshot.");
    json!([
        {"name": "screenshot", "description": "Take a screenshot of the display under the pointer. Returns the image and its size.",
         "inputSchema": schema(json!({}), &[])},
        {"name": "click", "description": "Click at a point of the latest screenshot.",
         "inputSchema": schema(json!({"x": x, "y": y,
            "button": {"type": "string", "enum": ["left", "right"], "default": "left"},
            "count": {"type": "integer", "enum": [1, 2, 3], "default": 1, "description": "2 for a double click, 3 for a triple click."}}), &["x", "y"])},
        {"name": "move", "description": "Move the pointer to a point of the latest screenshot.",
         "inputSchema": schema(json!({"x": x, "y": y}), &["x", "y"])},
        {"name": "drag", "description": "Press at one point, move to another, and release.",
         "inputSchema": schema(json!({"fromX": x, "fromY": y, "toX": x, "toY": y}), &["fromX", "fromY", "toX", "toY"])},
        {"name": "scroll", "description": "Scroll at a point. dy > 0 scrolls down and dx > 0 scrolls right, in pixels.",
         "inputSchema": schema(json!({"x": x, "y": y, "dx": {"type": "integer"}, "dy": {"type": "integer"}}), &["x", "y", "dx", "dy"])},
        {"name": "type", "description": "Type text into the frontmost app. A newline presses Return.",
         "inputSchema": schema(json!({"text": {"type": "string"}}), &["text"])},
        {"name": "key", "description": KEY_DESCRIPTION,
         "inputSchema": schema(json!({"combo": {"type": "string"}}), &["combo"])},
        {"name": "open_app", "description": OPEN_APP_DESCRIPTION,
         "inputSchema": schema(json!({"name": {"type": "string"}}), &["name"])},
        {"name": "list_apps", "description": "List the running apps.",
         "inputSchema": schema(json!({}), &[])},
        {"name": "wait", "description": "Wait, then take a screenshot.",
         "inputSchema": schema(json!({"ms": {"type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS}}), &["ms"])},
        {"name": "batch", "description": "Run 1 to 20 actions in order in one call, then return one screenshot. Use it for \
predictable sequences, for example type, `tab`, type; or click a field, type, `return`. Coordinates are pixels of \
the screenshot taken before the batch, so batch only steps whose targets you can already see; when a step's result \
decides what comes next, end the batch there and look at the screenshot it returns. Each step is checked like a \
single call; the batch stops at the first refused, declined, or failed step and says which steps ran.",
         "inputSchema": schema(json!({"steps": {"type": "array", "minItems": 1, "maxItems": MAX_BATCH_STEPS,
            "items": {"type": "object", "description": "A tool name plus that tool's arguments, for example {\"tool\": \"type\", \"text\": \"hello\"}.",
                "properties": {"tool": {"type": "string", "enum": BATCH_TOOLS}}, "required": ["tool"]}}}), &["steps"])}
    ])
}

fn number(args: &Map<String, Value>, key: &str) -> Result<f64, String> {
    args.get(key).and_then(Value::as_f64).ok_or_else(|| format!("`{key}` must be a number"))
}

fn integer(args: &Map<String, Value>, key: &str) -> Result<i64, String> {
    let value = number(args, key)?;
    if value.fract() != 0.0 || value.abs() > 1e9 {
        return Err(format!("`{key}` must be a whole number"));
    }
    Ok(value as i64)
}

fn string(args: &Map<String, Value>, key: &str) -> Result<String, String> {
    args.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("`{key}` must be a string"))
}

/// The call for `name`, `None` for an unknown tool, or why the arguments are invalid.
fn parse_call(name: &str, args: &Value) -> Option<Result<Call, String>> {
    let empty = Map::new();
    let args = args.as_object().unwrap_or(&empty);
    let point = |kx: &str, ky: &str| -> Result<(f64, f64), String> { Ok((number(args, kx)?, number(args, ky)?)) };
    let tool = || -> Result<Tool, String> {
        Ok(match name {
            "screenshot" => Tool::Screenshot,
            "click" => {
                let (x, y) = point("x", "y")?;
                let button = match args.get("button").map(|b| b.as_str()) {
                    None | Some(Some("left")) => MouseButton::Left,
                    Some(Some("right")) => MouseButton::Right,
                    _ => return Err("`button` must be \"left\" or \"right\"".into()),
                };
                let count = if args.contains_key("count") { integer(args, "count")? } else { 1 };
                if !(1..=3).contains(&count) {
                    return Err("`count` must be 1, 2, or 3".into());
                }
                Tool::Click { x, y, button, count: count as u8 }
            }
            "move" => {
                let (x, y) = point("x", "y")?;
                Tool::Move { x, y }
            }
            "drag" => Tool::Drag { from: point("fromX", "fromY")?, to: point("toX", "toY")? },
            "scroll" => {
                let (x, y) = point("x", "y")?;
                let delta = |key| integer(args, key).and_then(|v| i32::try_from(v).map_err(|e| e.to_string()));
                Tool::Scroll { x, y, dx: delta("dx")?, dy: delta("dy")? }
            }
            "type" => {
                let text = string(args, "text")?;
                // Newlines press Return (and are checked); tabs move the focus. Nothing else,
                // including Unicode line and paragraph separators that could act as Return.
                let control = |c: char| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}');
                if text.chars().any(|c| control(c) && !matches!(c, '\n' | '\r' | '\t')) {
                    return Err("`text` must not contain control characters other than newline and tab".into());
                }
                // The risk check sees the focus before the text; after a tab it may sit on a send button.
                if text.split_once('\t').is_some_and(|(_, rest)| rest.contains([' ', '\n', '\r'])) {
                    return Err("`text` must not type a space or a newline after a tab; send them in a separate call".into());
                }
                Tool::Type { text }
            }
            "key" => Tool::Key { combo: parse_combo(&string(args, "combo")?)? },
            "open_app" => {
                let name = string(args, "name")?.trim().to_string();
                if name.is_empty() {
                    return Err("`name` must not be empty".into());
                }
                Tool::OpenApp { name }
            }
            "list_apps" => Tool::ListApps,
            "batch" => Tool::Batch { steps: batch_steps(args)? },
            "wait" => {
                let ms = integer(args, "ms")?;
                if !(0..=MAX_WAIT_MS as i64).contains(&ms) {
                    return Err(format!("`ms` must be between 0 and {MAX_WAIT_MS}"));
                }
                Tool::Wait { ms: ms as u64 }
            }
            other => return Err(format!("unknown tool `{other}`")),
        })
    };
    const NAMES: &[&str] =
        &["screenshot", "click", "move", "drag", "scroll", "type", "key", "open_app", "list_apps", "wait", "batch"];
    if !NAMES.contains(&name) {
        return None;
    }
    let intent = args.get("intent").and_then(Value::as_str).map(str::to_string);
    Some(tool().map(|tool| Call { tool, intent }))
}

/// The steps of a `batch`, each parsed like a single call. Any invalid step refuses the batch.
fn batch_steps(args: &Map<String, Value>) -> Result<Vec<(&'static str, Call)>, String> {
    let steps = args.get("steps").and_then(Value::as_array).ok_or("`steps` must be a list of steps")?;
    if steps.is_empty() || steps.len() > MAX_BATCH_STEPS {
        return Err(format!("`steps` must hold 1 to {MAX_BATCH_STEPS} steps"));
    }
    let step = |n: usize, step: &Value| -> Result<(&'static str, Call), String> {
        let mut step = step.as_object().cloned().ok_or(format!("step {n} must be an object"))?;
        let name = step.remove("tool").and_then(|t| t.as_str().map(str::to_string));
        let name = name.ok_or(format!("step {n} needs a `tool`"))?;
        let tool = BATCH_TOOLS.iter().copied().find(|t| *t == name).ok_or_else(|| {
            format!("step {n}: `{name}` cannot be a batch step; use one of {}", BATCH_TOOLS.join(", "))
        })?;
        match parse_call(tool, &Value::Object(step)) {
            Some(Ok(call)) => Ok((tool, call)),
            Some(Err(error)) => Err(format!("step {n} ({tool}): {error}")),
            None => Err(format!("step {n}: unknown tool `{tool}`")),
        }
    };
    steps.iter().enumerate().map(|(i, s)| step(i + 1, s)).collect()
}

/// `text` on one line (control characters and line separators become spaces), cut to `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let flat: String = text.split(|c: char| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')).filter(|s| !s.trim().is_empty()).map(str::trim).collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &flat[..at]),
        None => flat,
    }
}

/// Splits text into chunks of at most `TEXT_CHUNK_UNITS` UTF-16 units, never inside a character.
fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if current.encode_utf16().count() + c.len_utf16() > TEXT_CHUNK_UNITS {
            out.push(std::mem::take(&mut current));
        }
        current.push(c);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

pub struct Helper {
    desktop: Arc<dyn Desktop>,
    dialogs: Arc<dyn Dialogs>,
    clock: Arc<dyn Clock>,
    control: Arc<Control>,
    token: String,
    own_pid: i32,
    limiter: Mutex<RateLimiter>,
    /// Serializes actions and holds the geometry of the latest screenshot.
    actions: Mutex<Option<ScreenshotGeometry>>,
    /// The app in front when the latest screenshot was taken: keys and text go only to it.
    seen_front: Mutex<Option<i32>>,
    /// Its server stopped (`retire`).
    retired: AtomicBool,
}

impl Helper {
    pub fn new(
        desktop: Arc<dyn Desktop>,
        dialogs: Arc<dyn Dialogs>,
        clock: Arc<dyn Clock>,
        control: Arc<Control>,
        token: String,
        own_pid: i32,
    ) -> Self {
        Helper {
            desktop,
            dialogs,
            clock,
            control,
            token,
            own_pid,
            limiter: Mutex::new(RateLimiter::new(MAX_ACTIONS_PER_SECOND, 1000)),
            actions: Mutex::new(None),
            seen_front: Mutex::new(None),
            retired: AtomicBool::new(false),
        }
    }

    /// Its server stopped: no call it let in does anything from now on, even once computer
    /// control is on again (another server, another helper).
    pub fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
    }

    pub fn authorized(&self, header: Option<&str>) -> bool {
        let Some((scheme, token)) = header.and_then(|h| h.trim().split_once(' ')) else {
            return false;
        };
        scheme.eq_ignore_ascii_case("bearer") && constant_time_eq(token.trim().as_bytes(), self.token.as_bytes())
    }

    pub fn handle(&self, request: &HttpRequest<'_>) -> HttpReply {
        if !self.authorized(request.authorization) {
            return rpc_reply(401, Value::Null, Err((-32001, "Unauthorized".into())));
        }
        if request.path != "/mcp" {
            return HttpReply { status: 404, body: None };
        }
        if request.method != "POST" {
            return HttpReply { status: 405, body: None };
        }
        let Ok(message) = serde_json::from_str::<Value>(request.body) else {
            return rpc_reply(400, Value::Null, Err((-32700, "Parse error".into())));
        };
        if !message.is_object() {
            return rpc_reply(400, Value::Null, Err((-32600, "Invalid request".into())));
        }
        // Notifications and responses get no reply.
        let (Some(method), Some(id)) = (message["method"].as_str(), message.get("id").cloned()) else {
            return HttpReply { status: 202, body: None };
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let outcome = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => self.tools_call(&params),
            _ => Err((-32601, format!("Method not found: {method}"))),
        };
        rpc_reply(200, id, outcome)
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params["protocolVersion"].as_str();
        let version = requested.filter(|v| PROTOCOL_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "ailfred-computer", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })
    }

    fn tools_call(&self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params["name"].as_str().ok_or((-32602, "Invalid params: `name` is required".to_string()))?;
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
        self.control.expire(self.clock.now_ms());
        let epoch = self.control.epoch();
        self.call_tool_from(epoch, name, &args).ok_or((-32602, format!("Unknown tool: {name}")))
    }

    /// A `tools/call` that arrived while the session was at `epoch`.
    fn call_tool_from(&self, epoch: u64, name: &str, args: &Value) -> Option<Value> {
        let outcome = parse_call(name, args)?.map_err(Failed::from).and_then(|call| self.run(epoch, call));
        Some(match outcome {
            Ok(content) => json!({"content": content}),
            Err(Failed(content)) => json!({"content": content, "isError": true}),
        })
    }

    /// Computer control is on, and this helper's server too. Read after the call's epoch: a
    /// server stops (`retire`) before switching off moves the epoch on.
    fn on(&self) -> bool {
        self.control.enabled() && !self.retired.load(Ordering::SeqCst)
    }

    fn run(&self, epoch: u64, call: Call) -> Result<Content, Failed> {
        if !self.on() {
            return Err(OFF.into());
        }
        self.check_permissions(&call.tool)?;
        let mut latest = lock(&self.actions);
        // Turned off while this call waited for the one before it.
        if !self.on() {
            return Err(OFF.into());
        }
        self.control.expire(self.clock.now_ms());
        if self.control.epoch() != epoch {
            return Err(STOPPED.into());
        }
        if !self.control.is_active(self.clock.now_ms()) {
            // The screenshot of an ended session no longer describes the screen.
            *latest = None;
            *lock(&self.seen_front) = None;
            if !self.with_dialog(|| self.dialogs.ask_grant()) {
                return Err(NOT_ALLOWED.into());
            }
            // Stop, the panic chord or the switch may have come while the user read the question.
            if !self.on() {
                return Err(OFF.into());
            }
            if self.control.epoch() != epoch {
                return Err(STOPPED.into());
            }
            self.control.grant(self.clock.now_ms());
        }
        // A batch counts each of its steps instead (`pace`).
        let counted = !matches!(call.tool, Tool::Batch { .. });
        if counted && !lock(&self.limiter).try_acquire(self.clock.now_ms()) {
            return Err(format!("Too many actions: at most {MAX_ACTIONS_PER_SECOND} per second. Wait a moment.").into());
        }
        self.act(epoch, &mut latest, call)
    }

    fn check_permissions(&self, tool: &Tool) -> Result<(), String> {
        if let Tool::Batch { steps } = tool {
            return steps.iter().try_for_each(|(_, step)| self.check_permissions(&step.tool));
        }
        let permissions = self.desktop.permissions();
        let sees = !matches!(tool, Tool::ListApps);
        let acts = matches!(
            tool,
            Tool::Click { .. }
                | Tool::Move { .. }
                | Tool::Drag { .. }
                | Tool::Scroll { .. }
                | Tool::Type { .. }
                | Tool::Key { .. }
        );
        if sees && !permissions.screenshots_supported {
            return Err(SCREENSHOTS_UNSUPPORTED.into());
        }
        if sees && !permissions.screen_recording {
            return Err(format!("{NEEDS_PERMISSION} Screen Recording permission. {GRANT_IT}"));
        }
        if acts && !permissions.accessibility {
            return Err(format!("{NEEDS_PERMISSION} Accessibility permission. {GRANT_IT}"));
        }
        Ok(())
    }

    /// Runs a dialog, then brings back the app that was in front (a dialog activates Ailfred).
    fn with_dialog(&self, dialog: impl FnOnce() -> bool) -> bool {
        let before = self.desktop.frontmost_app();
        let answer = dialog();
        if let Some(before) = before
            && self.desktop.frontmost_app().map(|app| app.pid) != Some(before.pid)
        {
            self.desktop.activate(before.pid);
        }
        answer
    }

    fn still_running(&self, epoch: u64) -> Result<(), String> {
        if self.retired.load(Ordering::SeqCst) || self.control.epoch() != epoch || !self.control.is_active(self.clock.now_ms()) {
            return Err(STOPPED.into());
        }
        Ok(())
    }

    fn post(&self, epoch: u64, event: InputEvent) -> Result<(), String> {
        self.still_running(epoch)?;
        self.desktop.post(&event)
    }

    fn refuse_blocked(&self, app: &AppIdentity) -> Result<(), String> {
        match blocked_reason(app, self.own_pid) {
            Some(label) => Err(format!("Ailfred does not control {label}. The action was refused.")),
            None => Ok(()),
        }
    }

    /// The app owning the window under `at`, refused when blocked or unknown.
    fn pointer_target(&self, at: Point) -> Result<AppIdentity, String> {
        let app = self.desktop.window_owner_at(at).ok_or(UNKNOWN_TARGET)?;
        self.refuse_blocked(&app).map(|()| app)
    }

    /// Right before a pointer post: the app under `at` is still `checked` (a window may have come
    /// over it while the user read a confirmation, or since the check).
    fn still_pointer_target(&self, at: Point, checked: &AppIdentity) -> Result<(), String> {
        let now = self.pointer_target(at)?;
        if now.pid != checked.pid {
            return Err(format!("The app under that point changed from {} to {}, so nothing was done. Take a new screenshot.", checked.name, now.name));
        }
        Ok(())
    }

    /// The frontmost app and the app owning the focused element (they differ for a
    /// non-activating panel), each refused when blocked. The one receiving the keys, for messages.
    /// `sent` says whether part of the input already landed, so a refusal never claims nothing was sent.
    fn keyboard_target(&self, sent: bool) -> Result<(String, Recipient), String> {
        let front = self.desktop.frontmost_app().ok_or("Cannot tell which app is in front.")?;
        self.refuse_blocked(&front)?;
        // The user may have switched apps since the agent last looked: it looks again first.
        if *lock(&self.seen_front) != Some(front.pid) {
            return Err(format!(
                "{} is in front now, not the app of the latest screenshot, so {}. Take a screenshot first.",
                front.name,
                if sent { "the rest was not sent" } else { "nothing was sent" }
            ));
        }
        let focused = self.desktop.focused_app().filter(|focused| focused.pid != front.pid);
        if let Some(focused) = &focused {
            self.refuse_blocked(focused)?;
        }
        let recipient = Recipient { front_pid: front.pid, focused_pid: focused.as_ref().map(|app| app.pid) };
        Ok((focused.map_or(front.name, |app| app.name), recipient))
    }

    fn confirm_if_risky(&self, risk: Option<String>, action: &str, target: &str) -> Result<(), String> {
        let Some(reason) = risk else {
            return Ok(());
        };
        // The reason quotes the model's intent and labels web pages set: one line each, cut short,
        // so they can't pose as the dialog's own text.
        let (target, reason) = (one_line(target, 80), one_line(&reason, 200));
        let message = format!("{action} in {target}.\n\nThis needs your OK because of {reason}.");
        let before = self.desktop.frontmost_app();
        if !self.with_dialog(|| self.dialogs.confirm(&message)) {
            return Err(format!("The user declined: {action} in {target}. Nothing was done."));
        }
        // The OK was for the app in front before the dialog; another app never gets the action.
        let Some(before) = before else {
            return Err(format!(
                "The user allowed {action} in {target}, but Ailfred cannot tell which app was in front before it asked, so nothing was done."
            ));
        };
        // The system hands the focus back a moment after the dialog closes.
        for _ in 0..RESTORE_TRIES {
            if self.desktop.frontmost_app().map(|app| app.pid) == Some(before.pid) {
                return Ok(());
            }
            self.desktop.activate(before.pid);
            self.clock.sleep_ms(RESTORE_STEP_MS);
        }
        if self.desktop.frontmost_app().map(|app| app.pid) == Some(before.pid) {
            return Ok(());
        }
        Err(format!(
            "The user allowed {action} in {target}, but {} could not be brought back to the front, so nothing was done.",
            before.name
        ))
    }

    fn screenshot(&self, latest: &mut Option<ScreenshotGeometry>) -> Result<Content, String> {
        let before = self.desktop.frontmost_app().map(|app| app.pid);
        let capture = self.desktop.capture()?;
        let geometry = capture.geometry;
        *latest = Some(geometry);
        let front = self.desktop.frontmost_app();
        // The image shows the app in front before and after it was taken; one that changed while
        // it was taken is not what the agent saw, so keys wait for the next screenshot.
        let seen = front.as_ref().map(|app| app.pid).filter(|pid| before == Some(*pid));
        *lock(&self.seen_front) = seen;
        let front = front.map(|app| app.name);
        let meta = json!({
            "imageWidth": geometry.image_width,
            "imageHeight": geometry.image_height,
            "scale": geometry.scale,
            "frontApp": front,
        });
        let data = crate::model::base64(&capture.data);
        Ok(vec![json!({"type": "image", "data": data, "mimeType": capture.mime_type}), text(meta.to_string())])
    }

    /// What was done, then a fresh screenshot once the screen settled.
    fn after(&self, latest: &mut Option<ScreenshotGeometry>, done: String, settle_ms: u64) -> Result<Content, String> {
        self.clock.sleep_ms(settle_ms);
        let mut content = vec![text(done)];
        match self.screenshot(latest) {
            Ok(blocks) => content.extend(blocks),
            Err(error) => content.push(text(format!("No screenshot: {error}"))),
        }
        Ok(content)
    }

    fn act(&self, epoch: u64, latest: &mut Option<ScreenshotGeometry>, call: Call) -> Result<Content, Failed> {
        match call.tool {
            Tool::Screenshot => Ok(self.screenshot(latest)?),
            Tool::ListApps => {
                let apps = self.desktop.list_apps()?;
                let apps: Vec<Value> = apps
                    .iter()
                    .map(|app| json!({"name": app.name, "bundleId": app.bundle_id, "active": app.active}))
                    .collect();
                Ok(vec![text(Value::Array(apps).to_string())])
            }
            Tool::Batch { steps } => self.batch(epoch, latest, steps, call.intent.as_deref()),
            tool => {
                let (done, settle_ms) = self.perform(epoch, latest, tool, call.intent.as_deref())?;
                Ok(self.after(latest, done, settle_ms)?)
            }
        }
    }

    /// Runs a batch's steps in order, then one screenshot. On the first failure it stops and
    /// reports the steps that ran, with a fresh screenshot while the session lasts.
    fn batch(
        &self,
        epoch: u64,
        latest: &mut Option<ScreenshotGeometry>,
        steps: Vec<(&'static str, Call)>,
        intent: Option<&str>,
    ) -> Result<Content, Failed> {
        let total = steps.len();
        let mut done: Vec<String> = Vec::new();
        let mut settle_ms = 0;
        for (i, (name, step)) in steps.into_iter().enumerate() {
            if i > 0 {
                // An app that is opening needs its full wait before anything targets it.
                self.clock.sleep_ms(if settle_ms == OPEN_APP_SETTLE_MS { settle_ms } else { STEP_PAUSE_MS });
            }
            // The batch's intent applies to every step, on top of the step's own.
            let intents: Vec<&str> = [intent, step.intent.as_deref()].into_iter().flatten().collect();
            let intents = (!intents.is_empty()).then(|| intents.join("; "));
            let outcome = self.pace(epoch).and_then(|()| self.perform(epoch, latest, step.tool, intents.as_deref()));
            match outcome {
                Ok((message, step_settle_ms)) => {
                    done.push(format!("{}. {message}", i + 1));
                    settle_ms = step_settle_ms;
                }
                Err(error) => {
                    let ran = done.len();
                    let at = i + 1;
                    let mut message = format!("Stopped at step {at} of {total} ({name}): {error}\nRan {ran} of {total} steps");
                    message.push_str(if ran == 0 { "." } else { ":\n" });
                    message.push_str(&done.join("\n"));
                    if self.still_running(epoch).is_err() {
                        return Err(Failed(vec![text(message)]));
                    }
                    return Err(Failed(self.after(latest, message, settle_ms.max(SETTLE_MS))?));
                }
            }
        }
        let summary = format!("Ran {total} of {total} steps:\n{}", done.join("\n"));
        Ok(self.after(latest, summary, settle_ms)?)
    }

    /// Counts one batch step against the rate limit, waiting for room instead of refusing.
    fn pace(&self, epoch: u64) -> Result<(), String> {
        loop {
            self.still_running(epoch)?;
            let wait_ms = {
                let mut limiter = lock(&self.limiter);
                let now = self.clock.now_ms();
                if limiter.try_acquire(now) {
                    return Ok(());
                }
                limiter.wait_ms(now)
            };
            self.clock.sleep_ms(wait_ms.clamp(1, WAIT_SLICE_MS));
        }
    }

    /// One action through every guard, then posted: what was done and how long the screen
    /// needs to settle before a screenshot. A single call and a batch step both come here.
    fn perform(
        &self,
        epoch: u64,
        latest: &Option<ScreenshotGeometry>,
        tool: Tool,
        intent: Option<&str>,
    ) -> Result<(String, u64), String> {
        let map = |latest: &Option<ScreenshotGeometry>, (x, y): (f64, f64)| -> Result<Point, String> {
            latest.ok_or(NEEDS_SCREENSHOT)?.to_point(x, y).map_err(|e| e.to_string())
        };
        match tool {
            Tool::Screenshot | Tool::ListApps | Tool::Batch { .. } => Err("Not an action.".into()),
            Tool::Click { x, y, button, count } => {
                let at = map(latest, (x, y))?;
                let owner = self.pointer_target(at)?;
                let target = owner.name.clone();
                let chain = self.desktop.element_at(at);
                let action = match (button, count) {
                    (MouseButton::Right, _) => format!("Right-click at ({x}, {y})"),
                    (_, 2) => format!("Double-click at ({x}, {y})"),
                    (_, 3) => format!("Triple-click at ({x}, {y})"),
                    _ => format!("Click at ({x}, {y})"),
                };
                self.confirm_if_risky(assess(Act::Click(&chain), intent), &action, &target)?;
                self.still_pointer_target(at, &owner)?;
                self.post(epoch, InputEvent::Click { at, button, count })?;
                Ok((format!("{action} in {target}: done."), SETTLE_MS))
            }
            Tool::Move { x, y } => {
                let at = map(latest, (x, y))?;
                let owner = self.pointer_target(at)?;
                let action = format!("Move the pointer to ({x}, {y})");
                self.confirm_if_risky(assess(Act::Other, intent), &action, &owner.name)?;
                self.still_pointer_target(at, &owner)?;
                self.post(epoch, InputEvent::Move(at))?;
                Ok((format!("{action}: done."), SETTLE_MS))
            }
            Tool::Drag { from, to } => {
                let (start, end) = (map(latest, from)?, map(latest, to)?);
                let source = self.pointer_target(start)?;
                let destination = self.pointer_target(end)?;
                let action = format!("Drag from ({}, {}) to ({}, {})", from.0, from.1, to.0, to.1);
                let target = if source.name == destination.name { source.name.clone() } else { format!("{} and {}", source.name, destination.name) };
                let (from_chain, to_chain) = (self.desktop.element_at(start), self.desktop.element_at(end));
                self.confirm_if_risky(assess(Act::Drag(&from_chain, &to_chain), intent), &action, &target)?;
                self.still_pointer_target(start, &source)?;
                self.still_pointer_target(end, &destination)?;
                self.post(epoch, InputEvent::Drag { from: start, to: end })?;
                Ok((format!("{action}: done."), SETTLE_MS))
            }
            Tool::Scroll { x, y, dx, dy } => {
                let at = map(latest, (x, y))?;
                let owner = self.pointer_target(at)?;
                let action = format!("Scroll by ({dx}, {dy}) at ({x}, {y})");
                self.confirm_if_risky(assess(Act::Other, intent), &action, &owner.name)?;
                self.still_pointer_target(at, &owner)?;
                self.post(epoch, InputEvent::Scroll { at, dx, dy })?;
                Ok((format!("{action}: done."), SETTLE_MS))
            }
            Tool::Type { text: typed } => {
                let (target, recipient) = self.keyboard_target(false)?;
                let typed = typed.replace("\r\n", "\n").replace('\r', "\n");
                let focus = self.desktop.focused_element();
                let action = format!("Type {} characters", typed.chars().count());
                self.confirm_if_risky(assess(Act::Type(&typed, focus.as_ref()), intent), &action, &target)?;
                let return_key = parse_combo("return")?;
                let total = typed.chars().count();
                let mut posted = 0;
                // Every chunk and Return goes only to the recipient approved above: the front
                // and focused apps are checked again before each post.
                let mut post_typed = |event: InputEvent, characters| -> Result<(), String> {
                    self.keyboard_target(posted > 0)
                        .and_then(|(_, current)| {
                            if current != recipient && posted > 0 {
                                Err(KEYS_CHANGED_WHILE_TYPING.into())
                            } else if current != recipient {
                                Err(KEYS_CHANGED_BEFORE_TYPING.into())
                            } else {
                                Ok(())
                            }
                        })
                        .map_err(|error| format!("{error} Typing stopped after {posted} of {total} characters."))?;
                    // A stop before the post sent nothing more; only a failed post leaves a range uncertain.
                    self.still_running(epoch)
                        .map_err(|error| format!("{error} Typing stopped after {posted} of {total} characters."))?;
                    self.desktop.post(&event).map_err(|error| {
                        format!("{error} Typing stopped within characters {}-{} of {total}.", posted + 1, posted + characters)
                    })?;
                    posted += characters;
                    Ok(())
                };
                for (i, line) in typed.split('\n').enumerate() {
                    if i > 0 {
                        post_typed(InputEvent::Key(return_key), 1)?;
                    }
                    for chunk in chunks(line) {
                        let characters = chunk.chars().count();
                        post_typed(InputEvent::Text(chunk), characters)?;
                    }
                }
                Ok((format!("{action} in {target}: done."), SETTLE_MS))
            }
            Tool::Key { combo } => {
                let (target, recipient) = self.keyboard_target(false)?;
                let focus = self.desktop.focused_element();
                let action = format!("Press {}", combo.label());
                self.confirm_if_risky(assess(Act::Key(combo, focus.as_ref()), intent), &action, &target)?;
                // Reading the focus or a confirmation can change the app in front: the key goes only
                // to the recipient approved above, as each typed chunk does.
                let (_, current) = self.keyboard_target(false)?;
                if current != recipient {
                    return Err(KEY_RECIPIENT_CHANGED.into());
                }
                self.still_running(epoch)?;
                self.desktop.post(&InputEvent::Key(combo))?;
                Ok((format!("{action} in {target}: done."), SETTLE_MS))
            }
            Tool::OpenApp { name } => {
                if let Some(label) = blocked_name(&name) {
                    return Err(format!("Ailfred does not control {label}. The app was not opened."));
                }
                let action = format!("Open {name}");
                self.confirm_if_risky(assess(Act::Other, intent), &action, SYSTEM)?;
                self.still_running(epoch)?;
                self.desktop.open_app(&name)?;
                Ok((format!("{action}: done."), OPEN_APP_SETTLE_MS))
            }
            Tool::Wait { ms } => {
                let mut left = ms;
                while left > 0 {
                    let slice = left.min(WAIT_SLICE_MS);
                    self.clock.sleep_ms(slice);
                    left -= slice;
                    self.still_running(epoch)?;
                }
                Ok((format!("Waited {ms} ms."), 0))
            }
        }
    }
}

#[cfg(test)]
mod tests;
