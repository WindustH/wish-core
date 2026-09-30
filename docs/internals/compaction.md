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
`context_tokens` status (`src/server/session/status.rs`).

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

On the Codex deployment the compaction is the ordinary streamed model call, with the session's
tools, tool choice, reasoning and cache key, so it reads the conversation calls' prompt cache. The
platform `/compact` endpoint receives only the model and the history
([protocol](protocol/upstream-compaction.md)).

The leading fixed prefix and the most recent `User` entry are reused verbatim, including metadata.
The prefix contains System messages and Developer messages explicitly marked `fixed: true` (or
legacy records without the field), stopping at its first other message. Only returned
`UpstreamCompaction` items form the body; echoed messages are not copied into the new context. A
reply without one fails. If there is no `User` message, that last part is omitted. Queued input
remains queued and is consumed normally by the executor. A later compaction sends the previous
opaque body as part of the full active context.

This path neither promotes nor seeds a standby generation. The standby handle stays empty; any
previously prepared local context is cleared, its list deleted, on successful cutover. Old generations and history
remain available. The compaction cursor points after the opaque body, before the retained user
message.

The three parts are indivisible: exceeding the target reports failure instead of deleting the
opaque body or latest user message. Unsupported response structure, provider/count errors and
cancellation preserve the active generation, without falling back to local summaries. Calls have
purpose `UpstreamCompaction`; their usage and timing are recorded separately from conversation
occupancy. `UpstreamCompactionStarted`/`UpstreamCompactionCompleted` events retain the upstream
response, account reading and warnings.

## Provider-switch handoff

[`src/server/session/selection.rs`](../../src/server/session/selection.rs) (server-owned) applies a
pending provider selection at a [run boundary](executor.md#run-boundaries).
`executor::compaction::handoff` finds the items that need a handoff, makes the call and builds the
new context; the server says which items need one and whether the provider switched from can read
them, both from the item metadata below, and commits the switch.

An encrypted compaction item can be read only by the provider that made it: upstream compaction
records that provider's id in the item's metadata (`provider`). Every other provider reads the item through
its readable handoff, also kept in the metadata (`handoff: {content, translated}`). Before each
call, `SessionModel` replaces an item the provider cannot read with that handoff, as a
`Developer { fixed: false }` instruction in the item's place, which the wire then places by its
own instruction rules ([model use](protocol/model-use.md)). The item itself stays in the context,
so switching back to the provider that made it resumes from the encrypted history, with the same
prompt prefix as before.

A selection needs a handoff when the new provider cannot read an item that has none, or has only a
placeholder that the current provider can now replace. The current provider, which must be able
to read the item, then gets a streamed request. It contains every entry up to and including the
item (normally just the fixed prefix and the item), plus a request for a self-contained handoff.
Tools, tool choice, reasoning and prompt cache stay as the conversation calls send them, so the
handoff reads their cached prefix; the prompt asks for no tool calls, and a reply that calls one
fails the handoff. The output cap is 8192 tokens, except on Codex Responses, which rejects output
caps. The reply must be 1-65 536 bytes of assistant text. Switching again later reuses the stored
handoff without another call.

The handoff is attached to a copy of the item that takes the item's place in a new active
generation. All entries around it keep their original order and IDs. Like a standby summary, the
copy is context rather than conversation: it joins no history, so the web does not show it, and
`CompactionTranslationCompleted` names its entry. Local compaction keeps it with the fixed prefix
and starts summarizing after it. The call has purpose `CompactionTranslation` and is attributed to
the old provider.

If the current provider cannot read the item or is unavailable, the context holds more than one
such item, or the handoff fails, the copy carries an explicit missing-context placeholder instead
(`translated: false`). `CompactionTranslationFailed` records the reason, and the selected provider
then continues. A later selection replaces the placeholder once a provider that can read the item
is current again. Cancellation leaves the old generation and pending selection intact.

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
 (awaited)                            eligible span at once, commit in order
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
spawned. It is committed when it finishes, and awaited before the run returns. That wait never
holds back input: when input is queued while the session is `Idle` and a summary is still in
flight, the run collects it at once and keeps polling the summary beside the next model call or
tool batch. Senders signal each queued input to the owning session for this. A config change at a
run boundary discards the summary. A background summary failure is recorded as `CompactionSummaryFailed`
and does not fail or suspend the run, and background planning errors are skipped silently. Inside
cutover, a summary or planning failure fails the compaction instead.

Only input covered by a completed conversation request is eligible: the entries that the latest
completed conversation call of the active generation sent (its `input_entry_count`). The latest
response and tool results remain raw. A span starts at the first unsummarized entry and ends at
the first turn or tool-batch boundary where the span measures at least `segment_tokens`. The
measurement flattens the span's messages into one message without tools; it is never sent.
`segment_tokens` is therefore a minimum, and an indivisible span is never split. Planning stops at
an `UpstreamCompaction` item.

A summary request repeats that latest conversation call's input unchanged, with the session's
tools, tool choice, reasoning, prompt cache and output cap, so it reads the prompt cache the call
wrote. It then appends one `User` instruction. The instruction names the span by the kind and
opening words (240 characters) of its first and last visible entries, adds the occurrence number
when those words repeat, and asks for replacement context for that span only, without tool calls.
The request is always streamed. At the output cap it is continued the same way as a conversation
call ([executor](executor.md#automatic-output-continuation)), and the segments' text becomes one
summary; a tool call instead fails the summary. The summary is stored as a `User` message with
origin `Summary`.

Cutover first catches up inside `Compacting`: it plans every remaining eligible span, starts all
their summary calls at once, and commits the results in span order, since each continues the
standby where the previous one ended. A failure fails the compaction but keeps the spans committed
before it. The spans share the prefix they repeat, so they differ only in their instruction. It
then builds the candidate from standby plus the raw tail. It never summarizes the
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
not treated as active-context occupancy. Their `input_entry_count` is the number of active entries
the request repeats before its instruction.

## Structure and failure

Only complete context units may be removed: reasoning/text/tool-call blocks and their tool-result
batch stay together. Every candidate is checked by the actual request renderer before counting
and committing. No signature is rewritten and no tool result is fabricated. A `ModelCaller` must
expose its protocol or implement `validate_request`.

Leading System messages and pinned Developer messages (`fixed: true`) are preserved verbatim until
the first unpinned message. Local compaction also keeps the encrypted compaction items right after
them (see [Provider-switch handoff](#provider-switch-handoff)). New Developer messages default to `fixed: false` at the HTTP API; old
stored messages without a `fixed` field keep their previous pinned behavior. Tool schemas also
remain. If no protocol-valid remaining context fits alongside that prefix, compaction fails and
preserves the old active generation. A context rejection cannot be handled by retrying an
unchanged context. There is no arbitrary compaction retry count or summary repetition detector.

Cancellation aborts pending counting or generation, keeps already committed standby summaries and
avoids switching an unvalidated candidate. A persisted `Compacting` state after an ungraceful exit
is Busy until `settle_interrupted()` runs. The server does that when it opens the session
([session](session.md#crash-settlement)).
