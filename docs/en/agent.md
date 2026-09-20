# Agent

`agent` executes model and tool effects selected by the [Session](session.md) state machine.
The caller supplies `Model` (including `Client`), `ToolExecutor` and the async runtime.
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

Call `run(model, &mut session, executor, control, observer)`. Model selection, request settings,
limits and serial/parallel tool mode live in `SessionConfig`. Requests follow `config.stream`.
The observer receives committed events recorded since that invocation started. Earlier events
remain accessible through paged history. Event notifications are not exactly-once deliveries:
a crash can occur between commit and notification; use history sequence numbers when replaying.

Interruption replay remains defined by the protocol. Incomplete text and transparent reasoning
can be replayed; incomplete tools and opaque reasoning are omitted. Session keeps the full partial
record as an event and appends the protocol's paired replay fragment. Stream failures keep their
diagnostics without accepting partial context. Started tools settle before normal cancellation
returns. Unknown external effects require reconciliation before `resume()`.

Signal `RunControl::cancel()` and await the runner. Dropping its future, or a storage failure during
execution, can leave an active persisted state. The runner refuses to repeat that work automatically.
External black-box checks live in `wish-test`; no Rust internal tests are added.
