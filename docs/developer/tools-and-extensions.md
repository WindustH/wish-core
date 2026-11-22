# Tools & Extensions

Wish separates built-in runtime capabilities (`src/tool/` + `src/task/`) from bundled supplier extension packages (`src/extensions/` + `src/extension_host/`).

## Capabilities and delivery modes

`src/capability.rs` defines the built-in `Capability` variants, and `src/application/capabilities.rs` maps per-session `ToolSwitches` and `ToolDelivery` (`tool` vs `command`):

- `view_image` (always enabled) and `shell` advertise directly as model function tools.
- Command-backed capabilities default to `"command"` for core tools (`blob`, `history_search`, `task_delivery`, `web_fetch`, `web_search`, `ask_user`, `sessions`) and `"tool"` (disabled) for media/ASR (`image_generate`, `video_generate`, `audio_generate`, `audio_transcribe`).
- Turning `shell` off automatically falls back to direct tool advertisement for any enabled capability.
- `SessionTools` (`application/session/tools.rs`) enforces switches identically whether invoked directly by the model or via the shell bridge (`/api/sessions/{id}/commands/{name}`). Changing delivery uses `Session::defer_instruction_config` so active prompt prefixes stay intact until the next generation boundary.
- Structured outcomes (`ToolOutcome::Report`, `SuccessWithProjection`) keep rich JSON in `content.output` for UI inspectors while projecting concise `model_text` / `model_value` (with `$WISH_*` aliases) onto the model wire.

## Shell, tasks, and the CLI bridge

`ShellTool` (`src/tool/shell.rs`) and `TaskManager` (`src/task/`) supervise session commands using Unix process groups (`process-wrap`) or Windows Job Objects without allocating a PTY:

- Every shell invocation registers as a task (256 MiB capture cap, up to 16 active tasks per session, optional pre/post edit diffs for up to 16 regular files <= 2 MiB).
- Foreground commands can detach into the background via `wish task background <id|self>` or `background_tools_on_input`, reporting completion with an 8 KiB tail. Input-triggered detachment waits up to one second for an inline result; this timer starts when new input is observed and does not delay exit, cancellation, explicit detachment, or the command's hard timeout. `ShellTool` initializes lazily on first shell enable and stays alive if shell is turned off later so background work finishes cleanly.
- `SessionSlot::open` issues an ephemeral `bridge_token` (2 UUIDs) injected alongside `WISH_URL`, `WISH_SESSION`, `WISH_TASK_ID`, `WISH_TMPDIR`, `$WISH_*` aliases, and `<data_dir>/bin` on `PATH`. Bridge routes validate that token directly rather than going through global `/api` auth.

## Extension packages (`src/extensions/` & `src/extension_host/`)

Search, webpage fetch, image, video, audio, and transcription suppliers live as self-contained packages under `src/extensions/<supplier>/` registered once in `src/extensions/package.rs`. Each package owns its `presets.json`, typed request validation, and `Executable` implementation, returning an erased `Invocation` so `application/` and `session/` contain zero vendor-specific branches.

Shared host runtimes enforce two key safety invariants:
- **Paid media safety**: Image, video, audio, and transcription runners snapshot `.media-jobs/<job>.json` before submitting a paid request and never automatically retry or fail over after an uncertain outcome (`outcome_unknown`). Recovery downloads serialize under per-job locks, status calls reject model overrides, and Runway ZIP outputs validate headers, CRCs, and expansion limits without extracting or decoding pixels.
- **Immutable fetch snapshots (`storage/snapshots.rs`)**: Continuation paging cursors bind to immutable snapshots paired with the originating extension and service ID.

`wish extension search`, `inspect`, and `call` expose the exact same metadata and execution paths used by direct tool calls.

## MCP, skills, peers, and folders

- **MCP (`src/mcp.rs`, `src/application/mcp.rs`)**: Wraps `rmcp` over `ReqwestTransport` and `process-wrap`. `McpHub` manages lazy per-session or shared instances, shares per-server tool catalogs, reaps idle instances every 5 seconds, forwards `notifications/cancelled` on client disconnects, and saves binary outputs under `data_dir/shell/<session_id>/mcp/`.
- **Skills (`src/application/skills.rs`)**: Scans `.agents/skills` in `cwd`, `<config_dir>/skills`, and `skills.dirs` on demand inside `blocking`, ranking matches across names, categories, descriptions, and CJK bigrams.
- **Peers (`application/peer_commands.rs`)**: Powers `wish session` with ownership checks (`created_by`), alias translation and blob sharing under `blob_publication`, and direct enqueueing (`source: session_message`) into the target session.
- **Folders (`storage/management/folders.rs`)**: Lives exclusively in `management.sqlite` (`folders` table and `sessions.{folder,pinned}` columns) so folder listings and tree moves only touch single index pointers.

## Session browsers

The optional `browser` capability owns a headless Chromium process and a private
temporary profile per live session. Set `browser_executable` to an absolute path
on the **server**, or leave it empty for executable discovery. Set
`defaults.tools.browser` for new sessions or change the individual session's
built-in tool settings. The default is off. Disabling the capability, unloading
the session or stopping the server releases the browser and its temporary profile.

Server Chromium can reach loopback and LAN services on the development host.
SSH authenticates incoming connections to Wish. A browser running on the server
can reach the local API listener directly. Configure `bearer_token_env` to require
a credential for those API requests as well.

The model function and `wish browser --output-json --stdin` use the same page
and action dispatcher. `observe` returns document-specific element references.
`evaluate` accepts a JavaScript function and a JSON argument. It runs in the
page and awaits the result. A shell script can orchestrate multiple browser
commands across navigations. Screenshots and JSON results larger than 24 KiB
are saved as session files. Run `wish browser --schema` for action arguments.

Browser calls can select the server or client with the `target` field.
`{"action":"targets"}` lists the two targets without launching Chromium.
A client attachment uses a single-use ticket scoped to the session and to the
client role. Each session accepts one attached device. Requests and replies
carry a sequence number. Disabling the capability or closing the attachment
ends the connection. An interrupted action has an uncertain outcome.
Existing calls with no target use the server while no device is attached.
When a device is attached, callers must specify the target.

The Tauri sidebar renders its device page locally. The attachment carries agent commands and results. Server Chromium runs independently and creates a screenshot only for an explicit screenshot action.
