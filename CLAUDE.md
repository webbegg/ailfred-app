# Ailfred

A personal macOS menu-bar assistant in Rust on the Rune UI framework: area screenshots with
marks, a quick input that slides up from the bottom of the screen, and a chat (side panel or its
own window) with Claude. Words used here and in the UI: see `CONTEXT.md`.

## Build and check
- `scripts/check` before every commit: build (all targets), clippy with warnings as errors, then
  the tests with settings in a scratch folder. `scripts/check <filter>` runs some tests only.
- `cargo run` runs a dev build (settings under `com.ailfred.dev`). `scripts/install-mac` builds,
  signs and installs `~/Applications/Ailfred.app` (`com.ailfred`) and opens it. Quit one before
  starting the other: they share the global shortcuts. `scripts/bundle-mac` only builds and signs
  the bundle; `scripts/make-dmg` packs it; `scripts/make-signing-identity` makes the local
  signing identity once per Mac. Installing and removing: `docs/install.md`.
- Rune is a path dependency (`../_references/rune`, plus the vendored crates in `[patch]`). CI
  (`.github/workflows/check.yml`) checks Rune out there at `RUNE_REV` and runs `scripts/check` on
  a Mac; the check script stays the gate before a commit.

## Layout
- One module per component in `src/`. A big one is `foo.rs` with child files in `src/foo/`
  (`geometry.rs`, `view.rs`, `tests.rs`, …), never `mod.rs`; the root re-exports what others use.
- Logic lives in plain types and functions without `Tx`, `Cx` or a window in their signature, so
  tests can call them; views only draw state and send events back.
- A Cargo workspace: the app at the root, and `crates/ailfred-core` with what doesn't draw: the
  conversation model (`reply.rs`), the engines (`pi.rs`, `mock_model.rs`), `spawn.rs` and
  `turn.rs`. The core never depends on Rune or `objc2-*`; `main.rs` imports its modules, so
  `crate::reply` and the rest read as before. UI state (text fields) stays in the app.
  `crates/ailfred-protocol` is the wire protocol later clients speak: the core's reply as AG-UI
  1.0 events and back, the commands, JSON Schemas and JSONL fixtures every client replays
  (`UPDATE_FIXTURES=1` rewrites them), a saved reply's events (`saved.rs`) and the messages
  snapshot a joining client gets (`snapshot.rs`). It depends on the core only; the app uses it
  to save replies whole (`src/history/convert.rs`).
- `server.rs` (with `src/server/`) is the local server, off until Settings turns it on: other
  apps watch and drive the chat's conversation over AG-UI on `127.0.0.1` (`docs/local-server.md`);
  the chat feeds its hub, its commands reach the chat through the `Host`.
- `main.rs` holds the `Host`: the windowless app that owns the overlay, the quick input, the chat
  window, the tray and the shortcuts, and routes the chat between side panel and window.
- macOS-only calls (`objc2-*`) stay in small modules (`capture.rs`, `login.rs`, `screens.rs`,
  `app_env.rs`).

## Tests
- Names are sentences about behaviour (`a_run_from_the_terminal_keeps_its_settings_apart…`).
- Test-first: see the new test fail, then pass. No mocks of our own code; headless view tests
  use Rune's `Runtime` (`Runtime::new(state, view, size, Env::new(true, 2.0))`, `rt.frame`,
  `rt.elements()`).
- What can't run headless (native windows, the Dock, permissions) gets a log line and a manual
  check, and says so in the commit.
- Refactors change no behaviour and no test.

## Commits and PRs
- Branch `type/description`; commit subjects `type(scope): what the user gets`, in English.
- One PR per finished slice, merged in order.
