# The web view

Ailfred's conversation can also be shown in a web view: a native window holding a WKWebView, opened from the menu bar's menu (**Open Web View**). The page is the React client in `web/` (Vite, TypeScript). It draws the same conversation as the side panel, live, with web components. A message sent from it, an answer to a question, declining one, a Stop, a Retry, or New conversation (its header's button), does what the same action in the side panel does (Retry shows on the replies the chat offers it on: every one that ended).

The web view does not use the local server (`docs/local-server.md`): it needs no token, no port, and works with the server switched off. It speaks the same protocol, the AG-UI events of `crates/ailfred-protocol`, over a bridge between the page and the app.

## The page

- `npm --prefix web run build` writes one self-contained file, `web/dist/index.html`: the script and styles are inlined, so nothing is fetched.
- The app loads that file's text with `loadHTMLString:baseURL:` (base URL nil). Run from an app bundle, it reads only `Contents/Resources/web/index.html` (copied by `scripts/bundle-mac`, which always runs `npm ci --ignore-scripts` first); outside a bundle, a debug build (`cargo run`) reads `<repo>/web/dist/index.html`. Without it, the window says how to build it.
- Its Content-Security-Policy, in a `<meta>` tag: `default-src 'none'; script-src 'sha256-…'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'`. The build (`inlineAll` in `web/vite.config.ts`) writes the inlined script's hash into `script-src` (`web/index.html` has `'none'` there), so no other script runs: no inline handler, no `javascript:` URL. No network at all. `npm run dev` drops the CSP.
- The app reads only messages posted from the main frame showing the page (`about:blank`). If the page's content process ends, the page loads again and joins afresh. Its context menu has no item that navigates or downloads (Open Link, Download Linked File, Search the Web, …).
- The page never navigates. The app lets through only the first load (`about:blank`, the HTML string) and a reload of it; it cancels every other navigation and refuses new windows (`window.open`, `target=_blank`). A link the user activates, or the page asks to open (see `open` below), goes to the default browser, only for `http:` and `https:`.
- The window is one at a time: **Open Web View** again brings it to the front. Closing it lets go of the web view; the chat stops feeding the hub unless the local server runs. The Web Inspector is on in debug builds only. ⌘X, ⌘C, ⌘V, ⌘A, ⌘Z, ⇧⌘Z and ⌘W work in it (the app has no Edit menu; the window sends them), on a layout that isn't Latin too (by the key's place, as on ANSI). Messages and answers queued for a window that closed never reach the next one.
- Model text is rendered as Markdown with raw HTML skipped.

## The bridge

From the app to the page, the app calls (`evaluateJavaScript`):

- `window.ailfred.receive(events)`: `events` is an array of AG-UI event objects, in order, exactly as `GET /v1/events` would send them (one frame may hold several: the first one after a join holds the `MESSAGES_SNAPSHOT` and the run in progress from its `RUN_STARTED`). A `MESSAGES_SNAPSHOT` replaces everything the page holds; the thread id is in `metadata.ailfred.threadId`.
- `window.ailfred.answer(id, status, body)`: the answer to the page's command `id`: `status` is the HTTP status `POST /v1/commands` would give (202, 400, 403, 409, 413, 503; 403: an answer to a question of Ailfred's guard, which the page shows as "Answer in Ailfred's window" with Decline only), `body` its JSON body (`{threadId, runId}` on 202, `{error}` otherwise).

Both calls are one statement each, their arguments JSON in which `<`, U+2028 and U+2029 are always written as JSON's backslash-u escapes, so nothing in a message can end the script.

From the page to the app, the page calls `window.webkit.messageHandlers.ailfred.postMessage(text)`, `text` a JSON string, one of:

- `{"type": "join"}`: sent once the page is ready (and again after a reload). The app answers with a `receive` holding the snapshot, then live events. Joining again replaces the earlier join: frames still on their way to it are dropped, and a fresh snapshot comes.
- `{"type": "command", "id": "<string>", "command": <command>}`: `<command>` is exactly a `POST /v1/commands` body (`run` with a user message or a `resume`, which may decline, `stop`, `retry` or `new_conversation`); the same commands, checks and answers as over HTTP (`docs/local-server.md`), the answer coming back through `answer`. A `command` without one, or one that isn't a command, is answered **400**; one without a string `id` is ignored.
- `{"type": "open", "url": "<url>"}`: opens an `http:`/`https:` link in the default browser; anything else is ignored.

Anything else the page posts (bad JSON too, or not a string) is ignored. A frame the page can't keep up with is never dropped: the bridge watches the hub through a sink of its own in the app's process (not the 256-frame channel of a server stream), each frame queued to the main thread in order. The local server stopping doesn't end it.

In the app: `src/web_view/bridge.rs` (the messages, the JavaScript, the links and navigations; tested), `src/web_view/glue.rs` (the window, the web view and its delegates; checked by hand), and `Host::web_view_message` in `src/main.rs`. The hub is the local server's (`src/server/hub.rs`), fed while the server runs or the web view is open (`server::Feeding`).

## Development

- `npm --prefix web run dev` serves the page in a browser with a fixture transport: `?fixture=question` replays `crates/ailfred-protocol/fixtures/question.jsonl` (any fixture name), so the parts can be styled without the app.
- `npm --prefix web run check`: type check, tests (the fixtures replayed through the page's state), build. `scripts/check` runs it.
