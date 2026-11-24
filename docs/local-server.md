# The local server

Apps on this Mac (a web view, a Swift app) can show and continue the conversation in Ailfred's
chat: the same conversation as the side panel or the chat window, not a copy. It speaks AG-UI
1.0 (`crates/ailfred-protocol`) over HTTP on `127.0.0.1`.

Off until turned on in Settings ("Local server"). Turning it on starts it; turning it off stops
it: it takes no new request, open streams end, and the discovery file is removed. It starts at
launch when the switch is on, and stops at quit.

## Finding it

`~/Library/Application Support/com.ailfred/server.json` (`com.ailfred.dev` for a `cargo run`):

```json
{"url":"http://127.0.0.1:52344","token":"<64 hex>","pid":4242}
```

Written owner-only (0600, in a 0700 folder, to a `.tmp` and renamed). The port and the token
are fresh on every start; the token is never logged. It's removed when the server stops, and
at every launch before the switch is read (a crash or a kill leaves it behind). A file can
still be stale while the app is gone: check that `pid` is alive (`kill -0 <pid>`) and is
Ailfred before sending anything, and read the file again when a connection is refused.

## HTTP

A small HTTP/1.1 server of Ailfred's own (`src/server/http.rs` on the connection layer in
`src/server/conn.rs`, heads read by `src/server/head.rs`). Computer control's server (its tools
for the model, `src/computer/server.rs`) stands on the same layer with limits of its own, in
its module doc. This one's:

- At most 32 connections open at once; one more is closed as it comes. Each has a thread of
  its own; the accepting thread serves none.
- A connection's first byte must come within 1 s, else it's closed without an answer. The
  head (request line and headers) must come within 5 s in all, however slowly it's sent, else
  **408** and the connection closes. At most 16 KiB and 64 headers, else **431**.
- Lines end in CRLF: a bare LF or CR is **400**, and so is a folded header line. Header names
  are read whatever their case.
- No `Transfer-Encoding` (**501**): a body has one `Content-Length` of digits (a malformed or
  repeated one is **400**), and a POST must have one (**411**).
- A body let in is read as it comes, within 30 s, up to its `Content-Length`. `Expect:
  100-continue` gets its `100 Continue` once the guards let the request in.
- Every answer says `Connection: close`, and the connection closes after it: one request per
  connection, no keep-alive, no pipelining. An answer not taken within 10 s is cut off.
- A command whose body is still coming when the server is switched off is never applied, even
  if the switch goes back on before the body ends: **503**.

**Local lockout.** Any process on this Mac, even one without the token, can keep the server
busy by holding connections: 32 that each send a byte now and then, opened again as they're
closed, leave no room for other clients. The app itself is unaffected (the chat, the side panel
and the window don't go through the server). This is inherent to a service on the loopback
interface; the limits above only make it cost the other process more.

## Guards

In this order, on the head alone. A refused request's body is never read: the answer is
written and the connection closed (a client still sending its body may then see the
connection reset instead of the answer).

1. `Host` must be `127.0.0.1:<port>` or `localhost:<port>`, the port in plain digits (no
   sign, no leading zero), else **403** (DNS rebinding).
2. A request with an `Origin` header gets **403**, whatever the origin: no web page may call it
   for now. Whether a loopback origin (a web view) may is T21's to decide.
3. `Authorization: Bearer <token>`, compared in constant time, else **401**.
4. Unknown path **404**, wrong method **405**.
5. A command body over 16 MiB (enough for base64 screenshots) gets **413**.
6. More than 8 commands in flight, or more than 4 event streams, get **503**.

Every refusal is JSON: `{"error": "<short reason>"}`.

**503 means not applied.** A command the chat didn't take within 10 s is given up on and never
applied later; a join that came before the chat was ready gets nothing. Send the same request
again (the same `runId`): it can't run twice.

## `GET /v1/events`

`text/event-stream`, one AG-UI event per `data: <json>` frame, and a `: ping` comment after 15 s
without one (a client that's gone is then dropped; its run goes on). The body ends when the
connection closes (no chunked encoding).

- A client that reads too slowly is dropped: 256 frames waiting for it, or one frame it takes
  over 10 s in all to read (however much it reads meanwhile), and its stream closes. Its run goes on; join again for a fresh snapshot.
- Joining before the chat is ready (just after launch) gets **503**: try again.

- On connect: a `MESSAGES_SNAPSHOT` of the conversation, its id (the `threadId` to send) in
  `metadata.ailfred.threadId`, without the runs still going; then the run going, from its `RUN_STARTED`; then everything live.
- A new conversation (Cmd+N in the chat window or the side panel, a `new_conversation`
  command, one picked from Recent) sends a new `MESSAGES_SNAPSHOT` at once, empty for a new
  one, under the `threadId` its first message will use. After Cmd+N in the side panel the old
  conversation still shows there as it slides out, but clients are on the new one already;
  its Retry, Continue and question answers start nothing there any more. Picking the old one
  back from Recent sends its snapshot again, and its `runId`s stay used. A new conversation is
  saved only once it has a message.
- The same conversation changed in place sends a fresh `MESSAGES_SNAPSHOT` too, then the runs
  going from their `RUN_STARTED`, as a client joining gets: a question declined (it's
  `"closed": true` there), or a Retry (from the chat or a client), whose snapshot leaves out
  the retried message; its new run brings it back. A retried message that isn't the
  conversation's last thus comes after the others until its run ends, when another snapshot
  puts it in its place, however that run ends (done, failed, lost, stopped, or stopped by the
  next reply, as answering the latest question does). A client never shows the old reply
  beside the new one.
- Every `MESSAGES_SNAPSHOT` reaches every client, whoever made the change (the window's Retry
  too): replace your whole state with it each time, then apply the runs that follow. A client
  that merges or ignores it shows the conversation out of date.
- Runs never interleave: pi's steered replies overlap, so a later run's events are held until
  the earlier run's `RUN_FINISHED` or `RUN_ERROR` is out. Use one replayer per run.
- Each `RUN_STARTED` carries the user message it answers in `input.messages` (`{run}-user`), so
  a message typed in the window, or sent by another client, shows before its reply.
- Stop ends a run `RUN_FINISHED` outcome `cancelled`; a failure `RUN_ERROR`; a model process that
  died `RUN_ERROR` code `ailfred:lost`. A step's `TOOL_CALL_RESULT` is cut to its last 64 KiB.
- Run ids: a run a client started keeps the client's `runId`; others get one minted,
  `{conversation}-{token}-{n}`. A reply already going when the server starts (or the web view
  opens) is a run named anew: a client joining gets it from its `RUN_STARTED`, what came so
  far encoded from the reply as it stands, then live. A snapshot names replies by their saved run ids
  (`{conversation}-{index}`), which differ from the live ones: on each `MESSAGES_SNAPSHOT`,
  replace your state; don't merge by id.
- In a snapshot, a question nothing waits for (answered, closed, or in a reply that failed or
  was stopped) says `"closed": true` or `"answered"`.

## `POST /v1/commands`

One `ailfred_protocol::command::Command` as JSON (`schema/command.schema.json`). Taken commands
answer **202** `{"threadId","runId"}`; the run's events come on the stream.

- `run` (a `RunAgentInput` tagged `"type":"run"`):
  - `runId` is 1 to 128 characters of `[A-Za-z0-9._:-]`, else **400** (for `stop` too).
    A `runId` shaped as a snapshot's saved run, `{threadId}-{digits}`, gets **400** (for
    `retry` too), so a `replyRunId` always names one reply.
  - `threadId` must be the conversation in the chat, and `runId` new in it, else **409**. A
    thread's used `runId`s are kept while it's the conversation and the one just left (Cmd+N,
    then picking it back from Recent): two threads at most; one left before them starts afresh.
  - Without `resume`: the last of `messages` must be the user's (text, and images as inline
    base64 PNG parts), else **400**. The images are checked before the **202**: at most 8 in
    a message; each header must say 1 to 8192 pixels a side, and all of them 40 megapixels at
    most in all (every header is read before any image is decoded); and each must decode;
    else **400**. It is sent as if typed in the chat: with pi it steers the
    run going, otherwise it waits for it, to the model picked there. The chat's conversation is
    the truth: the earlier `messages` (AG-UI's complete history) are not replayed to the model.
  - With `resume`: it answers the open questions of the latest reply, each by its `interruptId`
    as it was sent (the live run's `{run}-ask-{n}`, or the snapshot's; an id is matched whole,
    never taken apart), with `status: "resolved"` and
    `payload: {"answer": "…"}`; several picks travel as one string joined with `", "`. A
    partial resume answers the ones it names. An answered or closed question, a reply that
    failed or was stopped, or an unknown `interruptId` gets **409**; a question named twice
    **400**. The answers go as one user message, as in the chat, so a later snapshot shows them
    as the answering run's `{run}-user` message.
  - `status: "cancelled"` (no payload) declines a question: it closes and shows as "Not
    answered" in the chat; clients get a fresh `MESSAGES_SNAPSHOT` with the question
    `"closed": true`. **202** `{"threadId","runId"}`. With pi, whose dialog waits for the
    answer, the dialog is dismissed (as when the user closes it) and pi goes on: the decline
    goes as the user's message `Declined.`, as an answer does, and what pi says next is its
    reply, run `runId`. With other models nothing waits (the run ended with the question), so
    nothing goes to the model and no run starts (the `runId` is used up all the same). A
    resume with both answers and declines sends the answers as above (that run starts) and
    closes the declined ones. The same **409**s apply. The chat itself has no Decline: in the
    window a question is answered, or a new message is sent instead.
- `stop`: with the conversation's `threadId` and the `runId` of the run streaming right now,
  it does what the chat's Stop does, else **409**: a run queued behind it, or one that ended,
  can't be stopped.
- `new_conversation` `{"threadId"}`: with the conversation's `threadId` (else **409**), it
  does what Cmd+N does: a reply streaming stops and what's queued is dropped (the old
  conversation is saved so); in the chat window, the window empties; in the side panel, the
  panel slides out if it shows (no input rises). **202** `{"threadId"}`, the new one's; its
  empty `MESSAGES_SNAPSHOT` comes on the stream.
- `retry` `{"threadId","runId","replyRunId"}`: does what the chat's Retry does to the reply
  `replyRunId` names, as run `runId`. The chat offers Retry on every reply that ended (done,
  stopped or failed: the footer's, or the error's), not on one streaming or queued, so
  `replyRunId` names the reply by its run: the live run's `runId` (its `RUN_STARTED`), or the
  snapshot's `{run}` (its user message's id without `-user`), matched whole. Retrying stops a
  reply that streams. **202** `{"threadId","runId"}`, then a snapshot without the retried
  message, then the new run. A wrong thread, a used `runId`, or a `replyRunId` naming no reply
  that ended (one streaming or queued, one already retried, after Cmd+N) gets **409**. Live
  run ids and saved ones can't clash: a client's `runId` never has the saved shape.
- Malformed JSON, and a command of a type this version doesn't know (a newer client's), get
  **400** `{"error": "not a command: unknown variant …"}`.

## Not supported yet

- Tools a client offers (`tools`, e.g. its own capture).
- Picking a model as a command.

## Example

```sh
f=~/Library/Application\ Support/com.ailfred/server.json
url=$(jq -r .url "$f"); token=$(jq -r .token "$f")
curl -N -H "Authorization: Bearer $token" "$url/v1/events" &
thread=…  # the snapshot's metadata.ailfred.threadId
curl -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
  -d "{\"type\":\"run\",\"threadId\":\"$thread\",\"runId\":\"cli-1\",\"messages\":[{\"role\":\"user\",\"id\":\"cli-1-user\",\"content\":\"Hello\"}]}" \
  "$url/v1/commands"
```
