// The protocol crate's fixtures, read as text: every client replays them in its own tests.

const files = import.meta.glob("../../crates/ailfred-protocol/fixtures/*.jsonl", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/// Fixture `name`'s text (`question` for `question.jsonl`).
export function fixtureText(name: string): string {
  const entry = Object.entries(files).find(([path]) => path.endsWith(`/${name}.jsonl`));
  if (!entry) throw new Error(`no fixture ${name}`);
  return entry[1];
}

/// Fixture `name`'s lines, parsed.
export function fixture(name: string): unknown[] {
  return fixtureText(name)
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line));
}

/// The names of all fixtures but `commands`.
export const EVENT_FIXTURES = Object.keys(files)
  .map((path) => path.replace(/^.*\//, "").replace(/\.jsonl$/, ""))
  .filter((name) => name !== "commands")
  .sort();
