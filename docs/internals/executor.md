# Executor

`executor` executes model and tool effects selected by the [Session](session.md) state machine.
The caller supplies a `model::ModelCaller` (implemented by [`client::Client`](client.md), and by the
server's provider-switching wrapper), a `tool::ToolExecutor` and the async runtime. Session owns
state, configuration and history, including the `ToolCall`s and `ToolOutcome`s the executor carries
out; [storage](storage.md) owns persistence and caching.

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
or the events list. All other events are committed before the observer is notified, the
`Finished` event that ends a run included, however it ends. Completed
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
same with a no-op boundary; the fixtures in wish-test use it.

```rust
pub trait RunBoundary: Send {
  fn apply(&mut self, session: &mut Session, control: &ExecutionControl, delivered: &mut u64,
    observe: &mut (impl FnMut(&SessionEvent) + Send))
    -> impl Future<Output = Result<BoundaryResult, SessionError>> + Send;
}
pub enum BoundaryResult { Unchanged, Changed, Interrupted }
```

`apply` runs at the top of every loop iteration while the session is stable, including the
first, so current model/tool work always finishes first. `Changed` discards any in-flight standby
summary built for the old settings. `Interrupted` finishes the run as `Interrupted`. `delivered`,
how much of the history the observer has had, lets a long boundary publish its own events before
it awaits I/O.

The server's `SelectionBoundary` applies a pending provider/model selection at this point
(server-owned: [`src/server/session/selection.rs`](../../src/server/session/selection.rs)),
including the encrypted-compaction handoff that `executor::compaction::handoff` writes (see
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
 +-- run                 run / run_with_boundary, RunBoundary, run_scoped, the action loop
 +-- control             ExecutionControl: cancellation, run_until_cancelled, CancelOnDrop
 +-- observe             deliver_new_events / record_and_deliver to the run's observer
 +-- compaction          compact (manual), cutover inside Compacting, shared helpers
 |    +-- trigger        read_usage, find_cutover / force_cutover, CutoverPlan
 |    +-- standby        standby summaries: StandbySummarizer beside a run, catch_up at cutover
 |    +-- trim           the local cutover's candidate, trimmed to the target
 |    +-- upstream       the cutover through upstream compaction
 |    +-- handoff        encrypted-compaction handoff: plan, call, new context (used by the server)
 +-- model
 |    +-- caller         ModelCaller / ModelStream / CallResponse
 |    +-- execute        one logical model call: segments, live stream events, call timing
 |    +-- continuation   output-limit continuation and usage summing
 +-- tool                ToolExecutor, serial/parallel batch execution
```

`ToolCall` / `ToolOutcome` and `TokenEstimator` / `TokenMeasurement` are persisted data and live in
`session`. The model client, its `EventStream` and the per-attempt `AttemptObserver` are the
top-level `client` module ([client](client.md)). Protocol conversion and transport are separate
modules. Model retries never repeat tool side effects.

## Control scopes

```text
           outer ExecutionControl::cancel()
                        |
                   current run
                        |
       child ExecutionControl (cancelled on drop)
                        |
          stop model reads / settle tools
                        |
          save partial response and state
```

`run` (like `compaction::compact`) runs inside `run_scoped`, on a child of the caller's execution
control. Cancelling the outer control reaches the child and wakes whatever waits on it; the child
never cancels the outer one. The outer control is owned by whoever owns the run: the server creates
one per operation and cancels it for interrupt and shutdown. Explicitly cancelling it is sticky for
all its users. Observers can interrupt synchronously before the next effect. On every exit
(including future drop), the child is cancelled, so work scoped to the run ends with it. Tool
implementations still receive `&ExecutionControl` and must cooperate with cancellation rather than
be abandoned. There is no interrupt of the session's own: the control is the one way to stop a
run.

## Automatic output continuation

`executor/model` (`execute_model`) handles `MaxOutputLengthExceeded` as a request to continue
generating. It retains
protocol-approved text/reasoning, appends it and a continuation instruction (a `User` message) to a
private request, and calls the same model again with the original tools and output settings. The
instruction never enters Session history. This works for buffered and streamed responses; direct
`Client::call` still returns the provider's original response and stop reason. Standby summaries
are made by the same `execute_model`, without an observer
([compaction](compaction.md#local-compaction)).

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
