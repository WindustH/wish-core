# HTTP API reference

Wish serves one JSON API over HTTP, with Server-Sent Events (SSE) for live
output. Everything the [web client](https://github.com/WindustH/wish-web) does
goes through this API, so any other client can do the same.

- [Conventions](#conventions)
- [Endpoint index](#endpoint-index)
- [Service](#service)
- [Configuration](#configuration)
- [Providers](#providers)
- [Sessions](#sessions)
- [Input and execution](#input-and-execution)
- [History and records](#history-and-records)
- [Attachments](#attachments)
- [Side questions](#side-questions)
- [Usage statistics](#usage-statistics)
- [Event streams](#event-streams)

## Conventions

**Base path.** Application routes live under `/api`. Only `/health` and
`/version` are outside it.

**Authentication.** When `bearer_token_env` is set in the
[configuration](configuration.md), every matched `/api` route requires
`Authorization: Bearer <token>`. A missing or wrong token returns
`401 {"error":{"message":"unauthorized"}}`. `/health`, `/version` and unmatched
paths are public. Browsers cannot attach headers to `EventSource`, so browser
clients read SSE with `fetch`, or sit behind a proxy that injects the header,
as `wish-web`'s `serve.mjs` does.

**Cross-origin requests.** A server that requires a token also answers web
pages on other origins: it replies to CORS preflights without authentication
and adds `Access-Control-Allow-Origin: *` to every response, including `401`.
Such a page still needs the token for anything else. A server without a token
sends no CORS headers, so browsers only let pages from its own origin call it;
an arbitrary website cannot use a visitor's browser to reach a local Wish.

**Bodies.** Request and response bodies are JSON. Most request bodies reject
unknown fields. Every `/api` request body is limited to 32 MiB; a larger one
returns `413`.

**Core types.** Messages, session configuration and model responses use the
engine's own JSON form: externally tagged enums, for example:

```json
{"User": {"content": [{"Text": {"text": "Hello"}}]}}
```

Message variants are `System`, `Developer` (with optional `fixed`), `User`,
`Assistant`, `Reasoning`, `ToolUse`, `ToolResult` and `UpstreamCompaction`.
Content blocks are `Text {text}` and `Image {mime_type, data_base64}`. Every
variant accepts an optional `metadata` value that is stored but never sent to a
model.

**Times and sizes.** Timestamps are Unix milliseconds. Token counts are
integers; a count the provider did not report is `null`, never `0`.

**Pages.** Lists read with `?start=0&limit=50` return
`{"start", "items", "next"}`, where `next` is the next `start` or `null`.
`limit` must be positive. History endpoints use cursors instead; see
[History and records](#history-and-records).

**Revisions.** The configuration carries a `revision` (a hash of the file). A
save with a stale revision returns `409`. Session descriptors carry an integer
`revision` that increases on every change; `PATCH /api/sessions/{id}` accepts
`If-Match: <revision>` (a bare number) and returns `409` on a mismatch.

### Errors

Application errors share one envelope:

```json
{"error": {"message": "session is running", "details": null}}
```

`details` is set when an error comes from a model call. It holds the structured
engine error, for example
`{"Upstream": {"status": 429, "code": "rate_limit", "message": "...", "retry_after_ms": 2000}}`.

| Status | Meaning |
| --- | --- |
| 400 | Invalid request: bad values, unknown fields on JSON that is validated by hand, invalid configuration |
| 401 | Missing or wrong bearer token |
| 404 | Unknown session, provider, generation, history sequence or attachment |
| 409 | Conflict: the session is running or unsettled, a stale revision, a consumed queue entry, or the server is shutting down |
| 413 | Request body over 32 MiB |
| 415, 422 | Body is not JSON, or does not match the expected shape (plain-text response from the HTTP framework) |
| 500 | Storage or internal failure |
| 501 | The provider has no such capability configured (model list, account reading, token counting, upstream compaction) |
| 502 | The upstream call failed, including upstream 4xx such as 401 or 429; see `details` |

## Endpoint index

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/health` | Liveness (public) |
| GET | `/version`, `/api/version` | Build name and version |
| GET | `/api/status` | Session count and scheduler activity |
| GET | `/api/storage` | Bytes on disk |
| GET | `/api/events` | Application event stream (SSE) |
| GET, PUT | `/api/config` | Read or replace the configuration |
| GET | `/api/defaults` | Defaults for new sessions |
| GET | `/api/provider-presets` | Built-in provider presets |
| GET | `/api/shells` | Default shell and shells found on `PATH` |
| GET | `/api/proxy-environment` | Proxy variables visible to the server |
| GET | `/api/directories` | Browse server directories |
| GET | `/api/providers` | Configured providers |
| GET | `/api/providers/{id}` | One enabled provider |
| GET | `/api/providers/{id}/models` | Upstream model catalog page |
| GET | `/api/providers/{id}/account` | Upstream account reading |
| POST, GET | `/api/providers/{id}/chatgpt-login` | Start or poll a ChatGPT sign-in |
| POST | `/api/providers/{id}/chatgpt-login/complete` | Finish a sign-in from a pasted redirect URL |
| POST | `/api/providers/{id}/call` | Stateless model call |
| POST | `/api/providers/{id}/count-tokens` | Provider token count |
| POST | `/api/providers/{id}/compact` | Stateless upstream compaction |
| GET, POST | `/api/sessions` | List or create sessions |
| GET, PATCH, DELETE | `/api/sessions/{id}` | Read, update or delete a session |
| POST | `/api/sessions/{id}/fork` | Copy a session's context into a new session |
| PUT | `/api/sessions/{id}/config` | Replace the session configuration |
| PUT | `/api/sessions/{id}/metadata` | Replace the session metadata |
| PUT | `/api/sessions/{id}/shell` | Give the session its own shell, or follow the global one |
| POST | `/api/sessions/{id}/input` | Queue user input and run |
| POST | `/api/sessions/{id}/messages` | Queue one message without running |
| POST | `/api/sessions/{id}/run` | Start or resume execution |
| POST | `/api/sessions/{id}/interrupt` | Request cancellation |
| POST | `/api/sessions/{id}/compact` | Compact the context now |
| POST | `/api/sessions/{id}/context/clear` | Start an empty context |
| PATCH, DELETE | `/api/sessions/{id}/queue/{entry}` | Reorder or cancel queued input |
| GET | `/api/sessions/{id}/events` | Session event stream (SSE) |
| GET | `/api/sessions/{id}/history` | Conversation timeline |
| POST | `/api/sessions/{id}/history/query` | Filtered chronological history |
| POST | `/api/sessions/{id}/history/search` | Ranked text search |
| GET | `/api/sessions/{id}/history/{sequence}` | One history record with neighbours |
| GET | `/api/sessions/{id}/entries` | Stored messages |
| GET | `/api/sessions/{id}/queue` | Queued entry IDs |
| GET | `/api/sessions/{id}/generations` | Context generations |
| GET | `/api/sessions/{id}/generations/{generation}/entries` | Entries of one generation |
| GET | `/api/sessions/{id}/calls` | Model call records |
| POST | `/api/sessions/{id}/blobs` | Upload an attachment |
| GET | `/api/sessions/{id}/blobs/{blob}` | Download an attachment |
| GET | `/api/sessions/{id}/blobs/{blob}/meta` | Attachment metadata |
| POST | `/api/sessions/{id}/ask` | Side question on the session's context |
| GET | `/api/usage`, `/api/sessions/{id}/usage` | Usage totals |
| GET | `/api/usage/series`, `/api/sessions/{id}/usage/series` | Usage over time and streaming speed |
| GET | `/api/usage/daily`, `/api/sessions/{id}/usage/daily` | Daily usage for calendars |

## Service

### `GET /health`, `GET /version`

`{"status":"ok"}` and `{"name":"wish","version":"0.1.0"}`. Both are public.
`GET /api/version` returns the same as `/version` but requires authentication,
which makes it a convenient check of a proxy's token.

### `GET /api/status`

```json
{
  "counts": {"sessions": 12, "runs": null},
  "queue": {"active_sessions": 1, "ready_sessions": 0, "pending_items": 2, "compacting_sessions": 0},
  "uptime_ms": 5321000
}
```

`sessions` counts every stored session. `queue` counts only sessions loaded in
memory. Check `active_sessions` before restarting the server.

### `GET /api/storage`

`{"bytes": {"total", "blobs", "executions", "session_data", "service_data"}, "counts": {...}}`.
`session_data` is the engine database, `service_data` the management index,
`blobs` the attachments and `executions` the captured shell output. The
`counts` fields are reserved and currently `null`. Neither status endpoint
loads any conversation.

## Configuration

### `GET /api/config`, `PUT /api/config`

`GET` returns `{"revision": "...", "config": {...}}` with secrets replaced by
`"<redacted>"`: provider `api_key`, `refresh_token`, every `credentials` and
`headers` value, and `proxy.password`. `PUT` takes the same envelope. Sending
`"<redacted>"` back keeps the stored value.

A save is validated completely before it is written. It returns `400` for an
invalid configuration, `409` when the revision is stale or the file changed on
disk, and `501` when a provider pairs a protocol with a token-count or
compaction protocol it cannot serve. Providers, proxy, shell and session
defaults apply to the next operation without a restart; changing `listen`,
`data_dir` or `bearer_token_env` is refused. Every save emits
`configuration_changed` on [`/api/events`](#application-events). The field
reference is in [configuration](configuration.md).

### `GET /api/defaults`

`{"defaults": {...}, "session_config": {...}}`: the `defaults` block of the
configuration, and the `SessionConfig` a new session would get from it. Clients
use it to fill a new-session form.

### `GET /api/provider-presets`

`{"presets": [...]}`: the built-in presets from
`resources/provider-presets.json`. Each has `id`, `provider` (branding),
`region`, `billing`, `required_credentials`, `optional_credentials`,
`reasoning_efforts`, `max_output_tokens`, `protocols` (ready connection
settings per protocol), `unsupported_protocols` and `variants`.

### `GET /api/shells`

```json
{"default": {"name": "sh", "program": "/bin/sh", "args": ["-c"]},
 "installed": [{"name": "zsh", "program": "/usr/bin/zsh", "args": ["-lc"]}]}
```

The shell used when none is configured, and the shells found on the server's
`PATH`, each with the arguments it would run with.

### `GET /api/proxy-environment`

`{"variables": [{"name": "HTTPS_PROXY", "value": "http://127.0.0.1:7890", "redacted": false}]}`:
`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY`, in both cases, as the
server process sees them. Proxy URLs show only scheme, host and port;
credentials, paths and queries are hidden and marked `redacted`.

### `GET /api/directories?path=/home/me`

```json
{"path": "/home/me", "parent": "/home", "directories": ["projects", ".config"]}
```

Immediate subdirectories of an absolute path, sorted, hidden ones included.
`path` is canonical; `parent` is `null` at the root. `path=~` opens the server
user's `HOME`. A relative, missing, unreadable or non-directory path returns
`400`.

## Providers

### `GET /api/providers`, `GET /api/providers/{id}`

`{"items": [provider, ...]}`, including disabled providers. Each provider has
`id`, `display_name`, `preset`, `enabled`, `brand`, `reasoning_efforts`,
`max_output_tokens`, `protocol`, `base_url`, `token_count`, `compaction`,
`model_list`, `account_state` and `models`. Credentials and headers are never
included. `GET /api/providers/{id}` returns `404` for an unknown or disabled
provider.

### `GET /api/providers/{id}/models?cursor=&limit=100`

One page of the upstream catalog:

```json
{"protocol": "openai_models",
 "items": [{"id": "gpt-5", "name": null, "owner": "openai", "created_at": 1700000000,
            "context_window": null, "max_output_tokens": null}],
 "next_cursor": null, "warnings": []}
```

Pages are cached for five minutes. When a refresh fails, the expired page is
served with a warning naming its age. Returns `501` without `model_list`, `400`
without `model_list_path`, `502` when the upstream fails. Saving the
configuration empties the cache.

### `GET /api/providers/{id}/account`

The provider's account reading (balance, quota or rate limits, depending on the
`account_state` protocol). Returns `501` if none is configured.

### ChatGPT sign-in

For providers made from the `openai_codex` preset:

- `POST /api/providers/{id}/chatgpt-login` starts a ten-minute PKCE sign-in and
  returns `{"authorization_url", "expires_in": 600}`. It opens a temporary
  callback listener on `127.0.0.1:1455` (or `1457`) and cancels any previous
  attempt. Returns `409` if both ports are busy.
- `GET` on the same path returns `{"status": "idle"}` or
  `{"status", "error"}` with status `pending`, `processing`, `complete`,
  `failed` or `expired`.
- Open `authorization_url` in a browser. If that browser is on another machine,
  its redirect to `localhost` fails. Copy the full URL from its address bar and
  `POST {"callback_url": "..."}` to `.../chatgpt-login/complete`.

On success Wish stores the access token, refresh token, account ID and expiry
in the configuration, refreshes them in the background before they expire, and
emits `configuration_changed`.

### `POST /api/providers/{id}/call`

A stateless model call with an engine `Request`:

```json
{"model": "gpt-5", "stream": true, "tools": [],
 "conversation": [{"User": {"content": [{"Text": {"text": "Hi"}}]}}]}
```

Optional fields are `tool_choice`, `max_output_tokens`, `reasoning` and
`cache`. With `stream: false` the response is an engine `Response`
(`messages`, `stop_reason`, `usage`, `account_state`). With `stream: true` it
is an SSE stream of `model_event` records (engine `StreamEvent`), then `done`,
or `error` with `{"message"}` if the call fails after the stream started.
Nothing is stored and closing the connection cancels the call.

### `POST /api/providers/{id}/count-tokens`

Takes a `Request`, returns `{"input_tokens", "cached_input_tokens"}` from the
provider's counting endpoint. Returns `501` without `token_count`.

### `POST /api/providers/{id}/compact`

Takes `{"model", "conversation"}` and returns the upstream compaction result:
`{"conversation", "usage", "account_state", "warnings"}`. Returns `501` without
`compaction`.

## Sessions

### Session object

Session endpoints return a session as `{"session": descriptor, "status": status}`.

```json
{
  "session": {
    "id": "8c1f...", "name": "Refactor parser", "provider": "openai",
    "cwd": "/home/me/project", "shell": true,
    "created_at": 1758790000000, "updated_at": 1758790500000, "revision": 7
  },
  "status": {
    "phase": "Idle", "state": "Idle", "running": false,
    "config": {"model": "gpt-5", "stream": true, "tools": [...], "run": {"tools": "Serial"}},
    "metadata": {"tags": ["work"]},
    "active_generation": 3, "queue_head": 12, "queue_count": 0,
    "standby_preparing": false, "context_tokens": 48210
  }
}
```

Descriptor fields:

| Field | Meaning |
| --- | --- |
| `shell` | Whether the session has shell tools |
| `shell_command` | The session's own `{program, args}`. Absent when it follows the global `shell` setting |
| `pending_selection` | A provider/model change made while running, not yet in effect |

Status fields:

| Field | Meaning |
| --- | --- |
| `phase` | `Idle`, `Ready`, `CallingModel`, `ExecutingTools`, `Compacting` or `Suspended` |
| `state` | Full engine state, only while not running |
| `running` | An operation is in progress |
| `config` | The session's `SessionConfig`; the pending one when `selection_pending` is true |
| `metadata` | Any JSON value; the web client keeps `tags` here |
| `queue_head`, `queue_count` | First unconsumed queue position, and how many inputs wait |
| `context_tokens` | Input tokens of the last completed conversation call in the active context, made with the configured model. Compaction compares this with `trigger_tokens`. `null` until such a call exists |
| `selection_pending` | A model change waits for the next step boundary |
| `last_operation` | The last `operation_finished` or `operation_failed` event |

### Session configuration

`SessionConfig` rejects unknown fields.

| Field | Required | Meaning |
| --- | --- | --- |
| `model` | yes | Model ID at the provider |
| `stream` | yes | Stream model output |
| `tools` | yes | Must be `[]` on create; the server installs its own tool specifications |
| `run` | yes | `{"tools": "Serial"}` or `{"tools": "Parallel"}` |
| `max_output_tokens` | no | Output cap per call |
| `reasoning` | no | `{"enabled", "effort", "summary": "Auto" \| "None"}` |
| `cache` | no | `{"key", "breakpoints"}` prompt-cache hints |
| `compaction` | no | `{"trigger_tokens", "target_tokens", "segment_tokens", "estimator"}`; requires `0 < target < trigger` and `segment > 0`. Absent disables compaction |

Every session gets `history_search`, `history_read`, `history_query` and
`view_image`. A session created with `shell: true` also gets `shell_start`,
`shell_edit`, `shell_poll`, `shell_write` and `shell_kill`. On updates `tools`
may list these names; any other name is rejected.

### `GET /api/sessions`

Query: `start`, `limit` (default 50), `query` (substring of name or ID),
`phase` (a phase name, or `running`), `tag` (an element of `metadata.tags`),
`order` (`desc` by `updated_at`, the default, or `asc`). Returns
`{"items": [{"session", "status"}], "next"}` from the session index without
loading any conversation.

### `POST /api/sessions`

```json
{
  "provider": "openai",
  "cwd": "/home/me/project",
  "shell": true,
  "name": "Refactor parser",
  "metadata": {"tags": ["work"]},
  "config": {"model": "gpt-5", "stream": true, "tools": [], "run": {"tools": "Serial"}}
}
```

`provider`, `cwd` (an existing absolute directory) and `config` are required.
`initial_messages` optionally seeds the context with messages. Returns `201`
with the session. `404` if the provider is unknown or disabled.

### `GET /api/sessions/{id}`

Loads the session if needed and returns it.

### `PATCH /api/sessions/{id}`

Any of `name`, `provider`, `config`, `metadata`. `provider` requires `config`.
Returns the updated session.

- Renaming works at any time.
- While the session runs, only `provider` and `config` may change, and within
  `config` only `model`, `reasoning` and `max_output_tokens`. The change is
  stored as `pending_selection` and applies before the next model request,
  without cancelling the current one. `status.selection_pending` is `true`
  until then.
- Otherwise every field may change. If the context holds an upstream
  compaction item that the new provider cannot read, the switch waits for the
  next run, which first asks the old provider for a plain-text handoff.

### `DELETE /api/sessions/{id}`

Deletes the session, its attachments and its shell output, and stops its
background commands. Returns `204`, or `409` while it runs. Usage statistics
are kept.

### `POST /api/sessions/{id}/fork`

Creates `"<name> (copy)"` with the same provider, directory, shell flag,
configuration and metadata, seeded with the current context (not the full
history). The shell override is not copied. Returns `201` with the new
session, or `409` while the source runs.

### `PUT /api/sessions/{id}/config`, `PUT /api/sessions/{id}/metadata`

Replace the whole `SessionConfig` or metadata value. Both return the session,
or `409` while it runs.

### `PUT /api/sessions/{id}/shell`

`{"program": "/usr/bin/zsh", "args": null}` gives the session its own shell,
validated like the global one (`args: null` picks arguments from the shell's
name, `{}` means the platform default). `null` returns it to the global
setting. Takes effect from the next command, even while the session runs.
Returns `400` for a session without shell tools.

## Input and execution

### `POST /api/sessions/{id}/input`

The way to talk to a session. Queues a user message and starts a run if none is
active; input sent during a run waits in the queue.

```json
{"text": "Compare these <image-3f2a...>", "attachments": [
  {"id": "3f2a...", "kind": "image", "name": "chart.png", "placeholder": "<image-3f2a...>"}],
 "metadata": {}}
```

Attachments are [uploaded](#attachments) first. `kind` is `image` (sent to the
model as an image) or `file` (passed to the shell by its server path). A
`placeholder` places the attachment at that point in the text; attachments
without one follow the text. Returns `202 {"entry": <entry id>}`.

### `POST /api/sessions/{id}/messages`

Queues one `System`, `Developer` or `User` message without starting a run and
returns `201 {"entry": <entry id>}`. `Developer.fixed` defaults to `false`;
only fixed instructions survive compaction and context clearing. Call `run` to
process the queue.

### `POST /api/sessions/{id}/run`

Starts or resumes execution and returns `202 {"accepted": true}`. Returns
`409` if the session is already running or holds an unfinished execution from
a crash. The run continues after the HTTP connection closes; follow it on the
[event stream](#session-events).

### `POST /api/sessions/{id}/interrupt`

Requests cancellation and returns `{"requested": true}` if a run was
registered. Wait for `operation_finished` (or `status.running = false`) before
changing the configuration. Queued input left after an interrupt is processed
once cancellation settles.

### `POST /api/sessions/{id}/compact`

Compacts the context now instead of waiting for `trigger_tokens`. Returns
`202 {"accepted": true}`, `400` if the session has no `compaction`
configuration, `409` while running. See [compaction](internals/compaction.md)
for what happens.

### `POST /api/sessions/{id}/context/clear`

Starts a new context generation holding only the fixed instructions: leading
`System` messages and `Developer` messages with `fixed: true`. History stays
searchable. Returns the session, or `409` while running.

### `PATCH`, `DELETE /api/sessions/{id}/queue/{entry}`

`PATCH` with `{"before": <entry id>}` moves a pending input in front of
another; `{"before": null}` moves it to the end. `DELETE` cancels it. Both
return `204`, work during a run, and return `409` if either entry was already
consumed or cancelled.

## History and records

History is the permanent record of everything that happened in a session:
every message and lifecycle event, in `sequence` order. It survives compaction
and context clearing. Active context is a separate, smaller projection.

### `GET /api/sessions/{id}/history`

The conversation timeline, messages only.

| Query | Default | Meaning |
| --- | --- | --- |
| `limit` | 40 | Page size |
| `order` | `desc` | `desc` (newest first) or `asc` |
| `before` | | Continue below this sequence |
| `after` | | Continue above this sequence |
| `include_outcomes` | `false` | Also return each run's `Finished` event |

Returns `{"items": [{"record", "content": {"kind", "value"}}], "next", "end_sequence"}`.
`record` holds `sequence`, `recorded_at`, `generation` and `model_call_id`.
`next` is `null` on the last page; otherwise pass its `after_sequence` as
`before` (for `desc`) or `after` (for `asc`).

### `POST /api/sessions/{id}/history/query`

Chronological, filtered history:

```json
{
  "filter": {
    "kind": "message",
    "message_types": ["user", "assistant"],
    "since": 1700000000000,
    "until": 1800000000000,
    "metadata": [{"operation": "equals", "path": "$.custom.project", "value": "demo"}]
  },
  "page": {"limit": 50, "order": "newest_first"}
}
```

| Filter | Values |
| --- | --- |
| `kind` | `message` or `event` |
| `message_types` | `system`, `developer`, `user`, `assistant`, `reasoning`, `tool_use`, `tool_result`, `upstream_compaction` |
| `event_types` | Event names such as `Finished`, `StateChanged`, `CompactionSummary` |
| `origins` | `Imported`, `Input`, `Model`, `Tool`, `Interrupted`, `Context`, `Summary` |
| `generation`, `model_call_id`, `tool_name` | Exact match |
| `since`, `until` | Milliseconds; `since` inclusive, `until` exclusive |
| `metadata` | `{"operation": "equals" \| "exists", "path": "$.a.b", "value"}` on message metadata, scalar values only |

Conditions combine with AND; values within a list with OR. `page.order` is
`oldest_first` (default) or `newest_first`. Pass the returned `next` back as
`page.cursor` with the same filter. Items carry `message_type`, `event_type`,
`tool_name` and `record`.

### `POST /api/sessions/{id}/history/search`

```json
{"query": {"text": "database migration", "mode": "terms", "limit": 20},
 "filter": {"message_types": ["user", "assistant"]}}
```

`terms` (default) requires all words; `substring` matches literal text,
including Chinese and paths. Returns the best `limit` matches with snippets and
scores, plus `has_more` (there is no cursor, as relevance changes when history
grows). Text, readable reasoning and tool arguments and results are indexed;
images, encrypted reasoning, upstream compaction payloads and metadata are not.

### `GET /api/sessions/{id}/history/{sequence}?before=0&after=0`

The full record at `sequence` and up to `before`/`after` neighbours, as an
array of `{record, content}`. Use it to expand a search hit.

### Raw records

These return engine records, paged with `start`/`limit`:

| Path | Items |
| --- | --- |
| `/api/sessions/{id}/entries` | Stored messages: `{id, origin, message, recorded_at, model_call_id}` |
| `/api/sessions/{id}/queue` | Queued entry IDs. Positions below `status.queue_head` were consumed |
| `/api/sessions/{id}/generations` | Context generations: `{id, status, entries, config, compaction_cursor}` |
| `/api/sessions/{id}/generations/{generation}/entries` | Entry IDs in one generation |
| `/api/sessions/{id}/calls` | One record per logical model call: purpose, model, times, usage, status, `last_request_input_tokens` |

## Attachments

- `POST /api/sessions/{id}/blobs` takes the raw file as the body and returns
  `{"id", "mime_type", "byte_count", "path"}`. `id` is the SHA-256 of the
  content. PNG, JPEG, GIF and WebP are recognised as images; everything else is
  `application/octet-stream`. `path` is where the file lives on the server.
- `GET /api/sessions/{id}/blobs/{blob}` downloads it, always as
  `application/octet-stream` with `X-Content-Type-Options: nosniff`.
- `GET /api/sessions/{id}/blobs/{blob}/meta` returns
  `{"id", "mime_type", "byte_count"}` without the path.

Images are sent to models that accept image input. For a model configured
without `image` in its input modalities, or one that rejects the image before
responding, Wish sends a text notice with the file path instead.

## Side questions

`POST /api/sessions/{id}/ask` answers a question from the session's committed
context without changing the session:

```json
{"text": "What did we decide about retries?", "stream": true,
 "history": [{"question": "...", "answer": "..."}]}
```

It works while the agent is running, sees the context up to the last completed
step, and has no tools. `history` carries earlier turns of the same side
conversation (at most 32 turns and 128,000 bytes); nothing is stored. The
response has the same form as [`/call`](#post-apiprovidersidcall).

## Usage statistics

Usage is counted per logical model call. Retries and output continuations
belong to the call that caused them; compaction summaries are counted as calls
of their own. Session variants live under `/api/sessions/{id}/...` and return
`404` for a session that does not exist.

### `GET /api/usage?from_ms=&to_ms=`

Totals over an optional time range (all recorded usage without one):

```json
{"unit": "logical_model_call", "statistics": {
  "model_attempts": 120, "attempts_with_usage": 118, "attempts_without_usage": 2,
  "totals": {"usage_records": 118, "committed_responses": 117,
             "tokens": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0, "reasoning_tokens": 0},
             "cache": {"read_input_tokens": 0, "write_input_tokens": 0, "request_hit_ratio": null}},
  "by_provider_model": [{"provider": "openai", "model": "gpt-5", "totals": {...}}]}}
```

### `GET /api/usage/series?window=7d&bucket=1h`

| Query | Meaning |
| --- | --- |
| `window` | Required: `1d`, `7d`, `30d`, `90d`, `365d` or `custom` |
| `bucket` | `1h`, `6h` or `1d` (default) |
| `from_ms` | Required for `custom` |
| `to_ms` | End of the range, default now. Ranges are limited to 366 days |

Returns `groups`, one per provider and model, each with time `buckets` (tokens,
attempts, average streaming speed) and streaming speed `samples`. Speed is
sampled once per second on every streamed request, including retries and
compaction, and estimated from output bytes at four bytes per token. At most
the latest 10,000 samples are returned (`sampling.truncated`); bucket totals
always cover the whole range.

### `GET /api/usage/daily?days=365&end_date=2026-09-25&tz_offset_minutes=480`

Daily totals for a calendar view. `days` (1 to 366), `end_date` and
`tz_offset_minutes` (from -840 to 840) are required; `bucket_ms` optionally
changes the bucket size. Returns
`days: [{date, input_tokens, output_tokens, total_tokens, attempts}]`.

## Event streams

Both streams send SSE records with event name `wish` and a JSON body tagged by
`type`. They are live notifications, not a durable log: there are no replay
IDs, and anything missed is recovered by reading state again.

### Application events

`GET /api/events` tells clients when to refresh lists and settings.

| `type` | Payload | Meaning |
| --- | --- | --- |
| `snapshot` | | First record. Read state now |
| `session_changed` | `id` | A session was created, updated, started or stopped, or received input through `/input` or `/messages` |
| `session_deleted` | `id` | A session was deleted |
| `configuration_changed` | | The configuration was saved or credentials were refreshed |
| `gap` | | The subscriber fell behind and missed events. Read state again |

### Session events

`GET /api/sessions/{id}/events` follows one session.

| `type` | Payload | Meaning |
| --- | --- | --- |
| `snapshot` | `data: {session, status}`, `live_events`, `revision` | Sent first, and again whenever the subscriber falls behind. `live_events` rebuilds the output of the current turn so far; replace any preview with it |
| `session_event` | `event`, `revision` | One engine event: state changes, streamed text, reasoning and tool-call deltas, tool starts, accepted responses, compaction progress |
| `operation_finished` | `outcome` | A run or compaction ended: `"Completed"`, `"Interrupted"`, `"ToolOutcomeUnknown"`, `{"Failed": error}`, `{"ModelStopped": {"stop_reason"}}` or `{"StreamFailed": {"reason"}}` |
| `operation_failed` | `error` | The operation could not complete because of a server-side error |
| `deleted` | | The session was deleted. The stream ends after this record |

Streamed deltas are not stored. After a reconnect, the `snapshot` restores the
current turn, and [history](#history-and-records) holds everything accepted.
Subscribe before calling `run` if the first deltas matter. Disconnecting never
cancels a run.

### Lifecycle

- A run continues without any connected client. `interrupt` is the only way to
  stop it.
- On `SIGINT` or `SIGTERM`, Wish stops accepting work, cancels runs, keeps the
  partial output they had accepted, stops background shell commands, flushes
  storage and exits. Streams end without a final event.
- After a restart, sessions load on first use and never resume by themselves.
  If the process was killed during a tool call, the session reports the
  unfinished state and does not repeat the call; `interrupt` settles it.
