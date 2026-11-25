# Connected apps

With pi, the model can use the user's accounts (Gmail, Google Calendar, GitHub, Jira, Slack, …)
through [Composio](https://composio.dev)'s MCP server. Ailfred manages that setup in its own pi
(`~/Library/Application Support/com.ailfred/pi/agent/`), in `src/pi_setup/connections.rs`.

## Why a guard

Prompt injection arrives through what the agent reads: an email, an issue, a calendar invite can
carry instructions that aren't the user's. A model that can both read them and act on the same
accounts can be talked into sending, deleting or forwarding. So only a known read runs on its
own; everything else waits for the user's yes, and the dialog shows exactly what would run.

## What is managed

- **The `composio` server** in `mcp.json`: `https://connect.composio.dev/mcp`, `direct` exposure,
  a 120 s timeout, and `toolExposure` hiding `COMPOSIO_*_SKILL*`, `COMPOSIO_PROXY_EXECUTE`,
  `COMPOSIO_REMOTE_BASH_TOOL`, `COMPOSIO_REMOTE_WORKBENCH` and `COMPOSIO_SUBMIT_FEEDBACK`. It is
  there while Settings' switch is on and taken out when off; the other servers stay. Ailfred
  owns the entry: edits to it are replaced.
- **The guard**, `extensions/ailfred-connections-guard.js` (from `src/pi_setup/connections_guard.js`,
  plain JavaScript). It is always there, on or off: it costs nothing without the server, and a
  `composio` server added by hand is guarded too.
- **The hand-made guard** of the first spike, `extensions/composio-guard.ts`, is deleted (only
  that file): two guards would ask twice.

pi reads all of it as it starts; switching starts pi again for the next message.

## The rule

The guard classifies every call to a `mcp__composio__…` tool with an explicit table:

| Call | What happens |
|---|---|
| A slug in `READS` (curated Gmail, Calendar, GitHub, Jira and Slack reads, from Composio's toolkit docs) run through `COMPOSIO_MULTI_EXECUTE_TOOL` | Runs |
| `COMPOSIO_SEARCH_TOOLS`, `COMPOSIO_GET_TOOL_SCHEMAS`, `COMPOSIO_WAIT_FOR_CONNECTIONS` | Runs |
| Any other slug, empty or unknown ones included; any other Composio tool (`COMPOSIO_MANAGE_CONNECTIONS`, new ones) | Asks |
| A hidden tool, called directly or as a slug in a batch | Blocked |
| A batch whose slugs can't be read (no `tools` list, or an empty one) | Blocked |

A batch with one write asks about its writes. The dialog lists each one's slug, then its
arguments one `name: value` line each, flattened to one line (no control or direction-changing
characters, so a value can't fake another line) and cut short. With no one to ask (pi in print
or JSON mode), a write is blocked. Declined, the model is told so.

A new read is added to `READS` by hand, after checking in Composio's docs that it changes
nothing (not "marks as read", not "watch", not a query language that can write).

## Read-only mode

`AILFRED_CONNECTIONS=readonly` in pi's environment blocks every write without asking; reads still
run. Any value other than none, `""` or `ask` counts as read-only, so a typo can't open the
writes. The chat's pi doesn't set it; background routines will.

## Settings and sign-in

Settings › **Connected apps**. Until it is first switched, it is on if the pi's `mcp.json`
already had a `composio` server (set up by hand), else off; the choice is then kept.

While on, the line below says whether pi is signed in: `mcp-auth.json` has a `tokens` entry under
`mcp__composio|https://connect.composio.dev/mcp` (key names only; no value is read). **Sign in…**
runs `pi mcp login composio` with Ailfred's folders: pi opens Composio's page in the browser and
waits for it (up to 5 minutes). If that fails, Settings says why and gives the command to run in
Terminal:

```sh
PI_CODING_AGENT_DIR="$HOME/Library/Application Support/com.ailfred/pi/agent" ~/.pi/agent/bin/pi mcp login composio
```

The sign-in belongs to the server's name and URL, so switching off and on keeps it. Which apps are
connected inside Composio is managed there (or by the model, behind the guard's question).

## Output

What the model writes can also leak: an image whose URL carries private data is fetched the
moment it's drawn. The chat never fetches a remote image (it shows the alt text), and its links
don't open. The web view loads images only from `data:` URLs (its CSP, `docs/web-view.md`), shows
a remote one's alt text, opens a link only on a click, in the browser, with its real URL as the
tooltip.
