# Call statistics

`session::statistics::ModelCallRecord` describes one logical agent model call. Client retries and executor output continuations remain inside
that call; these records do not count individual network attempts or direct token-count API calls.
The purpose distinguishes `Conversation`, `CompactionSummary` and `UpstreamCompaction`. Summary entries link to their summary call; its input boundary
is zero because its prompt is constructed independently from a source span. Upstream compaction
records the full active input length and links its opaque entries to that call. Neither compaction
purpose supplies the conversation usage used to trigger the next compaction.
Each session has a paged list, exposed by `get_model_calls()` and `get_model_call(id)`.

```text
Session -> ModelCallRecord: model, generation, input boundary, times, usage, status
                     ^
                     | model_call_id
                 Entry -> Message
                 HistoryRecord -> event payload
```

All response messages from the same call share its `ModelCallId`, including accepted interrupted
replay fragments. Imported, queued, context-only and tool-executor result entries have no originating model
call ID. IDs are session-local and remain unique across runs and generation switches.
Usage belongs to the call: summing message-linked copies would count it more than once. The usage
copies in response events are diagnostic payloads; use the call list for call-level statistics.

`Entry.recorded_at` is its creation time. `HistoryRecord.recorded_at` is the fact's creation or
stream-receipt time, captured before storage dispatch/buffering. An enqueued entry's timestamp and
its later insertion into generation history can therefore differ. Event payloads remain unchanged;
read their timestamp and call reference from history. Sequence numbers define recording order.

Times are Unix milliseconds wrapped in `Timestamp`. `started_at` marks the session transition
starting a logical call. `first_event_at` is the first decoded stream event, not time to first
network byte or text token; it is absent for buffered replies. `finished_at` marks completion at
the runner, before the final state transaction. `elapsed_ms` uses a monotonic clock around the
caller invocation and stream processing, including batch writes but excluding final acceptance.
Wall-clock adjustments can make timestamps non-monotonic.

A call starts as `Running`, and finishes as `Completed`, `Interrupted` or `Failed`. Completed means
a full protocol response arrived, even if its stop reason or invalid tool batch makes the session
reject it. Read session events for acceptance decisions. Each stream batch updates the call's
first-event time, latest usage and stop reason in the same transaction. End-of-call statistics,
accepted entries and session state are committed together. Dropping a runner or failing storage
can leave a Running record; its outcome is unknown and must not be inferred as successful.

Cumulative stream usage replaces the previous reading. Missing counts stay `None`; observed usage
survives interruption, stream errors and rejected responses. These records preserve the protocol's
existing Usage representation; they do not reinterpret billing, estimate missing tokens, or supply
provider-level calibration aggregates yet.

For output continuation, usage sums the distinct requests while retaining unknown totals when any
request omits a field. The input boundary describes the initial Session request; continuation
instructions and additional request-local context remain internal to the model executor.
`last_request_input_tokens` separately records the last completed physical request's input usage;
it is not summed across continuations. It is `None` when that request omitted input usage or no
request completed. If a later continuation fails or is interrupted, the previous completed
request's reading remains. This describes an actual sent request, not the size of a newly edited
context.

`last_request_estimated_tokens` stores the local estimate of that same physical request when
compaction is configured. Its ratio with actual input usage calibrates later fallback estimates;
summary calls are excluded from conversation calibration and trigger decisions.
