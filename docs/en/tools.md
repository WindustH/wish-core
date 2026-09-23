# Built-in tools

`tool` contains implementations; `executor::tool` contains the execution contract and dispatch
logic. Applications register specifications in SessionConfig and pass a ToolExecutor to `run`.
The built-in `tool::shell::ShellTool` implements that contract directly. An application can also
delegate to it from a dispatcher that includes its own tools.

```rust
use wish_core::tool::shell::{ShellConfig, ShellTool};

let shell = ShellTool::new(ShellConfig::new(workspace, capture_directory)).await?;
let mut config = SessionConfig::new(model);
config.tools.extend(shell.get_specifications());
let mut session = Session::new(config)?;
// Enqueue input, then:
let outcome = executor::run(&model_caller, &mut session, &shell, &control, observer).await?;
// Keep shell alive across runs so execution IDs remain available.
shell.shutdown().await?;
```

Use an application-owned Tokio runtime with I/O and time enabled (`enable_all()`). Linux, macOS and BSD use Unix process groups; Windows uses Job Objects. The default is
`/bin/sh -c` on Unix and `%COMSPEC% /D /S /C` (falling back to cmd.exe) on Windows. Program, leading
arguments, working directory and environment overrides are configurable. PowerShell can be selected
with program=pwsh.exe and arguments such as `-NoProfile -NonInteractive -Command`. Command text is passed
as one argument. Commands inherit the application's environment and OS permissions. Each session
should have its own ShellTool and capture directory; clones intentionally share execution access.

## Shell operations

| Tool | Required Inputs | Optional Inputs | Behavior |
| --- | --- | --- | --- |
| `shell_start` | `command` | `edit`, `timeout`, `data`, `encoding`, `interactive` | Run a script; return completion or a background execution ID. |
| `shell_poll` | `execution_id` | `offset`, `max_bytes`, `wait_ms`, `encoding` | Read merged output by raw byte offset, optionally wait for new bytes. |
| `shell_write` | `execution_id`, `data` | `encoding`, `close` | Feed stdin and optionally close it; return the actual accepted byte count. |
| `shell_kill` | `execution_id` | `mode` | Terminate the process group/job and wait for the supervised child to be reaped. |

```text
start -> foreground wait -- exit -------> inline result
                 |       -- output -----> execution ID + captured prefix
                 |       -- soft timeout -> execution ID + captured prefix
                 |                              |
                 |                         poll / write / kill
                 |
                 +-- interrupt -> terminate group, reap, return captured output
```

Timeout is a soft wait in seconds: omitted or `-1` uses the configured default (10 seconds), `0`
returns immediately. It does not kill the command. Crossing the inline budget (default 64 KiB)
also returns early without killing it. There is no hard runtime or capture-size limit. Output is
written directly to an execution-specific `output.log`, with stdout/stderr sharing the same file.
Terminal and interrupted captures are retained; removal is up to the application.

Results include execution_id, process status, exit code/signal, output_path, output_bytes,
next_offset, eof, text and return_reason. Nonzero command exits are successful tool operations
whose process.exit_code reports the failure. Text decoding is lossy when needed; use base64
encoding to read exact bytes, including across UTF-8 boundaries. max_bytes limits each read, not
captured output. Small foreground results remain pollable too.

For file edits, pass `edit` as an absolute file path. The tool reads the file before launch and
after completion; a missing file counts as empty. Results include `edit` with `path` and `status`:
`pending` while running, then `complete` with `changed`, `binary`, and `diff`. Text diffs use
imara-diff's Histogram algorithm, indentation-aware hunk placement, and unified format with three
context lines. Unchanged text produces an empty diff. Binary/non-UTF-8 files report whether bytes
changed and set `diff` to null. A final read failure returns `status: "failed"` and `error` alongside
the normal process result; an initial read failure prevents launch.

Background starts return a pending edit; `poll` or `kill` retrieves the final diff. Completion,
nonzero exit, and interruption all capture actual file changes. The final result is retained, so
later polls do not include subsequent edits. Only the named file's contents are compared; file
permissions and concurrent writers are not tracked.

Stdin defaults to the null device. Initial data is decoded as UTF-8 or base64, written and closed
unless interactive=true. Initial input precedes subsequent writes; later writes serialize on the
stdin pipe. A write timeout (default 10 seconds) reports accepted_bytes and timed_out; retry only
the remaining bytes. Cancellation similarly reports any accepted bytes and leaves an existing
background command running. No PTY is allocated.

A foreground interruption terminates the process group/job, waits for reaping and returns a result
with return_reason=interrupted and captured output. The executor then finishes the interrupted
session turn. Already-background commands survive session interruption. Cancelling a poll does
not kill its command. On Unix, graceful kill sends SIGTERM followed by SIGKILL after the configured grace; force kill
sends SIGKILL immediately. On Windows, graceful kill attempts CTRL_BREAK_EVENT for the child
console group, then terminates its Job Object after the grace. If no shared console is available,
it terminates the job immediately. Force kill always terminates the job. Windows children are
created suspended, assigned to a kill-on-close Job Object, and only then resumed; job assignment
failure stops startup without running an unsupervised script. Ordinary descendants are cleaned up when the supervised
shell exits too; Unix commands that deliberately escape the process group are outside this supervision. Windows
jobs do not grant breakaway permission.

Call shutdown to stop and await all owned executions before shutting down the runtime. It also
prevents new starts. Dropping the last ShellTool sends force-kill requests; dropping an unfinished
supervisor kills its process group/job. These drop paths do not replace awaiting shutdown. Registry
locks cover handle lookup/insertion only; stdin has its own asynchronous serialization.

This ports the old shell's four operations and soft background handoff. It does not implement
old wishd's process adoption after restart, persistent execution indexing, stdin audit log or
background-completion session notifications. Captures survive on disk, but live execution IDs
belong to this ShellTool instance.

The external shell black-box suite runs natively on Unix or Windows. On a Linux host with the
Windows Rust target, MinGW and Wine installed, run it with `WISH_CORE_SHELL_TARGET=x86_64-pc-windows-gnu`
and `WISH_CORE_SHELL_RUNNER=wine`; Wine state stays in its temporary test directory; Mono/Gecko installer entry points are disabled.
Wine is opt-in and is never started by the default test command.

## History search

`tool::search_history::SearchHistoryTool::new(&session)` provides atomic history query tools (`history_search`, `history_read`, `history_query`) bound to that
session's permanent history. Register its specifications in `SessionConfig.tools` and delegate to it
from the application's tool dispatcher, alongside shell or other tools. It does not take a session
ID in tool arguments and cannot search another session.

```rust
let history_tool = wish_core::tool::search_history::SearchHistoryTool::new(&session);
let mut config = session.get_config().clone();
config.tools.extend(history_tool.get_specifications());
session.set_config(config)?;
```

| Tool | Required Inputs | Optional Inputs | Behavior |
| --- | --- | --- | --- |
| `history_search` | `text` | `mode`, `limit`, `filter` | Full-text and substring search over committed history. |
| `history_read` | `sequence` | `before`, `after` | Read original record at sequence with neighbor expansion. |
| `history_query` | None | `filter`, `page` | Chronological filtered and paged history queries. |

Search example:
```json
{"text":"历史决策","mode":"substring","limit":10,"filter":{"message_types":["user","assistant"],"since":1700000000000,"until":1800000000000}}
```

Read example:
```json
{"sequence":42,"before":2,"after":3}
```

Compression does not remove the historical messages this tool reads. Retrieval does not insert
old messages into active context; results arrive as an ordinary tool result for the model to review.
Blocking storage queries run outside the async executor thread. Cancellation before launch skips
the operation; cancellation during a read waits for its completion and returns Cancelled.
See [history queries](history.md) for indexing, pagination and metadata behavior.

## Local images and completion notifications

`tool::view_image::ViewImageTool` reads an absolute PNG/JPEG/GIF/WebP path (up to
20 MiB) and returns `ToolOutcome::SuccessWithInput`. The session stores all tool
results first, then appends the supplemental native image input. This preserves
call/result pairing for parallel batches. Applications own file snapshots and
model capability projection; the server module supplies both without erasing stored images.

`ShellTool::wait_for_completion(execution_id)` waits for a terminal process snapshot
and returns file/status metadata without loading output. Applications can turn this
into a durable session notification and decide whether to wake execution. Core does
not automatically enqueue notifications. Truncated output results include a notice
advising bounded reads or searches.
