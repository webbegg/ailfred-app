import { describe, expect, it } from "vitest";
import { applyAll, apply, empty, local, canAnswer, type Conversation, type ReplyTurn, type Part } from "../src/state";
import { EVENT_FIXTURES, fixture } from "./fixtures";

function replay(...names: string[]): Conversation {
  return names.reduce((state, name) => applyAll(state, fixture(name)), empty());
}

function replies(state: Conversation): ReplyTurn[] {
  return state.items.filter((i): i is ReplyTurn => i.kind === "reply");
}

function only(state: Conversation): ReplyTurn {
  const all = replies(state);
  expect(all).toHaveLength(1);
  return all[0];
}

function partsOf<K extends Part["kind"]>(reply: ReplyTurn, kind: K): Extract<Part, { kind: K }>[] {
  return reply.parts.filter((p): p is Extract<Part, { kind: K }> => p.kind === kind);
}

describe("replaying the fixtures", () => {
  it("knows every fixture the protocol crate has", () => {
    expect(EVENT_FIXTURES).toEqual(["answer", "diff", "error", "lost", "question", "steered", "steps", "stop"]);
  });

  it.each(EVENT_FIXTURES)("%s replays to a reply that is no longer streaming", (name) => {
    const state = replay(name);
    expect(state.run).toBeNull();
    expect(only(state).state).not.toBe("streaming");
  });

  it("an answer shows its thinking collapsed and done, then its markdown text", () => {
    const reply = only(replay("answer"));
    expect(reply.state).toBe("done");
    expect(reply.parts).toEqual([
      { kind: "thinking", text: "The user wants a greeting.", done: true },
      { kind: "text", text: "Hello, **world**." },
    ]);
  });

  it("steps keep their name, target, output and status, in runs between texts", () => {
    const reply = only(replay("steps"));
    expect(reply.parts.map((p) => p.kind)).toEqual(["text", "steps", "text", "steps"]);
    const [first, second] = partsOf(reply, "steps");
    expect(first.steps).toEqual([
      { id: "run-steps-step-1", name: "read", target: "src/main.rs", output: "fn main() {\n}\n", status: "done" },
      { id: "run-steps-step-2", name: "run", target: "cargo test", output: "error[E0425]: cannot find value `x`\n", status: "failed" },
    ]);
    expect(second.steps).toEqual([{ id: "run-steps-step-3", name: "search", target: "fn main", output: "", status: "done" }]);
  });

  it("a question shows its options, open while its reply waits for the answer", () => {
    const state = replay("question");
    const reply = only(state);
    expect(reply.state).toBe("done");
    const [q] = partsOf(reply, "question");
    expect(q).toEqual({ kind: "question", interruptId: "run-question-ask-1", prompt: "Which ones?", options: ["src/a.rs", "src/b.rs", "src/c.rs"], multi: true, answered: null, closed: false });
    expect(partsOf(reply, "question")).toHaveLength(1);
    expect(canAnswer(state, reply, q)).toBe(true);
  });

  it("a question answered is read only, and so is one a later run follows", () => {
    const asked = replay("question");
    const answered = local(asked, { type: "answered", interruptId: "run-question-ask-1", answer: "src/a.rs, src/c.rs" });
    const [q] = partsOf(only(answered), "question");
    expect(q.answered).toBe("src/a.rs, src/c.rs");
    expect(canAnswer(answered, only(answered), q)).toBe(false);
    const later = applyAll(asked, fixture("answer"));
    const [first] = replies(later);
    expect(canAnswer(later, first, partsOf(first, "question")[0])).toBe(false);
  });

  it("a diff shows its path and patch", () => {
    const reply = only(replay("diff"));
    expect(partsOf(reply, "diff")).toEqual([{ kind: "diff", path: "src/lib.rs", patch: "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-let x = 1;\n+let x = 2;\n" }]);
  });

  it("an error ends the reply failed and shows its message", () => {
    const reply = only(replay("error"));
    expect(reply.state).toBe("failed");
    expect(reply.parts).toEqual([
      { kind: "text", text: "Let me" },
      { kind: "error", message: "overloaded" },
    ]);
  });

  it("a lost model process stops the reply as interrupted, with no error part", () => {
    const reply = only(replay("lost"));
    expect(reply.state).toBe("stopped");
    expect(reply.interrupted).toBe(true);
    expect(reply.parts).toEqual([{ kind: "text", text: "Half an ans" }]);
  });

  it("a stopped run keeps what came, and is not interrupted", () => {
    const reply = only(replay("stop"));
    expect(reply.state).toBe("stopped");
    expect(reply.interrupted).toBe(false);
    expect(partsOf(reply, "steps")[0].steps[0]).toMatchObject({ target: "cargo build", output: "Compiling ailfred\n", status: "failed" });
  });

  it("a steered run follows the one before it, in order", () => {
    const state = replay("question", "steered");
    expect(replies(state).map((r) => r.runId)).toEqual(["run-question", "run-steered"]);
    expect(replies(state)[1].parts).toEqual([{ kind: "text", text: "Got it, switching." }]);
  });
});

describe("the run going", () => {
  it("streams from its RUN_STARTED until its end", () => {
    const lines = fixture("steps");
    let state = empty();
    state = apply(state, lines[0]);
    expect(state.run).toBe("run-steps");
    expect(state.threadId).toBe("thread-1");
    state = applyAll(state, lines.slice(1, 9));
    const reply = only(state);
    expect(reply.state).toBe("streaming");
    expect(partsOf(reply, "steps")[0].steps[0]).toMatchObject({ output: "fn main() {\n}\n", status: "running" });
    state = applyAll(state, lines.slice(9));
    expect(state.run).toBeNull();
  });

  it("shows the user message its RUN_STARTED carries before its reply", () => {
    const started = { type: "RUN_STARTED", threadId: "t", runId: "r1", input: { threadId: "t", runId: "r1", messages: [{ role: "user", id: "r1-user", content: [{ type: "text", text: "Look" }, { type: "image", source: { type: "data", value: "AAAA", mimeType: "image/png" } }] }] } };
    const state = apply(empty(), started);
    expect(state.items).toEqual([
      { kind: "user", id: "r1-user", text: "Look", images: ["data:image/png;base64,AAAA"] },
      { kind: "reply", runId: "r1", parts: [], state: "streaming", interrupted: false },
    ]);
  });

  it("a message the page sent shows until its run starts, and a refused one goes", () => {
    let state = local(empty(), { type: "sent", runId: "web-1", text: "Hi", images: [] });
    expect(state.pending).toEqual([{ kind: "user", id: "web-1-user", text: "Hi", images: [] }]);
    state = apply(state, { type: "RUN_STARTED", threadId: "t", runId: "web-1" });
    expect(state.pending).toEqual([]);
    expect(state.items[0]).toEqual({ kind: "user", id: "web-1-user", text: "Hi", images: [] });
    const refused = local(local(empty(), { type: "sent", runId: "web-2", text: "Yo", images: [] }), { type: "refused", runId: "web-2" });
    expect(refused.pending).toEqual([]);
  });

  it("a message the page sent stays through a snapshot of its conversation that hasn't it yet", () => {
    const snap = (threadId: string, messages: { role: "user"; id: string; content: string }[]) => ({ type: "MESSAGES_SNAPSHOT", metadata: { ailfred: { threadId } }, messages });
    const hi = { kind: "user", id: "web-1-user", text: "Hi", images: [] };
    let state = apply(empty(), snap("t", []));
    state = local(state, { type: "sent", runId: "web-1", text: "Hi", images: [] });
    state = apply(state, snap("t", [{ role: "user", id: "t-0-user", content: "Earlier" }]));
    expect(state.pending).toEqual([hi]);
    expect(state.items.map((i) => i.kind === "user" && i.text)).toEqual(["Earlier"]);
    expect(apply(state, snap("t", [{ role: "user", id: "web-1-user", content: "Hi" }])).pending).toEqual([]);
    expect(apply(state, { type: "RUN_STARTED", threadId: "t", runId: "web-1" }).pending).toEqual([]);
    // Another conversation: it was sent to the old one.
    expect(apply(state, snap("t2", [])).pending).toEqual([]);
  });

  it("a question another producer only asks at the end shows once", () => {
    const events = [
      { type: "RUN_STARTED", threadId: "t", runId: "r" },
      { type: "RUN_FINISHED", threadId: "t", runId: "r", outcome: { type: "interrupt", interrupts: [{ id: "i", reason: "input_required", message: "Which?", metadata: { ailfred: { multi: false, options: ["a"] } } }] } },
    ];
    const [q] = partsOf(only(applyAll(empty(), events)), "question");
    expect(q).toMatchObject({ interruptId: "i", prompt: "Which?", options: ["a"], multi: false });
  });

  it("an outcome of a type it doesn't know ends the run done", () => {
    const state = applyAll(empty(), [
      { type: "RUN_STARTED", threadId: "t", runId: "r" },
      { type: "RUN_FINISHED", threadId: "t", runId: "r", outcome: { type: "suspended", until: "later" } },
    ]);
    expect(only(state).state).toBe("done");
  });

  it("events before a run, after its end, or naming another run are skipped", () => {
    const text = { type: "TEXT_MESSAGE_CONTENT", messageId: "m", delta: "x" };
    expect(apply(empty(), text)).toEqual(empty());
    let state = applyAll(empty(), [{ type: "RUN_STARTED", threadId: "t", runId: "r" }, { type: "RUN_FINISHED", threadId: "t", runId: "other" }]);
    expect(state.run).toBe("r");
    state = applyAll(state, [{ type: "RUN_FINISHED", threadId: "t", runId: "r" }, text]);
    expect(only(state).parts).toEqual([]);
  });

  it("a run error after the run finished marks it failed after all", () => {
    const state = applyAll(empty(), fixture("answer").concat([{ type: "RUN_ERROR", message: "late" }]));
    const reply = only(state);
    expect(reply.state).toBe("failed");
    expect(reply.parts.at(-1)).toEqual({ kind: "error", message: "late" });
  });
});

describe("what isn't a known event", () => {
  it("an unknown event type, an unknown custom event, or a broken event is ignored", () => {
    const going = apply(empty(), { type: "RUN_STARTED", threadId: "t", runId: "r" });
    for (const event of [{ type: "STATE_DELTA", delta: [] }, { type: "CUSTOM", name: "someone:else", value: 1 }, { type: "TEXT_MESSAGE_CONTENT", messageId: "m" }, null, "text", 42]) {
      expect(apply(going, event)).toEqual(going);
    }
  });
});

describe("a messages snapshot", () => {
  const snapshot = {
    type: "MESSAGES_SNAPSHOT",
    metadata: { ailfred: { threadId: "thread-2" } },
    messages: [
      { role: "user", id: "c-0-user", content: "Fix it" },
      { role: "reasoning", id: "c-0-thinking-1", content: "Hmm." },
      { role: "assistant", id: "c-0-text-1", content: "Looking." },
      { role: "assistant", id: "c-0-steps-1", toolCalls: [{ id: "c-0-step-1", type: "function", function: { name: "read", arguments: '{"target":"a.rs"}' } }, { id: "c-0-step-2", type: "function", function: { name: "run", arguments: '{"target":"make"}' } }] },
      { role: "tool", id: "c-0-step-1-result", toolCallId: "c-0-step-1", content: "fn a() {}" },
      { role: "tool", id: "c-0-step-2-result", toolCallId: "c-0-step-2", content: "boom", error: "The step failed." },
      { role: "activity", id: "c-0-diff-1", activityType: "ailfred:diff", content: { patch: "+x", path: "a.rs" } },
      { role: "activity", id: "c-0-ask-1", activityType: "ailfred:ask", content: { interruptId: "c-0-ask-1", message: "Which?", multi: false, options: ["A", "B"], answered: "A" } },
      { role: "user", id: "c-1-user", content: "And this?" },
      { role: "activity", id: "c-1-ask-1", activityType: "ailfred:ask", content: { interruptId: "c-1-ask-1", message: "Sure?", multi: false, options: ["Yes"], closed: true } },
      { role: "activity", id: "c-1-ask-2", activityType: "ailfred:ask", content: { interruptId: "c-1-ask-2", message: "Really?", multi: false, options: ["Yes"] } },
      { role: "activity", id: "c-1-error-1", activityType: "ailfred:error", content: { message: "overloaded" } },
      { role: "activity", id: "c-1-other", activityType: "someone:else", content: {} },
    ],
  };

  it("replaces all the state, and takes its thread id from metadata", () => {
    const before = local(replay("steps"), { type: "sent", runId: "web-1", text: "x", images: [] });
    const state = apply(before, snapshot);
    expect(state.threadId).toBe("thread-2");
    expect(state.run).toBeNull();
    expect(state.pending).toEqual([]);
    expect(state.items.map((i) => (i.kind === "user" ? i.text : i.runId))).toEqual(["Fix it", "c-0", "And this?", "c-1"]);
  });

  it("reads back each part, its steps ended as they were, and a failed reply's questions closed", () => {
    const state = apply(empty(), snapshot);
    const [first, second] = replies(state);
    expect(first.state).toBe("done");
    expect(first.parts).toEqual([
      { kind: "thinking", text: "Hmm.", done: true },
      { kind: "text", text: "Looking." },
      {
        kind: "steps",
        steps: [
          { id: "c-0-step-1", name: "read", target: "a.rs", output: "fn a() {}", status: "done" },
          { id: "c-0-step-2", name: "run", target: "make", output: "boom", status: "failed" },
        ],
      },
      { kind: "diff", path: "a.rs", patch: "+x" },
      { kind: "question", interruptId: "c-0-ask-1", prompt: "Which?", options: ["A", "B"], multi: false, answered: "A", closed: false },
    ]);
    expect(second.state).toBe("failed");
    expect(second.parts.map((p) => (p.kind === "question" ? [p.interruptId, p.closed] : p.kind))).toEqual([["c-1-ask-1", true], ["c-1-ask-2", true], "error"]);
  });

  it("after a decline, the snapshot replaces the state with the question closed", () => {
    const asked = replay("question");
    const reply = only(asked);
    const [question] = partsOf(reply, "question");
    expect(canAnswer(asked, reply, question)).toBe(true);
    const declined = apply(asked, {
      type: "MESSAGES_SNAPSHOT",
      metadata: { ailfred: { threadId: "thread-1" } },
      messages: [
        { role: "user", id: "t-0-user", content: "Tidy up" },
        { role: "activity", id: "t-0-ask-1", activityType: "ailfred:ask", content: { interruptId: "t-0-ask-1", message: question.prompt, multi: true, options: question.options, closed: true } },
      ],
    });
    const after = only(declined);
    const [closed] = partsOf(after, "question");
    expect(after.runId).toBe("t-0");
    expect(closed.closed).toBe(true);
    expect(canAnswer(declined, after, closed)).toBe(false);
  });

  it("after a new conversation, the empty snapshot leaves nothing and the new thread", () => {
    const before = local(replay("answer", "steps"), { type: "sent", runId: "web-1", text: "x", images: [] });
    const fresh = apply(before, { type: "MESSAGES_SNAPSHOT", metadata: { ailfred: { threadId: "thread-new" } }, messages: [] });
    expect(fresh).toEqual({ ...empty(), threadId: "thread-new" });
  });

  it("after a retry, the snapshot without the retried message, then its new run, show one reply to it", () => {
    const done = apply(empty(), snapshot);
    const retried = applyAll(done, [
      { ...snapshot, messages: snapshot.messages.slice(0, 8) },
      { type: "RUN_STARTED", threadId: "thread-2", runId: "web-r", input: { threadId: "thread-2", runId: "web-r", messages: [{ role: "user", id: "web-r-user", content: "And this?" }] } },
    ]);
    expect(retried.items.map((i) => (i.kind === "user" ? i.text : i.runId))).toEqual(["Fix it", "c-0", "And this?", "web-r"]);
  });

  it("the run going after a snapshot streams on top of it", () => {
    const state = applyAll(empty(), [snapshot, { type: "RUN_STARTED", threadId: "thread-2", runId: "live-1" }]);
    expect(state.run).toBe("live-1");
    expect(replies(state).map((r) => r.runId)).toEqual(["c-0", "c-1", "live-1"]);
  });
});
