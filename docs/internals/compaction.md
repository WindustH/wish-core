# Compaction

Enable with `SessionConfig.compaction = Some(CompactionConfig { trigger_tokens, target_tokens,
segment_tokens, estimator: Default::default() })`. Budgets are input tokens, including fixed
instructions and tools. Validation requires `0 < target_tokens < trigger_tokens`,
`segment_tokens > 0`, and a finite, positive `estimator.bytes_per_token`. Compaction is disabled by
default; without it nothing triggers, including upstream compaction. The budgets should fit the
model's context window and output reserve. The server seeds new sessions from
`defaults.compaction` in [configuration](../configuration.md); a session's own config can change
them.

## Trigger

Cutover is considered at the top of each run-loop iteration while the session is `Ready`. The
automatic trigger reads `last_request_input_tokens` from the most recent `Completed`
`Conversation` call of the active generation made with the configured model. It fires when that
value is at least `trigger_tokens`. After a model switch or a cutover nothing triggers until such
a call completes. Missing usage is not zero. The server reports the same number as a session's
`context_tokens` status (`src/server/session.rs`).

Other reasons: `ContextRejected` after an explicit context-length rejection
([executor](executor.md#run)), and `Manual` from `executor::compaction::compact`, which the server
calls for `POST /api/sessions/{id}/compact`. `compact` requires compaction to be configured
(`InvalidCompaction` otherwise). It honors both Session interruption and `ExecutionControl`
cancellation, and returns a `RunOutcome`.

## Upstream compaction

When `ModelCaller::supports_upstream_compaction()` is true, the executor skips local standby
summarization. `Client` reports this capability when configured with `with_upstream_compaction`;
custom callers implement the capability and `compact_upstream` together.

```text
usage trigger / context rejection / manual request
                       |
       send the whole active context to upstream compact
                       |
       fixed prefix + returned opaque body + latest User message
                       |
          validate protocol and measure the new request
                       |
          atomically append a fresh active generation
```

The leading fixed prefix and the most recent `User` entry are reused verbatim, including metadata.
The prefix contains System messages and Developer messages explicitly marked `fixed: true` (or
legacy records without the field), stopping at its first other message. Only returned
`UpstreamCompaction` items form the body; echoed messages are not copied into the new context. A
reply without one fails. If there is no `User` message, that last part is omitted. Queued input
remains queued and is consumed normally by the executor. A later compaction sends the previous
opaque body as part of the full active context.

This path neither promotes nor seeds a standby generation. The standby handle stays empty; any
previously prepared local context is cleared on successful cutover. Old generations and history
remain available. The compaction cursor points after the opaque body, before the retained user
message.

The three parts are indivisible: exceeding the target reports failure instead of deleting the
opaque body or latest user message. Unsupported response structure, provider/count errors and
cancellation preserve the active generation, without falling back to local summaries. Calls have
purpose `UpstreamCompaction`; their usage and timing are recorded separately from conversation
occupancy. `UpstreamCompactionStarted`/`UpstreamCompactionCompleted` events retain the upstream
response, account reading and warnings.

## Provider-switch handoff

Server-owned: [`src/server/session/selection.rs`](../../src/server/session/selection.rs) applies a
pending provider selection at a [run boundary](executor.md#run-boundaries), and
`executor::compaction::translation` makes the call.

When the new provider cannot replay the active context's encrypted compaction item, the old
provider gets a streamed request. It contains every entry up to and including the item (normally
just the fixed prefix and the item), plus a request for a self-contained handoff. Tools, prompt
cache and reasoning are removed. The output cap is 8192 tokens, except on Codex Responses, which
rejects output caps. The reply must be 1-65 536 bytes of assistant text.

A successful handoff replaces the item with a `Developer { fixed: false }` message (metadata
`{"source":"upstream_compaction_handoff"}`) in a new active generation. All entries after the item
keep their original order and IDs. The handoff is rendered as an instruction at the start of the
new conversation, and can be included in a later local summary or upstream compaction. The call
has purpose `CompactionTranslation` and is attributed to the old provider.

If the old provider is unavailable, the context holds more than one encrypted item, or the
handoff fails, the new generation instead holds an explicit missing-context placeholder (metadata
source `upstream_compaction_handoff_unavailable`) in the same position. `CompactionTranslationFailed`
records the reason, and the selected provider then continues. Cancellation leaves the old
generation and pending selection intact.

## Local compaction

Callers without upstream compaction use incremental standby summaries:

```text
completed conversation request -> actual input usage
              |
     each run-loop iteration
              |
       +------+-----------------------------+
       | below trigger                      | at trigger / context rejection / manual
       v                                    v
 plan one closed old span             Compacting: summarize every remaining
 (awaited)                            eligible span, one by one
       |                                    |
 summarize it alongside the           standby + unprocessed active tail
 next model call or tool batch              |
       |                              validate with current protocol, measure
 append summary to standby                  |
 advance source cursor                over target? remove oldest complete unit
                                            |     (keep fixed prompt)
                                            v
                                      atomic generation switch
                                            |
                               seed only processed prefix into next standby
                               leave grafted raw tail eligible for later summary
```

After each state advance the executor plans at most one span, if none is in flight. Planning is
awaited and makes one count request (or estimate) per candidate boundary. The summary call then
runs concurrently with the next model call or tool batch, polled in the same task rather than
spawned. It is committed when it finishes, and awaited before the run returns. A config change at
a run boundary discards it. A background summary failure is recorded as `CompactionSummaryFailed`
and does not fail or suspend the run, and background planning errors are skipped silently. Inside
cutover, a summary or planning failure fails the compaction instead.

Only input covered by a completed conversation request is eligible. The latest response and tool
results remain raw. A span starts at the first unsummarized entry and ends at the first turn or
tool-batch boundary whose summary request measures at least `segment_tokens`. `segment_tokens` is
therefore a minimum, and an indivisible span is never split. Planning stops at an
`UpstreamCompaction` item.

A summary request uses the session's model, reasoning and output cap. It is always streamed, has
no tools, tool choice or prompt cache, and uses a fixed prompt that wraps the span's messages as
data. At the output cap it is continued the same way as a conversation call
([executor](executor.md#automatic-output-continuation)), and the segments' text becomes one
summary. The summary is stored as a `User` message with origin `Summary`.

Cutover first drains the plan loop: every remaining eligible span is summarized in turn, inside
`Compacting`. It then builds the candidate from standby plus the raw tail. It never summarizes the
latest raw tail, creates a handoff, or summarizes existing summaries again.
`Generation.compaction_cursor` identifies the first remaining raw entry. Removing a prefix adjusts
this position; the grafted tail stays eligible. Old generations and the complete history remain
unchanged. With no summaries, cutover trims the active context directly. If the untrimmed
candidate already fits the target, a `Usage` or `Manual` cutover changes nothing. Opaque upstream
compaction items are not converted into textual summaries.

## Counting

- Automatic cutover uses `last_request_input_tokens` from an actual completed conversation call
  (see [Trigger](#trigger)), not summed continuation usage or a prediction of the next request.
- For summary planning and candidate contexts, use `ModelCaller::count_tokens`. `Client` uses its
  explicitly configured token-count endpoint ([token-count](protocol/token-count.md)). API
  failures propagate; they do not silently become estimates. Counts describe exactly the submitted
  candidate, including instructions and tools.
- Without an endpoint, use the semantic fallback `TokenEstimator`: UTF-8 bytes per token (default
  4.0), per-message and per-block overhead, tool schemas and a configurable `image_tokens` budget
  (default 2048). Images are not counted as base64 text; metadata and display-only reasoning are
  excluded. Model-specific image tile formulas are not included.
- The latest qualifying conversation call calibrates fallback counts: its actual input usage
  divided by the estimate of that same request. This is a heuristic, not an exact tokenizer. Each
  measurement is labelled `ProviderCount`, `Estimate` or `CalibratedEstimate`; approximate targets
  are not guarantees of upstream acceptance. An explicit context rejection forces another
  replacement.

No token-count call is needed merely to decide whether a completed request reached the trigger.
Summary calls have their own `ModelCallRecord` with purpose `CompactionSummary`; their usage is
not treated as active-context occupancy. Their `input_entry_count` is zero because the request is
an independently constructed summary prompt, not a prefix of the active generation.

## Structure and failure

Only complete context units may be removed: reasoning/text/tool-call blocks and their tool-result
batch stay together. Every candidate is checked by the actual request renderer before counting
and committing. No signature is rewritten and no tool result is fabricated. A `ModelCaller` must
expose its protocol or implement `validate_request`.

Leading System messages and pinned Developer messages (`fixed: true`) are preserved verbatim until
the first unpinned message. New Developer messages default to `fixed: false` at the HTTP API; old
stored messages without a `fixed` field keep their previous pinned behavior. Tool schemas also
remain. If no protocol-valid remaining context fits alongside that prefix, compaction fails and
preserves the old active generation. A context rejection cannot be handled by retrying an
unchanged context. There is no arbitrary compaction retry count or summary repetition detector.

Cancellation aborts pending counting or generation, keeps already committed standby summaries and
avoids switching an unvalidated candidate. A persisted `Compacting` state after an ungraceful exit
is Busy until `settle_interrupted()` runs. The server does that when it opens the session
([session](session.md#crash-settlement)).
