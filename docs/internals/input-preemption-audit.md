# Input preemption audit

Status: implemented event-driven input preemption; original audit findings below are retained
as the pre-change baseline. The executable acceptance tests are `test_preemption_*` in
`wish-test/tests/test_wish_server.py`.
The disposable-server probe is `wish-test/tools/probe_input_preemption.py`.

## State machines

The session has one executor owner. Durable transitions are:

```
Idle / explicitly resumed Suspended
  -> Ready (consume queue, construct request)
  -> CallingModel
       -> Ready                         final text
       -> ExecutingTools                accepted tool batch
            -> Ready                    every call has a known outcome
            -> Suspended                unknown external effects
  -> Idle                               no more input or model work

Ready <-> Compacting                    cutover at a stable boundary
interrupt / rejected response / failure -> Suspended
```

`SessionSender` can append queue entries while the executor owns the session, but does not
change its phase or session header. The executor consumes queued entries in `start_turn`,
not while it is inside a model request or tool batch. A model response containing tool calls
enters `ExecutingTools` directly, even if new input arrived during that model request.

The scheduler's mutex prevents two model loops from owning the session. Its epoch invalidates
scheduled runs after interruption. A normal input schedules a waiter, not a second concurrent
model loop. Explicit interrupt currently schedules a new waiter to consume remaining inputs;
it is not the same operation as clearing the queue or killing detached tasks.

Shell process lifetime is a separate machine: registered/running -> exited/killed/unknown,
with foreground/background ownership orthogonal to process status. Backgrounding releases the
shell tool's wait without restarting the process. The tool returns a running-task handle;
completion later arrives as queued Developer input. Already-backgrounded tasks survive ordinary
run cancellation, but service shutdown terminates managed processes.

## Reproduced gaps (before implementation)

The probe uses real temporary servers and a local mock model; no production sessions change.

| Scenario | Observed result |
| --- | --- |
| Web-style input, shell already registered, `background_shell: true` | Task becomes background; new input consumed; model reaches Idle while old task runs |
| Input appended through `/messages` while shell runs | ExecutingTools, queue_count=1, task still foreground |
| Web-style input during a gated model call, before any task exists | After model returns: ExecutingTools, queue_count=1, newly started task still foreground |

The latter is deterministic: the mock response is released only after the second input is
committed and the test verifies that the task registry was empty at submission time.

Cause: `submit_input` enumerates existing shell tasks once. Queued-input notifications are
currently watched while waiting for a standby summary, not while executing tools.
`/messages`, group delivery and future tasks do not pass through that one-time detach sweep.

## Other constraints found in source

- Serial mode awaits each call before starting the next. Detaching one call must not silently
  convert dependent remaining serial work into concurrent side effects.
- Parallel mode uses `join_all`; even completed results are not committed until the batch
  returns. One non-detachable tool can block consumption of new input.
- Direct tools (fetch/search/question/session/etc.) are Rust futures, not shell tasks. A shell
  background command does not transfer ownership of these futures.
- Accepted calls must each have a paired result before another model request. Dropping futures
  and moving the phase to Ready would violate tool pairing and crash settlement.
- The run's child cancellation control is cancelled on exit. Detached generic work cannot
  inherit a lifetime that ends as soon as the responsive model run ends.
- `ask_user` must retain or explicitly settle its form, not silently lose it when yielding.
- Shell hard timeouts currently live in the foreground wait; returning from that wait after
  detachment removes that timer. A durable task supervisor must retain an absolute deadline.
- Completion-notification claiming is currently in-memory. A durable result and its notification
  need a crash-safe handoff to avoid lost or duplicate completions after restart.
- On restart, recorded running tasks become interrupted; unconfirmed tool effects become
  Unknown. Neither case authorizes automatically repeating the original side effect.
- The existing tools-definition observer deduplicates only within one model wrapper instance;
  making that history event session-initial-only is a separate persistence fix, not a scheduler
  shortcut or a frontend filter.

## Implementation contract

1. Input arrival is a session/executor concern, not a frontend flag or an HTTP-route side effect.
   Subscribe before inspecting durable pending input; recheck at entry to the tool phase and
   between serial calls. This handles both old pending input and arrival/spawn races.
2. Model requests already in flight finish normally unless the user explicitly interrupts.
   New input must not cause accepted calls to disappear or be falsely described as completed.
3. A running tool may yield only after a background owner has accepted its execution, cancellation
   policy and completion delivery. Persist a task handle and a paired "continuing in background"
   result before the model consumes the newly queued message.
4. Preserve serial ordering. Unstarted serial calls need an explicit deferred/not-executed result
   or a dependency-preserving background batch; they must never start concurrently by accident.
5. Settle parallel outcomes incrementally while retaining final call-order context pairing.
6. User input and group messages need a clear priority over background notifications. A completion
   wakes idle work but should not create an uncontrolled cascade of detaching unrelated tools.
7. Background completion is durable, linked to the original call/task, delivered once, and never
   represented as a second result for an already-paired tool call. Record final edits there too.
8. Separate stop-model, kill-task and shutdown. Preserve hard deadlines and mark crash uncertainty;
   do not automatically replay unknown operations.
9. Maintain a bounded task registry and clean up listeners after completion, cancellation,
   session deletion and shutdown. A new input must not create unlimited orphaned futures.

## Required acceptance coverage

- Input before/during/after model response, task registration and task completion.
- Pending input already present when entering tool phase; bursts of input and cancelled queue items.
- All ingress paths: `/input`, `/messages`, groups, queue controls and background completion.
- Serial and parallel batches; mixed shell/direct tools; partial success and unknown effects.
- Direct `ask_user` with an open form and late answer; tools carrying image input or edit metadata.
- Interrupt simultaneous with detach/completion; explicit task kill; retained timeout after detach.
- Shutdown/crash/restart at transfer and notification boundaries, with no duplicate side effects.
- Frontend queued/foreground/background/finished states agree with durable records after reconnect.

## Implemented behavior

- `run.background_tools_on_input` is snapshotted per session. New application defaults enable it;
  legacy session configs without the field remain disabled. Changing it does not recall work
  already transferred to the background. The legacy frontend `background_shell` flag is ignored.
- Tool entry subscribes to queue arrivals **before** checking pending durable input. After that,
  only committed enqueue events wake it; there is no periodic timer or condition polling.
  `/input`, `/messages` and group delivery share the durable queue notification mechanism.
- Serial batches explicitly pair unstarted remaining calls with deferred/not-executed results.
  Parallel outcomes are settled as they finish, with final context pairing kept in call order.
- Shell work retains its native task identity and hard deadline after detachment. Direct tools
  transfer to session/service-owned workers (32 concurrent direct workers per session), whose
  cancellation lifetime does not end with the model run. Open questions remain answerable.
- Both native shell and direct workers journal transfers. Completion event and queued input
  commit atomically. On reopen unresolved direct tools report Unknown; shell notifications use
  restored terminal snapshots. Neither recovery path restarts side effects. A process/storage
  failure before a transfer marker commits cannot guarantee a completion notification; engine
  tool settlement still must not replay an uncertain effect.
- Background completions wake idle work but do not detach unrelated foreground tools. Session
  deletion and service shutdown cancel owned workers; pruning skips live workers/shell tasks.
- This does not promise an always-running model or task-level completion verification. Compact
  continuation notices are a separate change.
