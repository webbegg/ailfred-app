// `npm run dev` only (main.tsx imports this behind `import.meta.env.DEV`): `?fixture=<name>`
// replays `crates/ailfred-protocol/fixtures/<name>.jsonl`.

import { FixtureTransport } from "./transport";

const files = import.meta.glob("../../crates/ailfred-protocol/fixtures/*.jsonl", { query: "?raw", import: "default" }) as Record<string, () => Promise<string>>;

export async function fixtureTransport(name: string | null): Promise<FixtureTransport> {
  const key = name === null ? undefined : Object.keys(files).find((path) => path.endsWith(`/${name}.jsonl`));
  if (name !== null && !key) console.warn(`no fixture ${name}; there are`, Object.keys(files).map((p) => p.replace(/^.*\/|\.jsonl$/g, "")));
  const text = key ? await files[key]() : "";
  const events = text
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line) as unknown);
  return new FixtureTransport(events);
}
