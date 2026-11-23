import { describe, expect, it } from "vitest";
import { build } from "vite";

/// The CSP the build writes, `<hash>` the inlined script's.
const CSP = `default-src 'none'; script-src 'sha256-<hash>'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'`;

/// The base64 SHA-256 of `text`'s UTF-8 bytes, as a CSP hash source has it.
async function sha256(text: string): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
  return btoa(String.fromCharCode(...digest));
}

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
    // Only the inlined script runs: its hash, never 'unsafe-inline'.
    const script = html.match(/<script>([\s\S]*?)<\/script>/)![1];
    expect(html).toContain(`<meta http-equiv="Content-Security-Policy" content="${CSP.replace("<hash>", await sha256(script))}">`);
    const scriptSrc = html.match(/script-src ([^;"]*)/)![1];
    expect(scriptSrc).not.toContain("'unsafe-inline'");
    expect(scriptSrc.trim().split(/\s+/)).toHaveLength(1);
    // The markup outside the inlined script and style links to no file.
    const markup = html.replace(/<script>[\s\S]*?<\/script>/g, "<script></script>").replace(/<style>[\s\S]*?<\/style>/g, "<style></style>");
    expect(markup).not.toMatch(/\s(src|href)=/);
    expect(markup).not.toContain("crossorigin");
    expect(markup).not.toContain('type="module"');
    expect(markup.match(/<script>/g)).toHaveLength(1);
    expect(markup.match(/<style>/g)).toHaveLength(1);
    // The script is a classic one: no module syntax, no eval.
    expect(script.length).toBeGreaterThan(10_000);
    expect(script).not.toMatch(/(^|[;\n])\s*(import|export)\s[\w{*]/);
    expect(script).not.toContain("import.meta");
    expect(script).not.toMatch(/\beval\(|new Function\(/);
    // No fixture made it in.
    for (const text of ["run-question-ask-1", "The user wants ", "Half an ans", "cargo build"]) expect(html).not.toContain(text);
  }, 120_000);
});
