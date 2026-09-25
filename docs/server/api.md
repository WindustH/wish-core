# HTTP API

All application routes start with `/api`. JSON is the core's Serde representation;
messages keep variants such as `User`, `Assistant`, `ToolUse` and `Reasoning`.
The core's optional fields remain optional. No old wish HTTP format adapters or storage
migrations are provided.

| Method | Path | Purpose |
|---|---|---|
| GET | `/health`, `/version` | Public liveness and build identity |
| GET | `/api/providers` | Configured provider descriptions, without credentials/headers |
| GET | `/api/providers/{id}` | One provider and its explicitly enabled capabilities |
| GET | `/api/providers/{id}/models?cursor=...&limit=100` | Upstream catalog page, cached five minutes |
| GET | `/api/providers/{id}/account` | Configured upstream account observation |
| POST, GET | `/api/providers/{id}/chatgpt-login` | Start Codex preset browser login; poll its status |
| POST | `/api/providers/{id}/chatgpt-login/complete` | Complete a remote browser login with its redirect URL |
| POST | `/api/providers/{id}/call` | Core `Request`; `stream` selects JSON or SSE |
| POST | `/api/providers/{id}/count-tokens` | Core `Request`; provider token count |
| POST | `/api/providers/{id}/compact` | `{model, conversation}`; upstream compaction result |
| GET | `/api/sessions?start=0&limit=50` | Persisted session descriptors |
| POST | `/api/sessions` | `{provider, cwd, config, shell?, metadata?}`; creates a session |
| GET | `/api/sessions/{id}` | Descriptor and current status |
| PUT | `/api/sessions/{id}/config` | Full core `SessionConfig`, at an inactive boundary |
| PUT | `/api/sessions/{id}/metadata` | Any JSON value, while inactive |
| POST | `/api/sessions/{id}/messages` | One core `Message`; returns its queued entry ID |
| POST | `/api/sessions/{id}/run` | Start/resume independent agent task; `202` |
| POST | `/api/sessions/{id}/interrupt` | `{requested: bool}` acknowledges cancellation; pending inputs resume after cancellation settles |
| POST | `/api/sessions/{id}/compact` | Start core compaction; requires configured budgets; `202` |
| GET | `/api/sessions/{id}/events` | Live SSE subscription |
| GET | `/api/sessions/{id}/entries` | Paged original message entries |
| GET | `/api/sessions/{id}/queue` | Paged queued entry IDs, including consumed positions |
| GET | `/api/sessions/{id}/generations` | Paged generation descriptors |
| GET | `/api/sessions/{id}/generations/{generation}/entries` | Paged context entry IDs |
| GET | `/api/sessions/{id}/calls` | Paged model-call usage, timestamps and outcomes |
| POST | `/api/sessions/{id}/history/query` | `{filter?, page?}` chronological indexed query |
| POST | `/api/sessions/{id}/history/search` | `{query, filter?}` ranked text search |
| GET | `/api/sessions/{id}/history/{sequence}?before=0&after=0` | Expand original payload and neighbors |

List APIs use `start` and positive `limit`, defaulting to 0 and 50, and return
`{start, items, next}`. Generation entries reference `/entries` positions. History queries
use their own stable cursor, not the list offset. Queries and call/entry pages remain
available during execution, without loading the full history. Queued messages become
searchable conversation history when consumed. The queue's consumed boundary is
`status.queue_head`, refreshed after each operation.

## History

```json
{
  "filter": {
    "kind": "message",
    "message_types": ["user", "assistant"],
    "since": 1700000000000,
    "until": 1800000000000,
    "metadata": [{"operation":"equals","path":"$.project","value":"demo"}]
  },
  "page": {"limit":50,"order":"newest_first"}
}
```

Pass returned `next` as `page.cursor` with the same filter and order. `since` is inclusive;
`until` is exclusive. Filters also accept `event_types`, `origins`, `generation`,
`model_call_id`, and `tool_name`. Enum spelling follows
[core history](../en/history.md).

Search example:

```json
{"query":{"text":"previous decision","mode":"terms","limit":20},
 "filter":{"message_types":["user","assistant"]}}
```

`substring` supports literal substring matching. Results contain references and snippets;
expand selected sequences to retrieve full messages. Compressed-away original content
remains in history. Opaque ciphertext and streamed payload copies are not text-indexed.

## Streams and lifecycle

Direct provider calls use the same request endpoint for both modes. `stream:false` returns
a core `Response`; `stream:true` sends `model_event` SSE records containing core `StreamEvent`,
then `done`. A failure before stream setup uses an HTTP error; a later failure sends `error`
and closes. These calls are stateless; disconnecting drops the upstream stream.
Session calls use the core executor, including output continuation and compaction.

Session SSE records have event name `wish` and JSON data tagged by `type`:

- `snapshot`: descriptor/status plus `live_events`, a coalesced current-turn preview. Sent on
  initial subscription and when a slow subscriber falls behind. Replace the previous live preview
  before applying these events; subsequent notifications follow the snapshot without overlap.
- `session_event`: core event, including model chunks and tool transitions.
- `operation_finished`: a core `RunOutcome`, which may represent failure/interruption.
- `operation_failed`: an infrastructure/session error that prevented normal completion.
- Older clients may receive `gap`; current servers resubscribe with a fresh snapshot instead.

Live notifications have no replay IDs and are not a durable cursor. Disconnecting does not
cancel a session. Subscribe before starting an operation if immediate deltas matter.
Model chunks are not persisted. Reconnection restores the current preview from memory; process
termination loses unfinished output. Final messages, graceful interruption fragments and lifecycle
events remain durable. Use history `sequence` to reconcile finalized messages after a snapshot.
Existing stored chunks remain readable; this change does not delete or renumber old history.

`status.phase` updates during execution; `status.state` is supplied when inactive.
Config, metadata and queue boundary are operation-boundary snapshots. `last_operation`
is a live completion report; durable outcomes are available from history and call records.
A loaded session with interrupted/unfinished state does not silently resume side effects.

## Providers and configuration

`protocol`, `base_url`, and `path` are required. Paths are explicit, including API prefixes;
`{model}` is substituted by core for model-addressed endpoints. Supported model-use IDs:

- `openai_chat`; `deepseek_chat`, `zai_chat`, `kimi_k2_chat`, `kimi_k3_chat`, `qwen_chat`,
  `minimax_chat`, `mimo_chat`, `tokenhub_chat`, `mistral_chat`.
- `openai_responses`, `plaintext_responses`, `codex_responses`.
- `anthropic_messages`; `deepseek_messages`, `zai_messages`, `kimi_messages`, `qwen_messages`,
  `minimax_messages`, `mimo_messages`, `tokenhub_messages`.
- `google_generate_content`, `google_vertex_generate_content`, `google_interactions`,
  `bedrock_converse`, `mistral_conversations`.

`auth` is `bearer` (default), `none`, `anthropic_key`, `google_key` or `sig_v4`.
`api_key_env` names a nonempty environment variable. `credentials_env` maps `region`,
`access_key_id`, `secret_access_key`, `session_token`, `account_id`, `workspace_id`, `team_id`,
`organization`, or `project` to environment variable names. Optional `headers` are static
outbound headers, omitted from HTTP descriptions. OAuth/ADC token acquisition and rotation
are not managed by this server; a configured bearer token is read at startup.

`token_count` explicitly selects `openai_responses`, `anthropic_messages` or
`google_generate_content`; core validates compatibility. `compaction` explicitly selects
`openai_responses` or `openai_responses_streamed`. `model_list` and `account_state` use core
protocol IDs; catalogs additionally require `model_list_path`. Account reads use the
provider's `base_url`. Provider configuration is editable through `/api/config`. `model_list_base_url` can select a separate catalog host.

Catalog pages are cached per provider for five minutes, keyed by host, path, cursor, page size
and authentication mode; one refetch runs at a time and later readers of the same page wait for
it. An expired page is refetched on the next read, and a failed refetch serves the expired page
with a warning naming its age instead of an error. Saving configuration rebuilds providers and
therefore empties their caches.

Session `config.compaction` contains `trigger_tokens`, `target_tokens` and `segment_tokens`;
its `estimator` is optional. Core selects upstream compaction when enabled and local standby
summaries otherwise. `calls` exposes usage once per logical model call, rather than copying
usage onto every response message.

Application errors return `{error:{message,details}}`. Core model errors retain their
structured representation in `details`. Invalid requests return 400, unknown IDs 404,
active-session conflicts 409, unconfigured upstream capabilities 501, and upstream failures
502. JSON/query extractor failures use Axum's standard rejection response.

## Web application management

- `GET /api/config` → `{revision,config}`. `PUT /api/config` accepts the same envelope;
  stale revisions return 409. Providers and defaults apply to subsequent operations.
  Header values, direct `api_key`, and `credentials` values are redacted; sending `<redacted>` retains an existing value. Credentials can be entered directly or reference server environment variables (`api_key_env` / `credentials_env`, which take precedence). Listen/data/auth settings require a file edit and restart.
  A provider header value may contain `{session}`. Session calls, token counts, BTW calls and upstream compaction expand it to the stable session ID; standalone provider calls use a fresh ID per request. OpenCode Go, OpenAI, Codex and Anthropic preset headers are supplied even for saved configs made before those defaults existed. Explicit provider headers override preset defaults. OpenAI Responses and Codex model calls send the same ID in `client_metadata.session_id` and `client_metadata.thread_id`, with `prompt_cache_key` defaulting to that ID; Codex compaction does likewise. Anthropic Messages model calls send it in the JSON string `metadata.user_id` under `session_id`. These body fields are added only for the official provider presets.
- `POST /api/providers/{id}/chatgpt-login` starts a ten-minute PKCE authorization for an
  `openai_codex` preset and returns `{authorization_url,expires_in}`. Open that URL in the
  browser on the Wish server's machine so the ChatGPT redirect can reach its local callback
  at `localhost:1455` (or `1457` if occupied). `GET` on the same path returns
  `{status,error}` where status is `idle`, `pending`, `complete`, `failed`, or `expired`.
  On success Wish saves the access token, rotating refresh token, account ID and expiry;
  it refreshes expiring credentials in the background. `GET /api/config` redacts both
  `api_key` and `refresh_token`, and a subsequent config save preserves redacted values.
  When accessing Wish remotely, the browser's `localhost` redirect may fail. Copy the
  complete URL from its address bar and POST `{callback_url}` to
  `/api/providers/{id}/chatgpt-login/complete`; Wish validates the redirect and state,
  then exchanges the authorization code using the PKCE verifier kept on the server.
  Starting a new login cancels the previous attempt.
- `GET /api/proxy-environment` returns the proxy environment variables visible to the Wish
  process as `{variables:[{name,value,redacted}]}`. Proxy URL output shows only the address and hides
  credentials, paths, query strings and fragments. `NO_PROXY` and `no_proxy` values are shown
  as configured. This is a read-only snapshot for the settings screen.
- `GET /api/shells` returns `{default:{name,program,args},installed:[{name,program,args}]}`:
  the shell used when none is configured and the shells found on the server's `PATH`, each
  with the arguments it would run with. This is a read-only snapshot for the settings screen.
- `GET /api/defaults` → `{defaults,session_config}` for new-session forms.
- `GET /api/directories?path=/home/windy` lists the immediate subdirectories of an absolute
  path on the Wish server. The response is
  `{path:"/home/windy",parent:"/home",directories:["repo",...]}`: `path` is canonical,
  `parent` is `null` at the filesystem root, and `directories` contains sorted names only
  (including hidden directories). Use a directory name with `path` to browse into it.
  `path=~` opens the server user's home directory from `HOME`, returning its canonical
  absolute path. It returns 400 if `HOME` is unavailable or cannot be read.
  A relative, missing, unreadable, or non-directory path returns 400. This read-only route
  uses the same optional bearer-token authentication as other `/api` routes.
- `GET /api/sessions?start=0&limit=50&query=&tag=&phase=&order=desc` returns
  `{items:[{session,status}],next}` from the session index, without loading histories.
- `PATCH /api/sessions/{id}` accepts name, provider, config and/or arbitrary JSON metadata.
  Optional `If-Match` checks the descriptor revision. Changes require a stable session.
- `DELETE /api/sessions/{id}` removes its core namespace, attachments and shell output (204).
  Interrupt an active session and wait for completion first. Aggregate usage observations remain.
- `POST /api/sessions/{id}/fork` copies current model context into an independent session (201).
- `POST /api/sessions/{id}/context/clear` starts a generation with only the leading fixed
  prefix: System messages and Developer messages explicitly marked `fixed: true` until the first
  unpinned message. New Developer input defaults to `fixed: false`; old stored messages without
  this field keep their previous pinned behavior. Historical messages and events are preserved.
- `PATCH /api/sessions/{id}/queue/{entry}` atomically moves a pending input before the entry in `{ "before": entry_id }`, or to the end with `{ "before": null }` (204). A consumed/cancelled source or target returns 409 without changing the queue. Message content and IDs are preserved.
- `DELETE /api/sessions/{id}/queue/{entry}` cancels a pending input at a stable boundary (204).
- `POST /api/sessions/{id}/input` accepts `{text,attachments:[{id,kind,name}],metadata}`;
  durably queues the input and schedules execution (202). Inputs sent during a run are queued.
  Interruption also invalidates previously scheduled continuations. `messages` remains the explicit
  enqueue-only protocol API; `run` explicitly starts/resumes execution.
- `POST /api/sessions/{id}/blobs` accepts raw bytes, returning `{id,mime_type,byte_count,path}`.
  `GET /api/sessions/{id}/blobs/{id}` downloads a session-scoped attachment. Images become native
  image blocks; files are exposed by local path to shell. Request bodies are limited to 32 MiB.
  `GET /api/sessions/{id}/blobs/{id}/meta` returns `{id,mime_type,byte_count}` without the
  server filesystem path; it requires a live session and an existing attachment.
- `GET /api/sessions/{id}/history?limit=40&order=desc&before=SEQUENCE` returns expanded
  message records `{items,next,end_sequence}`. `after` supports forward reconciliation.
  Indexed `history/query` and `history/search` retain message-type/time/metadata filters.
  Search returns a bounded ranked result set and `has_more`, not a pagination cursor.
- `GET /api/events` is live application SSE (`wish` events): snapshot, session_changed,
  session_deleted, configuration_changed, gap, shutdown. Re-fetch snapshots after reconnect/gap.
- `GET /api/usage`, `/usage/series?window=7d&bucket=1h`, `/usage/daily?days=365&end_date=2026-09-21&tz_offset_minutes=480` aggregate
  persisted **logical model calls**. Session-scoped counterparts live under `/sessions/{id}`.
  Streaming TPS is sampled once per second for each physical request, including retries,
  continuations, stateless asks and streamed compaction. Text, plaintext reasoning and
  tool-argument deltas are estimated at four UTF-8 bytes per token; signatures, opaque
  reasoning and display-only summaries do not count. Waiting/stalls produce zero points.
  Completion, failure or interruption flushes the final partial interval. Samples include
  attempt_id, at_ms, duration_ms, output_bytes, output_tokens, tps and source.
  Provider-reported billing usage remains separate. Samples persist in SQLite and pending
  sampler tasks are drained during graceful shutdown.
  Series returns the latest 10,000 scatter points, with sampling.truncated indicating
  omitted older points. Bucket/window aggregates include the full selected range; stored
  samples are not deleted by this display limit.
- `GET /api/status` reports stored-session count and loaded execution state; `/api/storage`
  reports on-disk bytes. Neither endpoint loads conversation histories.

Storage consists of core `wish.sqlite` and a separate `management.sqlite` session/call index,
plus session-scoped `blobs/` and `shell/`. Start with a fresh data directory: no legacy-format
migration or fallback is provided. The service and local WebUI share one trusted-user boundary.


## Presets, image input and background tools

`GET /api/provider-presets` returns 48 presets with branding, regional/billing variants,
credential requirements and protocol-specific connection settings. Provider `preset` selects
that metadata; `models` stores per-model overrides, including `input_modalities`, context and
output limits, and reasoning options. `enabled:false` disables calls without deleting settings.
Global `proxy` configuration applies to providers with `proxy_enabled:true`. Its `mode` is
`environment` (the default, preserving the server process's proxy environment variables),
`manual`, or `direct`. Manual mode requires an HTTP or HTTPS `url` and may include separate
`username` and `password` fields for basic authentication. The password is redacted by
`GET /api/config`; sending the redacted value back to `PUT /api/config` retains it.
`proxy_enabled:false` always connects that provider directly. Changes saved through
`PUT /api/config` apply to new model requests without restarting the server.

Global `shell` configuration selects the program shell commands run under. `program` is an
absolute path to an executable; empty uses `/bin/sh -c` on Unix and `%COMSPEC% /D /S /C` on
Windows. `args` are placed before the command text, which is passed as one final argument.
Omitted or `null`, they follow the shell's name: `-lc` for zsh and bash, `-l -c` for fish,
`-NoLogo -NoProfile -NonInteractive -Command` for PowerShell, `/D /S /C` for cmd, and `-c`
otherwise. A save that names a missing or non-executable program is rejected. A saved shell
applies to the next command in every session, including sessions already open; running
commands keep the shell they started with.

Every session has `history_search`, `history_read`, `history_query`, and `view_image`; `shell_start`, `shell_edit`, `shell_poll`, `shell_write`, and `shell_kill` remain opt-in. `view_image`
accepts an absolute local path to a PNG/JPEG/GIF/WebP, at most 20 MiB. The server snapshots
its bytes under the session's blob directory. The native image is placed after the complete
tool-result batch; the original image remains in stored context.

Session requests, token counting and compaction project stored images to native image input
plus a file notice. A model explicitly lacking `image` in its configured input modalities
receives only the notice. A model call explicitly rejected for image capability before the
client returns any event is retried once using file notices; unrelated errors and failures
after streaming begins are not retried this way. Switching models does not erase images.

Background shell completion produces one queued `Developer` message with metadata
`source:background_execution_finished` and structured `completion: {command, result}`. The UI shows this as a background Shell notice, separate from human input; its queue only lists user messages without an internal source marker. It can wake an idle session. An explicit session
interrupt suppresses that task's automatic wake, while retaining its notification for later.
Notifications are queued durably once received, but process supervision and undelivered
notifications are not recovered after a process crash. Shutdown stops notification waiters
and then terminates owned shell processes. Large output results advise bounded reads/search.

New sessions receive a fixed agent System message before custom instructions. Existing
sessions also receive these instructions at request time without rewriting their history.

## Stateless question

`POST /api/sessions/{id}/ask` accepts `{text,stream?,history?}` (stream defaults to false).
`history` is an optional array of `{question,answer}` BTW turns, sent by the client with each
follow-up. The server inserts these temporary user/assistant turns after the committed session
snapshot and before the new question; it does not store them. At most 32 nonempty turns and
128,000 UTF-8 bytes of history are accepted.
It snapshots committed active-generation context even while the agent is running, excluding
an unfinished tool batch and its assistant turn. It sends the final question with an explicit
one-shot instruction and no available tools. Nothing is enqueued and session history, state,
and model-call records are unchanged. This endpoint is request-scoped: it does not create
persisted stateless jobs or support replay/resume of disconnected responses.

Buffered replies use the core Response shape. Streamed replies use `model_event`, `done`,
and `error` SSE events, as direct provider calls do. Closing the response stops local streaming;
shutdown also cancels the call. The UI exposes this in a temporary BTW chat bubble. A single
response can still reach its configured output limit; it does not start an
agent loop or automatically execute/continue tool calls.

Provider configurations accept an optional display_name for presentation. Provider IDs remain stable session references. The domestic Zhipu Coding Plan Responses preset uses https://open.bigmodel.cn/api/v1/responses, as documented at https://docs.bigmodel.cn/cn/coding-plan/tool/codex; Chat retains its separate subscription endpoint.

Interrupt cancels the current execution and invalidates earlier pending schedulers. A fresh
scheduler waits for cancellation to finish, then consumes remaining queued inputs immediately.
With an empty queue the session stays stopped; this does not repeat previously consumed inputs.

While a session is executing, `PATCH /api/sessions/{id}` accepts provider plus config changes
limited to model, reasoning and max_output_tokens. Other config fields must remain unchanged.
The accepted selection is durable and returned immediately; `status.selection_pending=true`
means the current request still uses the previous selection. It takes effect at the next
executor boundary, before constructing the next model request, without cancelling the active
request. Repeated edits replace the pending selection; If-Match still guards revisions.
An unfinished standby summary using the previous selection is discarded at that boundary.
If the current generation contains an encrypted upstream compaction item and the selected provider
cannot replay it, Wish first asks the old provider to write a handoff. It replaces the item with
an unpinned Developer message in a new generation and keeps all later entries in order. If the
old provider is unavailable or translation fails, Wish records `CompactionTranslationFailed`,
inserts an explicit missing-context message, and continues with the selected provider. Cancelling
the run during handoff retains the old generation and pending selection.
