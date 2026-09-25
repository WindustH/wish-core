# Built-in tools

`tool` contains implementations; `executor::tool` contains the execution contract and dispatch.

```rust
pub trait ToolExecutor: Sync {
  fn execute(&self, call: &ToolCall, control: &ExecutionControl)
    -> impl Future<Output = ToolOutcome> + Send;
}
```

`ToolOutcome` is `Success(Value)`, `SuccessWithInput { output, input }`,
`SuccessWithMetadata { output, metadata }`, `Failed(String)`, `Cancelled` or `Unknown(String)`.
The model receives `{status, output}` or `{status, message}`. `Unknown` means an external effect
may have happened and suspends the run ([executor](executor.md#run)). `SuccessWithInput` appends
extra model input after the whole batch's results, preserving call/result pairing.
`SuccessWithMetadata` keeps its metadata on the stored result message, which is never sent to the
model.

The server's dispatcher is `SessionTools`
([`src/server/session/tools.rs`](../../src/server/session/tools.rs)). Every session gets
`history_search`, `history_read`, `history_query` and `view_image`. It gets the five shell tools
only when created with a shell. The server rewrites `config.tools` to exactly that set and
rejects a config naming any other tool (`no executor for tool X`)
([`src/server/session.rs`](../../src/server/session.rs)).

```rust
use crate::tool::shell::{ShellConfig, ShellTool};

let shell = ShellTool::new(ShellConfig::new(cwd, capture_dir)).await?;
let mut config = SessionConfig::new(model);
config.tools.extend(shell.get_specifications());
// Keep one ShellTool per session, alive across runs, so execution IDs remain valid.
let outcome = executor::run(&model_caller, &mut session, &shell, &control, observe).await?;
shell.shutdown().await?;
```

## Shell

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
([`src/server/session.rs`](../../src/server/session.rs), [API](../api.md#sessions)).

### Operations

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

`return_reason` on `shell_start`:

| Value | Meaning |
| --- | --- |
| `exited` | the command finished within the wait |
| `output_threshold` | the command finished, but its output exceeds `inline_bytes` |
| `output_exceeded_inline` | still running; output passed `inline_bytes` |
| `soft_timeout` | still running at the soft timeout |
| `interrupted` | the run was interrupted; the group was terminated |

Start, edit and poll results (and `wait_for_completion`) share one shape: `execution_id`,
`process` (`status`: `running`, `exited`, `killed` or `unknown`; `exit_code`; Unix-only
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
([`src/server/session/tools.rs`](../../src/server/session/tools.rs)). The notification fires even
if the model later polled the command to completion or killed it.

## History tools

`tool::search_history::SearchHistoryTool::new(&session)` binds a `HistoryReader` to one session's
permanent history. Tool arguments cannot select another session.

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
images ([`src/server/media.rs`](../../src/server/media.rs)). Each image becomes an
`[Image sha256:…]` notice plus its blob path, and the image itself is sent only when the model's
declared `input_modalities` allow it. If the upstream rejects images before streaming, the call is
retried once with notices only. Stored conversation always keeps the original images.
