# Session

`session` owns configuration, metadata (any JSON value), execution state, durable input,
generations and history. [`executor`](executor.md) executes its model/tool actions;
[storage](storage.md) owns SQLite and caching.

```text
session.rs          Session definition and exports
session/
 +-- lifecycle      create / load / import / delete
 +-- config         settings and metadata
 +-- queue          SessionSender: durable input, reordering, cancellation; consumption
 +-- reader         SessionReader: the session's lists, for any task
 +-- machine/       state, next actions, tool batches and run outcomes
 +-- context/       generations, compaction commits, requests and validation
 +-- history/       message entries, events, their ordered history, its index and queries
 +-- statistics/    model call records
 +-- persistence    stored header, stored kinds and the transaction boundaries
 +-- error          session errors
```

Each area contains its types and operations. `SessionTransaction` methods are implemented in
the area that owns the operation, while `persistence` owns the shared transaction boundaries:
`Session::update` for the owner's changes, `Session::read` for its reads (rolled back, so they
change nothing) and `SessionSender::transact` for changes made beside the owner. State, messages,
events and call observations can therefore commit together.

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
session.create_sender().enqueue_message(message)?;
// Later, including in another process:
drop(session);
let session = Session::load(storage, "my-session")?;
let page = session.reader().get_history().read_page(0, 128)?;
```

`Session::new(config)` uses an in-memory SQLite database. `import_history(messages)` appends a
protocol-valid conversation to the active context at a stable boundary: a message joins it and its
history as `Imported`, and one that was a summary or other context-only entry where it came from
(`Summary`, `Context`) keeps that origin and joins the context only, with later summaries starting
after it. The server imports a new session's initial messages this way, a fork's included.
`delete()` removes the session's whole storage namespace, including its history index. Session
requests always use `ToolChoice::Auto`. `RunOptions` only controls tool execution mode
(`ToolMode::Serial` by default, or `Parallel`); there is no maximum turn count. Unknown config
fields are rejected.

`Session::reader()` is the session's `SessionReader`: a cloneable, read-only view of its lists that
never claims the owner, so it can be kept while an executor holds the session and after the owner
is gone. `get_history`, `get_entries`, `get_events`, `get_generations`,
`get_generation_entry_ids(id)`, `get_message_queue` and `get_model_calls` return lazy list handles
for bounded page reads; `get_entry`, `get_event` and `get_generation` read one value, and
`read_generation_entry_ids` a generation's whole context. The reader also answers the history
queries ([history](history.md)). The session header's own fields are the owner's:
`get_active_generation`, `get_standby_generation`, `get_state`, `get_config`, `get_metadata` and
`get_queue_head`. Each session has one live owner per `Storage`; opening it again returns
`AlreadyOpen`. Dropping that handle releases ownership. Other tasks use a `SessionSender` and a
`SessionReader`. There are no revision checks or cross-process ownership leases.

## Input queue

Only `User`, `System` and `Developer` messages can be enqueued (`SessionError::InvalidInput`
otherwise). `create_sender()` returns a `SessionSender`, a durable input handle usable during
model/tool execution and after the owner is gone. `enqueue_message` atomically commits the
message, queue reference and `MessageQueued` event without interrupting execution or changing the
phase; the run collects the input at its next stable boundary (`collect_inputs` moves an idle
session to `Ready`). The runner consumes the whole pending queue at a stable boundary, as one
`InputsConsumed` event. Queue positions below `get_queue_head()` have been consumed; the queue log
remains addressable. Suspended sessions require explicit `resume()` before consuming more input.

Pending input can be edited until it is consumed, including while a model or tool runs:

| Operation | Effect | Event |
| --- | --- | --- |
| `move_queued_input(entry, before)` | moves a pending entry before another pending entry, or to the end with `None` | `InputMoved` |
| `cancel_queued_input(entry)` | removes a pending entry from the queue; the entry itself stays stored | `InputCancelled` |

Both are on `SessionSender`. Both run in one transaction with enqueue and consumption, and return
`InvalidEntry` for an entry that is not pending.

A sender never changes the session header. The owner keeps the header in memory and saves it whole
with each change it makes, so a change a sender made to it would be overwritten; senders only
append to and edit the session's lists, and `SessionSender::transact` refuses, and rolls back, a
transaction that changed the header.

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
separate [compaction](compaction.md) workflow.

Every generation list is the session's own `{key}/lists/{n}`. Sealed generations are never
garbage-collected; only `delete()` removes them. A standby's list is different: when the standby is
given a new one without being activated - prepared again, emptied, or replaced by a cutover's
trimmed context - the old list is deleted, since nothing reads it once no generation names it.

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
returns an error (server-owned: [`src/server/session.rs`](../../src/server/session.rs),
[`src/server/session/run.rs`](../../src/server/session/run.rs)).
Graceful interruption is handled by the [executor](executor.md). `resume()` does not check that
unknown tool outcomes were reconciled; it only requires a stable state.

## Timestamps and calls

Messages have creation timestamps on `Entry`; event timestamps live on `HistoryRecord`. Both also
carry an optional originating `model_call_id`. The reader's `get_model_calls()` pages logical model
calls ([statistics](statistics.md)). Message metadata is application-owned.

Use the [reader's history queries](history.md) for indexed message-type/time filters, full-text
search and selective expansion, including across compaction.

## Running beside the owner

A run is interrupted through the `ExecutionControl` its caller gave it; there is no session-level
interrupt ([executor](executor.md#control-scopes)). Dropping a run's future leaves active persisted
work unreconciled; it is not graceful shutdown.

`SessionSender::build_context_snapshot()` reads committed active context in one storage
transaction, including while the executor owns the session. An unfinished tool batch
and its assistant turn are excluded. The returned request does not include queued input
and does not mutate session state. The server's BTW endpoint `POST /api/sessions/{id}/ask` is built on
it (server-owned: [`src/server/http/ask.rs`](../../src/server/http/ask.rs),
[API](../api.md#side-questions)). It keeps the snapshot's tools, tool choice, reasoning and cache
unchanged so the question reuses the session's prompt cache, appends at most 32 earlier exchanges
(128 000 bytes) and the question, and persists nothing. The BTW instruction (answer, do not call
tools) is the first text of the first BTW question, so later asks in the same side conversation
repeat it unchanged too.

Configuration changes during a run go through a run boundary; see
[executor](executor.md#run-boundaries).
