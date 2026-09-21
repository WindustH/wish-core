# Compaction

Enable with `SessionConfig.compaction = Some(CompactionConfig { trigger_tokens, target_tokens,
segment_tokens, estimator: Default::default() })`. Budgets are input tokens, including fixed
instructions and tools; require `0 < target_tokens < trigger_tokens` and `segment_tokens > 0`.
Disabled by default. Applications choose budgets for their model's context window and output reserve.

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

Leading System/Developer messages are fixed and preserved verbatim. Tool schemas also remain.
If no protocol-valid remaining context fits alongside that prefix, compaction fails and preserves
the old active generation. An upstream rejection cannot be handled by retrying an unchanged
context. There is no arbitrary compaction retry count or summary repetition detector.

`executor::compaction::compact` requests manual cutover and returns a RunOutcome. Both Session
interruption and ExecutionControl cancellation are supported. Cancellation aborts pending counting
or generation, keeps already committed standby summaries and avoids switching an unvalidated
candidate. A persisted `Compacting` state after an ungraceful process exit remains Busy; automatic
recovery of interrupted I/O is not implemented.
