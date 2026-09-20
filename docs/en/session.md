# Session

`session` owns configuration, metadata (any JSON value), execution state, durable input,
generations and history. `agent` executes its model/tool actions; [storage](storage.md) owns
SQLite and caching.

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
imports paired stable history into one. Use `create`/`load` for persistence.

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

Events and state changes commit before the observer is notified. Stream deltas append only their
own event/history rows, without rewriting the session header. `SessionState::Suspended` references
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
since preparation. Old history and sealed generations remain readable. Automatic summarization
and garbage collection are not implemented.

Restarting restores committed data and phase. A session interrupted by a crash during model/tool
I/O remains active and returns `SessionError::Busy`; automatic recovery/reexecution of external
effects is not implemented. Graceful interruption remains handled by the [agent](agent.md).
