/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

/// The page is one file the app loads as a string (`docs/web-view.md`): its script and styles
/// go inline, as a classic script at the end of the body (no `type="module"`, no `crossorigin`),
/// and no other file is written.
function inlineAll(): Plugin {
  return {
    name: "ailfred-inline-all",
    apply: "build",
    enforce: "post",
    generateBundle(_options, bundle) {
      const page = bundle["index.html"];
      if (!page || page.type !== "asset") this.error("no index.html in the bundle");
      let html = String(page.source);
      const scripts: string[] = [];
      const styles: string[] = [];
      for (const [name, file] of Object.entries(bundle)) {
        if (name === "index.html") continue;
        if (file.type === "chunk") scripts.push(file.code);
        else if (name.endsWith(".css")) styles.push(String(file.source));
        else this.error(`a file the page would fetch: ${name}`);
        delete bundle[name];
      }
      if (scripts.length !== 1) this.error(`expected one script, got ${scripts.length}`);
      html = html
        .replace(/\s*<script\b[^>]*\bsrc=[^>]*><\/script>/g, "")
        .replace(/\s*<link\b[^>]*\brel="(stylesheet|modulepreload)"[^>]*>/g, "");
      // A `</script` or `</style` inside would end the element early.
      const style = styles.join("\n").replace(/<\/style/gi, "<\\/style");
      const script = scripts[0].replace(/<\/script/gi, "<\\/script");
      // Replacer functions: `$` in the code is not a pattern.
      html = html.replace("</head>", () => `<style>${style}</style>\n</head>`);
      html = html.replace("</body>", () => `<script>${script}</script>\n</body>`);
      page.source = html;
    },
  };
}

/// `npm run dev` serves the page from Vite's server, whose scripts and live reload the CSP
/// would block: only the build keeps it.
function devWithoutCsp(): Plugin {
  return {
    name: "ailfred-dev-without-csp",
    apply: "serve",
    transformIndexHtml: (html) => html.replace(/\s*<meta http-equiv="Content-Security-Policy"[^>]*>/, ""),
  };
}

export default defineConfig({
  plugins: [react(), inlineAll(), devWithoutCsp()],
  base: "./",
  build: {
    target: "safari16",
    modulePreload: false,
    cssCodeSplit: false,
    assetsInlineLimit: Number.MAX_SAFE_INTEGER,
    reportCompressedSize: false,
    rolldownOptions: {
      output: { format: "iife", codeSplitting: false },
    },
  },
  // The dev fixtures and the tests read `../crates/ailfred-protocol/fixtures`.
  server: { fs: { allow: [".."] } },
  test: {
    include: ["test/**/*.test.{ts,tsx}"],
    environment: "node",
  },
});
