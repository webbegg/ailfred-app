import { describe, expect, it } from "vitest";
import { BridgeTransport, FixtureTransport, chooseTransport, type BridgeWindow } from "../src/transport";
import { stopCommand } from "../src/commands";
import { fixture } from "./fixtures";

function fakeWindow() {
  const posted: unknown[] = [];
  const win: BridgeWindow = { webkit: { messageHandlers: { ailfred: { postMessage: (text: string) => posted.push(JSON.parse(text)) } } } };
  return { win, posted };
}

describe("the bridge transport", () => {
  it("posts a join once started, and delivers the app's events in order", () => {
    const { win, posted } = fakeWindow();
    const transport = new BridgeTransport(win);
    const got: unknown[] = [];
    // A frame that comes before the page starts listening is kept for it.
    win.ailfred!.receive([{ type: "early" }]);
    expect(posted).toEqual([]);
    transport.start((events) => got.push(...events));
    expect(posted).toEqual([{ type: "join" }]);
    win.ailfred!.receive([{ type: "a" }, { type: "b" }]);
    win.ailfred!.receive([{ type: "c" }]);
    expect(got).toEqual([{ type: "early" }, { type: "a" }, { type: "b" }, { type: "c" }]);
  });

  it("matches each answer to its command by id", async () => {
    const { win, posted } = fakeWindow();
    const transport = new BridgeTransport(win);
    transport.start(() => {});
    const first = transport.send(stopCommand("t", "r1"));
    const second = transport.send(stopCommand("t", "r2"));
    const [, a, b] = posted as { type: string; id: string; command: unknown }[];
    expect(a).toEqual({ type: "command", id: a.id, command: stopCommand("t", "r1") });
    expect(b.id).not.toBe(a.id);
    win.ailfred!.answer(b.id, 409, { error: "nothing to stop" });
    win.ailfred!.answer(a.id, 202, '{"threadId":"t","runId":"r1"}');
    win.ailfred!.answer("unknown", 202, {});
    expect(await first).toEqual({ status: 202, body: { threadId: "t", runId: "r1" } });
    expect(await second).toEqual({ status: 409, body: { error: "nothing to stop" } });
  });

  it("asks the app to open http and https links only", () => {
    const { win, posted } = fakeWindow();
    const transport = new BridgeTransport(win);
    transport.open("https://example.com/a");
    transport.open("http://example.com/b");
    transport.open("javascript:alert(1)");
    transport.open("file:///etc/passwd");
    transport.open("not a url");
    expect(posted).toEqual([
      { type: "open", url: "https://example.com/a" },
      { type: "open", url: "http://example.com/b" },
    ]);
  });

  it("is the one chosen inside the app's web view", () => {
    expect(chooseTransport(fakeWindow().win)).toBeInstanceOf(BridgeTransport);
    expect(chooseTransport({})).toBeNull();
  });
});

describe("the fixture transport", () => {
  it("starts the fixture's thread with an empty snapshot, then replays its events in order", async () => {
    const lines = fixture("answer");
    const transport = new FixtureTransport(lines, 0);
    const got: unknown[] = [];
    transport.start((events) => got.push(...events));
    await transport.done;
    expect((got[0] as { type: string }).type).toBe("MESSAGES_SNAPSHOT");
    expect(got.slice(1)).toEqual(lines);
  });
});
