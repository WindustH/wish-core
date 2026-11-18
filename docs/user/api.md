# HTTP API Reference

Everything [Wish Web](https://github.com/WindustH/wish-web) does runs over Wish's HTTP JSON API and Server-Sent Events (SSE) streams under `/api`. Only `/health`, `/version`, and static web assets sit outside `/api`.

## Basics

- **Authentication**: When `bearer_token_env` is configured, `/api/*` requires `Authorization: Bearer <token>` (`401` otherwise). The exception is the per-session shell bridge (`commands`, `task`, `mcp`, `skills`, `peers`), which accepts only the session's ephemeral `$WISH_SESSION_TOKEN`. Because browser `EventSource` cannot send headers, read SSE using `fetch` (or inject the header in a proxy).
- **CORS & Host checks**: Token-protected servers send `Access-Control-Allow-Origin: *` for cross-origin clients. Servers without a token omit CORS, restrict `Host` to loopback / `listen` / `allowed_hosts` (`421`), and block cross-origin state mutations (`403`).
- **Payload limits**: JSON bodies reject unknown fields on validated payloads and cap at **32 MiB** (`413`). Raw blob uploads (`/api/sessions/{id}/blobs` and `/api/session-drafts/{id}/blobs`) stream to disk up to **1 GiB**.
- **Engine JSON & timestamps**: Messages use externally tagged enums (`{"User": {"content": [{"Text": {"text": "Hello"}}]}}`). Timestamps are Unix milliseconds; unreported token counts are `null`.
- **Pagination & concurrency**: Offset endpoints use `?start=0&limit=50` (`{"start", "items", "next"}`); history uses opaque cursors. Config saves check `revision`; `PATCH /api/sessions/{id}` checks `If-Match: <revision>` (`409` on conflict).
- **Errors**: Failures return `{"error": {"message": "...", "details": null}}` (`details` carries structured upstream info on `502`, missing capability returns `501`, conflicts return `409`).

## Endpoint index

### Service & Storage
| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/health` | Public liveness check |
| `GET` | `/version`, `/api/version` | Build name and version (`/api/version` requires auth) |
| `GET` | `/api/status` | Session count, active scheduler queue, uptime |
| `GET` | `/api/events` | Global event stream (SSE) |
| `GET` | `/api/storage`, `/api/storage/detail`, `/api/storage/sessions` | Total, per-category, and per-session disk usage |
| `POST` | `/api/storage/prune`, `/api/storage/prune-usage` | Prune out-of-context history or leftover deleted-session usage |

### Configuration & Discovery
| Method | Path | Purpose |
| --- | --- | --- |
| `GET`, `PUT` | `/api/config` | Read or atomically save `config.json` (secrets masked as `<redacted>`) |
| `GET` | `/api/defaults` | Current `defaults` and initial `SessionConfig` |
| `GET` | `/api/provider-presets`, `/api/search-presets`, `/api/fetch-presets`, `/api/image-presets`, `/api/media-presets` | Built-in presets |
| `GET` | `/api/shells`, `/api/proxy-environment`, `/api/directories` | Host shells, redacted proxy env vars, directory browser (`path=~`, `include_files=true`) |
| `GET`, `POST` | `/api/search/providers`, `/api/search/providers/{id}/check` | Search readiness and live check |
| `GET`, `POST` | `/api/fetch/providers`, `/api/fetch/providers/{id}/check` | Fetch readiness and live check |
| `GET`, `POST` | `/api/mcp/servers`, `/api/mcp/servers/{id}/check` | MCP tool cache and live check |
| `GET` | `/api/skills`, `/api/skills/{name}` | Discovered skills and instructions |

### Providers & Logins
| Method | Path | Purpose |
| --- | --- | --- |
| `GET` | `/api/providers`, `/api/providers/{id}` | Configured providers (secrets omitted) |
| `GET` | `/api/providers/{id}/models` | Upstream model catalog page (5-min cache) |
| `POST` | `/api/provider-draft/models` | Test and list models for unsaved provider/account drafts |
| `GET` | `/api/providers/{id}/account` | Upstream balance, quota windows, or credit state |
| `POST`, `GET` | `/api/providers/{id}/chatgpt-login`, `.../complete` | ChatGPT PKCE sign-in (`{id}` or `account:<id>`) |
| `POST`, `GET` | `/api/providers/{id}/google-login`, `.../complete` | Google Antigravity PKCE sign-in (`{id}` or `account:<id>`) |
| `POST`, `GET` | `/api/providers/{id}/copilot-login` | GitHub Copilot device-code flow |
| `POST` | `/api/providers/{id}/call`, `/count-tokens`, `/compact` | Stateless model call, token count, and upstream compaction |

### Sessions, Folders & Execution
| Method | Path | Purpose |
| --- | --- | --- |
| `GET`, `POST` | `/api/sessions` | List or create sessions (`first_input` supports atomic creation + run) |
| `GET`, `PATCH`, `DELETE` | `/api/sessions/{id}` | Read, update (`If-Match`), or delete a session |
| `GET` | `/api/conversations` | Folder-scoped session/folder listing (or flat search) |
| `POST` | `/api/conversations/move`, `/api/conversations/pin` | Move or pin sessions and folders |
| `GET`, `POST`, `PATCH`, `DELETE` | `/api/folders`, `/api/folders/{id}` | Manage session folders |
| `POST` | `/api/sessions/{id}/fork`, `/generate-name` | Fork active context; suggest title (`first_message` or `context`) |
| `GET` | `/api/sessions/{id}/environment` | Public `$WISH_*` aliases (`WISH_BLOB_ROOT`, `WISH_TMPDIR`, `WISH_BLOB_ID_*`, `WISH_SESSION_ID_*`) |
| `PUT` | `/api/sessions/{id}/config`, `/metadata`, `/cwd`, `/shell`, `/tools` | Update session config, metadata, cwd, shell, or capability delivery |
| `POST` | `/api/sessions/{id}/input`, `/messages`, `/run`, `/interrupt`, `/compact` | Enqueue input (`background_shell: true` supported), run, stop, or compact |
| `POST`, `GET` | `/api/sessions/{id}/context/clear`, `/context/inspect` | Reset context keeping fixed instructions; inspect projected request |
| `PATCH`, `DELETE` | `/api/sessions/{id}/queue/{entry}` | Reorder or cancel pending queue item |
| `POST` | `/api/sessions/{id}/answer`, `/ask` | Reply/skip `ask_user` card; ask stateless BTW side question |
| `GET` | `/api/sessions/{id}/events` | Session event stream (SSE) |
| `GET`, `POST` | `/api/sessions/{id}/tasks`, `/tasks/background-shell` | List/control shell tasks or release foreground shell tasks |
| `POST` | `/api/sessions/{id}/media/{kind}/{job}/status` | Poll or refresh a durable image/video/audio job |

### History, Blobs, Usage & Bridge
| Method | Path | Purpose |
| --- | --- | --- |
| `GET`, `POST` | `/api/sessions/{id}/history`, `/history/query`, `/history/replies`, `/history/search`, `/history/{sequence}` | Timeline, filtered queries, reply backlinks, FTS5/substring search |
| `GET` | `/api/sessions/{id}/entries`, `/queue`, `/generations`, `/generations/{g}/entries`, `/generations/{g}/structure`, `/calls` | Raw engine records and generation structure |
| `POST`, `GET`, `DELETE`, `PATCH` | `/api/session-drafts/{id}/blobs`, `/api/sessions/{id}/blobs{,/link,/{b},/{b}/meta,/{b}/library}` | Stream upload, link local paths, download (range-capable), pin/hide, delete |
| `GET`, `POST` | `/api/sessions/{id}/blobs/{b}/image`, `/pdf`, `/pdf/page`, `/playback` | WebP previews (`width=384|768|1536`), PDF info/native page extraction, signed playback URL |
| `POST`, `PATCH`, `DELETE` | `/api/sessions/{id}/library/{folders,folders/{f},move,cleanup}` | Library folders, atomic moves, and safe cleanup |
| `GET` | `/api/usage`, `/usage/series`, `/usage/daily` (and `/sessions/{id}/usage*`) | Usage totals, speed samples, and daily heatmap buckets |
| `GET`, `POST`, `PATCH`, `DELETE` | `/api/sessions/{id}/{commands/{name},task,mcp/*,skills*,peers*}` | Shell bridge endpoints (authenticated via `$WISH_SESSION_TOKEN`) |

## Key workflows & payloads

### Creating sessions and sending messages

`POST /api/sessions` requires `provider`, `cwd` (an existing absolute path on the host), and `config` (`tools: []` on create; Wish populates configured tools from switches):

```json
{
  "provider": "openai",
  "cwd": "/home/me/project",
  "tools": { "shell": true },
  "name": "Refactor parser",
  "folder": null,
  "config": { "model": "gpt-5.4", "stream": true, "tools": [], "run": { "tools": "Parallel" } }
}
```

To create a session and send an initial prompt atomically—especially with large attachments—generate a client UUID `request_id`, upload files via `POST /api/session-drafts/{request_id}/blobs`, and pass `first_input` inside `POST /api/sessions`:

```json
"first_input": {
  "request_id": "b15605df-64a4-4e3e-aef4-c6b6a7e23b26",
  "text": "Review this screenshot",
  "attachments": [{ "id": "<blob_id>", "kind": "image", "name": "screen.png" }]
}
```

Retrying with the same `request_id` is idempotent (`409` if reused with different content).

For subsequent turns, `POST /api/sessions/{id}/input` queues input and starts execution when idle (`202 {"entry", "first_user_message"}`). Include `"background_shell": true` to move any blocking foreground shell commands to the background so the model handles your new message right away.

While a session runs, `PATCH /api/sessions/{id}` (with `If-Match: <revision>`) still accepts renames or model/reasoning/output-cap changes, queuing model updates as `pending_selection` for the next model turn.

### Answering `ask_user` and side questions (`BTW`)

`POST /api/sessions/{id}/answer` accepts `{"call_id", "answers": [{"selected": ["Option A"]}, {"text": "value"}]}` (or `{"skipped": true}` for individual items / `"skip": true` for the whole card) and returns `{"delivered": "now" | "later" | "dropped"}` (`"later"` queues a `Developer` message and resumes the session if the card had already timed out).

`POST /api/sessions/{id}/ask` takes `{"text", "stream": true, "history": [{"question": "...", "answer": "..."}]}` and answers against the session's last committed context without altering history or running tools.

### History, search, and tasks

- `GET /api/sessions/{id}/history?limit=40&order=desc&before=&after=&include_outcomes=false` returns timeline items and a `next` cursor (`after_sequence`).
- `POST /api/sessions/{id}/history/query` filters chronologically by `kind` (`message` | `event`), `message_types`, `event_types`, `origins`, `generation`, `model_call_id`, `tool_name`, `since`/`until`, and JSON path `metadata` predicates (`{"operation": "equals"|"exists", "path": "$.key", "value"}`).
- `POST /api/sessions/{id}/history/search` takes `{"query": {"text": "...", "mode": "terms"|"substring", "limit": 20}, "filter": {...}}`.
- `POST /api/sessions/{id}/tasks` runs task management actions (`{"action": "list"}`, `show`, `background`, `remove`, or `execute` with `start`/`poll`/`write`/`kill`).

### Interactive session terminal

`POST /api/sessions/{id}/terminal` is an authenticated **user** endpoint, separate from the model task tool. Request bodies are tagged by `action`:

| Action | Additional fields | Result |
| --- | --- | --- |
| `state` | none | `{terminal: null}` or the current terminal snapshot; never starts a shell |
| `start` | `cols`, `rows` | `{terminal: ...}`; reuses the running terminal, otherwise creates a new one |
| `poll` | `id`, `offset` | `{terminal, data, next_offset, reset, eof}`; waits up to 15 seconds for changes |
| `write` | `id`, `data` | `{ok: true}`; `data` is base64-encoded raw input, at most 32 KiB |
| `resize` | `id`, `cols`, `rows` | `{ok: true}`; updates the PTY window size |
| `close` | `id` | `{terminal: ...}` after the shell exits |

Snapshots contain `id`, `running`, nullable `exit_code` / `error`, `cols`, `rows`, and the **initial** `cwd`. Dimensions must be 2–500 columns and 2–300 rows. Output `data` is base64 raw PTY bytes (including ANSI escapes); `next_offset` is a byte cursor. Each response carries at most 64 KiB, with a 1 MiB ring retained per terminal. `reset: true` means the requested cursor is no longer retained: reset the terminal display before applying this chunk. Continue reading until `eof`, the shell has exited, and the returned chunk is empty. Never automatically retry uncertain input writes.

There is one terminal per session and at most 16 live terminals per server. Opening another browser shares that session's terminal, including input and dimensions. Hiding the sidebar or disconnecting a browser leaves the process running; explicit close, session deletion, or server shutdown stops it. Output is transient, not written into chat history or persisted across server restarts. The shell toggle gates start and input, while existing output and close remain accessible when disabled. This endpoint uses the same bearer and origin protections as other user APIs; it is not a model/CLI bridge action.

### Library links, bookmarks, previews, and cleanup

- `POST /api/sessions/{id}/library/link` accepts `{"path": "/absolute/file-or-folder", "parent": null}` and detects the path type. It returns `{"kind":"file","blob":...}` or `{"kind":"folder","folder":...}`. Files become visible in the selected virtual Library folder; folder links let you browse their live children without importing the tree. Repeated links preserve display names and reuse the same folder link within the same parent. Clients can submit mixed batches one path at a time and retry only failures. The file-only `/blobs/link` and folder-only `/library/folders/link` endpoints remain available.

- `POST /api/sessions/{id}/blobs` writes content-addressed blobs by SHA-256 (`?name=...&temporary=false`); `POST /api/sessions/{id}/blobs/link` registers `{"path": "/abs/path", "temporary": false}` without copying bytes.
- `PATCH /api/sessions/{id}/blobs/{blob}/library` accepts `display_name` alongside `visible` and `pinned`. Display names do not change blob IDs, source paths, original filenames or contents; repeated uploads/links preserve them.
- `POST /api/sessions/{id}/library/folders/link` accepts `{"path": "/absolute/directory", "parent": null, "name": "Optional display name"}`. `GET .../folders/{folder}/children?path=relative/subdir&limit=80&after=last-name` lists one level with a lexical cursor (maximum 200 items). Paths must remain within the linked directory; external symlinks and special files are excluded. Browsing does not import the tree.
- `POST .../folders/{folder}/open` with `{"path": "relative/file"}` registers a hidden file link for preview. `PATCH .../folders/{folder}/children` with `{"path": "relative/item", "name": "Display name"}` renames its Library label. Folder links are read-only: uploads/moves into them are rejected, and deleting a link leaves the original tree intact.
- `GET`/`POST /api/sessions/{id}/library/bookmarks` lists/adds message bookmarks. Send `{"seq": 42}` for the whole message or `{"seq": 42, "block": 0, "start": 6, "end": 10}` for a text excerpt. Ranges are UTF-16 offsets, start-inclusive/end-exclusive; the server validates and saves the original text. Duplicate targets return the existing bookmark. `PATCH .../bookmarks/{bookmark}` accepts `{"name": "Display name"}`; `DELETE` removes only the bookmark. Each session supports up to 1,000 bookmarks. A fork starts without bookmarks because its history sequences are newly assigned; pruning history can leave a bookmark whose original record is no longer available.
- `/image?width=384|768|1536` serves lossless WebP thumbnails, `/pdf/page?page=1` extracts standalone single-page PDFs (both support `ETag`/`304`), and `POST .../playback` issues short-lived signed URLs for `<audio>`/`<video>` tags.
- `POST /api/sessions/{id}/library/cleanup` previews (`{"missing_links": true, "unused_hidden": true, "dry_run": true}`) or deletes eligible files while protecting files referenced by active/standby contexts, pending inputs, or unfinished media jobs (`unreadable_jobs > 0` protects hidden files until damaged job records are fixed).

### Usage endpoints

- `GET /api/usage?from_ms=&to_ms=` (and `/api/sessions/{id}/usage`): Aggregate logical call counts, tokens, and cache hit ratios.
- `GET /api/usage/series?window=7d&bucket=1h` (`window`: `1d|7d|30d|90d|365d|custom`; `bucket`: `1h|6h|1d`): Bucketed token usage and 1-second streaming speed samples.
- `GET /api/usage/daily?days=365&end_date=2026-09-25&tz_offset_minutes=480`: Daily heatmap totals (`days`: 1–366).

### Event streams (SSE)

Both `/api/events` and `/api/sessions/{id}/events` emit `event: wish` frames carrying JSON tagged by `type`. Streams carry live deltas only without replay buffers—on `snapshot` or `gap`, refetch REST state.

- **Global**: `snapshot`, `session_changed`, `session_deleted`, `list_changed`, `configuration_changed`, `gap`.
- **Session**: `snapshot` (current descriptor/status, `live_events`, `retries`, optional `stream_checkpoint`), `upstream_retry`, `session_event` (turns, text/reasoning/tool deltas, native `Checkpoint`/`Rewind`/`Recovering`), `history_changed`, `history_pruned`, `operation_finished`, `operation_failed`, `deleted`.

### Shell bridge commands

Inside a session shell, `WISH_URL`, `WISH_SESSION`, and `WISH_SESSION_TOKEN` authorize CLI tools (`wish askuser`, `wish blob`, `wish history`, `wish extension`, `wish task`, `wish mcp`, `wish skill`, `wish session`). `/api/sessions/{id}/commands/{name}` accepts `askuser`/`ask_user`, `blob`, `history_search`, `history_read`, `history_query`, `extension_tools`, and registered extension tool names (such as `tavily_search` or `openai_image`).
