// The wire types the app sends and takes: AG-UI 1.0 events and Ailfred's commands, written by
// hand from `crates/ailfred-protocol/schema/{event,command}.schema.json`. Only the subset Ailfred
// uses; a newer producer may send more, so events are read through `parseEvent`, which skips
// what it doesn't know.

export const PROTOCOL_VERSION = "1.0";

/// Ailfred's own names, as in `crates/ailfred-protocol/src/encode.rs`.
export const STEP_OUTPUT = "ailfred:step_output";
export const DIFF = "ailfred:diff";
export const LOST = "ailfred:lost";
export const ASK = "ailfred:ask";
/// A snapshot's error activity (`snapshot.rs`).
export const ERROR = "ailfred:error";

export type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
export type JsonObject = { [key: string]: Json };

/// What every event carries besides its own fields.
export interface Base {
  timestamp?: number;
  metadata?: JsonObject;
}

export type PartSource =
  | { type: "data"; value: string; mimeType: string }
  | { type: "url"; value: string; mimeType?: string }
  | { type: "file"; value: string; provider?: string; mimeType?: string };

export type ContentPart =
  | { type: "text"; text: string }
  | { type: "image" | "audio" | "video" | "document"; source: PartSource };

export type Content = string | ContentPart[];

export interface ToolCall {
  id: string;
  type: "function";
  function: { name: string; arguments: string };
}

export type Message =
  | { role: "developer" | "system"; id: string; content: string }
  | { role: "assistant"; id: string; content?: string; toolCalls?: ToolCall[]; name?: string }
  | { role: "user"; id: string; content: Content; name?: string }
  | { role: "tool"; id: string; content: Content; toolCallId: string; error?: string }
  | { role: "activity"; id: string; activityType: string; content: JsonObject }
  | { role: "reasoning"; id: string; content: string };

export interface Interrupt {
  id: string;
  reason: string;
  message?: string;
  toolCallId?: string;
  responseSchema?: JsonObject;
  expiresAt?: string;
  metadata?: JsonObject;
}

/// Why a run ended. An outcome of another `type` (a newer producer's) reads as success.
export type RunOutcome =
  | { type: "success" }
  | { type: "interrupt"; interrupts: Interrupt[] }
  | { type: "cancelled" }
  | { type: string & {} };

export interface Resume {
  interruptId: string;
  status: "resolved" | "cancelled";
  payload?: Json;
  metadata?: JsonObject;
}

export interface RunAgentInput {
  threadId: string;
  runId: string;
  protocolVersion?: string;
  parentRunId?: string;
  state?: Json;
  messages: Message[];
  tools?: { name: string; description: string; parameters?: Json }[];
  context?: { description: string; value: string }[];
  forwardedProps?: Json;
  resume?: Resume[];
}

export type Event = Base &
  (
    | { type: "RUN_STARTED"; threadId: string; runId: string; protocolVersion?: string; parentRunId?: string; input?: RunAgentInput }
    | { type: "RUN_FINISHED"; threadId: string; runId: string; result?: Json; outcome?: RunOutcome }
    | { type: "RUN_ERROR"; message: string; code?: string }
    | { type: "TEXT_MESSAGE_START"; messageId: string; role?: string }
    | { type: "TEXT_MESSAGE_CONTENT"; messageId: string; delta: string }
    | { type: "TEXT_MESSAGE_END"; messageId: string }
    | { type: "REASONING_START"; messageId: string }
    | { type: "REASONING_MESSAGE_START"; messageId: string; role: "reasoning" }
    | { type: "REASONING_MESSAGE_CONTENT"; messageId: string; delta: string }
    | { type: "REASONING_MESSAGE_END"; messageId: string }
    | { type: "REASONING_END"; messageId: string }
    | { type: "TOOL_CALL_START"; toolCallId: string; toolCallName: string; parentMessageId?: string }
    | { type: "TOOL_CALL_ARGS"; toolCallId: string; delta: string }
    | { type: "TOOL_CALL_END"; toolCallId: string }
    | { type: "TOOL_CALL_RESULT"; messageId: string; toolCallId: string; content: Content; role?: "tool" }
    | { type: "ACTIVITY_SNAPSHOT"; messageId: string; activityType: string; content: JsonObject; replace?: boolean }
    | { type: "MESSAGES_SNAPSHOT"; messages: Message[] }
    | { type: "CUSTOM"; name: string; value: Json }
  );

export type EventType = Event["type"];

/// The fields each known type must have, and of what kind (the schema's `required`).
const REQUIRED: Record<EventType, Record<string, "string" | "object" | "array" | "any">> = {
  RUN_STARTED: { threadId: "string", runId: "string" },
  RUN_FINISHED: { threadId: "string", runId: "string" },
  RUN_ERROR: { message: "string" },
  TEXT_MESSAGE_START: { messageId: "string" },
  TEXT_MESSAGE_CONTENT: { messageId: "string", delta: "string" },
  TEXT_MESSAGE_END: { messageId: "string" },
  REASONING_START: { messageId: "string" },
  REASONING_MESSAGE_START: { messageId: "string", role: "string" },
  REASONING_MESSAGE_CONTENT: { messageId: "string", delta: "string" },
  REASONING_MESSAGE_END: { messageId: "string" },
  REASONING_END: { messageId: "string" },
  TOOL_CALL_START: { toolCallId: "string", toolCallName: "string" },
  TOOL_CALL_ARGS: { toolCallId: "string", delta: "string" },
  TOOL_CALL_END: { toolCallId: "string" },
  TOOL_CALL_RESULT: { messageId: "string", toolCallId: "string", content: "any" },
  ACTIVITY_SNAPSHOT: { messageId: "string", activityType: "string", content: "object" },
  MESSAGES_SNAPSHOT: { messages: "array" },
  CUSTOM: { name: "string", value: "any" },
};

export const EVENT_TYPES = Object.keys(REQUIRED) as EventType[];

function kindOf(v: unknown): "string" | "object" | "array" | "other" {
  if (typeof v === "string") return "string";
  if (Array.isArray(v)) return "array";
  if (v !== null && typeof v === "object") return "object";
  return "other";
}

/// An event as received, or null when it's of a type this page doesn't know (a newer core's) or
/// lacks a field its type needs: such an event is skipped, never fatal.
export function parseEvent(raw: unknown): Event | null {
  if (kindOf(raw) !== "object") return null;
  const obj = raw as Record<string, unknown>;
  const type = obj.type;
  if (typeof type !== "string" || !Object.hasOwn(REQUIRED, type)) return null;
  for (const [field, kind] of Object.entries(REQUIRED[type as EventType])) {
    if (!(field in obj) || (kind !== "any" && kindOf(obj[field]) !== kind)) return null;
  }
  return obj as unknown as Event;
}

/// A command: exactly a `POST /v1/commands` body (`schema/command.schema.json`).
export type Command =
  | ({ type: "run" } & RunAgentInput)
  | { type: "stop"; threadId: string; runId: string }
  /// A new conversation in place of `threadId`'s; answered with the new `threadId`.
  | { type: "new_conversation"; threadId: string }
  /// Retries the reply of run `replyRunId` (its live run's id, or a snapshot's) as run `runId`.
  | { type: "retry"; threadId: string; runId: string; replyRunId: string };

/// The app's answer to a command: the HTTP status `POST /v1/commands` would give, and its body.
export interface Answer {
  status: number;
  body: { threadId?: string; runId?: string; error?: string } & Record<string, unknown>;
}

/// What the page posts to the app (`docs/web-view.md`).
export type BridgeMessage = { type: "join" } | { type: "command"; id: string; command: Command } | { type: "open"; url: string };
