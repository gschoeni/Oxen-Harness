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
| `turn.delivery_ready` | background work (a `wait: false` generation or fleet, a background shell task) finished; a running turn delivers it itself, an idle chat needs `POST …/turns/deliver` |
| `agent.token` | streamed assistant text (batched ~512 bytes) |
| `agent.tool` | tool call start (`detail` = args) / end (`detail` = result) |
| `agent.tool_delta` | streaming fragments of a tool call's JSON args |
| `agent.tool_progress` | a running tool's live output (a shell command streaming), keyed by `call_id` |
| `agent.usage` | live token usage around each model call |
| `agent.compacted` / `agent.compression` / `agent.retry` | context + resilience notices (`agent.retry` names the failed call's `model` and `endpoint`, its HTTP `status` when there was a reply, and `detail`: the provider's raw error body or the transport error chain — the material behind the one-line `error`, for a client's error-detail view) |
| `agent.notice` | a one-line notice about something the agent did on its own (`kind` = `background_task`: a finished task's output was delivered to the model; `nudge`: the model is being re-called with a corrective; `fleet`: a `spawn_agents` fleet run with `wait: false` finished and its report was delivered) |
| `agent.question` | `ask_user_question` — answer via `POST /v1/questions/{id}/answer` |
| `agent.approval_request` | permission gate — answer via `POST /v1/approvals/{id}/answer` |
| `agent.approval` | pending/resolved thread markers for gated calls |
| `agent.canvas` / `agent.canvas_writing` / `agent.open_file` | host-surface documents/files (`agent.canvas` carries `path` when the model showed a project file: a client that watches files re-reads it on change instead of waiting for another call) |
| `fleet.started` / `fleet.agent` / `fleet.agent_activity` / `fleet.budget` / `fleet.completed` | parallel subagent lanes (`budget`: where the turn's shared tree budget stands — tokens, model calls, spawns against their caps — after a lane's spend changed); every event names its `fleet` (a `wait: false` fleet can overlap another in one session), `fleet.started` names the `call` (the model's tool-call id) when a model's `spawn_agents` / `map_agents` / `send_to_agent` started it — so a client places the lanes beside that call in the thread — and `fleet.agent` names the `lane` (its session id — lanes persist under the parent session, so `send_to_agent` / `read_agent` and the `…/agents/{agent}` routes address it); `agent_activity` `kind: note` is a one-line notice (a refused command, a nudge, a compaction, a retry) |
| `tasks.changed` | the session's background shell tasks after one started, ended, or was killed — the whole list (`TaskSummary`), so a client never merges |
| `media.changed` | the project's media library after a generation was queued, progressed, finished, or failed — `root` names the project and `items` is the whole library (`MediaItem`, newest first), so a client never merges; `session` is the chat whose generation changed |
| `review.progress` / `review.token` / `review.tool` | code-review pipeline progress |
| `preview.status` / `preview.console` | dev-server lifecycle + page errors |
| `local.status` / `models.progress` | local-model loading / downloads (app-wide, no `session`) |

## REST surface

Sessions and turns:

```
GET    /v1/sessions                     list (SessionSummary[])
POST   /v1/sessions                     new chat → SessionInfo
GET    /v1/sessions/{id}                resume → SessionView {info, messages, running, pending} — `pending` replays the `agent.question`/`agent.approval_request` a running turn is parked on, for a client that missed the live event
DELETE /v1/sessions/{id}
GET    /v1/sessions/{id}/messages       raw persisted transcript (JSON values)
POST   /v1/sessions/{id}/turns          {prompt, attachments?} → {text}
POST   /v1/sessions/{id}/turns/retry    re-drive the trailing user turn → {text}
POST   /v1/sessions/{id}/turns/deliver  run a turn that starts from finished background results → {text}, or null when none are pending (or a turn is running)
POST   /v1/sessions/{id}/interject      {text} → {accepted} — steer the running turn
POST   /v1/sessions/{id}/cancel         stop the in-flight turn (no-op when idle)
POST   /v1/sessions/{id}/fleets/{fleet}/cancel   stop one `spawn_agents` fleet (named on its `fleet.started`) without ending the turn; 404 once it has ended
GET    /v1/sessions/{id}/agents          every subagent lane of the session, running and finished → [AgentSummary {id, label, fleet, status, summary, tokens, rounds, elapsed_secs, created_at}]
GET    /v1/sessions/{id}/tasks           the session's background shell tasks → [TaskSummary {id, command, running, exit_code, killed, elapsed_secs, last_line}]
POST   /v1/sessions/{id}/tasks/{task}/kill   kill one background task (its whole process group); 404 when unknown
GET    /v1/media?root=                  the project's image/video generations, newest first → [MediaItem {id, session, kind, model, prompt, params, refs, sources, parent, path, poster, status, cost_usd, created_at, provider, …}] (`sources` traces each reference to the chat attachment, earlier generation, or project file it came from; `provider` is the hub's completed record) (defaults to the active project)
POST   /v1/media/{generation}/cancel    cancel an in-flight generation by its hub id
GET    /v1/media/prefs                  media preferences → MediaPrefs {default_image_model, default_video_model, output_dir, per_generation_usd, per_run_usd}
PUT    /v1/media/prefs                  replace the media preferences (applies to new/resumed chats)
GET    /v1/media/models?kind=           the hub's image/video models → [MediaModelSummary {id, kind, price, developer, summary, inputs}]
POST   /v1/sessions/{id}/agents/{agent}/cancel     stop one running lane (its id rides on `fleet.agent`); the rest of its fleet carries on; 404 once it has ended
POST   /v1/sessions/{id}/agents/{agent}/interject  {text} → {accepted} — steer one running lane
GET    /v1/sessions/{id}/agents/{agent}/patch     saved isolated patch as a JSON string
POST   /v1/sessions/{id}/agents/{agent}/patch     JSON string containing the exact reviewed patch → 204; rejects stale artifacts, running lanes, or conflicts
POST   /v1/sessions/{id}/agents/{agent}/follow-up {text} → JSON result string; continues the saved lane (fleet events stream while it runs)
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

Threads (the overview every project page and chat list reads):

```
GET    /v1/threads                   → ThreadSnapshot {entries, running}
POST   /v1/sessions/{id}/seen        record "the user just looked at this thread" → new mark (unix secs)
POST   /v1/sessions/{id}/title       name a chat: {title} (blank = title by first message again)
POST   /v1/sessions/{id}/finish      mark a thread finished → finished_at (unix secs)
DELETE /v1/sessions/{id}/finish      reopen it
```

Each `ThreadEntry` is derived truth about one native session: title, freshness
(`last_activity_at`), whether the transcript stops mid-turn (`mid_turn` — the
reply never arrived), the opening of its newest reply (`last_reply`), the
training-data curation verdict (`review_status`), when the user last looked at
it (`seen_at` — activity newer than this is "finished while you were away",
per thread), and `finished_at` — the one piece of human-authored state, set by
the finish route and cleared by reopening (or by the thread running again; a
running thread can't be finished). `running` comes from the host's in-flight
registry, so it is correct even after a client restart.

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

### Agent result lifecycle

`fleet.agent.phase` also includes `partial` and `cancelled`; neither means
success. Completion events are emitted after the lane result is saved and its
live registration removed. Clients can fetch history immediately on completion.
`GET …/agents` includes all descendants and reports `parent`, `depth` (direct
children are 0), `model`, `stop`, and `has_patch`, in addition to the existing
fields. Cancel and interject acknowledgments must be checked: an agent may
finish between selection and delivery. Keep unsent text when delivery fails.


## Workbench API

`POST /v1/sessions/{id}/workbench/{action}` uses the normal bearer authorization
and binds all paths and runs to the recorded session workspace. The same actions
are exposed by the desktop `workbench_request` command. See
[the workbench guide](app/WORKBENCH.md) for payloads and lifecycle behavior.

`view.open` emits `{session, view, path}` and requests a view change within that
conversation. `view.context` emits `{session, path, text}` to add context to the
composer without sending a message automatically. Renderer state is explicitly
reported through `report`; opening a file does not imply that it was rendered.

`register_views` accepts `{views: ViewDefinition[]}` from the trusted renderer to
advertise bundled modules for the current session. Each definition contains
`id`, `title`, `description`, `file_patterns`, `requires_file`, optional `priority`
and `document_schema`. It does not execute code or install packages. The installed
package bridge does not expose this action.

`develop` accepts `{action, source, id?, title?, digest?}`. `source` is a
workspace-relative package directory. Actions: `scaffold`, `check`, `preview`,
`status`, `test`, `install`, `pause`, `resume`, `rollback`, `stop`. Preview/install require
an exact checked digest. The response includes candidate/preview revisions,
capabilities, diagnostics, runtime/test results, and a project `report_path`.
`generation` changes on each preview restart, including unchanged source; clients
remount on this value. `previous` identifies the rollback snapshot. Rollback pauses
updates without editing source. Reports are scoped to source and conversation and
restore as inactive after a host restart.
Native package-only diagnostic and retained-state methods are instance-bound;
they are not exposed through the general HTTP dispatcher.
