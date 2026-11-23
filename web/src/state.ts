// The conversation the page shows, built from the app's AG-UI events by a pure reducer. It reads
// events as the app's own replayer does (`crates/ailfred-protocol/src/replay.rs`): each event
// becomes the reply events of the core (`Taken`, `Think`, `Text`, `Step`, …), applied to the
// reply going as `Reply::apply` does (`crates/ailfred-core/src/reply.rs`). A `MESSAGES_SNAPSHOT`
// replaces everything, read as `snapshot.rs` reads one back.

import { ASK, DIFF, ERROR, LOST, STEP_OUTPUT, parseEvent, type Content, type Event, type Interrupt, type Json, type JsonObject, type Message } from "./protocol";

export type StepStatus = "running" | "done" | "failed";

export interface Step {
  /// Its tool call's id.
  id: string;
  /// `read`, `search`, `run`, `edit`, `other`, or another producer's tool.
  name: string;
  /// What it works on: its arguments' `target` (another producer's tool: its name).
  target: string;
  output: string;
  status: StepStatus;
}

export type Part =
  /// Collapsed by default; `done` once something else came after it.
  | { kind: "thinking"; text: string; done: boolean }
  /// Markdown.
  | { kind: "text"; text: string }
  /// Steps in a row.
  | { kind: "steps"; steps: Step[] }
  | { kind: "diff"; path: string; patch: string }
  /// A question: `answered` once it is; `closed` when nothing waits for its answer.
  | { kind: "question"; interruptId: string; prompt: string; options: string[]; multi: boolean; answered: string | null; closed: boolean }
  | { kind: "error"; message: string };

export type QuestionPart = Extract<Part, { kind: "question" }>;

/// `stopped` with `interrupted`: the model's process ended (`ailfred:lost`); without, the user
/// stopped it.
export type ReplyState = "streaming" | "done" | "stopped" | "failed";

export interface UserTurn {
  kind: "user";
  id: string;
  text: string;
  /// Data URLs.
  images: string[];
}

export interface ReplyTurn {
  kind: "reply";
  runId: string;
  parts: Part[];
  state: ReplyState;
  interrupted: boolean;
}

export type Item = UserTurn | ReplyTurn;

/// A tool call of the run going: a step once its arguments are in.
interface Call {
  id: string;
  name: string;
  args: string;
  stepped: boolean;
  /// Its output so far.
  output: string;
}

/// What the page keeps of the run going, as a replayer does.
interface Live {
  threadId: string;
  runId: string;
  calls: Call[];
  /// The interrupt ids asked so far.
  asked: string[];
}

export interface Conversation {
  /// The conversation's id: the `threadId` commands carry.
  threadId: string | null;
  items: Item[];
  /// The run streaming, if one is.
  run: string | null;
  /// Messages the page sent whose run hasn't started yet.
  pending: UserTurn[];
  live: Live | null;
  /// The last run that ended, while nothing else started: a `RUN_ERROR` after its
  /// `RUN_FINISHED` fails it after all.
  ended: string | null;
}

export function empty(): Conversation {
  return { threadId: null, items: [], run: null, pending: [], live: null, ended: null };
}

/// What the page itself does to the conversation.
export type Local =
  /// It sent a message, run `runId`: shown until that run starts.
  | { type: "sent"; runId: string; text: string; images: string[] }
  /// The app refused it.
  | { type: "refused"; runId: string }
  /// The app took an answer to question `interruptId`.
  | { type: "answered"; interruptId: string; answer: string };

export function local(state: Conversation, action: Local): Conversation {
  switch (action.type) {
    case "sent": {
      const turn: UserTurn = { kind: "user", id: userId(action.runId), text: action.text, images: action.images };
      return { ...state, pending: [...state.pending, turn] };
    }
    case "refused":
      return { ...state, pending: state.pending.filter((p) => p.id !== userId(action.runId)) };
    case "answered": {
      const items = state.items.map((item) => {
        if (item.kind !== "reply" || !item.parts.some((p) => p.kind === "question" && p.interruptId === action.interruptId)) return item;
        const parts = item.parts.map((p) => (p.kind === "question" && p.interruptId === action.interruptId ? { ...p, answered: action.answer } : p));
        return { ...item, parts };
      });
      return { ...state, items };
    }
  }
}

/// Whether question `part` of `reply` still takes an answer: not answered or closed, in the
/// latest reply, which ended done (one that failed or was stopped waits for nothing).
export function canAnswer(state: Conversation, reply: ReplyTurn, part: QuestionPart): boolean {
  const latest = state.items.findLast((i): i is ReplyTurn => i.kind === "reply");
  return latest === reply && reply.state === "done" && part.answered === null && !part.closed;
}

export function applyAll(state: Conversation, events: readonly unknown[]): Conversation {
  return events.reduce<Conversation>(apply, state);
}

/// The conversation after event `raw`. An event of a type it doesn't know, a broken one, or one
/// out of its run's lifecycle changes nothing.
export function apply(state: Conversation, raw: unknown): Conversation {
  const event = parseEvent(raw);
  if (!event) return state;
  if (event.type === "MESSAGES_SNAPSHOT") return fromSnapshot(event.messages, event.metadata);
  if (event.type === "RUN_STARTED") return started(state, event);
  const live = state.live;
  if (!live) {
    if (event.type === "RUN_ERROR" && state.ended !== null) {
      // It failed after it finished: the reply shows it failed after all.
      const failed = updateReply(state, state.ended, (r) => {
        r.parts.push({ kind: "error", message: event.message });
        r.state = "failed";
      });
      return { ...failed, ended: null };
    }
    return state;
  }
  if (event.type === "RUN_FINISHED" && (event.threadId !== live.threadId || event.runId !== live.runId)) return state;
  const nextLive: Live = { ...live, calls: live.calls.map((c) => ({ ...c })), asked: [...live.asked] };
  const updates = kinds(nextLive, event);
  let next = updateReply(state, live.runId, (reply) => updates.forEach((u) => reduce(reply, u)));
  next = { ...next, live: nextLive };
  if (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") {
    const ended = event.type === "RUN_FINISHED" ? live.runId : null;
    next = { ...next, live: null, run: null, ended };
  }
  return next;
}

function userId(runId: string): string {
  return `${runId}-user`;
}

function started(state: Conversation, event: Extract<Event, { type: "RUN_STARTED" }>): Conversation {
  let base = state;
  if (state.live) {
    // Two runs interleave, which the app never sends: the one going ends as it was.
    base = updateReply(state, state.live.runId, (r) => reduce(r, { t: "done" }));
  }
  const id = userId(event.runId);
  const said = event.input?.messages.findLast((m) => m.role === "user");
  const pending = base.pending.find((p) => p.id === id);
  const items = [...base.items];
  if (said) items.push({ kind: "user", id: said.id, ...userContent(said.content) });
  else if (pending) items.push(pending);
  items.push({ kind: "reply", runId: event.runId, parts: [], state: "streaming", interrupted: false });
  return {
    threadId: base.threadId ?? event.threadId,
    items,
    run: event.runId,
    pending: base.pending.filter((p) => p.id !== id),
    live: { threadId: event.threadId, runId: event.runId, calls: [], asked: [] },
    ended: null,
  };
}

/// `state` with reply `runId` copied and changed by `change`.
function updateReply(state: Conversation, runId: string, change: (reply: ReplyTurn) => void): Conversation {
  const at = state.items.findLastIndex((i) => i.kind === "reply" && i.runId === runId);
  if (at < 0) return state;
  const reply = cloneReply(state.items[at] as ReplyTurn);
  change(reply);
  const items = [...state.items];
  items[at] = reply;
  return { ...state, items };
}

function cloneReply(reply: ReplyTurn): ReplyTurn {
  const parts = reply.parts.map((p): Part => (p.kind === "steps" ? { ...p, steps: p.steps.map((s) => ({ ...s })) } : { ...p }));
  return { ...reply, parts };
}

/// One of the core's reply events, as the replayer makes them.
type Update =
  | { t: "taken" }
  | { t: "think"; text: string }
  | { t: "text"; text: string }
  | { t: "step"; id: string; name: string; target: string }
  | { t: "output"; id: string; text: string }
  | { t: "stepEnd"; id: string; ok: boolean }
  | { t: "diff"; path: string; patch: string }
  | { t: "ask"; interruptId: string; prompt: string; options: string[]; multi: boolean }
  | { t: "fail"; message: string }
  | { t: "lost" }
  | { t: "done" }
  | { t: "stop" };

const str = (v: Json | undefined): string => (typeof v === "string" ? v : "");
const obj = (v: Json | undefined): JsonObject | null => (v !== null && typeof v === "object" && !Array.isArray(v) ? v : null);
const strings = (v: Json | undefined): string[] => (Array.isArray(v) ? v.filter((o): o is string => typeof o === "string") : []);

/// The updates `event` makes to the reply going (`Replayer::kinds`).
function kinds(live: Live, event: Event): Update[] {
  const call = (id: string) => live.calls.find((c) => c.id === id);
  const step = (id: string): Update[] => {
    const c = call(id);
    if (!c || c.stepped) return [];
    c.stepped = true;
    return [{ t: "step", id: c.id, ...stepOf(c.name, c.args) }];
  };
  switch (event.type) {
    case "REASONING_START":
      return [{ t: "think", text: "" }];
    case "REASONING_END":
      return [{ t: "taken" }];
    case "REASONING_MESSAGE_CONTENT":
      return [{ t: "think", text: event.delta }];
    case "TEXT_MESSAGE_START":
      return [{ t: "text", text: "" }];
    case "TEXT_MESSAGE_CONTENT":
      return [{ t: "text", text: event.delta }];
    case "TOOL_CALL_START":
      live.calls.push({ id: event.toolCallId, name: event.toolCallName, args: "", stepped: false, output: "" });
      return [];
    case "TOOL_CALL_ARGS": {
      const c = call(event.toolCallId);
      if (c) c.args += event.delta;
      return [];
    }
    case "TOOL_CALL_END":
      return step(event.toolCallId);
    case "TOOL_CALL_RESULT": {
      const out = step(event.toolCallId);
      const c = call(event.toolCallId);
      if (!c) return out;
      // What it returned past what came streaming.
      const content = textOf(event.content);
      if (content.startsWith(c.output) && content.length > c.output.length) {
        out.push({ t: "output", id: c.id, text: content.slice(c.output.length) });
        c.output = content;
      }
      out.push({ t: "stepEnd", id: c.id, ok: obj(event.metadata?.ailfred)?.ok !== false });
      return out;
    }
    case "ACTIVITY_SNAPSHOT":
      if (event.activityType !== DIFF) return [];
      return [{ t: "diff", path: str(event.content.path), patch: str(event.content.patch) }];
    case "CUSTOM": {
      const value = obj(event.value);
      if (!value) return [];
      if (event.name === STEP_OUTPUT) {
        const { toolCallId: id, delta } = value;
        if (typeof id !== "string" || typeof delta !== "string") return [];
        const out = step(id);
        const c = call(id);
        if (c) {
          c.output += delta;
          out.push({ t: "output", id, text: delta });
        }
        return out;
      }
      if (event.name === ASK) {
        if (typeof value.interruptId !== "string" || typeof value.message !== "string") return [];
        live.asked.push(value.interruptId);
        return [{ t: "ask", interruptId: value.interruptId, prompt: value.message, options: strings(value.options), multi: value.multi === true }];
      }
      return [];
    }
    case "RUN_FINISHED": {
      const outcome = event.outcome;
      if (outcome?.type === "cancelled") return [{ t: "stop" }];
      if (outcome?.type === "interrupt" && "interrupts" in outcome && Array.isArray(outcome.interrupts)) {
        // Ours were asked where they came; another producer's only come here.
        const unseen = outcome.interrupts.filter((i) => !live.asked.includes(i.id));
        return [...unseen.map(ask), { t: "done" }];
      }
      // Success, and an outcome this version doesn't know.
      return [{ t: "done" }];
    }
    case "RUN_ERROR":
      return event.code === LOST ? [{ t: "lost" }] : [{ t: "fail", message: event.message }];
    default:
      return [];
  }
}

/// The question an interrupt asks (`replay::ask`).
function ask(interrupt: Interrupt): Update {
  const ours = obj(interrupt.metadata?.ailfred);
  return { t: "ask", interruptId: interrupt.id, prompt: interrupt.message ?? "", options: strings(ours?.options), multi: ours?.multi === true };
}

/// The name and target of a call to `name` with arguments `args` (`replay::step_of`).
function stepOf(name: string, args: string): { name: string; target: string } {
  let parsed: Json = null;
  try {
    parsed = JSON.parse(args) as Json;
  } catch {
    // No arguments, or not JSON: no target.
  }
  const target = str(obj(parsed)?.target);
  const known = ["read", "search", "run", "edit", "other"].includes(name);
  return { name, target: known || target !== "" ? target : name };
}

/// A message's text: its text parts', joined, when it comes in parts.
function textOf(content: Content): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((p) => (p && p.type === "text" && typeof p.text === "string" ? p.text : "")).join("");
}

/// A user message's text, and its inline images as data URLs.
function userContent(content: Content): { text: string; images: string[] } {
  const images = Array.isArray(content)
    ? content.flatMap((p) => (p && p.type === "image" && p.source?.type === "data" && typeof p.source.value === "string" ? [`data:${p.source.mimeType};base64,${p.source.value}`] : []))
    : [];
  return { text: textOf(content), images };
}

/// Applies `u` to `reply` in place, as `Reply::apply` (and `Reply::stop`) do.
function reduce(reply: ReplyTurn, u: Update): void {
  if (reply.state !== "streaming") return;
  const parts = reply.parts;
  const last = parts.at(-1);
  // Anything but more thinking ends the thinking open.
  if (u.t !== "think") for (const p of parts) if (p.kind === "thinking") p.done = true;
  const stepById = (id: string) => {
    for (const p of parts) if (p.kind === "steps") for (const s of p.steps) if (s.id === id) return s;
    return undefined;
  };
  const endSteps = (status: StepStatus) => {
    for (const p of parts) if (p.kind === "steps") for (const s of p.steps) if (s.status === "running") s.status = status;
  };
  switch (u.t) {
    case "think":
      if (last?.kind === "thinking" && !last.done) last.text += u.text;
      else parts.push({ kind: "thinking", text: u.text, done: false });
      break;
    case "text":
      if (last?.kind === "text") last.text += u.text;
      else parts.push({ kind: "text", text: u.text });
      break;
    case "step": {
      const s: Step = { id: u.id, name: u.name, target: u.target, output: "", status: "running" };
      if (last?.kind === "steps") last.steps.push(s);
      else parts.push({ kind: "steps", steps: [s] });
      break;
    }
    case "output": {
      const s = stepById(u.id);
      if (s) s.output += u.text;
      break;
    }
    case "stepEnd": {
      const s = stepById(u.id);
      if (s && s.status === "running") s.status = u.ok ? "done" : "failed";
      break;
    }
    case "diff":
      parts.push({ kind: "diff", path: u.path, patch: u.patch });
      break;
    case "ask":
      parts.push({ kind: "question", interruptId: u.interruptId, prompt: u.prompt, options: u.options, multi: u.multi, answered: null, closed: false });
      break;
    case "fail":
      endSteps("failed");
      parts.push({ kind: "error", message: u.message });
      reply.state = "failed";
      break;
    case "lost":
      endSteps("failed");
      reply.state = "stopped";
      reply.interrupted = true;
      break;
    case "done":
      endSteps("done");
      reply.state = "done";
      break;
    case "stop":
      endSteps("failed");
      reply.state = "stopped";
      break;
    case "taken":
      break;
  }
}

/// The conversation a snapshot's `messages` hold (`snapshot::conversation_from_snapshot`): each
/// user message, and what follows it until the next as its reply.
function fromSnapshot(messages: Message[], metadata: JsonObject | undefined): Conversation {
  const threadId = obj(metadata?.ailfred)?.threadId;
  const items: Item[] = [];
  let reply: ReplyTurn | null = null;
  /// The reply's tool call ids.
  let calls: string[] = [];
  const replies: ReplyTurn[] = [];
  for (const m of Array.isArray(messages) ? messages : []) {
    if (!m || typeof m !== "object") continue;
    if (m.role === "user") {
      items.push({ kind: "user", id: m.id, ...userContent(m.content) });
      reply = null;
      calls = [];
      continue;
    }
    if (items.length === 0) items.push({ kind: "user", id: "", text: "", images: [] });
    if (!reply) {
      const user = items.at(-1) as UserTurn;
      const runId = user.id.endsWith("-user") ? user.id.slice(0, -"-user".length) : `reply-${items.length}`;
      reply = { kind: "reply", runId, parts: [], state: "streaming", interrupted: false };
      items.push(reply);
      replies.push(reply);
    }
    const r = reply;
    switch (m.role) {
      case "reasoning":
        // A thinking of its own, even right after another.
        reduce(r, { t: "taken" });
        reduce(r, { t: "think", text: m.content });
        break;
      case "assistant":
        if (typeof m.content === "string") reduce(r, { t: "text", text: m.content });
        for (const c of m.toolCalls ?? []) {
          calls.push(c.id);
          reduce(r, { t: "step", id: c.id, ...stepOf(c.function.name, c.function.arguments) });
        }
        break;
      case "tool": {
        if (!calls.includes(m.toolCallId)) break;
        const output = textOf(m.content);
        if (output !== "") reduce(r, { t: "output", id: m.toolCallId, text: output });
        reduce(r, { t: "stepEnd", id: m.toolCallId, ok: m.error === undefined });
        break;
      }
      case "activity": {
        const c = m.content ?? {};
        if (m.activityType === DIFF) reduce(r, { t: "diff", path: str(c.path), patch: str(c.patch) });
        else if (m.activityType === ASK) {
          reduce(r, { t: "ask", interruptId: str(c.interruptId) || m.id, prompt: str(c.message), options: strings(c.options), multi: c.multi === true });
          const q = r.parts.at(-1);
          if (q?.kind === "question") {
            q.answered = typeof c.answered === "string" ? c.answered : null;
            q.closed = q.answered === null && c.closed === true;
          }
        } else if (m.activityType === ERROR) reduce(r, { t: "fail", message: str(c.message) });
        break;
      }
    }
  }
  for (const r of replies) {
    // Read whole: its thinking ends, and it's done unless it failed, its steps as they were.
    reduce(r, { t: "taken" });
    if (r.state === "streaming") r.state = "done";
    if (r.state === "failed") for (const p of r.parts) if (p.kind === "question" && p.answered === null) p.closed = true;
  }
  return { ...empty(), threadId: typeof threadId === "string" ? threadId : null, items };
}
