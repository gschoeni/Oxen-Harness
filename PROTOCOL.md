# The oxen-harness wire protocol

How to build a UI on the harness without touching the desktop app: one SSE
stream of events plus a small REST surface, served by `harness-server` and
spoken natively by the desktop app (its Tauri events are the same shapes on a
different bus).

The layering:

```
harness-protocol   the wire types: ProtocolEvent + command DTOs (serde + JSON Schema)
harness-host       SessionService — multi-session orchestration, generic over EventSink
harness-server     axum: REST + SSE over a SessionService        ← HTTP UIs start here
app/src-tauri      Tauri: IPC commands + webview events over the same service
```

`crates/harness-protocol/tests/wire.rs` and `crates/harness-server/tests/http.rs`
are the executable spec; this document is the map.

## Running the server

```sh
cargo run -p harness-server -- --token dev-token          # 127.0.0.1:4770
oxen-harness-server --port 4770 --project ~/code/my-app   # binary form
```

Without `--token` a random token is generated and printed. Configuration
(connection + API keys, model catalog, tool/skill prefs, permission rules)
comes from the same `~/.oxen-harness` the CLI and desktop use. The server
binds `127.0.0.1` by default — v1 is a single-user, local protocol.

Try it: `examples/web-chat.html` is a dependency-free single-file client that
exercises everything below.

## Authentication

Every `/v1` route requires `Authorization: Bearer <token>`. The SSE endpoint
also accepts `?token=<token>` because browser `EventSource` cannot set
headers. Failures return `401 {"error": "…"}`; all errors share that shape.

## The event stream

```
GET /v1/events                 all sessions, session-tagged
GET /v1/events?session=<id>    one session (app-wide events still included)
```

Server-sent events; each frame is `id: <n>` plus `data: <ProtocolEvent JSON>`.
Reconnecting with `Last-Event-ID` (which `EventSource` does automatically)
replays missed events from a 4096-event buffer.

Every event carries a `type` tag and, when session-scoped, a `session` field.
The catalog (see `harness-protocol/src/event.rs` for the exact fields):

| type | meaning |
|---|---|
| `turn.started` / `turn.completed` / `turn.failed` | turn lifecycle (completed carries the final `text`) |
| `agent.token` | streamed assistant text (batched ~512 bytes) |
| `agent.tool` | tool call start (`detail` = args) / end (`detail` = result) |
| `agent.tool_delta` | streaming fragments of a tool call's JSON args |
| `agent.usage` | live token usage around each model call |
| `agent.compacted` / `agent.compression` / `agent.retry` | context + resilience notices |
| `agent.question` | `ask_user_question` — answer via `POST /v1/questions/{id}/answer` |
| `agent.approval_request` | permission gate — answer via `POST /v1/approvals/{id}/answer` |
| `agent.approval` | pending/resolved thread markers for gated calls |
| `agent.canvas` / `agent.canvas_writing` / `agent.open_file` | host-surface documents/files |
| `fleet.started` / `fleet.agent` / `fleet.agent_activity` / `fleet.completed` | parallel subagent lanes |
| `review.progress` / `review.token` / `review.tool` | code-review pipeline progress |
| `preview.status` / `preview.console` | dev-server lifecycle + page errors |
| `local.status` / `models.progress` | local-model loading / downloads (app-wide, no `session`) |

## REST surface

Sessions and turns:

```
GET    /v1/sessions                     list (SessionSummary[])
POST   /v1/sessions                     new chat → SessionInfo
GET    /v1/sessions/{id}                resume → SessionView {info, messages, running}
DELETE /v1/sessions/{id}
GET    /v1/sessions/{id}/messages       raw persisted transcript (JSON values)
POST   /v1/sessions/{id}/turns          {prompt, attachments?} → {text}
POST   /v1/sessions/{id}/turns/retry    re-drive the trailing user turn → {text}
POST   /v1/sessions/{id}/interject      {text} → {accepted} — steer the running turn
POST   /v1/sessions/{id}/cancel         stop the in-flight turn (no-op when idle)
POST   /v1/sessions/{id}/refresh-client rebuild the agent's client (after saving a key)
```

`POST …/turns` stays open until the turn settles and resolves with the final
text; the streaming happens on `/v1/events`, so fire-and-forget + SSE is the
normal UI pattern. Only one turn (or review/loop) runs per session at a time;
different sessions run concurrently.

`POST …/interject` delivers a user message *into* the running turn: it enters
the transcript at the turn loop's next safe point (framed so the model knows
it arrived mid-work), and a message that lands while the final reply streams
forces one more model round rather than being dropped. `accepted: false`
means no turn was running — send the text as an ordinary `…/turns` prompt
instead.

Round-trips (ids arrive on the stream):

```
POST /v1/questions/{id}/answer   {answers: [{header, question, selected: [..]}]}
POST /v1/approvals/{id}/answer   {decision: "once"|"session"|"project"|"trash"|"bypass"|"deny", message?}
```

An unanswered id is simply forgotten server-side if its chat goes away; UIs
can answer late without error (it returns 200 and does nothing).

Runners:

```
POST /v1/sessions/{id}/review    {base_branch?} → ReviewResult (events: review.*, fleet.*)
POST /v1/sessions/{id}/loop      {name?, goal?} → LoopResult   (events: agent.*)
```

Models and connection:

```
GET  /v1/models        cloud-model catalog
POST /v1/model         {model} → SessionInfo — swaps the current chat in place
GET  /v1/connection    host + key presence (never the secrets)
PUT  /v1/connection    {host, api_key, brave_api_key} → 204 (then refresh-client per live session)
```

Attachments (for non-local clients that can't pass file paths):

```
POST /v1/attachments?filename=note.png   body = raw bytes → {path, bytes}
```

The returned `path` goes into a later turn's `attachments` array.

Misc: `GET /v1/health` → `{status: "ok", protocol: "v1"}`.

## Typed clients

The protocol self-describes. Generate the JSON Schema and feed it to any
schema-to-types generator:

```sh
cargo run -p harness-protocol --bin protocol-schema > protocol-schema.json
npx json-schema-to-typescript protocol-schema.json   # for example
```

## What is deliberately not in v1

- **Multi-user / remote deployment** — one bearer token, no tenancy; bind
  stays loopback unless you pass `--host` and accept the risk (the agent's
  tools operate on the server's filesystem).
- **Native surfaces** — file dialogs, drag-drop, the desktop's embedded
  preview/browser webviews. `preview.status` carries the dev-server URL; a
  web UI renders it in an iframe or a new tab instead.
- **Settings management** (tools/skills/themes/permissions editing, local
  model downloads) — desktop/CLI only for now; the server reads the shared
  config they write.
