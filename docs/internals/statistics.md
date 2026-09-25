# Call statistics

`session::statistics::ModelCallRecord` describes one logical agent model call. Client retries and
executor output continuations stay inside that call. These records do not count individual network
attempts or direct token-count API calls. Each session has a paged list, exposed by
`get_model_calls()` and `get_model_call(id)`.

| Field | Meaning |
| --- | --- |
| `id` | `ModelCallId`, session-local, never reused across runs or generation switches |
| `purpose` | `Conversation`, `CompactionSummary`, `UpstreamCompaction` or `CompactionTranslation` |
| `generation` | active generation when the call started |
| `model`, `stream` | the session's model and response mode when the call started |
| `input_entry_count` | active-generation prefix the request was built from |
| `started_at`, `first_event_at`, `finished_at` | `Timestamp` (Unix ms) |
| `elapsed_ms` | monotonic duration of the caller invocation and stream consumption |
| `status` | `Running`, then `Completed`, `Interrupted` or `Failed` |
| `usage` | the protocol's `Usage`; missing fields stay `None` |
| `last_request_input_tokens` | input usage of the last completed physical request |
| `last_request_estimated_tokens` | local estimate of that same request, when compaction is configured |
| `stop_reason` | the upstream's stop reason, when seen |

```text
Session -> ModelCallRecord: purpose, model, generation, input boundary, times, usage, status
                     ^
                     | model_call_id
                 Entry -> Message
                 HistoryRecord -> event payload
```

## Purposes

- `Conversation`: one turn's model call, including its output continuations.
- `CompactionSummary`: a standby summary ([compaction](compaction.md#local-compaction)). Its
  `input_entry_count` is zero, because the prompt is built independently from a source span. The
  record is appended complete when the summary commits. `started_at` is derived as
  `finished_at - elapsed_ms`, `first_event_at` is absent, and `last_request_input_tokens` holds
  the plan's measurement, not actual usage. `stream` is recorded `false` although summary requests
  stream.
- `UpstreamCompaction`: records the full active input length and links its opaque entries to the
  call.
- `CompactionTranslation`: the provider-switch handoff
  ([compaction](compaction.md#provider-switch-handoff)). It always streams, and is recorded
  before the selection changes, so its `model` is the old provider's.

No compaction purpose supplies the usage that triggers the next compaction or calibrates
estimates; only `Conversation` calls do.

## Links

All response messages from the same conversation call share its `ModelCallId`, including accepted
interrupted replay fragments. Imported, queued, context-only and tool-result entries have no
originating model call ID. A standby summary entry and its `CompactionSummary` event carry the
summary's own call, even though a conversation call may still be running beside it when the
summary commits. Usage belongs to the call: summing message-linked copies would count it more than once.
The usage copies in response events are diagnostic payloads; use the call list for call-level
statistics.

## Times and status

`Entry.recorded_at` is its creation time. `HistoryRecord.recorded_at` is the time the fact was
recorded. An enqueued entry's timestamp and its later insertion into generation history can
therefore differ. Event payloads remain unchanged; read their timestamp and call reference from
history. Sequence numbers define recording order.

`started_at` marks the session transition starting a logical call. `first_event_at` is the first
decoded stream event, not time to first network byte or text token; it is absent for buffered
replies. `finished_at` marks completion at the runner, before the final state transaction.
`elapsed_ms` uses a monotonic clock around the caller invocation and stream processing, excluding
final acceptance. Wall-clock adjustments can make timestamps non-monotonic.

A call record is written twice: as `Running` when the call starts, and again with its outcome,
times, usage and stop reason. For conversation calls the second write shares the transaction that
accepts the call's entries and moves session state. Stream events do not update the record in
between. `Completed` means a full protocol response arrived,
even if its stop reason or invalid tool batch makes the session reject it; read session events for
acceptance decisions. A crash leaves a `Running` record until `settle_interrupted()` closes it as
`Interrupted` or `Failed` ([session](session.md#crash-settlement)). Never infer success from a
`Running` record.

Cumulative stream usage replaces the previous reading. Missing counts stay `None`; observed usage
survives interruption, stream errors and rejected responses. Records keep the protocol's `Usage`
as reported. They do not reinterpret billing or estimate missing tokens.

For output continuation, usage sums the distinct requests while keeping a total unknown when any
request omits it ([executor](executor.md#automatic-output-continuation)). `input_entry_count`
describes the initial Session request. `last_request_input_tokens` is not summed: it is the last
completed physical request's input usage, `None` when that request omitted it or none completed.
If a later continuation fails or is interrupted, the previous completed request's reading remains.
`last_request_estimated_tokens` pairs with it to calibrate fallback estimates
([compaction](compaction.md#counting)).

## Server-level statistics

Server-owned. After each run, and before a provider switch, the server copies the session's call
records into its management database's `calls` table, tagged with the session's provider
(`src/server/management.rs`). The usage endpoints aggregate that table
([API](../api.md#usage-statistics)). Separately, a `StreamObserver` on every provider client
samples visible streamed output once per second per attempt into `stream_samples`
(`src/server/sampling.rs`). Tokens there are estimated at 4 bytes each; these samples are
throughput observations, not billing usage.
