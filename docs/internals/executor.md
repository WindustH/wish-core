# Executor

`executor` executes model and tool effects selected by the [Session](session.md) state machine.
The caller supplies a `model::ModelCaller` (implemented by `model::Client`, and by the server's
provider-switching wrapper), a `tool::ToolExecutor` and the async runtime. Session owns state,
configuration and history; [storage](storage.md) owns persistence and caching.

```text
Idle -- queued input / resume --> Ready
                                   |
                               consume input
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
                                  Ready  <---> Compacting (cutover only)
                                 /     \
                         more work     no work
                             |            |
                        CallingModel     Idle

interrupt / failure / rejected response / unknown tool outcome --> Suspended
Suspended -- explicit resume() --> Ready
```

## Run

Call `run(model_caller, &mut session, tool_executor, control, observe)`. Model selection, request
settings and serial/parallel tool mode live in `SessionConfig`. Requests follow `config.stream` and
always use automatic tool selection. There is no turn cap: the runner continues through tool
rounds until the model finishes, cancellation is requested, or execution fails or suspends. A
response whose stop reason is neither `Stop` nor `ToolUse` suspends as `ModelStopped`. A malformed
tool batch is recorded as `ResponseRejected` and suspends as `Failed`.

Stream events reach the observer immediately and are live-only: they are never written to history
or the events list. All other events are committed before the observer is notified. Completed
responses, interruption fragments and call observations are committed when the call ends. Storage
errors are returned to the caller. History exposes committed records only, and notifications are
not exactly-once across crashes; use history when replaying earlier records.

Interruption replay is defined by the protocol. Incomplete text and transparent reasoning can be
replayed; incomplete tools and opaque reasoning are omitted
([model-use](protocol/model-use.md#interrupted-streams)). Session keeps the full partial record as
a `ResponseInterrupted` event and appends the protocol's paired replay fragment. Stream failures
keep their diagnostics without accepting partial context. Started tools settle before normal
cancellation returns. `resume()` after `ToolOutcomeUnknown` is not blocked, so reconciling unknown
external effects is the resumer's job.

Tool batches run in `config.run.tools` mode:

- `Serial`: calls run in order. After an `Unknown` outcome, the remaining calls are `Cancelled`.
- `Parallel`: every registered call is marked started, then all run concurrently. Outcomes are
  recorded in call order once all finish.

Calls whose name is not in `config.tools` fail with `unknown tool` without starting. Any `Unknown`
outcome ends the run as `ToolOutcomeUnknown` after the batch's results are stored.

When compaction is configured and a model call is rejected for context length (stop reason
`ContextLengthExceeded` or `Error::is_context_length_exceeded`), the run first records the failure
(`Finished(Failed)`, so the session is Suspended). It then compacts with reason `ContextRejected`.
If that produced a new generation, it calls `resume()` and continues; otherwise it returns the
failure.

## Run boundaries

`run_with_boundary(..., boundary)` applies pending configuration only at stable points. `run` is the
same with a no-op boundary.

```rust
pub trait RunBoundary: Send {
  fn apply(&mut self, session: &mut Session, control: &ExecutionControl, cursor: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send))
    -> impl Future<Output = Result<BoundaryResult, SessionError>> + Send;
}
pub enum BoundaryResult { Unchanged, Changed, Interrupted }
```

`apply` runs at the top of every loop iteration while the session is stable, including the
first, so current model/tool work always finishes first. `Changed` discards any in-flight standby
summary built for the old settings. `Interrupted` finishes the run as `Interrupted`. The cursor
lets a long boundary publish its own events before it awaits I/O.

The server's `SelectionBoundary` applies a pending provider/model selection at this point,
including the encrypted-compaction handoff (server-owned:
[`src/server/session/selection.rs`](../../src/server/session/selection.rs); see
[compaction](compaction.md#provider-switch-handoff)).

## Shutdown

Do not abort or drop the run future to shut down: that leaves an active persisted state, and the
runner refuses to repeat that work automatically. SIGKILL and power loss can lose recent commits
([storage](storage.md)).

The binary installs SIGINT/SIGTERM handlers (Ctrl-C only on Windows) and shuts down in this order
(server-owned: [`src/server/mod.rs`](../../src/server/mod.rs),
[`src/server/app.rs`](../../src/server/app.rs)):

```text
SIGINT / SIGTERM
       |
App::begin_shutdown: refuse new work, cancel the app stop token,
                     cancel every running session's ExecutionControl
       |
axum stops serving; background tasks drain (runs, notifications, samplers)
       |
ShellTool::shutdown() for every session (force-kill, reap)
       |
Storage::shutdown() -> result reported
```

A tool executor must cooperate with cancellation for the run to finish.

## Module layout

```text
executor
 +-- run                 run / run_with_boundary, RunBoundary, action dispatch and acceptance
 +-- control             ExecutionControl for executor cancellation
 +-- observe             history event delivery to the observer
 +-- compaction          standby summaries, cutover, manual compact
 |    +-- upstream       upstream compaction cutover
 |    +-- translation    encrypted-compaction handoff call (used by the server)
 +-- model
 |    +-- caller         ModelCaller / ModelStream
 |    +-- client         Client / CallResponse / EventStream / CodeAgentIdentity
 |    +-- execute        model execution and call timing
 |    +-- continuation   output-limit continuation and usage summing
 |    +-- observer       StreamObserver / StreamObserverFactory (per-attempt observation)
 |    +-- tokens         TokenEstimator / TokenMeasurement
 +-- tool                ToolCall / ToolExecutor / ToolOutcome, serial/parallel execution
```

Protocol conversion and transport are separate modules. Retry is described in
[client](client.md#retry). Model retries never repeat tool side effects.

## Control scopes

```text
SessionHandle::interrupt()          outer ExecutionControl::cancel()
              |                                 |
              +--------- current run -----------+
                              |
                 fresh child ExecutionControl
                              |
                stop model reads / settle tools
                              |
                 save partial response and state
```

`run` creates a fresh child execution control and registers the run with Session. Session interrupts
cancel only that child; they never mark the outer control cancelled. The outer control is shared
by whoever owns the run: the server creates one per run and cancels it for interrupt and shutdown.
Explicitly cancelling it is sticky for all its users. Observers can interrupt synchronously before
the next effect. On every exit (including future drop), the child scope is cancelled and the
session registration is removed. Tool implementations still receive `&ExecutionControl` and must
cooperate with cancellation rather than be abandoned.

## Automatic output continuation

`executor/model` handles `MaxOutputLengthExceeded` as a request to continue generating. It retains
protocol-approved text/reasoning, appends it and a continuation instruction (a `User` message) to a
private request, and calls the same model again with the original tools and output settings. The
instruction never enters Session history. This works for buffered and streamed responses; direct
`Client::call` still returns the provider's original response and stop reason. Standby summaries
use the same continuation ([compaction](compaction.md#local-compaction)).

Session sees one logical call and one final response. Returned message order is preserved across
segments. Stream block indices are offset across segments; intermediate output-limit Stop events
are suppressed and usage notifications reflect cumulative observed usage. Intermediate tool calls
are not executed: they must be reissued in full. Incomplete opaque reasoning is dropped according
to protocol replay rules. Buffered replies have no block completion certificates, so tool calls
and opaque reasoning are conservatively omitted when capped.

Cancellation returns an interruption containing eligible output from all segments so far. There
is no continuation count limit. Empty or repeated fragments do not stop continuation and repeated
content is not deduplicated; another output-limit response always requests continuation.
Output continuation does not compact context or promise byte-exact continuation by the model.

Usage is summed across requests, including the repeated input. A total field remains unknown if
any constituent request omitted that field (or the sum overflows). Cumulative usage events within
one request replace prior readings instead of being added. `ModelCallRecord` remains a single
logical-call record; intermediate requests do not create session generations, entries or calls.
`last_request_input_tokens` is the exception: it records only the last completed physical request
([statistics](statistics.md)).

[Compaction](compaction.md) prepares standby summaries alongside model and tool work, and switches
generations on actual usage thresholds, explicit context rejection or a manual request. The session
is `Compacting` only during cutover; tools are settled before that phase starts.

Built-in tool implementations live under `tool`; see [built-in tools](tools.md).
