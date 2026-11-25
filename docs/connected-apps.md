# Connected apps

With pi, the model can use the user's accounts (Gmail, Google Calendar, GitHub, Jira, Slack, …)
through [Composio](https://composio.dev)'s MCP server. Ailfred manages that setup in its own pi
(`~/Library/Application Support/com.ailfred/pi/agent/`), in `src/pi_setup/connections.rs`.

## Why a guard

Prompt injection arrives through what the agent reads: an email, an issue, a calendar invite can
carry instructions that aren't the user's, and so can what's on the screen. A model that can
read them and also act (send, delete, run a command, write a file) can be talked into acting for
someone else. The guard keeps the two apart, the "Rule of Two": once the conversation has read
something untrusted, nothing that acts runs without the user's yes, and the dialog shows exactly
what would run.

## What is managed

- **The guard**, `extensions/ailfred-connections-guard.js` (from `src/pi_setup/connections_guard.js`,
  plain JavaScript). It is always there, on or off: it costs nothing without the server, a
  Composio server added by hand is guarded too, and it guards computer control's screenshots
  either way. It's written beside its place (`.js.new`), moved in, and read back byte for byte
  before each pi start and again on Cmd+N, as pi's new session goes, once the run going settled
  (pi's new session loads its extensions afresh, and a `bash` still running could change it
  before); only then does the server go into `mcp.json`. If that fails on Cmd+N, pi takes no
  message until it starts again, as when the guard didn't load (below). Failing either way, every
  Composio server is taken out of `mcp.json` (the `composio` entry, and any other whose `url`
  says `composio` anywhere, percent-decoded, any case, fullwidth letters as ASCII: fail-closed,
  so `composio%2Edev` or `composio。dev` count, whatever its name, hand-made ones too), the
  switch shows off again, and Settings' line
  under it names them and says why. Taking them out, rather than refusing to start pi, keeps the
  rest of the assistant working; an entry the user made by hand has to be added again.
- **The `composio` server** in `mcp.json`: `https://connect.composio.dev/mcp`, `direct` exposure,
  a 120 s timeout, and `toolExposure` hiding `COMPOSIO_*_SKILL*`, `COMPOSIO_PROXY_EXECUTE`,
  `COMPOSIO_REMOTE_BASH_TOOL`, `COMPOSIO_REMOTE_WORKBENCH` and `COMPOSIO_SUBMIT_FEEDBACK`. It is
  there while Settings' switch is on and taken out when off; the other servers stay. Ailfred
  owns the entry: edits to it are replaced.
- **The check that the guard loaded.** The guard registers the command `/ailfred-connections-guard`.
  Each time Ailfred starts pi with a Composio server (by name or by url, above) or computer
  control's `computer` server in its `mcp.json`, and again after each Cmd+N's new session, it
  asks pi for its commands (`get_commands`) before any message. The command counts only as an
  extension's, registered from the guard's own file (`sourceInfo.path`, as written or as its
  links resolve), under its name or pi's `:1`, `:2` renaming when two extensions register it.
  Without it (the guard didn't load), the run going stops and every message fails, saying so,
  until pi starts again. A message sent in the moment before pi answers reaches pi and is
  stopped then, long before its model can ask for a tool.
- **The hand-made guard** of the first spike, `extensions/composio-guard.ts`, is deleted (only
  that file): two guards would ask twice.

pi reads all of it as it starts; switching starts pi again for the next message.

## Composio's tools

The guard knows a Composio tool by its name: `mcp__composio__…`, or any `mcp__<server>__COMPOSIO_…`
whatever the server is called (a second server on the same URL is guarded the same way). Each
call is classified with an explicit table:

| Call | What happens |
|---|---|
| A slug in `READS` (curated Gmail, Calendar, GitHub, Jira and Slack reads, from Composio's toolkit docs) run through `COMPOSIO_MULTI_EXECUTE_TOOL` | Runs |
| `COMPOSIO_SEARCH_TOOLS`, `COMPOSIO_GET_TOOL_SCHEMAS`, `COMPOSIO_WAIT_FOR_CONNECTIONS` | Runs |
| Any other slug, empty or unknown ones included; an action with more than `tool_slug` and `arguments`; any other Composio tool (`COMPOSIO_MANAGE_CONNECTIONS`, new ones) | Asks |
| A call with a non-empty `memory` or a true `sync_response_to_workbench` (Composio keeps them: memory is given back to later runs) | Asks, even for reads, showing them |
| A hidden tool, called directly (a hash suffix pi adds included) or as a slug in a batch | Blocked |
| A batch whose slugs can't be read (no `tools` list, or an empty one) | Blocked |

`READS` leaves out what makes a copy Composio hosts for download (`GMAIL_GET_ATTACHMENT`,
`GITHUB_GET_RAW_REPOSITORY_CONTENT`): those ask. Deprecated reads Composio still serves stay, with
their replacements (`GITHUB_LIST_NOTIFICATIONS`, `JIRA_FIND_USERS2`, `JIRA_GET_WORKLOG`,
`JIRA_SEARCH_ISSUES`, `GMAIL_FETCH_EMAILS`). A new read is added by hand, after checking in
Composio's docs that it changes nothing (not "marks as read", not "watch", not a query language
that can write, not a downloadable copy).

## Tainted conversations

The guard's other half covers what pi does on its own once untrusted content is in:

- **What taints.** The result of any Composio tool that ran (searches and schemas too: Composio
  writes that text), of any computer-control tool (`mcp__computer__…`: screenshots show
  whatever is on the screen), and of pi's resource tools (`read_mcp_resource`,
  `list_mcp_resources`, `list_mcp_resource_templates`) on a server whose name has `composio` in
  it, on `computer`, or on every server (no `server` given). A call the guard lets through
  taints before it runs. Nothing else taints: see below.
- **What it gates.** While tainted, every tool asks first except pi's reads (`read`, `grep`,
  `find`, `ls`), Composio's (above), computer control's (it asks the user itself) and the tools
  that only call other tools (`codemode`, `tool_search`: each call they make comes to the guard
  on its own). So `bash`, `write`, `edit`, another MCP server's tools, and any tool the guard
  doesn't know ask, the dialog showing the exact command, path or arguments. Declined, or with
  no one to ask, the call is blocked and the model is told so.
- **How long.** The taint is the pi session's. The guard keeps it as a session entry
  (`ailfred-connections-taint`) and, on `session_start` (a resume, a reload, a fork), finds it
  again there, or in the session's tool results (nested calls included) should the entry be
  missing. A new session starts clean: Cmd+N is pi's `new_session`, which loads the guard
  afresh. When Ailfred brings an earlier conversation into a new pi session (pi started again, a
  conversation reopened), it writes the earlier turns into the first message; if any of them
  read a connected app or the screen, that message says so (`pi::UNTRUSTED_CONTEXT`), and the
  guard taints the new session. The earlier turns count as untrusted from the first one whose
  reply read a connected app or the screen, whether that reply wrote any text or not.
- Before anything untrusted came in, the chat works as it always did: `bash`, `write` and
  `edit` run without asking, except on pi's own setup (next).

## pi's own setup

Tainted or not, the guard asks before a change that could switch it off or reach the sign-ins:

- `write` or `edit` of a file inside pi's agent folder (`PI_CODING_AGENT_DIR`, else
  `~/.pi/agent`): its `extensions/` (this guard among them), `mcp.json`, `mcp-auth.json`,
  `settings.json`, anything there. The path is resolved as pi reads it (`@`, `~`, relative to
  pi's folder, `.` and `..`), any case; symlinks aren't followed.
- A `bash` command that names (any case, quotes and backslashes left out) the agent folder's
  path or that of the folder holding it (pi's HOME and working folder sit there too, next to
  `agent/`), a path segment `agent` reached through `..` or `~` (`../agent`, `~/../agent`,
  `$HOME/../agent`, `cd .. && ls agent`), `extensions/` together with `..`, `mcp-auth`,
  `auth.json`, `ailfred-connections-guard.js` or `PI_CODING_AGENT_DIR`. A bare `mcp.json`
  doesn't count (`cat .mcp.json`, `.vscode/mcp.json` in a repo run as before); one in pi's
  agent folder comes with a path to it. This is best effort: a command can reach those files
  without naming them (a variable, a glob, a script it runs).

On top of that, Ailfred writes the guard again and checks its bytes before each pi start and on
Cmd+N, then asks pi for its command (above).

Each of the guard's dialogs gets the run's abort signal (`ctx.signal`): stopping the run settles
an open one as "no", so the action is blocked rather than left waiting.

## The dialog

It never approves what it doesn't show. Each action is its slug (or tool name), then its
arguments one `name: value` line each, whole: who it reaches first (by name, any case: `to`,
`…recipient…`, `send_to`, `…_to`, `cc`, `bcc`, `attendee…`, `channel…`, `user…`, `email…`,
`owner`, `repo…`, `assignee…`, `issue…`, …, and `command`, `path`, `url`), then the subject or
title, then the rest in order, the body (`body`, `message_body`, `text`, `message`, …) last. A
batch that asks shows the call's own fields too (everything besides `tools`: `memory`,
`thought`, …) as its first action. A character Unicode draws as nothing or that controls text
(control and format characters, zero-width ones, direction overrides, tags, the soft hyphen,
line separators, newlines, variation selectors, the combining grapheme joiner, Hangul fillers:
all of `\p{Cc}`, `\p{Cf}`, `\p{Zl}`, `\p{Zp}` and `\p{Default_Ignorable_Code_Point}`) is written
out as `\u{…}`, so a value can't hide something, reorder what's shown or fake another line. At
most 8 actions, 24 arguments each and 2000 characters a value: a call past that is blocked
("Too large to show for approval: ask for one action at a time.") rather than cut.

Its title starts with `Ailfred guard: `. Only Ailfred's own window answers such a question: the
local server answers **403** to a client's answer, and the web view shows "Answer in Ailfred's
window" (a client can still decline). Another extension could start its title so too; that only
makes it stricter.

An answer reaches pi's dialog only when it was given to that very question: each question
carries the id of pi's dialog it stands for, the answer carries it back, and pi's side answers
only the dialog open with that id. Anything else is a message, never an answer: a typed or
client-sent message, a steer, a Continue, and a **Retry**, the window's, the web view's or a
client's. Retrying the reply to an answer ("Yes") sends its words as a new message: pi's dialog
open then, which may be the guard's next question, is cancelled (its action blocked), and
whatever the model does next asks again. A stale answer (pi started again, or another dialog
opened since) answers nothing either: it goes as a message, and a stale decline does nothing.

## Read-only mode

`AILFRED_CONNECTIONS=readonly` in pi's environment, read once as the guard loads, blocks without
asking all that would ask: every Composio write, every call that keeps memory or a workbench
copy at Composio, and, tainted or not, `bash`, `write`, `edit`, other servers' tools and unknown
tools. Reads still run. Any value other than none, `""` or `ask` counts as read-only, so a typo
can't open the writes. The chat's pi doesn't set it; background routines will (with
`--tools 'mcp__composio__*'` too, but read-only mode holds without it).

## What isn't protected

- **Only Composio's and computer control's results taint.** The output of `bash` and `read`, of
  other MCP servers' tools, of `gh`, `curl` or any other command, a web page, and text the user
  pastes can carry instructions just the same, and none of them taints the session.
- **An untainted session's tools aren't gated.** Before the conversation reads a connected app
  or the screen, `bash`, `write` and `edit` run as they always did (pi's own setup aside, above);
  what the user types is trusted. Such a `bash` command can still modify or delete the guard's
  file without naming it; Ailfred writes it again before pi's next start or new session, and the
  check then stops pi if it didn't load, but the session going keeps running as it is.
- **Credentials.** pi keeps Composio's tokens in `mcp-auth.json`, readable by any process the user
  runs, an approved `bash` command included, and by pi's `read` tool, which the guard doesn't
  gate. The guard stops the model from writing there, or naming it in `bash`, unasked; it
  doesn't hide it from the user's own programs.
- **Composio servers by another name, in the guard.** Ailfred finds a server whose url says
  `composio` whatever its name, but the guard knows Composio's tools by name (`mcp__composio__…`, or
  `…__COMPOSIO_…`): an app tool such a server served under a name without `COMPOSIO_` would be
  another server's tool to it, gated only once tainted.
- **The replay's taint** sees Composio and computer-control steps by tool name; a resource read
  (`read_mcp_resource`) taints pi's session but isn't counted when Ailfred replays the
  conversation into a new one.
- **What the user approves.** A yes runs exactly what the dialog showed; reading it is the
  user's part.
- **Composio's side.** Which apps are connected and what Composio does with a call are
  Composio's.

## Settings and sign-in

Settings › **Connected apps**. Until it is first switched, it is on if the pi's `mcp.json`
already had a Composio server (set up by hand, by name or by host), else off; the choice is then
kept.

While on, the line below says whether pi is signed in: `mcp-auth.json` has a `tokens` entry under
`mcp__composio|https://connect.composio.dev/mcp`, neither null nor empty (key names and emptiness
only; no value is read). **Sign in…** runs `pi mcp login composio` with Ailfred's folders: pi
opens Composio's page in the browser and waits for it (up to 5 minutes). If that fails, Settings
says why and gives the command to run in Terminal, with the same pi (`$AILFRED_PI` included) and
the same folders and HOME as the app's:

```sh
HOME='…/com.ailfred/pi/home' PI_CODING_AGENT_DIR='…/com.ailfred/pi/agent' ~/.pi/agent/bin/pi mcp login composio
```

The sign-in belongs to the server's name and URL, so switching off and on keeps it. Which apps are
connected inside Composio is managed there (or by the model, behind the guard's question).

## Output

What the model writes can also leak: an image whose URL carries private data is fetched the
moment it's drawn. The chat never fetches a remote image (it shows the alt text), and its links
don't open (no link handler: styled text, not a Tab stop). The web view loads images only from
`data:` URLs (its CSP's `img-src data:`, `docs/web-view.md`), shows a remote one's alt text,
opens a link only on a click, in the browser, with its real URL as the tooltip.
