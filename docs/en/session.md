# Session

`session` owns configuration, metadata (any JSON value), execution state, durable input,
generations and history. `executor` executes its model/tool actions; [storage](storage.md) owns
SQLite and caching.

```text
session.rs          Session definition and public exports
session/
 +-- lifecycle      create / load / import
 +-- config         settings and metadata
 +-- control        handles and interruption intentions
 +-- queue          durable input and consumption
 +-- machine/       state, next actions and run outcomes
 +-- context/       generations, compaction, requests and validation
 +-- history/       message entries, events and their ordered history
 +-- statistics/    model call records and usage observations
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
 +-- list handles -----------> entries / events / history references
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

`Session::new(config)` uses an in-memory SQLite database for ephemeral use. `from_request`
imports paired stable history into one. Session requests always use `ToolChoice::Auto`, including
when the imported request selected another strategy. `RunOptions` only controls tool execution mode;
there is no maximum turn count. Unknown config fields are rejected. Use `create`/`load` for persistence.

All getters for long collections return read-only, lazy handles. `get_history`, `get_entries`,
`get_events`, `get_generations`, `get_generation_entries` and `get_message_queue` support bounded
page reads. Queue positions below `get_queue_head()` have been consumed; the queue log remains
addressable. Small config/state getters expose the owning handle's working state. Each session
has one live owner; opening it again returns `AlreadyOpen`. Dropping that handle releases ownership.
Other tasks use `SessionSender` and read-only list handles. There are no revision checks or
cross-process ownership leases.

`create_sender()` returns a durable input handle usable during model/tool execution. Sending
atomically commits the message, queue reference and `MessageQueued` event without interrupting
execution. The runner consumes a fixed queue prefix at a stable boundary. Suspended sessions
require explicit `resume()` before consuming more input.

Ordinary events and state changes commit before the observer is notified. Stream events are
notified immediately, then batched with their history rows and call observations, without rewriting
the session header. `SessionState::Suspended` references
the final `Finished` event; use `get_event` to inspect the stored outcome. History is a permanent
record of facts; active generation is the model's context projection.

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
since preparation. Old history and sealed generations remain readable. Automatic summarization and cutover use the separate [compaction](compaction.md) workflow.
Garbage collection is not implemented.

Restarting restores committed data and phase. A session interrupted by a crash during model/tool
I/O remains active and returns `SessionError::Busy`; automatic recovery/reexecution of external
effects is not implemented. Graceful interruption remains handled by the [executor](executor.md).

Messages have creation timestamps on `Entry`; event timestamps live on `HistoryRecord`, preserving
receipt time across batch writes. Both also carry an optional originating `model_call_id`.
`get_model_calls()` pages logical model calls, and `get_model_call(id)` retrieves their shared
usage, times and status. See [call statistics](statistics.md). Message metadata remains application-owned.

Use a [HistoryReader](history.md) for indexed message-type/time filters, full-text search and
selective expansion, including across compaction.

## Session control

`create_handle()` returns a cloneable `SessionHandle` with `enqueue_message(message)` and
`interrupt()`. Use it while the executor holds `&mut Session`. `Session::interrupt()` exposes
the same intention directly when the session is available. Interruption returns true when a run
is registered (including repeated requests), false when idle, suspended without a runner, or after
the runner has exited. Acknowledgement is not completion: await `executor::run` to finish cleanup.

A request targets only the current run. No interrupt is queued for a future run, and a new run gets
fresh execution cancellation state. Registration is removed on return, error, or future drop.
Dropping a future still leaves active persisted work unreconciled; it is not graceful shutdown.
Handles belong to a live Session owner. After dropping/loading that owner, create a new control
handle; old handles cannot interrupt its replacement. SessionSender remains available for durable
input alone, including while the owner is absent. Interrupt intentions are not persisted.
