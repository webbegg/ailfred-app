# Ailfred

A personal macOS menu-bar assistant in Rust on the Rune UI framework: area screenshots with
marks, a quick input that slides up from the bottom of the screen, and a chat (side panel or its
own window) with Claude. Words used here and in the UI: see `CONTEXT.md`.

## Build and check
- `scripts/check` before every commit: build (all targets), clippy with warnings as errors, then
  the tests with settings in a scratch folder. `scripts/check <filter>` runs some tests only.
- `cargo run` runs a dev build (settings under `com.ailfred.dev`). `scripts/install-mac` builds,
  signs and installs `~/Applications/Ailfred.app` (`com.ailfred`) and opens it. Quit one before
  starting the other: they share the global shortcuts.
- Rune is a path dependency (`../_references/rune`, plus the vendored crates in `[patch]`), so
  there is no CI: the check script is the gate.

## Layout
- One module per component in `src/`. A big one is `foo.rs` with child files in `src/foo/`
  (`geometry.rs`, `view.rs`, `tests.rs`, …), never `mod.rs`; the root re-exports what others use.
- Logic lives in plain types and functions without `Tx`, `Cx` or a window in their signature, so
  tests can call them; views only draw state and send events back.
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
