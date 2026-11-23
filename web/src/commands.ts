// The commands the page sends: exactly `POST /v1/commands` bodies (`docs/local-server.md`), as
// `crates/ailfred-protocol/src/command.rs` builds them. The app keeps the conversation's history
// itself (a run's earlier messages aren't replayed to the model), so the page sends only the new
// user message, and none with an answer.

import { PROTOCOL_VERSION, type Command, type ContentPart, type Message } from "./protocol";

/// An image to send: base64 bytes and their type (`image/png`).
export interface Image {
  mimeType: string;
  data: string;
}

/// A fresh run id: `web-` and random letters and digits, well within the server's 1 to 128
/// characters of `[A-Za-z0-9._:-]`.
export function newRunId(): string {
  const bytes = new Uint8Array(12);
  crypto.getRandomValues(bytes);
  return `web-${Array.from(bytes, (b) => b.toString(36).padStart(2, "0")).join("")}`;
}

/// A user message: its text, then its images as inline parts; plain text when there are none
/// (`command::user_message`).
export function userMessage(id: string, text: string, images: Image[]): Message {
  if (images.length === 0) return { role: "user", id, content: text };
  const parts: ContentPart[] = [{ type: "text", text }, ...images.map((i): ContentPart => ({ type: "image", source: { type: "data", value: i.data, mimeType: i.mimeType } }))];
  return { role: "user", id, content: parts };
}

/// Run `runId` of thread `threadId`: the user says `text` with `images`.
export function sendCommand(threadId: string, runId: string, text: string, images: Image[]): Command {
  return { type: "run", threadId, runId, protocolVersion: PROTOCOL_VERSION, messages: [userMessage(`${runId}-user`, text, images)] };
}

/// Run `runId` of thread `threadId`: the user answers question `interruptId` with `answer`.
export function answerCommand(threadId: string, runId: string, interruptId: string, answer: string): Command {
  return { type: "run", threadId, runId, protocolVersion: PROTOCOL_VERSION, messages: [], resume: [{ interruptId, status: "resolved", payload: { answer } }] };
}

/// Stop run `runId` of thread `threadId`, the one streaming.
export function stopCommand(threadId: string, runId: string): Command {
  return { type: "stop", threadId, runId };
}

/// A question's answer (`Reply::answer`): what was written in its Other field, else the options
/// picked, in their order, joined with `", "`. Empty when there's neither.
export function answerText(options: string[], picked: number[], other: string): string {
  if (other.trim() !== "") return other.trim();
  return [...picked]
    .sort((a, b) => a - b)
    .flatMap((i) => (i >= 0 && i < options.length ? [options[i]] : []))
    .join(", ");
}
