import { describe, expect, it } from "vitest";
import schemaText from "../../crates/ailfred-protocol/schema/command.schema.json?raw";
import { answerCommand, answerText, declineCommand, newConversationCommand, newRunId, retryCommand, sendCommand, stopCommand } from "../src/commands";
import type { Command } from "../src/protocol";
import { fixture } from "./fixtures";

const [image, answered, stop, declined, newConversation, retry] = fixture("commands") as Command[];

describe("the commands the page builds", () => {
  it("a message with an image is the fixture's run", () => {
    const png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    expect(sendCommand("thread-1", "run-image", "What's in this screenshot?", [{ mimeType: "image/png", data: png }])).toEqual(image);
  });

  it("a message with text only sends it as plain content", () => {
    expect(sendCommand("t", "r", "Hello", [])).toEqual({ type: "run", threadId: "t", runId: "r", protocolVersion: "1.0", messages: [{ role: "user", id: "r-user", content: "Hello" }] });
  });

  it("an answer resumes the question by its interrupt id, as the fixture does", () => {
    const answer = answerText(["src/a.rs", "src/b.rs", "src/c.rs"], [2, 0], "");
    const command = answerCommand("thread-1", "run-answered", "run-question-ask-1", answer);
    // The server keeps its own history: the page sends none.
    expect(command).toEqual({ ...answered, messages: [] });
  });

  it("an answer is what was written in Other, else the picks in their order", () => {
    expect(answerText(["a", "b", "c"], [2, 0], "  ")).toBe("a, c");
    expect(answerText(["a", "b"], [1], " mine ")).toBe("mine");
    expect(answerText(["a"], [], "")).toBe("");
  });

  it("a stop is the fixture's stop", () => {
    expect(stopCommand("thread-1", "run-stop")).toEqual(stop);
  });

  it("a decline resumes the question cancelled, without a payload, as the fixture does", () => {
    expect(declineCommand("thread-1", "run-declined", "run-question-ask-1")).toEqual({ ...declined, messages: [] });
  });

  it("a new conversation and a retry are the fixture's", () => {
    expect(newConversationCommand("thread-1")).toEqual(newConversation);
    expect(retryCommand("thread-1", "run-retry", "run-question")).toEqual(retry);
  });

  it("a fresh run id is web- and at most 128 of the characters the server takes", () => {
    const ids = new Set(Array.from({ length: 50 }, newRunId));
    expect(ids.size).toBe(50);
    for (const id of ids) expect(id).toMatch(/^web-[A-Za-z0-9._:-]{1,124}$/);
  });
});

describe("the command schema's required fields", () => {
  type Schema = { required?: string[]; properties?: Record<string, { const?: string }>; oneOf?: Schema[]; allOf?: Schema[]; $ref?: string };
  const schema = JSON.parse(schemaText) as Schema & { $defs: Record<string, Schema> };
  const def = (ref: string) => schema.$defs[ref.replace("#/$defs/", "")];
  /// The variant of `oneOf` whose `tag` field's const is `value`.
  const variant = (of: Schema, tag: string, value: string) => of.oneOf!.find((v) => v.properties?.[tag]?.const === value)!;
  const missing = (required: string[] | undefined, obj: object) => (required ?? []).filter((k) => !(k in obj));

  function check(command: Command) {
    const errors: string[] = [];
    if (command.type !== "run") {
      const of = variant(schema, "type", command.type);
      if (!of) return [`no ${command.type} in the schema`];
      errors.push(...missing(of.required, command));
    } else {
      const run = schema.oneOf!.find((v) => v.allOf)!;
      errors.push(...missing(def(run.allOf![0].$ref!).required, command), ...missing(run.allOf![1].required, command));
      for (const m of command.messages) {
        errors.push(...missing(variant(def("#/$defs/Message"), "role", m.role).required, m).map((k) => `${m.id}.${k}`));
        if (m.role === "user" && Array.isArray(m.content)) {
          for (const part of m.content) {
            errors.push(...missing(variant(def("#/$defs/ContentPart"), "type", part.type).required, part));
            if ("source" in part) errors.push(...missing(variant(def("#/$defs/PartSource"), "type", part.source.type).required, part.source));
          }
        }
      }
      for (const r of command.resume ?? []) errors.push(...missing(def("#/$defs/Resume").required, r));
    }
    return errors;
  }

  it("every command the page builds has them", () => {
    expect(check(sendCommand("t", "r", "hi", [{ mimeType: "image/png", data: "AAAA" }]))).toEqual([]);
    expect(check(answerCommand("t", "r", "i", "a"))).toEqual([]);
    expect(check(stopCommand("t", "r"))).toEqual([]);
    expect(check(declineCommand("t", "r", "i"))).toEqual([]);
    expect(check(newConversationCommand("t"))).toEqual([]);
    expect(check(retryCommand("t", "r", "r0"))).toEqual([]);
    expect(check({ type: "stop", runId: "r" } as Command)).toEqual(["threadId"]);
    expect(check({ type: "retry", threadId: "t", runId: "r" } as Command)).toEqual(["replyRunId"]);
    expect(check({ type: "jump", threadId: "t" } as unknown as Command)).toEqual(["no jump in the schema"]);
  });
});
