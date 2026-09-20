# Call statistics

`session::statistics::ModelCallRecord` describes one logical agent model call. Client retries remain inside
that call; these records do not count individual network attempts or direct token-count API calls.
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

Legacy Entry/history JSON loads missing timestamps and call IDs as `None`. Loading an older
session creates its empty call list transactionally. Historical calls and times are not invented.
