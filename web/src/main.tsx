import { createRoot } from "react-dom/client";
import { App } from "./App";
import type { Answer } from "./protocol";
import { chooseTransport, type BridgeWindow, type Transport } from "./transport";
import "./styles.css";

/// Outside the app's web view and outside `npm run dev`: nothing to talk to.
const nowhere: Transport = {
  start: () => {},
  send: async (): Promise<Answer> => ({ status: 503, body: { error: "This page only works inside Ailfred's web view." } }),
  open: () => {},
};

async function transport(): Promise<Transport> {
  const bridge = chooseTransport(window as unknown as BridgeWindow);
  if (bridge) return bridge;
  if (import.meta.env.DEV) {
    // Only in `npm run dev`: the build has no fixtures.
    const { fixtureTransport } = await import("./devFixtures");
    return fixtureTransport(new URLSearchParams(location.search).get("fixture"));
  }
  return nowhere;
}

void transport().then((t) => createRoot(document.getElementById("root")!).render(<App transport={t} />));
