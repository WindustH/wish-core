# Executor & Compaction

`src/executor/` drives the agent loop across model calls, tool batches, output continuations, run boundaries, and context compaction via two abstract traits: `ModelCaller` and `ToolExecutor`.

```text
Idle ──(queued input / resume)──▶ Ready
                                    │
                            consume pending queue
                                    │
                                    ▼
                              CallingModel ──(tool calls)──▶ ExecutingTools
                                    │                              │
                                (final)                      settle batch
                                    └──────────────┬───────────────┘
                                                   ▼
                                            stable boundary ◀──▶ Compacting
                                                   │
                                      ┌────────────┴────────────┐
                                  more work                  no work
                                      ▼                         ▼
                                 CallingModel                 Idle

interrupt / failure / rejected response / ToolOutcomeUnknown ──▶ Suspended
```

## Run loop, boundaries, and cancellation

`executor::run_with_boundary` loops without a turn cap until the run completes, suspends, or runs out of work:

- **Boundaries**: At the start of each stable iteration, `RunBoundary::apply` runs pending updates. The server's `SelectionBoundary` applies mid-run provider/model switches here, discarding stale standby summaries and executing encrypted-compaction handoffs before the next model turn.
- **Cancellation**: `run_scoped` creates a child `ExecutionControl` with `CancelOnDrop`. Outer cancellation wakes active model streams and propagates into running tools, so tool implementations must honor `&ExecutionControl` for clean shutdown.
- **Tool batches**: `ToolMode::Parallel` runs up to 8 tools concurrently (at most 64 calls per response) and commits outcomes in call order; `ToolMode::Serial` stops on the first `Unknown` outcome and cancels remaining calls. Any `Unknown` outcome suspends the run as `ToolOutcomeUnknown`.
- **Context overflow recovery**: When compaction is configured and a call fails with `ContextLengthExceeded`, the executor records the failure, compacts with `CompactionReason::ContextRejected`, and resumes automatically if a new generation succeeds.
- **Event ordering**: Streamed text/reasoning deltas notify observers immediately without hitting SQLite; all durable transitions, responses, and `Finished` events commit before notifying observers.

## Output-limit continuation

When a model stops with `MaxOutputLengthExceeded`, `executor/model/execute.rs` continues inside the same logical `ModelCallRecord`:

- Eligible completed text and reasoning are retained using the protocol's output-limit replay rules (discarding partial tool calls and bound signatures).
- A private in-memory `User` continuation prompt asks the model to continue without entering session history.
- Continuation stops after **8 segments** or **262,144 accumulated output tokens**, whichever comes first. Usage totals sum across hops while `last_request_input_tokens` tracks only the last completed physical request.

## Compaction (`executor/compaction/`)

Compaction runs when `SessionConfig.compaction` is present (`0 < target_tokens < trigger_tokens`, `segment_tokens > 0`).

- **Triggers**: Automatic cutover checks `last_request_input_tokens` from the latest `Completed` `Conversation` call on the active generation matching the configured model. Overflow (`ContextRejected`) and manual calls (`POST /api/sessions/{id}/compact`) use the same cutover pipeline.
- **Sizing**: Candidate requests are measured via `ModelCaller::count_tokens` when supported, or via `TokenEstimator` (`4.0` bytes/token, `2048` tokens/image) calibrated by the actual-to-estimated ratio of the last qualifying conversation call.

### Local standby summarization

When upstream compaction isn't available, `StandbySummarizer` stays ahead of the context ceiling incrementally:

1. Between turns below `trigger_tokens`, whenever at least `segment_tokens` of closed turns/tool-batches accumulate after `compaction_cursor` (excluding the latest tail), the executor launches a background streamed summary (`Purpose::CompactionSummary`) concurrent with the next model or tool step.
2. Each summary request repeats the active conversation prefix and tool schemas, preserving cache reuse where supported, but sets tool selection to `None` on wires that support disabling calls. Anthropic Messages supports `none`. Bedrock Converse does not, so summary-only copies omit tools and project historical tool calls/results into ordinary text; the stored conversation is unchanged. A tool response is never executed or committed as a summary and produces an explicit summary error. Only non-empty plain text summaries commit into `standby`; preparation yields cleanly if new user input arrives while it is running. Context handoff calls use the same tool constraint.
3. At cutover (`Compacting`), remaining spans summarize in batches of two, merge with the raw tail, validate against the active request renderer, trim the oldest complete non-fixed units if still over `target_tokens`, and promote `standby` to `active`.

### Upstream compaction and handoffs

- **Upstream compaction**: When `ModelCaller::supports_upstream_compaction()` is true (OpenAI Responses `/compact` or Codex streamed `compaction_trigger`), cutover keeps the fixed prefix (`System` + `Developer { fixed: true }` or legacy records), inserts returned `UpstreamCompaction` items, and appends a non-fixed `Developer` notice quoting the latest user turn.
- **Provider-switch handoff**: Encrypted `UpstreamCompaction` items tag their originating `provider` in metadata. Switching to a provider that cannot read them runs one streamed `CompactionTranslation` call on the old provider (or inserts a `translated: false` placeholder if unavailable), storing a readable `handoff` in metadata so foreign wires receive a non-fixed `Developer` summary while the encrypted item stays intact for switching back.

## Ordered shutdown

On `SIGINT`/`SIGTERM` (or Windows close/shutdown):
1. `App::begin_shutdown` rejects new work and cancels all session `ExecutionControl`s.
2. HTTP stops serving and active runs/samplers drain.
3. `ShellTool::shutdown()` force-kills and reaps remaining shell subprocesses.
4. `Storage::shutdown()` drains pending writes, checkpoints WAL (`FULL`), and closes SQLite connections.
