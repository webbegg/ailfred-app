//! The real agent: pi in RPC mode (`pi --mode rpc`) as a child process, spoken to in JSON
//! lines over its stdin and stdout. [`Core`] holds the protocol (what to write for a message
//! or a Stop, and which reply [`Event`]s each record from pi makes) without any I/O;
//! [`PiEngine`] runs it against the process.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::fs::File;
use std::process::Command;

use crate::spawn::Process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Where a reply's events go; called from any thread.
pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// A message for pi.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub text: String,
    /// Its shots, as PNG bytes.
    pub pngs: Vec<Arc<Vec<u8>>>,
    /// The user's answer to pi's question (a dialog), not a new message.
    pub answer: bool,
}
use crate::reply::{Event, Tool};

/// What a step's output keeps of a tool's result.
const OUTPUT_CHARS: usize = 8000;
/// A step's target (a command) is cut to this many characters.
const TARGET_CHARS: usize = 80;

/// Splits pi's stdout into records: on LF bytes only (a line reader that also splits on
/// U+2028 or U+2029 breaks JSON strings holding them), a CR before the LF dropped.
#[derive(Default)]
pub struct Lines {
    held: Vec<u8>,
}

impl Lines {
    /// The complete lines in `chunk` and what was held from before; blank ones skipped.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.held.extend_from_slice(chunk);
        let mut lines = Vec::new();
        while let Some(at) = self.held.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.held.drain(..=at).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if !line.iter().all(u8::is_ascii_whitespace) {
                lines.push(String::from_utf8_lossy(&line).into_owned());
            }
        }
        lines
    }
}

/// A dialog pi waits on (an extension's `select`, `confirm`, `input` or `editor`).
struct Dialog {
    id: String,
    confirm: bool,
}

/// The protocol, without I/O: each call returns the commands to write to pi, and sends the
/// reply's events to its sink.
#[derive(Default)]
pub struct Core {
    next_id: u64,
    /// The reply the events go to; none once stopped.
    sink: Option<Sink>,
    /// pi runs a prompt (accepted, not settled yet).
    busy: bool,
    /// An abort was written for the run going.
    aborting: bool,
    /// A message sent while a run was going: it waits for that run to settle.
    queued: Option<(Prompt, Sink)>,
    dialog: Option<Dialog>,
    /// pi's tool call ids, as step ids.
    steps: HashMap<String, u32>,
    next_step: u32,
    /// The run has text already: a later message's text starts a new paragraph.
    texted: bool,
    /// The last assistant message ended in this error (a retry that works clears it).
    error: Option<String>,
    /// pi's process ended and isn't back yet.
    down: bool,
    /// A new session waits for the run going to settle.
    renew: bool,
}

impl Core {
    /// The user's message `prompt`, its reply streaming into `sink`.
    pub fn send(&mut self, prompt: Prompt, sink: Sink) -> Vec<Value> {
        if self.down {
            sink(Event::Fail("The assistant is starting again. Try again in a moment.".into()));
            return Vec::new();
        }
        if prompt.answer
            && let Some(dialog) = self.dialog.take()
        {
            self.sink = Some(sink);
            let mut response = json!({"type": "extension_ui_response", "id": dialog.id});
            if dialog.confirm {
                response["confirmed"] = json!(prompt.text.trim().eq_ignore_ascii_case("yes"));
            } else {
                response["value"] = json!(prompt.text);
            }
            return vec![response];
        }
        let mut out = self.stop();
        if self.busy {
            self.queued = Some((prompt, sink));
        } else {
            out.push(self.prompt(prompt, sink));
        }
        out
    }

    /// Stop: pi aborts, and what it still sends goes nowhere.
    pub fn stop(&mut self) -> Vec<Value> {
        self.sink = None;
        self.queued = None;
        let mut out = Vec::new();
        if let Some(dialog) = self.dialog.take() {
            out.push(json!({"type": "extension_ui_response", "id": dialog.id, "cancelled": true}));
        }
        if self.busy && !self.aborting {
            self.aborting = true;
            out.push(self.command(json!({"type": "abort"})));
        }
        out
    }

    /// A record from pi.
    pub fn record(&mut self, record: &Value) -> Vec<Value> {
        let kind = record["type"].as_str().unwrap_or_default();
        match kind {
            "response" if record["command"] == "prompt" => {
                if record["success"] == false {
                    let error = record["error"].as_str().unwrap_or("The assistant couldn't take that message.");
                    self.emit(Event::Fail(error.to_owned()));
                    return self.settled();
                }
                if record["data"]["disposition"] == "handled" {
                    self.emit(Event::Done);
                    return self.settled();
                }
            }
            "message_start" if record["message"]["role"] == "assistant" => self.error = None,
            "message_update" => {
                let update = &record["assistantMessageEvent"];
                match update["type"].as_str().unwrap_or_default() {
                    "thinking_delta" => self.emit(Event::Think(delta(update))),
                    "text_start" if self.texted => self.emit(Event::Text("\n\n".into())),
                    "text_delta" => {
                        self.texted = true;
                        self.emit(Event::Text(delta(update)));
                    }
                    _ => {}
                }
            }
            "message_end" => {
                let message = &record["message"];
                if message["role"] == "assistant" && message["stopReason"] == "error" {
                    let error = message["errorMessage"].as_str().unwrap_or("Something went wrong.");
                    self.error = Some(error.to_owned());
                }
            }
            "tool_execution_start" => {
                self.next_step += 1;
                let id = self.next_step;
                self.steps.insert(call_id(record), id);
                let (tool, target) = step_of(record["toolName"].as_str().unwrap_or("tool"), &record["args"]);
                self.emit(Event::Step { id, tool, target });
            }
            "tool_execution_end" => {
                if let Some(&id) = self.steps.get(&call_id(record)) {
                    let text = result_text(&record["result"]);
                    if !text.is_empty() {
                        self.emit(Event::StepOutput { id, text });
                    }
                    self.emit(Event::StepEnd { id, ok: record["isError"] != true });
                }
            }
            "extension_ui_request" => self.dialog(record),
            "agent_settled" => {
                match self.error.take() {
                    Some(error) => self.emit(Event::Fail(error)),
                    None => self.emit(Event::Done),
                }
                return self.settled();
            }
            _ => {}
        }
        Vec::new()
    }

    /// Cmd+N: pi starts a new session (after the run going stops).
    pub fn new_conversation(&mut self) -> Vec<Value> {
        let mut out = self.stop();
        if self.busy {
            self.renew = true;
        } else {
            out.push(self.command(json!({"type": "new_session"})));
        }
        out
    }

    /// pi is up (again): messages go to it.
    pub fn started(&mut self) {
        self.down = false;
    }

    /// pi's process ended: the reply streaming fails, and so does one waiting for it.
    pub fn exited(&mut self) {
        const STOPPED: &str = "The assistant stopped unexpectedly.";
        if self.busy || self.dialog.is_some() {
            self.emit(Event::Fail(STOPPED.into()));
        }
        if let Some((_, sink)) = self.queued.take() {
            sink(Event::Fail(STOPPED.into()));
        }
        self.down = true;
        // The pi that comes back starts a session of its own.
        self.renew = false;
        self.busy = false;
        self.aborting = false;
        self.dialog = None;
        self.sink = None;
        self.queued = None;
    }

    /// The run is over: a message waiting for it goes now.
    fn settled(&mut self) -> Vec<Value> {
        self.busy = false;
        self.aborting = false;
        self.dialog = None;
        self.sink = None;
        let mut out = Vec::new();
        if std::mem::take(&mut self.renew) {
            out.push(self.command(json!({"type": "new_session"})));
        }
        if let Some((prompt, sink)) = self.queued.take() {
            out.push(self.prompt(prompt, sink));
        }
        out
    }

    fn prompt(&mut self, prompt: Prompt, sink: Sink) -> Value {
        self.sink = Some(sink);
        self.busy = true;
        (self.texted, self.error) = (false, None);
        self.steps.clear();
        self.next_step = 0;
        let mut command = json!({"type": "prompt", "message": prompt.text});
        if !prompt.pngs.is_empty() {
            let images: Vec<Value> = prompt.pngs.iter().map(|png| json!({"type": "image", "mimeType": "image/png", "data": crate::model::base64(png)})).collect();
            command["images"] = Value::Array(images);
        }
        self.command(command)
    }

    /// `command` with an id of its own.
    fn command(&mut self, mut command: Value) -> Value {
        self.next_id += 1;
        command["id"] = json!(format!("a{}", self.next_id));
        command
    }

    fn emit(&self, event: Event) {
        if let Some(sink) = &self.sink {
            sink(event);
        }
    }

    /// An extension's dialog: asked as a question, the reply over until it's answered.
    fn dialog(&mut self, record: &Value) {
        let method = record["method"].as_str().unwrap_or_default();
        if !matches!(method, "select" | "confirm" | "input" | "editor") {
            return;
        }
        let Some(id) = record["id"].as_str() else { return };
        let mut prompt = record["title"].as_str().unwrap_or("The assistant asks:").to_owned();
        if let Some(message) = record["message"].as_str().filter(|m| !m.is_empty()) {
            prompt = format!("{prompt}\n\n{message}");
        }
        let options = match method {
            "select" => record["options"].as_array().into_iter().flatten().filter_map(|o| o.as_str().map(str::to_owned)).collect(),
            "confirm" => vec!["Yes".to_owned(), "No".to_owned()],
            _ => Vec::new(),
        };
        self.dialog = Some(Dialog { id: id.to_owned(), confirm: method == "confirm" });
        self.emit(Event::Ask { prompt, options, multi: false });
        self.emit(Event::Done);
    }
}

fn delta(update: &Value) -> String {
    update["delta"].as_str().unwrap_or_default().to_owned()
}

fn call_id(record: &Value) -> String {
    record["toolCallId"].as_str().unwrap_or_default().to_owned()
}

/// `text` cut to `max` characters, an ellipsis marking the cut.
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_owned(),
    }
}

/// What tool `name` does, and what it acts on.
pub fn step_of(name: &str, args: &Value) -> (Tool, String) {
    let arg = |keys: &[&str]| keys.iter().find_map(|k| args[*k].as_str()).map(str::to_owned);
    let path = || arg(&["path", "file_path", "filePath"]);
    let (tool, target) = match name {
        "read" => (Tool::Read, path()),
        "write" | "edit" => (Tool::Edit, path()),
        "bash" => (Tool::Run, arg(&["command"]).map(|c| c.lines().next().unwrap_or_default().to_owned())),
        "grep" | "find" => (Tool::Search, arg(&["pattern"]).or_else(path)),
        "ls" => (Tool::Search, path()),
        "web_search" | "fetch_content" => (Tool::Search, arg(&["query", "url"])),
        _ => (Tool::Other, None),
    };
    (tool, cut(&target.unwrap_or_else(|| name.to_owned()), TARGET_CHARS))
}

/// The text of a tool's result (`{content: [{type: "text", text}]}`), cut to [`OUTPUT_CHARS`].
fn result_text(result: &Value) -> String {
    let texts = result["content"].as_array().into_iter().flatten().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str());
    cut(&texts.collect::<Vec<_>>().join("\n"), OUTPUT_CHARS)
}

/// pi as a child process, started again when it ends.
pub struct PiEngine {
    shared: Arc<Shared>,
}

/// What the engine and its supervising thread share.
struct Shared {
    core: Mutex<Core>,
    /// pi's stdin while it runs; closing it asks pi to shut down.
    stdin: Mutex<Option<File>>,
    child: Mutex<Option<Process>>,
    /// The engine is gone: pi isn't started again.
    closed: AtomicBool,
    grace: Duration,
}

/// Makes the command that starts pi again.
type Make = Box<dyn Fn() -> Command + Send + Sync>;

/// How long after its `attempt`th crash in a row pi starts again: 1 s, doubling to 30 s.
pub fn backoff(attempt: u32) -> Duration {
    const SECONDS: [u64; 6] = [1, 2, 4, 8, 16, 30];
    Duration::from_secs(SECONDS[(attempt as usize).min(SECONDS.len() - 1)])
}

/// A run of pi this long counts as having worked: the next crash starts the backoff over.
const LASTED: Duration = Duration::from_secs(30);

/// The wait before the next start after pi ran for `lived`; a run that lasted resets the count.
pub fn restart_delay(attempt: &mut u32, lived: Duration) -> Duration {
    if lived >= LASTED {
        *attempt = 0;
    }
    let wait = backoff(*attempt);
    *attempt += 1;
    wait
}

impl PiEngine {
    /// Runs pi from `make` and starts it again whenever it ends, until dropped; on drop it gets
    /// `grace` to exit after its stdin closes, then SIGTERM, then (`grace` later) SIGKILL.
    pub fn supervised(make: impl Fn() -> Command + Send + Sync + 'static, grace: Duration) -> std::io::Result<Self> {
        Self::start(make(), Some(Box::new(make)), grace)
    }

    /// Runs `command` (pi with `--mode rpc` among its arguments) once, not again when it ends.
    #[cfg(test)]
    pub fn spawn(command: Command) -> std::io::Result<Self> {
        Self::start(command, None, SHUTDOWN_GRACE)
    }

    fn start(command: Command, make: Option<Make>, grace: Duration) -> std::io::Result<Self> {
        let shared = Arc::new(Shared { core: Mutex::default(), stdin: Mutex::default(), child: Mutex::default(), closed: AtomicBool::new(false), grace });
        let stdout = shared.launch(command)?;
        let running = shared.clone();
        std::thread::spawn(move || supervise(&running, stdout, make));
        Ok(PiEngine { shared })
    }

    /// pi's process id, while it runs.
    #[cfg(test)]
    fn pid(&self) -> Option<u32> {
        self.shared.child.lock().unwrap().as_ref().map(Process::id)
    }
}

/// How long pi gets to exit once its stdin closes, and again after SIGTERM.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl Shared {
    /// Starts pi from `command`, its stdio piped (stderr to the log), answering for itself in
    /// macOS's privacy checks (`spawn`): no grant of Ailfred's reaches pi or its shell. Its stdout.
    fn launch(&self, command: Command) -> std::io::Result<File> {
        let spawned = crate::spawn::spawn(&command)?;
        let stderr = spawned.stderr;
        std::thread::spawn(move || log(stderr));
        *self.stdin.lock().unwrap() = Some(spawned.stdin);
        *self.child.lock().unwrap() = Some(spawned.process);
        // The engine went while it started: its drop may have looked before it was there.
        if self.closed.load(Ordering::Acquire) {
            self.stdin.lock().unwrap().take();
            if let Some(mut child) = self.child.lock().unwrap().take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(std::io::Error::other("the engine is gone"));
        }
        self.core.lock().unwrap().started();
        Ok(spawned.stdout)
    }

    /// Waits for pi's process to end once its stdout closed; false if the engine went first
    /// (its drop ends the process).
    fn reap(&self) -> bool {
        loop {
            if self.closed.load(Ordering::Acquire) {
                return false;
            }
            let mut child = self.child.lock().unwrap();
            match child.as_mut().map(Process::try_wait) {
                None | Some(Err(_)) | Some(Ok(Some(_))) => {
                    child.take();
                    return true;
                }
                Some(Ok(None)) => {}
            }
            drop(child);
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Sleeps `wait` unless the engine goes meanwhile; false if it did.
    fn sleep(&self, wait: Duration) -> bool {
        let end = Instant::now() + wait;
        while Instant::now() < end {
            if self.closed.load(Ordering::Acquire) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20).min(end.saturating_duration_since(Instant::now())));
        }
        !self.closed.load(Ordering::Acquire)
    }
}

/// Reads pi until it ends, then (with `make`) starts it again after a backoff, until the
/// engine is dropped.
fn supervise(shared: &Shared, stdout: File, make: Option<Make>) {
    let mut stdout = Some(stdout);
    let mut attempt = 0;
    loop {
        let started = Instant::now();
        if let Some(out) = stdout.take() {
            read(out, &shared.core, &shared.stdin);
        }
        {
            // Together, so no message slips in between pi going and its stdin closing.
            let mut core = shared.core.lock().unwrap();
            core.exited();
            shared.stdin.lock().unwrap().take();
        }
        if !shared.reap() {
            return;
        }
        let Some(make) = &make else { return };
        eprintln!("Ailfred: pi ended; starting it again");
        if !shared.sleep(restart_delay(&mut attempt, started.elapsed())) {
            return;
        }
        match shared.launch(make()) {
            Ok(out) => stdout = Some(out),
            Err(e) => eprintln!("Ailfred: pi couldn't start: {e}"),
        }
    }
}

/// Hands pi's records to `core` until its stdout closes, writing back what that calls for.
fn read(mut stdout: impl Read, core: &Mutex<Core>, stdin: &Mutex<Option<File>>) {
    let mut lines = Lines::default();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = match stdout.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for line in lines.push(&chunk[..n]) {
            let Ok(record) = serde_json::from_str::<Value>(&line) else {
                eprintln!("Ailfred: pi wrote a line that isn't JSON: {}", cut(&line, 200));
                continue;
            };
            let commands = core.lock().unwrap().record(&record);
            write(stdin, commands);
        }
    }
}

/// pi's diagnostics go to Ailfred's log; never read as records.
fn log(stderr: impl Read) {
    use std::io::BufRead;
    for line in std::io::BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
        eprintln!("pi: {}", String::from_utf8_lossy(&line));
    }
}

/// Writes `commands` to pi, one JSON line each.
fn write(stdin: &Mutex<Option<File>>, commands: Vec<Value>) {
    if commands.is_empty() {
        return;
    }
    let mut stdin = stdin.lock().unwrap();
    let Some(pipe) = stdin.as_mut() else { return };
    let mut bytes = Vec::new();
    for command in commands {
        bytes.extend(command.to_string().into_bytes());
        bytes.push(b'\n');
    }
    if pipe.write_all(&bytes).and_then(|()| pipe.flush()).is_err() {
        // It's gone; the reader reports it.
        stdin.take();
    }
}

impl PiEngine {
    /// Sends `prompt`, its reply streaming into `sink`; a reply streaming is stopped first.
    pub fn send(&mut self, prompt: Prompt, sink: Sink) {
        let shared = &self.shared;
        // The core's lock is held while writing, so pi can't go in between (`supervise`).
        let mut core = shared.core.lock().unwrap();
        let commands = core.send(prompt, sink);
        write(&shared.stdin, commands);
    }

    /// Stop: pi aborts, and what it still sends goes nowhere.
    pub fn stop(&mut self) {
        let mut core = self.shared.core.lock().unwrap();
        let commands = core.stop();
        write(&self.shared.stdin, commands);
    }

    /// Cmd+N: pi starts a new session.
    pub fn new_conversation(&mut self) {
        let mut core = self.shared.core.lock().unwrap();
        let commands = core.new_conversation();
        write(&self.shared.stdin, commands);
    }
}

impl Drop for PiEngine {
    /// pi shuts down: its stdin closes, then SIGTERM if it's still there after the grace, then
    /// SIGKILL; it's reaped either way.
    fn drop(&mut self) {
        let shared = &self.shared;
        shared.closed.store(true, Ordering::Release);
        shared.stdin.lock().unwrap().take();
        let Some(mut child) = shared.child.lock().unwrap().take() else { return };
        let gone = |child: &mut Process, wait: Duration| {
            let end = Instant::now() + wait;
            loop {
                if !matches!(child.try_wait(), Ok(None)) {
                    return true;
                }
                if Instant::now() >= end {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        if gone(&mut child, shared.grace) {
            return;
        }
        // Only a process not yet reaped is signalled, so its id can't have been reused.
        let _ = child.terminate();
        if !gone(&mut child, shared.grace) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::{self, Receiver};

    use super::*;

    fn channel() -> (Sink, Receiver<Event>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |e| drop(tx.lock().unwrap().send(e))), rx)
    }

    fn prompt(text: &str) -> Prompt {
        Prompt { text: text.into(), pngs: Vec::new(), answer: false }
    }

    fn answer(text: &str) -> Prompt {
        Prompt { text: text.into(), pngs: Vec::new(), answer: true }
    }

    #[test]
    fn a_messages_shots_go_with_it_as_images() {
        let mut core = Core::default();
        let (sink, _rx) = channel();
        let png = Arc::new(vec![1u8, 2, 3]);
        let wrote = core.send(Prompt { text: "what's this?".into(), pngs: vec![png], answer: false }, sink);
        assert_eq!(wrote[0]["images"], json!([{"type": "image", "mimeType": "image/png", "data": "AQID"}]));
        let (sink, _rx) = channel();
        core.stop();
        feed(&mut core, &[SETTLED]);
        assert!(core.send(prompt("plain"), sink)[0].get("images").is_none(), "none without shots");
    }

    #[test]
    fn a_new_conversation_asked_as_pi_crashed_isnt_started_twice() {
        let mut core = Core::default();
        let (sink, _rx) = channel();
        core.send(prompt("hi"), sink);
        feed(&mut core, &[STARTED]);
        core.new_conversation();
        // pi goes before settling; the one that comes back starts afresh anyway.
        core.exited();
        core.started();
        let (sink, _rx) = channel();
        core.send(prompt("first of the new one"), sink);
        assert!(feed(&mut core, &[STARTED, SETTLED]).is_empty(), "no stale new_session");
    }

    #[test]
    fn a_new_conversation_is_a_new_session_once_the_run_stops() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        assert_eq!(types(&core.new_conversation()), ["new_session"]);
        core.send(prompt("hi"), sink);
        feed(&mut core, &[STARTED]);
        assert_eq!(types(&core.new_conversation()), ["abort"], "the run stops first");
        let late = text("late");
        assert_eq!(types(&feed(&mut core, &[&late, SETTLED])), ["new_session"]);
        assert_eq!(rx.try_iter().count(), 0);
    }

    /// Feeds `records` (JSON lines) to `core`; the commands it wrote back.
    fn feed(core: &mut Core, records: &[&str]) -> Vec<Value> {
        records.iter().flat_map(|r| core.record(&serde_json::from_str(r).expect(r))).collect()
    }

    fn types(commands: &[Value]) -> Vec<&str> {
        commands.iter().map(|c| c["type"].as_str().unwrap_or("?")).collect()
    }

    const STARTED: &str = r#"{"type":"response","command":"prompt","success":true,"data":{"disposition":"started"}}"#;
    const ASSISTANT: &str = r#"{"type":"message_start","message":{"role":"assistant","content":[]}}"#;
    const TEXT_START: &str = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}"#;
    const END_OK: &str = r#"{"type":"message_end","message":{"role":"assistant","stopReason":"stop"}}"#;
    const SETTLED: &str = r#"{"type":"agent_settled","aborted":false}"#;

    fn text(delta: &str) -> String {
        json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":delta}}).to_string()
    }

    #[test]
    fn records_split_on_lf_only() {
        let mut lines = Lines::default();
        let sep = "a\u{2028}b";
        let record = json!({"type":"x","text":sep}).to_string();
        let bytes = format!("{record}\r\n\n{record}\n{{\"half\":").into_bytes();
        let (a, b) = bytes.split_at(5);
        let mut got = lines.push(a);
        got.extend(lines.push(b));
        assert_eq!(got, vec![record.clone(), record.clone()]);
        assert_eq!(lines.push(b"1}\n"), vec!["{\"half\":1}".to_owned()]);
    }

    #[test]
    fn a_message_is_prompted_and_its_reply_streams_until_settled() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        let wrote = core.send(prompt("hi"), sink);
        assert_eq!(types(&wrote), ["prompt"]);
        assert_eq!(wrote[0]["message"], "hi");
        assert!(wrote[0]["id"].is_string(), "commands carry an id");
        let think = r#"{"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"hmm"}}"#;
        let (hel, lo) = (text("Hel"), text("lo"));
        let wrote = feed(&mut core, &[STARTED, r#"{"type":"agent_start"}"#, ASSISTANT, think, TEXT_START, &hel, &lo, END_OK, r#"{"type":"agent_end","messages":[]}"#, SETTLED]);
        assert!(wrote.is_empty());
        let got: Vec<Event> = rx.try_iter().collect();
        assert_eq!(got, [Event::Think("hmm".into()), Event::Text("Hel".into()), Event::Text("lo".into()), Event::Done]);
    }

    #[test]
    fn a_later_message_of_the_run_starts_a_new_paragraph() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        let (one, two) = (text("One."), text("Two."));
        feed(&mut core, &[STARTED, ASSISTANT, TEXT_START, &one, END_OK, ASSISTANT, TEXT_START, &two, END_OK, SETTLED]);
        let texts: String = rx.try_iter().filter_map(|e| if let Event::Text(t) = e { Some(t) } else { None }).collect();
        assert_eq!(texts, "One.\n\nTwo.");
    }

    #[test]
    fn tool_calls_become_steps() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        feed(&mut core, &[
            STARTED,
            r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{"command":"ls -la\necho"}}"#,
            r#"{"type":"tool_execution_start","toolCallId":"c2","toolName":"read","args":{"path":"src/a.rs"}}"#,
            r#"{"type":"tool_execution_end","toolCallId":"c1","toolName":"bash","result":{"content":[{"type":"text","text":"a\nb"}]},"isError":false}"#,
            r#"{"type":"tool_execution_end","toolCallId":"c2","toolName":"read","result":{"content":[{"type":"text","text":"nope"}]},"isError":true}"#,
            SETTLED,
        ]);
        let got: Vec<Event> = rx.try_iter().collect();
        assert_eq!(got, [
            Event::Step { id: 1, tool: Tool::Run, target: "ls -la".into() },
            Event::Step { id: 2, tool: Tool::Read, target: "src/a.rs".into() },
            Event::StepOutput { id: 1, text: "a\nb".into() },
            Event::StepEnd { id: 1, ok: true },
            Event::StepOutput { id: 2, text: "nope".into() },
            Event::StepEnd { id: 2, ok: false },
            Event::Done,
        ]);
    }

    #[test]
    fn each_tool_has_its_kind_and_target() {
        let cases: [(&str, Value, Tool, &str); 7] = [
            ("edit", json!({"path":"a.rs"}), Tool::Edit, "a.rs"),
            ("write", json!({"file_path":"b.md"}), Tool::Edit, "b.md"),
            ("grep", json!({"pattern":"fn main"}), Tool::Search, "fn main"),
            ("find", json!({"pattern":"*.rs"}), Tool::Search, "*.rs"),
            ("ls", json!({"path":"src"}), Tool::Search, "src"),
            ("mem_search", json!({"query":"x"}), Tool::Other, "mem_search"),
            ("bash", json!({"command": "x".repeat(200)}), Tool::Run, ""),
        ];
        for (name, args, tool, target) in cases {
            let (t, got) = step_of(name, &args);
            assert_eq!(t, tool, "{name}");
            if name == "bash" {
                assert_eq!(got.chars().count(), TARGET_CHARS + 1, "cut with an ellipsis");
            } else {
                assert_eq!(got, target, "{name}");
            }
        }
    }

    #[test]
    fn a_failed_reply_shows_its_error_unless_a_retry_works() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        let failed = r#"{"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"529 overloaded"}}"#;
        feed(&mut core, &[STARTED, ASSISTANT, failed, SETTLED]);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Event::Fail("529 overloaded".into())]);

        let (sink, rx) = channel();
        core.send(prompt("again"), sink);
        let ok = text("Fine.");
        feed(&mut core, &[STARTED, ASSISTANT, failed, r#"{"type":"auto_retry_start","attempt":1}"#, ASSISTANT, TEXT_START, &ok, END_OK, SETTLED]);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Event::Text("Fine.".into()), Event::Done]);
    }

    #[test]
    fn a_refused_prompt_fails_the_reply() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        feed(&mut core, &[r#"{"type":"response","command":"prompt","success":false,"error":"No model selected"}"#]);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Event::Fail("No model selected".into())]);
        // Not busy: the next message is prompted at once.
        let (sink, _rx) = channel();
        assert_eq!(types(&core.send(prompt("again"), sink)), ["prompt"]);
    }

    #[test]
    fn stop_aborts_and_what_comes_after_goes_nowhere() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        feed(&mut core, &[STARTED]);
        assert_eq!(types(&core.stop()), ["abort"]);
        assert!(core.stop().is_empty(), "one abort");
        let late = text("late");
        feed(&mut core, &[ASSISTANT, &late, r#"{"type":"agent_settled","aborted":true}"#]);
        assert_eq!(rx.try_iter().count(), 0);
        // Idle again: nothing to abort.
        assert!(core.stop().is_empty());
    }

    #[test]
    fn a_message_sent_while_a_run_goes_waits_for_it_to_settle() {
        let mut core = Core::default();
        let (first, first_rx) = channel();
        core.send(prompt("one"), first);
        feed(&mut core, &[STARTED]);
        let (second, second_rx) = channel();
        assert_eq!(types(&core.send(prompt("two"), second)), ["abort"]);
        let late = text("late");
        let wrote = feed(&mut core, &[&late, r#"{"type":"agent_settled","aborted":true}"#]);
        assert_eq!(types(&wrote), ["prompt"]);
        assert_eq!(wrote[0]["message"], "two");
        let hi = text("Hi");
        feed(&mut core, &[STARTED, ASSISTANT, TEXT_START, &hi, END_OK, SETTLED]);
        assert_eq!(first_rx.try_iter().count(), 0);
        assert_eq!(second_rx.try_iter().collect::<Vec<_>>(), [Event::Text("Hi".into()), Event::Done]);
    }

    #[test]
    fn a_dialog_is_a_question_and_the_answer_goes_back_to_it() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        feed(&mut core, &[STARTED, r#"{"type":"extension_ui_request","id":"u1","method":"select","title":"Allow it?","options":["Allow","Block"]}"#]);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [
            Event::Ask { prompt: "Allow it?".into(), options: vec!["Allow".into(), "Block".into()], multi: false },
            Event::Done,
        ]);
        let (sink, rx) = channel();
        let wrote = core.send(answer("Allow"), sink);
        assert_eq!(wrote, [json!({"type":"extension_ui_response","id":"u1","value":"Allow"})]);
        let ok = text("Done it.");
        feed(&mut core, &[ASSISTANT, TEXT_START, &ok, END_OK, SETTLED]);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [Event::Text("Done it.".into()), Event::Done]);
    }

    #[test]
    fn a_confirm_asks_yes_or_no() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        feed(&mut core, &[STARTED, r#"{"type":"extension_ui_request","id":"u2","method":"confirm","title":"Clear it?","message":"All is lost."}"#]);
        assert_eq!(rx.try_iter().next(), Some(Event::Ask { prompt: "Clear it?\n\nAll is lost.".into(), options: vec!["Yes".into(), "No".into()], multi: false }));
        let (sink, _rx) = channel();
        assert_eq!(core.send(answer("No"), sink), [json!({"type":"extension_ui_response","id":"u2","confirmed":false})]);
    }

    #[test]
    fn a_new_message_instead_of_an_answer_cancels_the_dialog() {
        let mut core = Core::default();
        let (sink, _rx) = channel();
        core.send(prompt("hi"), sink);
        feed(&mut core, &[STARTED, r#"{"type":"extension_ui_request","id":"u3","method":"input","title":"Name?"}"#]);
        let (sink, _rx) = channel();
        let wrote = core.send(prompt("never mind"), sink);
        assert_eq!(wrote[0], json!({"type":"extension_ui_response","id":"u3","cancelled":true}));
        assert_eq!(types(&wrote[1..]), ["abort"]);
    }

    #[test]
    fn notices_and_status_records_make_no_events() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        let wrote = feed(&mut core, &[
            STARTED,
            r#"{"type":"extension_ui_request","id":"n1","method":"notify","message":"hello"}"#,
            r#"{"type":"extension_ui_request","id":"n2","method":"setStatus","statusKey":"k","statusText":"x"}"#,
            r#"{"type":"queue_update","steering":[],"followUp":[]}"#,
        ]);
        assert!(wrote.is_empty());
        assert_eq!(rx.try_iter().count(), 0);
    }

    #[test]
    fn while_pi_is_down_a_message_fails_instead_of_waiting_for_it() {
        let mut core = Core::default();
        let (sink, _rx) = channel();
        core.send(prompt("hi"), sink);
        core.exited();
        let (sink, rx) = channel();
        assert!(core.send(prompt("lost"), sink).is_empty());
        assert!(matches!(rx.try_recv(), Ok(Event::Fail(_))));
        core.started();
        let (sink, _rx) = channel();
        assert_eq!(types(&core.send(prompt("next"), sink)), ["prompt"], "not left busy");
    }

    #[test]
    fn a_message_waiting_for_the_run_fails_when_pi_ends() {
        let mut core = Core::default();
        let (first, _first_rx) = channel();
        core.send(prompt("one"), first);
        feed(&mut core, &[STARTED]);
        let (second, second_rx) = channel();
        core.send(prompt("two"), second);
        core.exited();
        assert!(matches!(second_rx.try_recv(), Ok(Event::Fail(_))), "its reply isn't left streaming");
    }

    #[test]
    fn a_pi_started_after_the_engine_went_is_ended_at_once() {
        let shared = Shared { core: Mutex::default(), stdin: Mutex::default(), child: Mutex::default(), closed: AtomicBool::new(true), grace: Duration::from_millis(100) };
        assert!(shared.launch(sh("while :; do sleep 0.05; done")).is_err());
        assert!(shared.child.lock().unwrap().is_none());
        assert!(shared.stdin.lock().unwrap().is_none());
    }

    #[test]
    fn pi_ending_mid_reply_fails_it() {
        let mut core = Core::default();
        let (sink, rx) = channel();
        core.send(prompt("hi"), sink);
        core.exited();
        assert!(matches!(rx.try_iter().next(), Some(Event::Fail(_))));
    }

    /// A stand-in for pi: reads the prompt, answers it, then waits for stdin to close.
    #[test]
    fn the_engine_runs_a_process_that_speaks_rpc() {
        let script = format!(
            "read line; printf '%s\\n' '{STARTED}' '{ASSISTANT}' '{TEXT_START}' '{}' '{END_OK}' '{SETTLED}'; cat > /dev/null",
            text("Hi from pi")
        );
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(script);
        let mut engine = PiEngine::spawn(command).expect("spawned");
        let (sink, rx) = channel();
        engine.send(prompt("hi"), sink);
        let first = rx.recv_timeout(Duration::from_secs(5)).expect("a reply");
        assert_eq!(first, Event::Text("Hi from pi".into()));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Event::Done));
    }

    #[test]
    fn restarts_back_off_from_one_second_to_thirty() {
        let waits: Vec<u64> = (0..8).map(|a| backoff(a).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30, 30]);
        let mut attempt = 0;
        let quick = Duration::from_secs(2);
        let got: Vec<u64> = (0..3).map(|_| restart_delay(&mut attempt, quick).as_secs()).collect();
        assert_eq!(got, [1, 2, 4]);
        assert_eq!(restart_delay(&mut attempt, Duration::from_secs(60)).as_secs(), 1, "a run that lasted starts the count over");
        assert_eq!(restart_delay(&mut attempt, quick).as_secs(), 2);
    }

    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg(script);
        command
    }

    fn replying() -> String {
        format!("while read line; do printf '%s\\n' '{STARTED}' '{ASSISTANT}' '{TEXT_START}' '{}' '{END_OK}' '{SETTLED}'; done", text("Back"))
    }

    #[test]
    fn pi_starts_again_after_it_crashes() {
        let starts = Arc::new(AtomicU32::new(0));
        let counted = starts.clone();
        let reply = replying();
        let mut engine = PiEngine::supervised(
            move || if counted.fetch_add(1, Ordering::SeqCst) == 0 { sh("read line; exit 1") } else { sh(&reply) },
            Duration::from_millis(200),
        )
        .expect("spawned");
        let (sink, rx) = channel();
        engine.send(prompt("hi"), sink);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Event::Fail(_))));
        std::thread::sleep(Duration::from_millis(1500));
        let (sink, rx) = channel();
        engine.send(prompt("again"), sink);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Event::Text("Back".into())));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Event::Done));
        assert_eq!(starts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn one_restart_at_a_time() {
        let starts = Arc::new(AtomicU32::new(0));
        let counted = starts.clone();
        let engine = PiEngine::supervised(
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
                sh("exit 1")
            },
            Duration::from_millis(200),
        )
        .expect("spawned");
        std::thread::sleep(Duration::from_millis(2200));
        // At 0 s, then 1 s later; the next waits 2 s more (slack for a busy machine).
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        drop(engine);
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(starts.load(Ordering::SeqCst), 2, "no start once dropped");
    }

    #[test]
    fn dropping_the_engine_ends_pi_even_when_it_wont_go() {
        let engine = PiEngine::supervised(|| sh("trap '' TERM; while :; do sleep 0.05; done"), Duration::from_millis(200)).expect("spawned");
        std::thread::sleep(Duration::from_millis(100));
        let pid = engine.pid().expect("running") as libc::pid_t;
        let started = std::time::Instant::now();
        drop(engine);
        assert!(started.elapsed() < Duration::from_secs(3));
        // Gone and reaped: no zombie answers a signal 0.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    /// The real pi, set up as Ailfred's own (the user's sign-ins and model, nothing else of
    /// theirs), in a scratch data folder: `cargo test real_pi -- --ignored`.
    #[test]
    #[ignore = "talks to the real pi and its model"]
    fn real_pi_answers() {
        let data = std::env::temp_dir().join("ailfred-real-pi");
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let pi = crate::pi_setup::pi_binary(&home).expect("pi installed");
        let mut engine = crate::pi_setup::start(&pi, &data, &home).expect("pi starts");
        let (sink, rx) = channel();
        engine.send(prompt("Reply with exactly the word: pong"), sink);
        let mut got = Vec::new();
        while let Ok(e) = rx.recv_timeout(Duration::from_secs(120)) {
            let end = matches!(e, Event::Done | Event::Fail(_));
            got.push(e);
            if end {
                break;
            }
        }
        eprintln!("{got:?}");
        let text: String = got.iter().filter_map(|e| if let Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(got.last(), Some(&Event::Done), "{got:?}");
        assert!(text.to_lowercase().contains("pong"), "{text}");
    }

    #[test]
    fn the_engine_fails_the_reply_when_the_process_ends() {
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("read line; exit 1");
        let mut engine = PiEngine::spawn(command).expect("spawned");
        let (sink, rx) = channel();
        engine.send(prompt("hi"), sink);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Event::Fail(_))));
    }
}
