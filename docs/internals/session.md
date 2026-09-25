# Session

`session` owns configuration, metadata (any JSON value), execution state, durable input,
generations and history. [`executor`](executor.md) executes its model/tool actions;
[storage](storage.md) owns SQLite and caching.

```text
session.rs          Session definition and exports
session/
 +-- lifecycle      create / load / import / delete
 +-- config         settings and metadata
 +-- control        handles and interruption intentions
 +-- queue          durable input, reordering, cancellation and consumption
 +-- machine/       state, next actions and run outcomes
 +-- context/       generations, compaction commits, requests and validation
 +-- history/       message entries, events, their ordered history and its index
 +-- statistics/    model call records
 +-- persistence    stored header and SessionTransaction
 +-- error          session errors
```

Each area contains its types and operations. `SessionTransaction` methods are implemented in
the area that owns the operation, while `persistence` owns the shared transaction boundary.
State, messages, events and call observations can therefore commit together.

```text
Session header                 independently stored, paged collections
 +-- metadata / config
 +-- state ------------------> current tool batch / final outcome event
 +-- active / standby -------> generation headers --> ordered entry IDs
 +-- queue cursor -----------> queued entry IDs
 +-- active_model_call ------> model call records
 +-- list handles -----------> entries / events / history references / model calls
```

Each message and event is its own database element. History rows reference `EntryId` or `EventId`;
no event payload or growing ID array is embedded in history. Generation headers contain a list
ID and a stable source position. Session headers never contain the full conversation, tool batch
or history. `build_request()` resolves only the active generation; a model request still needs
its complete current context in memory.

```rust
let storage = Storage::open("wish.sqlite", StorageOptions::default())?;
let mut session = Session::create(storage.clone(), "my-session", SessionConfig::new("model"))?;
session.enqueue_message(message)?;
// Later, including in another process:
drop(session);
let session = Session::load(storage, "my-session")?;
let page = session.get_history().read_page(0, 128)?;
```

`Session::new(config)` uses an in-memory SQLite database. `from_request` imports paired stable
history into one. `import_history(messages)` appends a protocol-valid conversation to an existing
session at a stable boundary. `create_context_entry(message)` stores an entry outside the active
context, for use in a prepared generation. `delete()` removes the session's whole storage
namespace, including its history index. Session requests always use `ToolChoice::Auto`, including
when an imported request selected another strategy. `RunOptions` only controls tool execution
mode (`ToolMode::Serial` by default, or `Parallel`); there is no maximum turn count. Unknown config
fields are rejected.

All getters for long collections return read-only, lazy handles. `get_history`, `get_entries`,
`get_events`, `get_generations`, `get_generation_entries`, `get_message_queue` and
`get_model_calls` support bounded page reads. `get_active_generation` and `get_standby_generation`
load one header. Small config/state getters expose the owning handle's working state. Each session
has one live owner per `Storage`; opening it again returns `AlreadyOpen`. Dropping that handle
releases ownership. Other tasks use `SessionSender` and read-only list handles. There are no
revision checks or cross-process ownership leases.

## Input queue

Only `User`, `System` and `Developer` messages can be enqueued (`SessionError::InvalidInput`
otherwise). `create_sender()` returns a durable input handle usable during model/tool execution.
Sending atomically commits the message, queue reference and `MessageQueued` event without
interrupting execution. The runner consumes the whole pending queue at a stable boundary, as one
`InputsConsumed` event. Queue positions below `get_queue_head()` have been consumed; the queue log
remains addressable. Suspended sessions require explicit `resume()` before consuming more input.

Pending input can be edited until it is consumed, including while a model or tool runs:

| Operation | Effect | Event |
| --- | --- | --- |
| `move_queued_input(entry, before)` | moves a pending entry before another pending entry, or to the end with `None` | `InputMoved` |
| `cancel_queued_input(entry)` | removes a pending entry from the queue; the entry itself stays stored | `InputCancelled` |

Both are on `SessionSender` and `SessionHandle`, and `cancel_queued_input` also on `Session`. Both
run in one transaction with enqueue and consumption, and return `InvalidEntry` for an entry that
is not pending.

## Events and generations

Ordinary events and state changes commit before the observer is notified. Stream events are
delivered to the observer live and are never written to history or the events list. Final
responses, explicit interruption fragments and call observations are persisted at completion.
`SessionState::Suspended` references the final `Finished` event; use `get_event` to inspect the
stored outcome. History is a permanent record of facts; the active generation is the model's
context projection.

```text
prepare_standby_generation(entry IDs)    activate_standby_generation()
             |                                      |
 store replacement references             append new active tail to standby
 remember source generation + length      seal old active; promote standby
                                           create fresh standby
```

Both operations are atomic and require stable state. Preparation validates tool pairing and
rejects unconsumed queued entries. Replacement references are stored in a fresh list; preparation
events retain the list ID and its original length. Activation preserves the entries appended
since preparation. Old history and sealed generations remain readable. The server's
`POST /api/sessions/{id}/context/clear` uses this pair. Automatic summarization and cutover use the
separate [compaction](compaction.md) workflow. Sealed generations are never garbage-collected;
only `delete()` removes a session's data.

## Crash settlement

Restarting restores committed data and phase. A session interrupted by a crash during model/tool
I/O stays in its active phase and returns `SessionError::Busy`. Nothing is re-executed
automatically. `settle_interrupted()` closes such a run without replaying anything:

| Persisted phase | Settlement |
| --- | --- |
| `CallingModel` | finish as `Interrupted` |
| `ExecutingTools` | started calls get an `Unknown` result and the run finishes `ToolOutcomeUnknown`; unstarted calls are cancelled, and if none had started the run finishes `Interrupted` |
| `Compacting` | restore the phase compaction started from; settle it as above if it was active, otherwise finish `Interrupted` unless already suspended |

A `Running` model call record is closed as `Interrupted` or `Failed` in the same transaction.
The server calls `settle_interrupted` when it opens a session that is not stable, and after a run
returns an error (server-owned: [`src/server/session.rs`](../../src/server/session.rs)).
Graceful interruption is handled by the [executor](executor.md). `resume()` does not check that
unknown tool outcomes were reconciled; it only requires a stable state.

## Timestamps and calls

Messages have creation timestamps on `Entry`; event timestamps live on `HistoryRecord`. Both also
carry an optional originating `model_call_id`. `get_model_calls()` pages logical model calls, and
`get_model_call(id)` retrieves one ([statistics](statistics.md)). Message metadata is
application-owned.

Use a [HistoryReader](history.md) for indexed message-type/time filters, full-text search and
selective expansion, including across compaction.

## Session control

`create_handle()` returns a cloneable `SessionHandle` with `enqueue_message`, the queue edits above,
`interrupt()` and `build_context_snapshot()`. Use it while the executor holds `&mut Session`.
`Session::interrupt()` exposes the same intention directly when the session is available.
Interruption returns true when a run is registered (including repeated requests), false when
idle, suspended without a runner, or after the runner has exited. Acknowledgement is not
completion: await `executor::run` to finish cleanup. The server interrupts through the run's
`ExecutionControl` instead ([executor](executor.md#control-scopes)).

A request targets only the current run. No interrupt is queued for a future run, and a new run gets
fresh execution cancellation state. Registration is removed on return, error, or future drop.
Dropping a future still leaves active persisted work unreconciled; it is not graceful shutdown.
Handles belong to a live Session owner. After dropping/loading that owner, create a new control
handle; old handles cannot interrupt its replacement. `SessionSender` remains usable for durable
input alone, including while the owner is absent. Interrupt intentions are not persisted.

`SessionHandle::build_context_snapshot()` reads committed active context in one storage
transaction, including while the executor owns the session. An unfinished tool batch
and its assistant turn are excluded. The returned request does not include queued input
and does not mutate session state. The server's BTW endpoint `POST /api/sessions/{id}/ask` is built on
it (server-owned: [`src/server/http/ask.rs`](../../src/server/http/ask.rs),
[API](../api.md#side-questions)). It inserts a system note after the fixed prefix, strips tools,
appends at most 32 earlier exchanges (128 000 bytes) and the question, and persists nothing.

Configuration changes during a run go through a run boundary; see
[executor](executor.md#run-boundaries).
