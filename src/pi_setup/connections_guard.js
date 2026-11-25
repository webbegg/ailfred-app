// Ailfred's guard for the connected apps: the tools of the `composio` server in the mcp.json of
// Ailfred's pi. What the model reads there (an email, an issue, a calendar invite) can carry
// instructions that aren't the user's, so only what this table knows to be a read runs on its
// own; anything else, unknown slugs included, waits for the user's yes in the chat.
//
// - READS and META_READS run without asking.
// - Every other action asks: each Composio tool slug run through COMPOSIO_MULTI_EXECUTE_TOOL that
//   isn't in READS, and each Composio tool that isn't in META_READS (COMPOSIO_MANAGE_CONNECTIONS,
//   one Composio adds later). With no one to ask (no UI), or in read-only mode, it's blocked.
// - Read-only mode: the AILFRED_CONNECTIONS variable, set by Ailfred, is "readonly". Any value
//   but none, "" or "ask" counts as read-only, so a typo doesn't open the writes.
// - The tools Ailfred hides from the model (BLOCKED) are blocked even when called, also as a
//   slug inside a batch; so is a call whose actions can't be read.
//
// Written to pi's agent folder by `pi_setup::connections`; plain JavaScript, no imports.

const PREFIX = "mcp__composio__";
const EXECUTE = "COMPOSIO_MULTI_EXECUTE_TOOL";
const MODE = "AILFRED_CONNECTIONS";
const TITLE = "Allow these actions on your connected apps?";

// Composio tool slugs that only read, from Composio's toolkit docs
// (docs.composio.dev/toolkits/<toolkit>.md). Not here: anything that sends, creates, changes,
// deletes, marks as read, watches or runs a query language that can write.
const READS = new Set([
  // Gmail
  "GMAIL_FETCH_EMAILS",
  "GMAIL_FETCH_MESSAGE_BY_MESSAGE_ID",
  "GMAIL_FETCH_MESSAGE_BY_THREAD_ID",
  "GMAIL_GET_ATTACHMENT",
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
  "GOOGLECALENDAR_CALENDAR_LIST_LIST",
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
  "GITHUB_GET_RAW_REPOSITORY_CONTENT",
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
// side, feedback sent out, skills. Never run, however they're reached.
const BLOCKED = [/^COMPOSIO_PROXY_EXECUTE$/, /^COMPOSIO_REMOTE_BASH_TOOL$/, /^COMPOSIO_REMOTE_WORKBENCH$/, /^COMPOSIO_SUBMIT_FEEDBACK$/, /^COMPOSIO_.*_SKILL/];

// How much of an action the dialog shows.
const MAX_ACTIONS = 10;
const MAX_FIELDS = 12;
const MAX_VALUE = 160;

const isBlocked = (name) => BLOCKED.some((pattern) => pattern.test(name));
const named = (slug) => slug || "(no tool named)";

// What a call is: not the connected apps' ("other"), a read, a write (with its `actions`, each
// `{ slug, args }`), or blocked (with its `reason`).
function classify(toolName, input) {
  if (typeof toolName !== "string" || !toolName.startsWith(PREFIX)) return { kind: "other" };
  const tool = toolName.slice(PREFIX.length);
  if (isBlocked(tool)) return { kind: "blocked", reason: `Blocked: ${tool} isn't available to the assistant.` };
  if (META_READS.has(tool)) return { kind: "read" };
  if (tool !== EXECUTE) return { kind: "write", actions: [{ slug: tool, args: input }] };
  // `{ tools: [{ tool_slug, arguments }], ... }`. A slug that isn't a string is empty: a write.
  const tools = input?.tools;
  if (!Array.isArray(tools) || tools.length === 0) {
    return { kind: "blocked", reason: "Blocked: no tool slug found in the call, so it can't be checked." };
  }
  const actions = tools.map((tool) => ({
    slug: typeof tool?.tool_slug === "string" ? tool.tool_slug.trim().toUpperCase() : "",
    args: tool?.arguments,
  }));
  const hidden = actions.filter((action) => isBlocked(action.slug));
  if (hidden.length > 0) {
    return { kind: "blocked", reason: `Blocked: ${hidden.map((a) => a.slug).join(", ")} isn't available to the assistant.` };
  }
  const writes = actions.filter((action) => !READS.has(action.slug));
  return writes.length === 0 ? { kind: "read" } : { kind: "write", actions: writes };
}

function readOnly(mode) {
  const value = typeof mode === "string" ? mode.trim().toLowerCase() : "";
  return value !== "" && value !== "ask";
}

// `text` on one line, without control or direction-changing characters (so a value can't fake
// another line or reorder what's shown), cut to `max` characters.
function oneLine(text, max = MAX_VALUE) {
  const flat = Array.from(String(text), (ch) => (visible(ch) ? ch : " "))
    .join("")
    .replace(/\s+/g, " ")
    .trim();
  const chars = Array.from(flat);
  return chars.length > max ? `${chars.slice(0, max - 1).join("")}…` : flat;
}

// Not a control character (C0, DEL, C1), a line or paragraph separator, or a mark or override
// that changes the text's direction.
function visible(ch) {
  const c = ch.codePointAt(0);
  const hidden = c <= 0x1f || (c >= 0x7f && c <= 0x9f) || c === 0x200e || c === 0x200f || (c >= 0x2028 && c <= 0x202e) || (c >= 0x2066 && c <= 0x2069);
  return !hidden;
}

function shown(value) {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

// An action's arguments, one `name: value` line each, the empty ones left out.
function fields(args) {
  if (args === undefined || args === null || args === "") return [];
  if (typeof args !== "object" || Array.isArray(args)) return [oneLine(shown(args))];
  const entries = Object.entries(args).filter(([, value]) => value !== undefined && value !== null && value !== "");
  const lines = entries.slice(0, MAX_FIELDS).map(([name, value]) => `${oneLine(name, 40)}: ${oneLine(shown(value))}`);
  if (entries.length > MAX_FIELDS) lines.push(`…and ${entries.length - MAX_FIELDS} more`);
  return lines;
}

// What the dialog says: each action's slug, then its arguments indented under it.
function describe(actions) {
  const lines = [];
  for (const action of actions.slice(0, MAX_ACTIONS)) {
    lines.push(oneLine(named(action.slug), 80));
    for (const line of fields(action.args)) lines.push(`  ${line}`);
  }
  if (actions.length > MAX_ACTIONS) lines.push(`…and ${actions.length - MAX_ACTIONS} more actions`);
  return lines.join("\n");
}

async function guard(event, ctx, mode) {
  const call = classify(event?.toolName, event?.input);
  if (call.kind === "other" || call.kind === "read") return undefined;
  if (call.kind === "blocked") return { block: true, reason: call.reason };
  const names = call.actions.map((action) => oneLine(named(action.slug), 80)).join(", ");
  if (readOnly(mode)) {
    return { block: true, reason: `Blocked: the connected apps are read-only here, and ${names} isn't a known read.` };
  }
  if (!ctx?.hasUI) return { block: true, reason: `Blocked (no one to ask): ${names}` };
  const ok = await ctx.ui.confirm(TITLE, describe(call.actions));
  if (!ok) return { block: true, reason: `The user declined: ${names}` };
  return undefined;
}

export default function (pi) {
  pi.on("tool_call", (event, ctx) => guard(event, ctx, process.env[MODE]));
}
