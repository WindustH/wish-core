# Compaction

Enable with `SessionConfig.compaction = Some(CompactionConfig { trigger_tokens, target_tokens,
segment_tokens, estimator: Default::default() })`. Budgets are input tokens, including fixed
instructions and tools; require `0 < target_tokens < trigger_tokens` and `segment_tokens > 0`.
Disabled by default. Applications choose budgets for their model's context window and output reserve.

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

The leading fixed prefix and the most recent User entry are reused verbatim, including metadata.
The prefix contains System messages and Developer messages explicitly marked `fixed: true`,
stopping at its first other message. Only returned `UpstreamCompaction` items form the body; echoed messages are not copied
into the new context. If there is no User message, that last part is omitted. Queued input remains
queued and is consumed normally by the executor. A later compaction sends the previous opaque
body as part of the full active context.

This path neither promotes nor seeds a standby generation. The existing standby handle remains
empty for compatibility with session APIs; any previously prepared local context is cleared on
successful cutover. Old generations and history remain available. The compaction cursor points
after the opaque body, before the retained user message.

When a session switches from an upstream-compaction provider to a provider that cannot replay its
encrypted item, the old provider gets a streamed request containing the fixed prefix, the item,
and a request for a self-contained handoff. A successful handoff replaces the item with a
`Developer { fixed: false }` message in a new generation. All entries after the item keep their
original order and IDs. The handoff is rendered as an instruction at the start of the new
conversation, but can be included in a later local summary or upstream compaction. The handoff
request has an 8192 output-token cap where the old wire accepts one; Codex Responses omits it
because that deployment rejects output caps. The call and usage are attributed to the old provider.
If the old provider is unavailable or the handoff fails, the new generation contains an explicit
missing-context placeholder in the same position, and `CompactionTranslationFailed` records the
reason. The selected provider then continues. Cancellation leaves the old generation and pending
selection intact.

The three parts are indivisible: exceeding the target reports failure instead of deleting the
opaque body or latest user message. Unsupported response structure, provider/count errors and
cancellation preserve the active generation, without falling back to local summaries. Calls have
purpose `UpstreamCompaction`; their usage and timing are recorded separately from conversation
occupancy. Start/completion events retain the upstream response, account reading and warnings.

## Local compaction

Callers without upstream compaction continue to use incremental standby summaries:

```text
completed conversation request -> actual input usage
              |
        stable boundary
              |
       +------+-------------------------+
       | below trigger                  | at trigger / explicit context rejection
       v                                v
 summarize a closed old span     standby + unprocessed active tail
       |                                |
 append summary to standby       validate with current protocol
 advance source cursor                  |
       |                         measure assembled request
 continue active generation             |
                                 over target? remove oldest complete unit
                                        |     (keep fixed prompt)
                                        v
                                 atomic generation switch
                                        |
                           seed only processed prefix into next standby
                           leave grafted raw tail eligible for later summary
```

Standby preparation runs incrementally at stable executor boundaries, one span per pass. It does
not spawn a background worker or run a summary concurrently with a conversation call. Only input
covered by a completed conversation request is eligible. The latest response and tool results
remain raw. Turn/tool-batch boundaries can delay a span beyond `segment_tokens`; a single
indivisible span is never split to satisfy that budget.

Cutover does not force a final summary, create a handoff or summarize existing summaries again.
`Generation.compaction_cursor` identifies the first remaining raw entry. Removing a prefix adjusts
this position; the grafted tail stays eligible. Old generations and the complete history remain
unchanged. If no summary has been prepared yet, cutover trims the active context directly.
Opaque upstream compaction items are not converted into textual summaries.

## Counting

- Automatic cutover uses `last_request_input_tokens` from an actual completed conversation call,
  not summed continuation usage or a prediction of the next request. Missing usage is not zero.
- For summary planning and candidate contexts, use `ModelCaller::count_tokens`. `Client` uses its
  explicitly configured token-count endpoint. API failures propagate; they do not silently become
  estimates. Counts describe exactly the submitted candidate, including instructions and tools.
- Without an endpoint, use the semantic fallback inherited from old wish: UTF-8 bytes per token,
  message/block overhead, tool schemas and a configurable image budget (default 2048). Images are
  not counted as base64 text; metadata and display-only reasoning are excluded from replay sizing.
  Model-specific image tile formulas are not included in this implementation.
- The latest actual conversation request calibrates fallback counts using actual input usage /
  estimate of that same request. This is a heuristic, not an exact tokenizer. Each measurement is
  labelled `ProviderCount`, `Estimate` or `CalibratedEstimate`; approximate targets are not guarantees
  of upstream acceptance. An explicit context rejection forces another replacement.

No token-count call is needed merely to decide whether a completed request reached the trigger.
Summary calls have their own `ModelCallRecord` with purpose `CompactionSummary`; their usage is
not treated as active-context occupancy. Their `input_entry_count` is zero because the request is
an independently constructed summary prompt, not a prefix of the active generation.

## Structure and failure

Only complete context units may be removed: reasoning/text/tool-call blocks and their tool-result
batch stay together. Every candidate is checked by the actual request renderer before counting
and committing. No signature is rewritten and no tool result is fabricated. Custom ModelCaller
implementations must expose their protocol or implement `validate_request`.

Leading System messages and explicitly pinned (`fixed: true`) Developer messages are preserved
verbatim until the first unpinned message. New Developer messages default to `fixed: false` at the
HTTP API; old stored messages without a `fixed` field retain their previous pinned behavior.
Tool schemas also remain.
If no protocol-valid remaining context fits alongside that prefix, compaction fails and preserves
the old active generation. An upstream rejection cannot be handled by retrying an unchanged
context. There is no arbitrary compaction retry count or summary repetition detector.

`executor::compaction::compact` requests manual cutover and returns a RunOutcome. Both Session
interruption and ExecutionControl cancellation are supported. Cancellation aborts pending counting
or generation, keeps already committed standby summaries and avoids switching an unvalidated
candidate. A persisted `Compacting` state after an ungraceful process exit remains Busy; automatic
recovery of interrupted I/O is not implemented.
