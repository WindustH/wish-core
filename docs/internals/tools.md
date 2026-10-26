# Built-in tools

`tool` contains implementations; `executor::tool` contains the execution contract and dispatch.
`ToolCall` and `ToolOutcome` are session data (`session::{ToolCall, ToolOutcome}`): the session
keeps them in its state and history. `ToolCall::parse_arguments` reads a call's arguments into a
serde type, with the error text the model is told.

```rust
pub trait ToolExecutor: Sync {
  fn execute(&self, call: &ToolCall, control: &ExecutionControl)
    -> impl Future<Output = ToolOutcome> + Send;
}
```

`ToolOutcome` is `Success(Value)`, `SuccessWithInput { output, input }`,
`SuccessWithMetadata { output, metadata }`, `Report { text, output, metadata, failed }`,
`Failed(String)`, `Cancelled` or `Unknown(String)`. New built-in reports separate readable model
text from structured program output and UI metadata. The durable result retains all three;
protocol adapters project only `model_text` onto the model wire. Failed reports use the native
Anthropic/Bedrock error flag where available. Older stored results retain their wire shape,
so this change does not rewrite an existing conversation prefix. `Unknown` means an external effect
may have happened and suspends the run ([executor](executor.md#run)). `SuccessWithInput` appends
extra model input after the whole batch's results, preserving call/result pairing.
`SuccessWithMetadata` keeps its metadata on the stored result message, which is never sent to the
model.

The application's dispatcher is `SessionTools`
([`src/application/session/tools.rs`](../../src/application/session/tools.rs)). Every session gets
`history_search`, `history_read`, `history_query` and `view_image`. The session's switches
(`descriptor.tools`) add the simple `shell` tool, `ask_user` and `web_search`; the `mcp` and `skills`
switches add no tool (see [MCP](mcp.md) and [skills](skills.md)). The `ShellTool` starts the first time the shell is switched on and stays
alive while it is off, so background commands still report. The server rewrites `config.tools` to
exactly that set and rejects a config naming any other tool (`no executor for tool X`), in the same
file.

```rust
use crate::tool::shell::{ShellConfig, ShellTool};

let shell = ShellTool::new(ShellConfig::new(cwd, capture_dir)).await?;
let mut config = SessionConfig::new(model);
config.tools.extend(shell.get_specifications());
// Keep one ShellTool per session, alive across runs, so execution IDs remain valid.
let outcome = executor::run(&model_caller, &mut session, &shell, &control, observe).await?;
shell.shutdown().await?;
```

## Shell and tasks

The model sees `shell({command, timeout?, edit?, check_diff?})`. It runs a new shell in the session's working
directory and waits until exit. Omitted timeout means no limit; a positive timeout is a hard
runtime limit and terminates the process tree. Large output does not background the command:
the structured result retains the last 2000 lines / 50 KiB and points to the complete capture.
The model sees ordinary command output without encoding, empty process fields or task identifiers.
Failure, timeout, background execution and truncation add only necessary diagnostics or next steps.
Model text reserves room within its 50 KiB budget for notices and up to 8 KiB of requested diff;
full edits remain in the durable task record and UI metadata. Background completion notifications
include an 8 KiB output tail and edit summaries, avoiding an obligatory follow-up status read.

`tool/shell.rs` adapts model calls. `task/` owns the process registry, supervision, input/output,
background transfer and edit captures. `application/task_access.rs` enforces session ownership
and schedules completion notifications. HTTP and CLI remain adapters. `storage/task_records.rs`
writes task records alongside their output, atomically. Completed records survive restart;
previously running records become `interrupted`, since restarting Wish cannot recover process
ownership. Graceful shutdown terminates managed tasks.

Every foreground shell call is registered too. `wish task background <id>` releases its foreground
wait without restarting or signalling the process. Inside that shell, `self` resolves to the
injected `WISH_TASK_ID`. A detached task survives cancellation of that foreground invocation;
explicit task termination and service shutdown still stop it.

```text
wish task list --running
wish task start --interactive -- 'python worker.py'
wish task show <id>
wish task poll <id> --offset 0 --max-bytes 4096
printf 'input\n' | wish task write <id> --stdin --close
wish task background <id>
wish task wait <id>
wish task kill <id> --force
wish task remove <id>
```

`start` defaults to immediate background execution; `--wait` is a soft wait, distinct from the
simple tool's hard timeout. The shell tool accepts optional `edit` (absolute file paths) and
`check_diff`; full diffs remain metadata unless `check_diff` is true. Task management has no edit operation. `remove` refuses running tasks and deletes completed task
records and captures. Commands return readable text by default; prefix `--output-json` for scripts.
The HTTP task API continues to return structured data.
The task list is scoped to its owning session. User API callers can still inspect and terminate
existing tasks with the session's shell switch off; starting a new command is refused.

### Process configuration

Linux, macOS and BSD supervise commands with Unix process groups; Windows uses Job Objects. Other
platforms are refused by `ShellTool::new`, which also canonicalizes `cwd` (it must be an existing
directory) and creates the capture directory. Command text is passed as one final argument after
the shell's leading arguments. Commands inherit the server's environment (plus `ShellConfig.env`
overrides) and OS permissions. No PTY is allocated.

| `ShellConfig` field | Default |
| --- | --- |
| `command` | `ShellCommand::platform_default()`: `/bin/sh -c`, or `%COMSPEC%` (falling back to `cmd.exe`) `/D /S /C` |
| `soft_timeout` | 10 s |
| `inline_bytes` | 64 KiB |
| `poll_bytes` (default `max_bytes`) | 64 KiB |
| `kill_grace` | 1 s |
| `stdin_write_timeout` | 10 s |

`ShellCommand::for_program(path)` picks leading arguments by the program's name: `-lc` for zsh and
bash, `-l -c` for fish, `-NoLogo -NoProfile -NonInteractive -Command` for pwsh/powershell,
`/D /S /C` for cmd, `-c` otherwise. `ShellConfig.command` is an `Arc<RwLock<ShellCommand>>` read
at each start, so replacing it affects the next command of every tool sharing it.

Server-owned: a session created with a shell gets its own `ShellTool` with the session's `cwd` and
capture directory `data_dir/shell/<session id>`. The command is the session's own shell if set
(`PUT /api/sessions/{id}/shell`), otherwise the configured global shell, which follows later
config saves. An override that no longer resolves falls back to the global shell when the session
opens. Deleting a session removes its capture directory
([`src/application/session.rs`](../../src/application/session.rs),
[`src/application/session/tools.rs`](../../src/application/session/tools.rs), [API](../api.md#sessions)).

### Task operations and legacy tool calls

The following tool names are accepted by the compatibility adapter for old callers but are not
advertised to models. Their operations are available through `wish task`.

| Tool | Required Inputs | Optional Inputs | Behavior |
| --- | --- | --- | --- |
| `shell_start` | `command` | `timeout`, `data`, `encoding`, `interactive` | Run a script; return completion or a background execution ID. |
| `shell_edit` | `command`, `diff` | `check_diff` | Run a script that edits files to exit; return output and per-file changes. |
| `shell_poll` | `execution_id` | `offset`, `max_bytes`, `wait_ms`, `encoding` | Read merged output by raw byte offset, optionally wait for new bytes. |
| `shell_write` | `execution_id`, `data` | `encoding`, `close` | Feed stdin and optionally close it; return the accepted byte count. |
| `shell_kill` | `execution_id` | `mode` (`graceful` default, `force`) | Terminate the process group/job and wait for the supervised child to be reaped. |

```text
start -> foreground wait -- exit -------> inline result
                 |       -- output -----> execution ID + captured prefix
                 |       -- soft timeout -> execution ID + captured prefix
                 |                              |
                 |                         poll / write / kill
                 |
                 +-- interrupt -> terminate group, reap, return captured output
```

`timeout` is a soft wait in seconds. Omitted or `-1` uses `soft_timeout`, `0` returns immediately,
and other negative or non-finite values are invalid. It does not kill the command. Output beyond
`inline_bytes` also returns early without killing it. There is no hard runtime or capture-size
limit. Output is written directly to `<capture_dir>/<execution id>/output.log`, with stdout and
stderr sharing the file. Execution IDs are `<pid>-<unix nanos>-<counter>`. Captures are retained;
the server removes them only with the session.

`return_reason` on `wish task start` / legacy `shell_start`:

| Value | Meaning |
| --- | --- |
| `exited` | the command finished within the wait |
| `output_threshold` | the command finished, but its output exceeds `inline_bytes` |
| `output_exceeded_inline` | still running; output passed `inline_bytes` |
| `soft_timeout` | still running at the soft timeout |
| `interrupted` | the run was interrupted; the group was terminated |

Start, edit and poll results (and `wait_for_completion`) share one shape: `execution_id`,
`process` (`status`: `running`, `exited`, `killed`, `interrupted` or `unknown`; `exit_code`; Unix-only
`term_signal`; `error`; initial stdin bytes and error), `output_path`, `output_bytes`,
`next_offset`, `eof`, `encoding`, `text` and `lossy`. `output_notice` appears when unread output
remains and advises a bounded read or a search of `output_path`. Start and edit add
`return_reason`, and edit adds `edits`. `shell_kill` returns `{execution_id, process}`.
`shell_write` returns `{execution_id, write: {accepted_bytes, stdin_available, interrupted,
timed_out, error}}`.

Nonzero command exits are successful tool operations whose `process.exit_code` reports the
failure. Text decoding is lossy when needed; use base64 encoding to read exact bytes, including
across UTF-8 boundaries. `max_bytes` limits each read, not captured output. Small foreground
results remain pollable too.

### `shell_edit`

`diff` lists the absolute paths the command edits (at least one). Each file is read before launch
and after exit, and a missing file counts as empty. The command has no stdin and no timeout, and
never moves to the background. The call returns when it exits or is interrupted, so every diff is
final. `return_reason` is `exited` or `interrupted`, and inline output is capped at `inline_bytes`.

Results carry `edits`, one per path, with `path` and `status`: `complete` with `changed`,
`binary` and `diff`, or `failed` with `error`. Without `check_diff` the model receives each entry
without `diff`, as `SuccessWithMetadata` with the full entries in the result's metadata. Text
diffs use imara-diff's Histogram algorithm, indentation-aware hunk placement, and unified format
with three context lines and JSON-quoted `---`/`+++` paths. Unchanged text produces an empty diff.
Binary files (non-UTF-8, or containing NUL) report whether bytes changed and set `diff` to null. A
final read failure returns `status: "failed"` and `error` alongside the normal process result; an
initial read failure prevents launch. If the supervisor loses the process (`unknown`), the result
is `ToolOutcome::Unknown` and no diffs are returned.

Completion, nonzero exit, and interruption all capture actual file changes. Polls of an edit's
execution return its output only, never its diffs. Only the named files' contents are compared;
file permissions and concurrent writers are not tracked.

### Stdin, interruption and kill

Stdin defaults to the null device. Initial `data` is decoded as UTF-8 or base64, written and
closed unless `interactive` is true. Initial input precedes subsequent writes; later writes
serialize on the stdin pipe. A write timeout reports `accepted_bytes` and `timed_out`; retry only
the remaining bytes. Cancellation mid-write reports `interrupted: true` with any accepted bytes and
leaves an existing background command running.

A foreground interruption of `shell_start` or `shell_edit` gracefully terminates the process
group/job, waits for reaping and returns a result with `return_reason: interrupted` and captured
output. Dropping an in-flight start or edit force-kills its command. Already-background commands
survive session interruption. Cancelling a poll does not kill its command.

On Unix, graceful kill sends SIGTERM followed by SIGKILL after `kill_grace`; force kill sends
SIGKILL immediately. On Windows, graceful kill attempts CTRL_BREAK_EVENT for the child console
group, then terminates its Job Object after the grace. If no shared console is available, it
terminates the job immediately. Force kill always terminates the job. Windows children are created
suspended, assigned to a kill-on-close Job Object, and only then resumed; job assignment failure
stops startup without running an unsupervised script. Ordinary descendants are cleaned up when
the supervised shell exits too; Unix commands that deliberately escape the process group are
outside this supervision. Windows jobs do not grant breakaway permission.

`shutdown()` force-kills and awaits all owned executions and prevents new starts; the server calls
it for every session during [shutdown](executor.md#shutdown). Dropping the last `ShellTool` sends
force-kill requests; dropping an unfinished supervisor kills its process group/job. These drop
paths do not replace awaiting shutdown. The registry mutex covers lookup, and spawn plus
registration, so shutdown never misses a started process. Stdin has its own asynchronous
serialization.

Live execution IDs belong to one `ShellTool` instance and do not survive a restart; captures on
disk do.

### Background completion notifications

`ShellTool::wait_for_completion(execution_id)` waits for a terminal process snapshot and returns
the poll-shaped result with empty `text`, without loading output.

Server-owned: when a `shell_start` result is still `running`, `SessionTools` spawns a task that
awaits `wait_for_completion`. It then enqueues a `Developer { fixed: false }` message with metadata
`{source: "background_execution_finished", execution_id, completion: {command, result}}` and wakes
the session unless it was interrupted since the start. The task is dropped on server shutdown
([`src/application/session/tools.rs`](../../src/application/session/tools.rs)). The notification fires even
if the model later polled the command to completion or killed it.

## History tools

`tool::history::HistoryTools::new(&session)` binds the session's `SessionReader` to its permanent
history. Tool arguments cannot select another session.

| Tool | Required Inputs | Optional Inputs | Behavior |
| --- | --- | --- | --- |
| `history_search` | `text` | `mode` (`terms` default, `substring`), `limit` (20), `filter` | Full-text and substring search over committed history. |
| `history_read` | `sequence` | `before`, `after` (0) | Read the original record at `sequence` with neighbors; returns `{items}`. |
| `history_query` | none | `filter`, `limit` (50), `order` (`oldest_first` default, `newest_first`), `cursor` | Filtered, cursor-paged history in sequence order. |

An omitted `filter` means `kind: message`. Pass the previous page's `next` unchanged as `cursor`,
with the same filter and order. Filters, search modes, indexing and paging are described in
[history](history.md).

Search example:
```json
{"text":"历史决策","mode":"substring","limit":10,"filter":{"message_types":["user","assistant"],"since":1700000000000,"until":1800000000000}}
```

Retrieval does not insert old messages into active context; results arrive as an ordinary tool
result. Compaction does not remove the history these tools read. Queries run on `spawn_blocking`.
Cancellation before launch skips the operation; cancellation during a read waits for it to finish
and returns `Cancelled`. The tools' own arguments and results are excluded from full-text indexing.

## `view_image`

`tool::view_image::ViewImageTool` reads an absolute path to a PNG, JPEG, GIF or WebP file, detected
by magic bytes, up to 20 MiB. It returns `SuccessWithInput` with output
`{path, mime_type, byte_count}` and the image as input. The session stores all tool results first,
then appends the images as one `User` message per call (metadata `tool_call_id`, `tool_name`),
preserving call/result pairing for parallel batches.

Server-owned: `SessionTools` saves the bytes to `data_dir/blobs/<session id>/<sha256>`, adds
`output.session_path`, and keeps only the image block. At request time the server projects stored
images ([`src/application/session_model.rs`](../../src/application/session_model.rs)). Each image becomes an
`[Image sha256:…]` notice plus its blob path, and the image itself is sent only when the model's
declared `input_modalities` allow it. If the upstream rejects images before streaming, the call is
retried once with notices only. Stored conversation always keeps the original images.

## `web_search`

`tool::web_search::WebSearchTool::new(backend, conversation)` searches the web through `backend`, a
`SearchBackend` the application supplies: it runs one search and returns the `Answer` (the
provider that answered, its display name and the results) or why every provider failed.
`conversation` names the session, for a service that keeps one conversation across searches. The
tool's description and schema never name a service, so the model's request stays the same
whichever provider answers.

| Input | Meaning |
| --- | --- |
| `query` | Required; at most 400 characters |
| `allowed_domains`, `blocked_domains` | At most 20 domain names each; subdomains included |
| `recency` | `day`, `week`, `month` or `year` |
| `max_results` | 1 to 20, default 10 |

The model receives the results as text: numbered title and URL, site and date when known, a
snippet of at most 600 characters, any warnings, and a note that the results come from the web and
are not verified. The result is `SuccessWithMetadata`, with `{query, provider, provider_name,
results, warnings}` kept on the stored result message for the page. A failure of every provider is
`Failed("web search failed: …")`; an interrupt cancels the search.

Server-owned: `SessionTools` supplies a backend that asks the configured search providers in
order ([`src/application/search.rs`](../../src/application/search.rs),
[configuration](../configuration.md#web-search)); the session's `web_search` switch adds the tool.

## `ask_user`

`tool::ask_user::AskUserTool::new(changed)` puts a form of questions to the person using the
session and waits for the answers. It is an ordinary tool call: the run stays in `ExecutingTools`,
input sent meanwhile waits in the queue, and an interrupt cancels the call.

| Input | Meaning |
| --- | --- |
| `questions` | 1 to 8 questions, each `{type, question, header?}` plus its type's fields |
| `timeout_seconds` | 1 to 604800. Without it the call waits until answered, skipped or interrupted |

A `choice` question has 2 to 8 `options` with unique labels (`{label, description?}`, or a bare
label string), `multi_select` (default false) and `allow_other` (default true). A `text` question
takes `placeholder` and `multiline`. A malformed form fails the call with a message saying what to
fix.

The result's `output.status` is one of:

- `answered`, with `answers` in question order: `{question, type, selected, other?}`,
  `{question, type, text}` or `{question, type, skipped: true}`;
- `skipped`, when the user declined the whole form;
- `timed_out`, with `waited_seconds`, when the timeout passed first.

The tool keeps the open forms in memory and calls `changed` whenever they change; `snapshot()`
lists them and `answer(call_id, answers, skip)` hands answers to the waiting call. The server
publishes the list as `status.pending_questions` and takes answers through
`POST /api/sessions/{id}/answer`. A form that timed out stays open. Answered later, the server
enqueues `late_answer_message`, a `Developer { fixed: false }` message with metadata
`{source: "ask_user_answer", call_id, questions, answers}`, and wakes the session, as a background
completion does; skipped, it is dropped. A restart closes every open form.

## Capability delivery

Session/default tool settings keep the enable switches and add `task_delivery`,
`web_fetch_delivery`, `web_search_delivery`, and `ask_user_delivery`: `tool` or `command`.
Task defaults to command delivery; the other capabilities retain direct-tool delivery.
Command delivery removes that capability from the model tool list and supplies command
instructions instead. When shell is disabled, enabled web/question capabilities fall back to
direct tools, so they remain reachable.

`wish webfetch URL`, `wish websearch QUERY`, and `wish askuser --json '{"questions":[...]}'`
share the direct tools' implementations. All three support `--json`, `--stdin` and `--schema`.
Task direct delivery exposes one `task` tool using the task API's request object.

Command calls/results are stored as application history events and projected into ordinary
frontend tool cards, including search metadata and question forms. They are not synthetic
model messages. A cancelled request records cancellation as well.

Global delivery settings are creation defaults. `App::create_session` copies them into the
session descriptor and `SessionSlot::open` uses that descriptor on reopen. Settings saves do
not propagate delivery changes to existing sessions. Legacy descriptors deserialize with fixed
compatibility defaults rather than consulting current global settings.

`tool::arguments` omits null/blank optional parameters according to each tool's schema before
its ordinary parser runs. Required values and stdin payload whitespace are preserved. Tool
implementations validate meaning and report the field plus recovery instructions. The task
application parser applies the same rules to HTTP and direct calls; the CLI is only an adapter.

Delivery changes use `Session::defer_instruction_config`: the model sees a tail system notice,
while instruction replacements and the target tool list are saved for the next generation.
Adding a direct tool takes effect immediately; removing its definition solely for command delivery
can wait. Permission revocation takes effect immediately. Reopening and starting another run do
not rewrite that generation's initial instructions.

`ToolExecutor::accepts_unadvertised_call` defaults to false. The application explicitly recognizes
Task, Web Fetch, Web Search and Ask User independently of presentation mode, then checks capability
permissions in its dispatcher so disabled capabilities return a specific error. Generic executors still reject unknown tools. Both serial and parallel batch
execution use this contract, and both routes retain the same validators and timeline semantics.

## Command presentation

Native Web Fetch, Web Search, Ask User and Task calls share presentation with their `wish`
command adapters. CLI `--output-json` is an explicit leading option for programmatic consumers;
`--json` continues to mean JSON input for web/ask commands. Web Fetch snapshots paginate at
20,000 characters, 32 KiB, or 1,800 newlines, preserving room for title, URL and cursor instructions
under the enclosing shell limit. Continuation always reads the same snapshot. Ask User prints
questions with selected/written answers, skipped questions and timeout guidance without JSON
escaping. Structured API responses and timeline data remain available independently.
