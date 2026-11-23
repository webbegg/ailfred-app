import { describe, expect, it } from "vitest";
import { build } from "vite";

const CSP = `default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:`;

/// The production build's files, in memory.
async function built(): Promise<{ fileName: string; source: string }[]> {
  const root = new URL("..", import.meta.url).pathname;
  // Vitest runs with NODE_ENV=test, which would make this a development build.
  const env = (globalThis as unknown as { process: { env: Record<string, string | undefined> } }).process.env;
  const was = env.NODE_ENV;
  env.NODE_ENV = "production";
  let result;
  try {
    result = await build({ root, configFile: `${root}vite.config.ts`, mode: "production", logLevel: "silent", build: { write: false } });
  } finally {
    env.NODE_ENV = was;
  }
  const outputs = (Array.isArray(result) ? result : [result]) as { output: { fileName: string; type: string; source?: string | Uint8Array; code?: string }[] }[];
  return outputs.flatMap((o) => o.output).map((f) => ({ fileName: f.fileName, source: f.type === "chunk" ? f.code! : String(f.source) }));
}

describe("the production build", () => {
  it("is one self-contained index.html that runs under the CSP", async () => {
    const files = await built();
    expect(files.map((f) => f.fileName)).toEqual(["index.html"]);
    const html = files[0].source;
    expect(html).toContain(`<meta http-equiv="Content-Security-Policy" content="${CSP}">`);
    // The markup outside the inlined script and style links to no file.
    const markup = html.replace(/<script>[\s\S]*?<\/script>/g, "<script></script>").replace(/<style>[\s\S]*?<\/style>/g, "<style></style>");
    expect(markup).not.toMatch(/\s(src|href)=/);
    expect(markup).not.toContain("crossorigin");
    expect(markup).not.toContain('type="module"');
    expect(markup.match(/<script>/g)).toHaveLength(1);
    expect(markup.match(/<style>/g)).toHaveLength(1);
    // The script is a classic one: no module syntax, no eval.
    const script = html.match(/<script>([\s\S]*?)<\/script>/)![1];
    expect(script.length).toBeGreaterThan(10_000);
    expect(script).not.toMatch(/(^|[;\n])\s*(import|export)\s[\w{*]/);
    expect(script).not.toContain("import.meta");
    expect(script).not.toMatch(/\beval\(|new Function\(/);
    // No fixture made it in.
    for (const text of ["run-question-ask-1", "The user wants ", "Half an ans", "cargo build"]) expect(html).not.toContain(text);
  }, 120_000);
});
