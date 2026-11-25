// Ailfred's guard for the connected apps (the tools of Composio's server in the mcp.json of
// Ailfred's pi) and for what the model does once it has read them. What the model reads there
// (an email, an issue, a calendar invite), or sees on the screen, can carry instructions that
// aren't the user's.
//
// - Composio: READS and META_READS run without asking. Every other action asks: each slug run
//   through COMPOSIO_MULTI_EXECUTE_TOOL that isn't in READS (or carries more than its slug and
//   arguments), each Composio tool that isn't in META_READS, and a call that would keep
//   `memory` or a workbench copy at Composio. The tools Ailfred hides (BLOCKED) are blocked,
//   also as a slug in a batch; so is a batch whose actions can't be read. A Composio tool is
//   known by its name (`…__COMPOSIO_…`) whatever server serves it.
// - Tainted: once a result of a Composio tool or of computer control (screenshots show what's on
//   the screen), or a resource read from either (RESOURCES), came into the session, every tool but pi's reads (read, grep, find, ls), the
//   tools that call other tools (codemode, tool_search: each call they make comes here too),
//   computer control (it asks the user itself) and Composio's (above) asks first: bash, write,
//   edit, another server's tools, and any tool this doesn't know. The taint is the session's: an
//   entry in it (TAINT) and, should that be missing, its tool results; a new session starts
//   without. A message that carries an earlier conversation's untrusted turns into a new session
//   (UNTRUSTED_CONTEXT, from Ailfred's core) taints it too.
// - pi's own setup: write or edit of a file in pi's agent folder (PI_CODING_AGENT_DIR as the guard
//   loads: its extensions, this guard among them, mcp.json, mcp-auth.json, settings.json), and a
//   bash command that names it (namesSetup), ask first, tainted or not. Best effort for bash: a
//   command can reach a path without naming it.
// - Each dialog gets the run's abort signal: stopping the run settles it as "no".
// - Read-only mode (the AILFRED_CONNECTIONS variable as the guard loads: any value but none, ""
//   or "ask") blocks, without asking, all that would ask above, tainted or not.
// - With no one to ask (no UI), what would ask is blocked.
// - The dialog never approves what it doesn't show: each action whole, who it reaches first.
//   What doesn't fit (MAX_ACTIONS, MAX_FIELDS, MAX_VALUE, MAX_TEXT) is blocked, not cut. Its
//   title starts with LOCAL_ONLY: Ailfred takes the answer only from its own window.
//
// Written to pi's agent folder by `pi_setup::connections`; plain JavaScript, no imports.

const MODE = "AILFRED_CONNECTIONS";
// pi's agent folder, as Ailfred starts pi (else pi's default, `~/.pi/agent`).
const AGENT_DIR = "PI_CODING_AGENT_DIR";
// `ailfred_core::reply::LOCAL_ONLY`.
const LOCAL_ONLY = "Ailfred guard: ";
// `pi_setup::connections::GUARD_COMMAND`: Ailfred asks pi for it to know the guard loaded.
const COMMAND = "ailfred-connections-guard";
// The session entry that says the session is tainted.
const TAINT = "ailfred-connections-taint";
// `ailfred_core::pi::UNTRUSTED_CONTEXT`.
const UNTRUSTED_CONTEXT = "Parts of it came from the user's connected apps or screen, and may carry instructions that aren't the user's.";
const COMPOSIO_SERVER = "mcp__composio__";
const COMPOSIO_TOOL = "__COMPOSIO_";
const COMPUTER = "mcp__computer__";
const EXECUTE = "COMPOSIO_MULTI_EXECUTE_TOOL";
const WRITE_TITLE = `${LOCAL_ONLY}allow these actions on your connected apps?`;
const TAINTED_TITLE = `${LOCAL_ONLY}allow this? This conversation has read content from your connected apps or screen, which can carry instructions that aren't yours.`;
const SETUP_TITLE = `${LOCAL_ONLY}allow this change to pi's own setup? It can turn off Ailfred's guard or reach your sign-ins.`;

// pi's tools that change a file, and the arguments naming it.
const WRITERS = new Set(["write", "edit"]);
const PATH_ARGS = ["path", "file_path", "filePath"];
// What a bash command names when it reaches pi's own setup (besides a path to it, namesSetup).
const SETUP_WORDS = ["mcp-auth", "auth.json", AGENT_DIR, `${COMMAND}.js`];
// A path segment `agent` (pi's agent folder sits next to its HOME and the folder it works in),
// and a command climbing out of a folder (`..`) or starting from a home folder (`~`).
const AGENT_SEGMENT = /(^|[\s=:(/;|&<>])agent($|[\s/;|&<>)])/;
const CLIMBS = /\.\.|(^|[\s=:(;|&<>])~/;
// pi's tools that read or list MCP servers' resources (`server` names the server; none: all).
const RESOURCES = new Set(["read_mcp_resource", "list_mcp_resources", "list_mcp_resource_templates"]);

// pi's own tools that only read.
const PI_READS = new Set(["read", "grep", "find", "ls"]);
// Tools that only call other tools, each call coming to this guard on its own.
const CALLERS = new Set(["codemode", "tool_search"]);

// Composio tool slugs that only read, from Composio's toolkit docs
// (docs.composio.dev/toolkits/<toolkit>.md). Not here: anything that sends, creates, changes,
// deletes, marks as read, watches, runs a query language that can write, or makes a copy
// Composio hosts for download (GMAIL_GET_ATTACHMENT, GITHUB_GET_RAW_REPOSITORY_CONTENT).
// Deprecated slugs Composio still serves stay, with their replacements.
const READS = new Set([
  // Gmail
  "GMAIL_FETCH_EMAILS",
  "GMAIL_FETCH_MESSAGE_BY_MESSAGE_ID",
  "GMAIL_FETCH_MESSAGE_BY_THREAD_ID",
  "GMAIL_GET_CONTACTS",
  "GMAIL_GET_DRAFT",
  "GMAIL_GET_LABEL",
  "GMAIL_GET_PEOPLE",
  "GMAIL_GET_PROFILE",
  "GMAIL_LIST_DRAFTS",
  "GMAIL_LIST_HISTORY",
  "GMAIL_LIST_LABELS",
  "GMAIL_LIST_MESSAGES",
  "GMAIL_LIST_THREADS",
  "GMAIL_SEARCH_PEOPLE",
  "GMAIL_WHO_AM_I",
  // Google Calendar
  "GOOGLECALENDAR_CALENDAR_LIST_GET",
  "GOOGLECALENDAR_COLORS_GET",
  "GOOGLECALENDAR_EVENTS_GET",
  "GOOGLECALENDAR_EVENTS_INSTANCES",
  "GOOGLECALENDAR_EVENTS_LIST",
  "GOOGLECALENDAR_EVENTS_LIST_ALL_CALENDARS",
  "GOOGLECALENDAR_FIND_EVENT",
  "GOOGLECALENDAR_FIND_FREE_SLOTS",
  "GOOGLECALENDAR_FREE_BUSY_QUERY",
  "GOOGLECALENDAR_GET_CALENDAR",
  "GOOGLECALENDAR_GET_CURRENT_DATE_TIME",
  "GOOGLECALENDAR_LIST_CALENDARS",
  "GOOGLECALENDAR_SETTINGS_GET",
  "GOOGLECALENDAR_SETTINGS_LIST",
  // GitHub
  "GITHUB_FIND_PULL_REQUESTS",
  "GITHUB_FIND_REPOSITORIES",
  "GITHUB_GET_A_BRANCH",
  "GITHUB_GET_A_COMMIT",
  "GITHUB_GET_A_PULL_REQUEST",
  "GITHUB_GET_A_REPOSITORY",
  "GITHUB_GET_A_REPOSITORY_README",
  "GITHUB_GET_A_USER",
  "GITHUB_GET_A_WORKFLOW_RUN",
  "GITHUB_GET_AN_ISSUE",
  "GITHUB_GET_AN_ISSUE_COMMENT",
  "GITHUB_GET_REPOSITORY_CONTENT",
  "GITHUB_GET_THE_AUTHENTICATED_USER",
  "GITHUB_GET_THE_LATEST_RELEASE",
  "GITHUB_LIST_BRANCHES",
  "GITHUB_LIST_CHECK_RUNS_FOR_A_REF",
  "GITHUB_LIST_COMMITS",
  "GITHUB_LIST_COMMITS_ON_A_PULL_REQUEST",
  "GITHUB_LIST_ISSUE_COMMENTS",
  "GITHUB_LIST_ISSUES_ASSIGNED_TO_THE_AUTHENTICATED_USER",
  "GITHUB_LIST_LABELS_FOR_A_REPOSITORY",
  "GITHUB_LIST_NOTIFICATIONS",
  "GITHUB_LIST_NOTIFICATIONS_FOR_THE_AUTHENTICATED_USER",
  "GITHUB_LIST_PULL_REQUESTS",
  "GITHUB_LIST_PULL_REQUESTS_FILES",
  "GITHUB_LIST_RELEASES",
  "GITHUB_LIST_REPOSITORIES_FOR_THE_AUTHENTICATED_USER",
  "GITHUB_LIST_REPOSITORY_ISSUES",
  "GITHUB_LIST_REVIEW_COMMENTS_ON_A_PULL_REQUEST",
  "GITHUB_LIST_REVIEWS_FOR_A_PULL_REQUEST",
  "GITHUB_LIST_WORKFLOW_RUNS_FOR_A_REPOSITORY",
  "GITHUB_SEARCH_CODE",
  "GITHUB_SEARCH_COMMITS",
  "GITHUB_SEARCH_ISSUES_AND_PULL_REQUESTS",
  "GITHUB_SEARCH_REPOSITORIES",
  "GITHUB_SEARCH_USERS",
  // Jira
  "JIRA_FIND_USERS",
  "JIRA_FIND_USERS2",
  "JIRA_GET_ALL_PROJECTS",
  "JIRA_GET_ALL_STATUSES",
  "JIRA_GET_COMMENT",
  "JIRA_GET_CURRENT_USER",
  "JIRA_GET_FIELDS",
  "JIRA_GET_ISSUE",
  "JIRA_GET_ISSUE_TYPES",
  "JIRA_GET_ISSUE_WATCHERS",
  "JIRA_GET_ISSUE_WORKLOGS",
  "JIRA_GET_PROJECT",
  "JIRA_GET_SPRINT",
  "JIRA_GET_TRANSITIONS",
  "JIRA_GET_WORKLOG",
  "JIRA_LIST_BOARDS",
  "JIRA_LIST_ISSUE_COMMENTS",
  "JIRA_LIST_PRIORITIES",
  "JIRA_LIST_SPRINTS",
  "JIRA_SEARCH_FOR_ISSUES_USING_JQL_GET",
  "JIRA_SEARCH_FOR_ISSUES_USING_JQL_POST",
  "JIRA_SEARCH_ISSUES",
  "JIRA_WHO_AM_I",
  // Slack
  "SLACK_FETCH_CONVERSATION_HISTORY",
  "SLACK_FETCH_MESSAGE_THREAD_FROM_A_CONVERSATION",
  "SLACK_FETCH_TEAM_INFO",
  "SLACK_FIND_CHANNELS",
  "SLACK_FIND_USER_BY_EMAIL_ADDRESS",
  "SLACK_FIND_USERS",
  "SLACK_GET_USER_PRESENCE",
  "SLACK_LIST_ALL_CHANNELS",
  "SLACK_LIST_ALL_USERS",
  "SLACK_LIST_CONVERSATIONS",
  "SLACK_LIST_PINNED_ITEMS",
  "SLACK_LIST_REMINDERS",
  "SLACK_LIST_SCHEDULED_MESSAGES",
  "SLACK_LIST_STARRED_ITEMS",
  "SLACK_RETRIEVE_CONVERSATION_INFORMATION",
  "SLACK_RETRIEVE_CONVERSATION_MEMBERS_LIST",
  "SLACK_RETRIEVE_DETAILED_USER_INFORMATION",
  "SLACK_RETRIEVE_MESSAGE_PERMALINK_URL",
  "SLACK_RETRIEVE_USER_PROFILE_INFORMATION",
  "SLACK_SEARCH_ALL",
  "SLACK_SEARCH_FILES",
  "SLACK_SEARCH_MESSAGES",
  "SLACK_WHO_AM_I",
]);

// Composio's own tools that only look things up: finding a tool, reading its schema, waiting
// for a connection the user is making.
const META_READS = new Set(["COMPOSIO_SEARCH_TOOLS", "COMPOSIO_GET_TOOL_SCHEMAS", "COMPOSIO_WAIT_FOR_CONNECTIONS"]);

// The tools Ailfred hides in mcp.json (`toolExposure`): code and raw requests run on Composio's
// side, feedback sent out, skills. Never run, however they're reached (a hash suffix pi adds to
// names that collide included).
const BLOCKED = [/^COMPOSIO_PROXY_EXECUTE/, /^COMPOSIO_REMOTE_BASH_TOOL/, /^COMPOSIO_REMOTE_WORKBENCH/, /^COMPOSIO_SUBMIT_FEEDBACK/, /^COMPOSIO_.*SKILL/];

// The arguments of Composio's tools that keep something at Composio: `memory` is given back to
// later runs, and a workbench copy stays there.
const KEPT = ["memory", "sync_response_to_workbench"];

// How much the dialog shows; what doesn't fit is blocked, never cut.
const MAX_ACTIONS = 8;
const MAX_FIELDS = 24;
const MAX_VALUE = 2000;
const MAX_NAME = 100;
const MAX_TEXT = 20000;
const TOO_LARGE = "Too large to show for approval: ask for one action at a time.";

// Field names (any case) shown first, in this order: who or what the action reaches; then its
// subject or title; then the rest as they came; its body last.
const FIRST = [
  /^to$/,
  /recipient/,
  /^send_to$/,
  /_to$/,
  /^cc$/,
  /^bcc$/,
  /^attendee/,
  /^invitee/,
  /^channel/,
  /^user/,
  /^email/,
  /^member/,
  /^team/,
  /^owner$/,
  /^org/,
  /^repo/,
  /^project/,
  /^assignee/,
  /^reviewer/,
  /^issue/,
  /^calendar/,
  /^(from|command|path|file_path|url|cwd)$/,
];
const SUBJECT = /^(subject|title|summary|name)$/;
const BODY = /^(body|message_body|message|text|markdown_text|html|content|description|comment)$/;

const isBlocked = (name) => BLOCKED.some((pattern) => pattern.test(name));
const isExecute = (tool) => tool === EXECUTE || tool.startsWith(`${EXECUTE}_`);
const named = (slug) => slug || "(no tool named)";
const length = (text) => Array.from(text).length;

// The Composio tool `name` calls (`COMPOSIO_…`, or what Composio's server calls it), or null.
function composioTool(name) {
  if (!name.startsWith("mcp__")) return null;
  const at = name.lastIndexOf(COMPOSIO_TOOL);
  if (at >= 0) return name.slice(at + 2);
  if (name.startsWith(COMPOSIO_SERVER)) return name.slice(COMPOSIO_SERVER.length);
  return null;
}

// What tool `name` returns, called with `input`, can carry instructions that aren't the user's:
// Composio's tools, computer control's, and the resources of either (or of every server).
function untrusted(name, input) {
  if (typeof name !== "string") return false;
  if (composioTool(name) !== null || name.startsWith(COMPUTER)) return true;
  if (!RESOURCES.has(name)) return false;
  const server = typeof input?.server === "string" ? input.server.trim().toLowerCase() : "";
  return server === "" || server.includes("composio") || server === "computer";
}

function readOnly(mode) {
  const value = typeof mode === "string" ? mode.trim().toLowerCase() : "";
  return value !== "" && value !== "ask";
}

// Holds something: not null, false, 0, "", or an object or array of only such.
function holds(value) {
  if (value === undefined || value === null || value === false || value === 0) return false;
  if (typeof value === "string") return value.trim() !== "";
  if (Array.isArray(value)) return value.some(holds);
  if (typeof value === "object") return Object.values(value).some(holds);
  return true;
}

// What a call to Composio's tool `tool` is: a read, an action to ask about (`actions`, each
// `{ slug, args }`, and whether any is a write), or blocked (with its `reason`).
function classify(tool, input) {
  if (isBlocked(tool)) return { kind: "blocked", reason: `Blocked: ${escaped(tool)} isn't available to the assistant.` };
  const kept = KEPT.filter((name) => holds(input?.[name]));
  const keeps = kept.length > 0 ? [{ slug: `${tool} (kept at Composio)`, args: Object.fromEntries(kept.map((name) => [name, input[name]])) }] : [];
  if (META_READS.has(tool)) return keeps.length > 0 ? { kind: "ask", actions: keeps, writes: false } : { kind: "read" };
  if (!isExecute(tool)) return { kind: "ask", actions: [{ slug: tool, args: input }], writes: true };
  // The call's own fields besides its actions, all shown when it asks.
  const own = input && typeof input === "object" && !Array.isArray(input) ? Object.fromEntries(Object.entries(input).filter(([key]) => key !== "tools")) : {};
  // `{ tools: [{ tool_slug, arguments }], ... }`. A slug that isn't a string is empty: a write.
  const tools = input?.tools;
  if (!Array.isArray(tools) || tools.length === 0) {
    return { kind: "blocked", reason: "Blocked: no tool slug found in the call, so it can't be checked." };
  }
  const actions = tools.map((entry) => {
    const slug = typeof entry?.tool_slug === "string" ? entry.tool_slug.trim().toUpperCase() : "";
    // More than its slug and arguments: what else it says is shown, and it asks.
    const rest = entry && typeof entry === "object" ? Object.keys(entry).filter((key) => key !== "tool_slug" && key !== "arguments") : [];
    const args = rest.length > 0 ? Object.fromEntries(Object.entries(entry).filter(([key]) => key !== "tool_slug")) : entry?.arguments;
    return { slug, args, plain: rest.length === 0 };
  });
  const hidden = actions.filter((action) => isBlocked(action.slug));
  if (hidden.length > 0) {
    return { kind: "blocked", reason: `Blocked: ${hidden.map((a) => escaped(a.slug)).join(", ")} isn't available to the assistant.` };
  }
  const writes = actions.filter((action) => !(action.plain && READS.has(action.slug)));
  if (writes.length === 0 && keeps.length === 0) return { kind: "read" };
  const call = holds(own) ? [{ slug: kept.length > 0 ? `${tool} (kept at Composio)` : tool, args: own }] : [];
  return { kind: "ask", actions: [...call, ...writes], writes: writes.length > 0 };
}

// A character that would hide, reorder or fake text: a control or format character (C0, DEL,
// C1, zero-width ones, direction marks and overrides, tags, the soft hyphen, the BOM, …), a line
// or paragraph separator, or any other character Unicode says to draw as nothing (variation
// selectors, the combining grapheme joiner, Hangul fillers, …).
const INVISIBLE = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}\p{Default_Ignorable_Code_Point}]/u;

// `text` with each invisible character written out as `\u{…}`, so the dialog shows it on one
// line, as it is.
function escaped(text) {
  return Array.from(String(text), (ch) => {
    const c = ch.codePointAt(0);
    return INVISIBLE.test(ch) || (c >= 0xe0000 && c <= 0xe007f) ? `\\u{${c.toString(16)}}` : ch;
  }).join("");
}

function shown(value) {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

function rank(name) {
  const key = name.toLowerCase();
  const first = FIRST.findIndex((pattern) => pattern.test(key));
  if (first >= 0) return first;
  if (SUBJECT.test(key)) return FIRST.length;
  if (BODY.test(key)) return FIRST.length + 2;
  return FIRST.length + 1;
}

// An action's arguments, one `name: value` line each, who it reaches first, the empty ones left
// out; or what's too large to show.
function fields(args) {
  if (args === undefined || args === null || args === "") return { lines: [] };
  const entries =
    typeof args === "object" && !Array.isArray(args)
      ? Object.entries(args).filter(([, value]) => value !== undefined && value !== null && value !== "")
      : [["value", args]];
  if (entries.length > MAX_FIELDS) return { tooLarge: `${entries.length} arguments, more than ${MAX_FIELDS}` };
  const ordered = entries.map((entry, i) => ({ entry, i, rank: rank(entry[0]) })).sort((a, b) => a.rank - b.rank || a.i - b.i);
  const lines = [];
  for (const { entry } of ordered) {
    const [name, value] = entry;
    const text = shown(value);
    if (length(name) > MAX_NAME) return { tooLarge: "an argument's name is too long" };
    if (length(text) > MAX_VALUE) return { tooLarge: `${escaped(name)} is longer than ${MAX_VALUE} characters` };
    lines.push(`${escaped(name)}: ${escaped(text)}`);
  }
  return { lines };
}

// What the dialog says: each action's slug, then its arguments indented under it, all whole;
// or what's too large to show.
function describe(actions) {
  if (actions.length > MAX_ACTIONS) return { tooLarge: `${actions.length} actions, more than ${MAX_ACTIONS}` };
  const lines = [];
  for (const action of actions) {
    const slug = named(action.slug);
    if (length(slug) > MAX_NAME) return { tooLarge: "a tool's name is too long" };
    lines.push(escaped(slug));
    const shownFields = fields(action.args);
    if (shownFields.tooLarge) return shownFields;
    for (const line of shownFields.lines) lines.push(`  ${line}`);
  }
  const text = lines.join("\n");
  if (length(text) > MAX_TEXT) return { tooLarge: "more than the dialog shows" };
  return { text };
}

// Asks the user about `actions` in a dialog titled `title`, `names` naming them to the model;
// blocked when there's no one to ask, the user declines, or it can't be shown whole.
async function ask(ctx, title, actions, names) {
  if (!ctx?.hasUI) return { block: true, reason: `Blocked (no one to ask): ${names}` };
  const dialog = describe(actions);
  if (dialog.tooLarge) return { block: true, reason: `Blocked: ${dialog.tooLarge}. ${TOO_LARGE}` };
  // Stopping the run settles the dialog as "no".
  const ok = await ctx.ui.confirm(title, dialog.text, { signal: ctx.signal });
  if (ctx.signal?.aborted) return { block: true, reason: `Blocked (the run was stopped): ${names}` };
  if (!ok) return { block: true, reason: `The user declined: ${names}` };
  return undefined;
}

// Composio's tool `tool` called with `input`.
function connected(tool, input, ctx, readonly) {
  const call = classify(tool, input);
  if (call.kind === "read") return undefined;
  if (call.kind === "blocked") return { block: true, reason: call.reason };
  const names = call.actions.map((action) => escaped(named(action.slug)).slice(0, 120)).join(", ");
  if (readonly) {
    const what = call.writes ? `${names} isn't a known read` : `this call would keep memory or a workbench copy at Composio`;
    return { block: true, reason: `Blocked: the connected apps are read-only here, and ${what}.` };
  }
  return ask(ctx, WRITE_TITLE, call.actions, names);
}

// `path` as an absolute path, lowercased (macOS's disks ignore case): `@` and `~` as pi reads
// them, relative to `cwd`, `.` and `..` taken out. Symlinks aren't followed.
function resolved(path, cwd, home) {
  let text = String(path).trim();
  if (text.startsWith("@")) text = text.slice(1);
  if (text === "~" || text.startsWith("~/")) text = `${home ?? ""}${text.slice(1)}`;
  if (!text.startsWith("/")) text = `${cwd ?? ""}/${text}`;
  const parts = [];
  for (const part of text.split("/")) {
    if (part === "..") parts.pop();
    else if (part !== "" && part !== ".") parts.push(part);
  }
  return `/${parts.join("/")}`.toLowerCase();
}

// pi's agent folder, resolved, as the guard loads.
function agentDir() {
  const home = process.env.HOME ?? "";
  const dir = process.env[AGENT_DIR];
  return resolved(typeof dir === "string" && dir.trim() !== "" ? dir : `${home}/.pi/agent`, process.cwd(), home);
}

// The call `name` with `input` reaches pi's own setup in `setup` (its agent folder): a write or
// edit of a file in it, or a bash command naming it.
function touchesSetup(name, input, ctx, setup) {
  if (WRITERS.has(name)) {
    const home = process.env.HOME ?? "";
    const cwd = typeof ctx?.cwd === "string" ? ctx.cwd : process.cwd();
    return PATH_ARGS.some((key) => {
      if (typeof input?.[key] !== "string") return false;
      const path = resolved(input[key], cwd, home);
      return path === setup || path.startsWith(`${setup}/`);
    });
  }
  if (name !== "bash") return false;
  return typeof input?.command === "string" && namesSetup(input.command, setup);
}

// `text` names the folder `path` (not one whose name only starts so).
function mentions(text, path) {
  if (path === "" || path === "/") return false;
  for (let at = text.indexOf(path); at >= 0; at = text.indexOf(path, at + 1)) {
    if (!/[\w.-]/.test(text[at + path.length] ?? "")) return true;
  }
  return false;
}

// The bash `command` names pi's own setup in `setup` (its agent folder), any case, quotes and
// backslashes left out: the agent folder's path or that of the folder holding it (pi's HOME and
// working folder are there too), SETUP_WORDS, `extensions/` with `..`, or a path segment `agent`
// reached through `..` or `~` (`../agent`, `~/../agent`, `$HOME/../agent`). Best effort: a
// command can reach them without naming them (a variable, a glob, a script it runs).
function namesSetup(command, setup) {
  const text = command.toLowerCase().replace(/\\(.)/gs, "$1").replace(/["'`]/g, "");
  if (mentions(text, setup) || mentions(text, setup.slice(0, setup.lastIndexOf("/")))) return true;
  if (SETUP_WORDS.some((word) => text.includes(word.toLowerCase()))) return true;
  if (text.includes("..") && text.includes("extensions/")) return true;
  return CLIMBS.test(text) && AGENT_SEGMENT.test(text);
}

// What the call `event` gets: undefined (it runs) or `{ block, reason }`.
async function guard(event, ctx, readonly, tainted, setup) {
  const name = typeof event?.toolName === "string" ? event.toolName : "";
  const tool = composioTool(name);
  if (tool !== null) return connected(tool, event.input, ctx, readonly);
  if (PI_READS.has(name) || CALLERS.has(name) || name.startsWith(COMPUTER)) return undefined;
  // bash, write, edit, another server's tools, any other tool.
  const shownName = escaped(named(name)).slice(0, 120);
  if (readonly) {
    return { block: true, reason: `Blocked: ${shownName} doesn't run in read-only mode, where only the connected apps' known reads and read, grep, find and ls do.` };
  }
  if (touchesSetup(name, event.input, ctx, setup)) return ask(ctx, SETUP_TITLE, [{ slug: name, args: event.input }], shownName);
  if (!tainted()) return undefined;
  return ask(ctx, TAINTED_TITLE, [{ slug: name, args: event.input }], shownName);
}

// The session holds what taints it: the guard's entry, or a result of a tool that taints
// (also one called from another tool, in its result's `nestedCalls`).
function taintedIn(entries) {
  return entries.some((entry) => {
    if (entry?.type === "custom" && entry.customType === TAINT) return true;
    const message = entry?.type === "message" ? entry.message : undefined;
    if (message?.role !== "toolResult") return false;
    if (untrusted(message.toolName)) return true;
    let nested = "";
    try {
      nested = JSON.stringify(message.nestedCalls ?? "");
    } catch {}
    return /"mcp__(composio__|computer__|[^"]*__COMPOSIO_)/.test(nested);
  });
}

function entriesOf(ctx) {
  try {
    const entries = ctx?.sessionManager?.getEntries?.();
    return Array.isArray(entries) ? entries : [];
  } catch {
    return [];
  }
}

export default function (pi) {
  // Read once, as the guard loads.
  const readonly = readOnly(process.env[MODE]);
  const setup = agentDir();
  // This session's (pi loads the guard afresh for each one).
  let tainted = false;
  const taint = (why) => {
    if (tainted) return;
    tainted = true;
    try {
      pi.appendEntry(TAINT, { why: String(why).slice(0, 200) });
    } catch {}
  };
  const isTainted = (ctx) => {
    if (!tainted && taintedIn(entriesOf(ctx))) tainted = true;
    return tainted;
  };

  pi.registerCommand?.(COMMAND, {
    description: "Ailfred's guard for the connected apps is loaded (says whether this session is tainted)",
    handler: async (_args, ctx) => {
      ctx?.ui?.notify?.(isTainted(ctx) ? "Tainted: bash, write and edit ask first." : "Not tainted.", "info");
    },
  });
  pi.on("session_start", (_event, ctx) => {
    tainted = taintedIn(entriesOf(ctx));
  });
  pi.on("input", (event) => {
    if (typeof event?.text === "string" && event.text.includes(UNTRUSTED_CONTEXT)) taint("an earlier conversation");
  });
  pi.on("tool_result", (event) => {
    if (untrusted(event?.toolName, event?.input)) taint(event.toolName);
  });
  pi.on("tool_call", async (event, ctx) => {
    const verdict = await guard(event, ctx, readonly, () => isTainted(ctx), setup);
    // Let through, its result comes in: tainted before it runs.
    if (verdict === undefined && untrusted(event?.toolName, event?.input)) taint(event.toolName);
    return verdict;
  });
}
