import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { Markdown } from "../src/components/Markdown";
import { QuestionView } from "../src/components/Question";
import { ConversationView } from "../src/components/Conversation";
import { applyAll, empty } from "../src/state";
import { fixture } from "./fixtures";

const noop = () => {};

describe("markdown", () => {
  it("renders the model's markdown, and raw html as nothing", () => {
    const html = renderToStaticMarkup(<Markdown text={"Hello **world**\n\n<script>alert(1)</script>\n\n<img src=x onerror=alert(1)> ok"} onOpen={noop} />);
    expect(html).toContain("<strong>world</strong>");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("<img");
    expect(html).not.toContain("onerror");
  });

  it("renders gfm tables, and links the page opens itself", () => {
    const html = renderToStaticMarkup(<Markdown text={"| a | b |\n|---|---|\n| 1 | 2 |\n\n[site](https://example.com) and [bad](javascript:alert(1))"} onOpen={noop} />);
    expect(html).toContain("<table>");
    expect(html).toContain('href="https://example.com"');
    expect(html).not.toContain("javascript:");
  });
});

describe("a question", () => {
  const question = { kind: "question" as const, interruptId: "i", prompt: "Which ones?", options: ["src/a.rs", "src/b.rs"], multi: true, answered: null, closed: false };

  it("shows its prompt, an option button each, and an Other field while open", () => {
    const html = renderToStaticMarkup(<QuestionView part={question} open onAnswer={noop} />);
    expect(html).toContain("Which ones?");
    expect(html.match(/<button[^>]*class="option[^"]*"/g)).toHaveLength(2);
    expect(html).toContain("src/a.rs");
    expect(html).toContain('placeholder="Other…"');
  });

  it("shows its answer, and no buttons, once answered", () => {
    const html = renderToStaticMarkup(<QuestionView part={{ ...question, answered: "src/b.rs" }} open={false} onAnswer={noop} />);
    expect(html).toContain("src/b.rs");
    expect(html).not.toContain("<button");
    expect(html).not.toContain("<input");
  });
});

describe("the conversation", () => {
  const props = { onOpen: noop, onAnswer: noop };

  it("shows steps with their targets and outputs", () => {
    const html = renderToStaticMarkup(<ConversationView state={applyAll(empty(), fixture("steps"))} {...props} />);
    expect(html).toContain("src/main.rs");
    expect(html).toContain("cannot find value");
    expect(html).toContain("failed");
  });

  it("keeps thinking collapsed, and shows an error", () => {
    const thought = renderToStaticMarkup(<ConversationView state={applyAll(empty(), fixture("answer"))} {...props} />);
    expect(thought).toMatch(/<details class="thinking"(?![^>]*open)/);
    const failed = renderToStaticMarkup(<ConversationView state={applyAll(empty(), fixture("error"))} {...props} />);
    expect(failed).toContain("overloaded");
  });

  it("draws a user message's images from their data urls", () => {
    const started = { type: "RUN_STARTED", threadId: "t", runId: "r", input: { threadId: "t", runId: "r", messages: [{ role: "user", id: "r-user", content: [{ type: "text", text: "See" }, { type: "image", source: { type: "data", value: "AAAA", mimeType: "image/png" } }] }] } };
    const html = renderToStaticMarkup(<ConversationView state={applyAll(empty(), [started])} {...props} />);
    expect(html).toContain('src="data:image/png;base64,AAAA"');
  });
});
