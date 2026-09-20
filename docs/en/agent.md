# Agent

`agent` executes model and tool effects selected by the [Session](session.md) state machine.
The caller supplies `ModelCaller` (implemented by `Client`), `ToolExecutor` and the async runtime.
Session owns state, configuration and history; [storage](storage.md) owns persistence and caching.

```text
Idle -- queued input / resume --> Ready
                                   |
                         check limit; consume input
                         build request from active
                                   |
                                   v
                              CallingModel
                             /            \
                      tool calls        final answer
                          |                  |
                          v                  |
                    ExecutingTools           |
                          |                  |
                    settle whole batch       |
                          +---------+--------+
                                    |
                             stable boundary
                                    |
                                  Ready
                                 /     \
                         more work     no work
                             |            |
                        CallingModel     Idle

interrupt / failure / limit / unknown outcome --> Suspended
Suspended -- explicit resume() --> Ready
```

Call `run(model_caller, &mut session, executor, control, observer)`. Model selection, request settings,
limits and serial/parallel tool mode live in `SessionConfig`. Requests follow `config.stream`.
Stream events reach the observer immediately, then are persisted in batches at 100 ms, 64 events,
or 256 KiB of encoded events, whichever comes first. The timer also runs while waiting for the
next chunk. One oversized event is accepted and triggers an immediate batch commit. All other
events are committed before notification. Completion, cancellation and stream errors flush the
remaining events before recording the resulting state. Storage errors are returned to the caller.

History exposes committed records only; stream notifications do not promise persistence.
Concurrent queued input may be recorded before an earlier, still-buffered stream event. History
sequence numbers describe recording order, while each stream's own event order is preserved.
Notifications are not exactly-once across crashes; use history when replaying earlier records.

Interruption replay remains defined by the protocol. Incomplete text and transparent reasoning
can be replayed; incomplete tools and opaque reasoning are omitted. Session keeps the full partial
record as an event and appends the protocol's paired replay fragment. Stream failures keep their
diagnostics without accepting partial context. Started tools settle before normal cancellation
returns. Unknown external effects require reconciliation before `resume()`.

Signal `RunControl::cancel()` and await the runner. Dropping its future, or a storage failure during
execution, can leave an active persisted state. The runner refuses to repeat that work automatically.
External black-box checks live in `wish-test`; no Rust internal tests are added.

The application owns OS signal handling. For graceful termination:

```text
SIGINT / SIGTERM
       |
stop accepting new work
       |
cancel runs and await their completion (including tool cleanup)
       |
Storage::shutdown() -> check result
       |
exit
```

Do not abort/drop the run future to implement graceful shutdown. SIGKILL and power loss cannot
run this sequence and may lose the last batch. A tool executor must cooperate with cancellation
for the run to finish; core does not install process-global signal handlers.
