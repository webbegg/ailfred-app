// How the page talks to the app: over the web view's bridge (`docs/web-view.md`), or, in
// `npm run dev`, to a fixture replayed in the browser.

import type { Answer, BridgeMessage, Command } from "./protocol";

export interface Transport {
  /// Starts listening: `onEvents` gets each frame of AG-UI events, in order.
  start(onEvents: (events: unknown[]) => void): void;
  /// Sends `command`; resolves with the app's answer.
  send(command: Command): Promise<Answer>;
  /// Opens `url` outside the page (the default browser), if it's `http:` or `https:`.
  open(url: string): void;
}

/// What the page needs of `window` inside the app's web view.
export interface BridgeWindow {
  webkit?: { messageHandlers?: { ailfred?: { postMessage(text: string): void } } };
  ailfred?: {
    receive(events: unknown[]): void;
    answer(id: string, status: number, body: unknown): void;
  };
}

/// Whether `url` is a link the app opens: `http:` or `https:`.
export function openable(url: string): boolean {
  try {
    const { protocol } = new URL(url);
    return protocol === "http:" || protocol === "https:";
  } catch {
    return false;
  }
}

/// The bridge to the app: it defines `window.ailfred.receive` and `window.ailfred.answer` for
/// the app to call, and posts JSON to `window.webkit.messageHandlers.ailfred`.
export class BridgeTransport implements Transport {
  private onEvents: ((events: unknown[]) => void) | null = null;
  /// Frames that came before `start`.
  private early: unknown[][] = [];
  private waiting = new Map<string, (answer: Answer) => void>();
  private next = 1;
  private readonly win: BridgeWindow;

  constructor(win: BridgeWindow) {
    this.win = win;
    win.ailfred = {
      receive: (events) => {
        const frame = Array.isArray(events) ? events : [];
        if (this.onEvents) this.onEvents(frame);
        else this.early.push(frame);
      },
      answer: (id, status, body) => {
        const resolve = this.waiting.get(id);
        if (!resolve) return;
        this.waiting.delete(id);
        resolve({ status: Number(status), body: answerBody(body) });
      },
    };
  }

  start(onEvents: (events: unknown[]) => void): void {
    this.onEvents = onEvents;
    for (const frame of this.early.splice(0)) onEvents(frame);
    this.post({ type: "join" });
  }

  send(command: Command): Promise<Answer> {
    const id = `c${this.next++}`;
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      try {
        this.post({ type: "command", id, command });
      } catch (e) {
        this.waiting.delete(id);
        resolve({ status: 503, body: { error: `the app didn't take it: ${String(e)}` } });
      }
    });
  }

  open(url: string): void {
    if (openable(url)) this.post({ type: "open", url });
  }

  private post(message: BridgeMessage): void {
    this.win.webkit?.messageHandlers?.ailfred?.postMessage(JSON.stringify(message));
  }
}

/// An answer's body: the app may give it as an object or as JSON text.
function answerBody(body: unknown): Answer["body"] {
  let value = body;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return { error: value as string };
    }
  }
  return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Answer["body"]) : {};
}

/// The bridge if the page is inside the app's web view, else null.
export function chooseTransport(win: BridgeWindow): Transport | null {
  return win.webkit?.messageHandlers?.ailfred ? new BridgeTransport(win) : null;
}

/// A fixture replayed for `npm run dev`: an empty snapshot of its thread, then its events, one
/// every `delay` ms. A message sent is answered with an echo, streamed word by word, that Stop
/// stops.
export class FixtureTransport implements Transport {
  private onEvents: ((events: unknown[]) => void) | null = null;
  private queue: { event: unknown; runId: string | null }[] = [];
  private timer: ReturnType<typeof setTimeout> | null = null;
  private going: string | null = null;
  private readonly threadId: string;
  private readonly delay: number;
  private finished: () => void = () => {};
  /// Resolves once the fixture's own events are out.
  readonly done: Promise<void>;

  constructor(events: unknown[], delay = 120) {
    this.delay = delay;
    const started = events.find((e): e is { type: string; threadId: string } => (e as { type?: string })?.type === "RUN_STARTED");
    this.threadId = started?.threadId ?? "fixture";
    this.done = new Promise((resolve) => (this.finished = resolve));
    this.queue.push({ event: { type: "MESSAGES_SNAPSHOT", messages: [], metadata: { ailfred: { threadId: this.threadId } } }, runId: null });
    for (const event of events) this.queue.push({ event, runId: null });
  }

  start(onEvents: (events: unknown[]) => void): void {
    this.onEvents = onEvents;
    this.pump();
  }

  async send(command: Command): Promise<Answer> {
    if (command.threadId !== this.threadId) return { status: 409, body: { error: "not this conversation" } };
    if (command.type === "stop") {
      if (command.runId !== this.going) return { status: 409, body: { error: "no such run streaming" } };
      this.queue = this.queue.filter((q) => q.runId !== command.runId);
      this.emit({ type: "RUN_FINISHED", threadId: this.threadId, runId: command.runId, outcome: { type: "cancelled" } });
      return { status: 202, body: { threadId: this.threadId, runId: command.runId } };
    }
    const runId = command.runId;
    const said = command.resume?.length ? `You answered: ${JSON.stringify(command.resume[0].payload)}` : "You said that. This is the fixture transport: no model here.";
    const user = command.messages.at(-1) ?? { role: "user", id: `${runId}-user`, content: said };
    const input = { threadId: this.threadId, runId, messages: [user] };
    const events: unknown[] = [{ type: "RUN_STARTED", threadId: this.threadId, runId, input }, { type: "TEXT_MESSAGE_START", messageId: `${runId}-text-1`, role: "assistant" }];
    for (const word of said.split(/(?<= )/)) events.push({ type: "TEXT_MESSAGE_CONTENT", messageId: `${runId}-text-1`, delta: word });
    events.push({ type: "TEXT_MESSAGE_END", messageId: `${runId}-text-1` }, { type: "RUN_FINISHED", threadId: this.threadId, runId, outcome: { type: "success" } });
    for (const event of events) this.queue.push({ event, runId });
    this.pump();
    return { status: 202, body: { threadId: this.threadId, runId } };
  }

  open(url: string): void {
    if (openable(url)) globalThis.open?.(url, "_blank", "noopener");
  }

  private emit(event: unknown): void {
    const e = event as { type?: string; runId?: string };
    if (e.type === "RUN_STARTED") this.going = e.runId ?? null;
    if (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR") this.going = null;
    this.onEvents?.([event]);
  }

  private pump(): void {
    if (this.timer !== null || !this.onEvents) return;
    if (this.queue.length === 0) {
      this.finished();
      return;
    }
    this.timer = setTimeout(() => {
      this.timer = null;
      const next = this.queue.shift();
      if (next) this.emit(next.event);
      this.pump();
    }, this.delay);
  }
}
