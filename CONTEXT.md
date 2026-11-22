# Ailfred: words

One meaning per word, in code, UI copy and prompts alike.

**Shot**:
A capture of an area of the screen, with its marks burned in. Saved as a PNG (named like macOS
screenshots) and put on the clipboard, or attached to a message.
_Avoid_: screenshot (in code), image (for a capture)

**Mark**:
An annotation drawn on a shot: arrow, box, text, pixelation.
_Avoid_: annotation, drawing

**Overlay**:
The full-screen layer over the frozen display where a shot's area is picked and marked.
_Avoid_: capture window

**Quick input**:
The text field that slides up from the bottom of the screen (⌃⌥Space). Sending from it starts or
continues the conversation.
_Avoid_: launcher, prompt bar

**Chip**:
A shot attached to the next message, shown small in the quick input or the composer.
_Avoid_: thumbnail, attachment (in UI copy)

**Side panel**:
The chat docked at the side of the screen, in the quick input's window. It slides in and out.
_Avoid_: sidebar, drawer

**Chat window**:
The chat popped out of the side panel into its own macOS window, with a Dock icon while open.
_Avoid_: detached panel

**Chat**:
The conversation's view and state wherever it lives (side panel or chat window): its messages,
replies, draft and chips. It moves between the two whole.

**Conversation**:
The messages and replies since the last Cmd+N. It has an id (`YYYYMMDD-HHMMSS-xxxx`), is kept in a
conversation file, and the last one updated comes back into the chat at launch.
_Avoid_: session, thread

**Conversation file**:
A conversation on disk: `<id>.json` in `conversations/` under the app's settings folder (owner-only,
written to a `.tmp` and renamed), its shots as `<id>/<n>.png` beside it. A damaged field falls back
to its default; a file that won't parse is skipped and left alone. Written by one thread, never
the main one.
_Avoid_: history file, save file

**Recent**:
The tray menu's list of the 10 most recently updated conversations, by title (else the start of
their first message). Picking one stops what streams and loads it into the chat.
_Avoid_: history (in UI copy)

**Title**:
A conversation's short name: asked of a cheap model (Haiku) once the first reply is done, unless
the user wrote one (`title_by_user`), which always wins. No title (a failed or slow call) shows
the start of the first message instead.
_Avoid_: name, subject

**Message**:
What the user sent: text and its shots.

**Reply**:
The model's answer to one message, streamed in parts: thinking, text (markdown), agent steps, a
diff, a question with options, an error. A reply is streaming, done, stopped or failed.
_Avoid_: response, completion

**Model**:
What writes the replies, picked in Settings: Claude, through the Anthropic API with the user's
key, else through the installed Claude Code; or **pi**, Ailfred's own pi (`pi.rs`, `pi_setup.rs`)
on the model and sign-ins of the user's pi setup, which runs tools and asks its own questions.
Each is a `model::Model`. The mock model (`mock_model.rs`) in tests and with
`AILFRED_MOCK_MODEL=1`.
_Avoid_: agent, backend

**Status**:
What Ailfred is doing, app-wide (`status.rs`): idle, starting, thinking, working (on a tool),
needs you (a question), an error, or replied (while the chat was hidden). The menu-bar icon
shows it with a badge, its tooltip in words.
_Avoid_: agent state, activity

**Host**:
The windowless app at the root (`main.rs`) that owns every window, the tray and the shortcuts.
