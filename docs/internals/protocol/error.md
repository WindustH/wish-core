# Error

`src/protocol/error.rs` is the one vocabulary every layer above an attempt matches on:

| Variant | Meaning |
| --- | --- |
| `Build(String)` | a request we will not render or dispatch |
| `Unsupported { feature, subject, reason }` | a feature asked of a client or protocol that does not carry it |
| `Malformed(String)` | a payload that does not fit its wire |
| `Upstream { status, code, message, retry_after_ms }` | a failure the service reported; `status: None` for a refusal inside a `2xx` |
| `Transport(TransportFailure)` | a failure of the attempt itself; `TransportFailure.retryable` carries the judgment |
| `Renewal { expires_at }` | the credential had already expired when the call was dispatched, so nothing was sent |

`Error` is `Serialize`/`Deserialize`, so it is stored in `RunOutcome::Failed` and
`IncompleteReason::Failed`. Nothing is judged twice. The transport reports the attempt's phase and
whether another attempt is safe. A dialect or `http_error.rs` reports what the payload said.
`dispatch` reports `Build` and `Renewal`. `Error::is_retryable` is the single retry verdict, which
the [client](../client.md#retry) spends.

One call, with every place an error can leave it:

```
  the axes ──(0) an ask off them ───────────────────▶  Error::Unsupported
  a client     a feature the protocol does not carry    never worth a retry
     │
  Request ─────────┐
  Outbound ×       ├─(1) render, resolve, dispatch ────▶  Error::Build / Error::Renewal
  Credentials ─────┘                                     we cannot render or prove this request
                     │  a Call
                     ▼
               (2) one attempt ──────────────────────▶  Error::Transport(TransportFailure)
                                                         connect, await headers, read body
                     │  a Reply or a ReplyStream
                     ▼
               (3) the reply head ───────────────────▶  Error::Upstream
                                                         the wire's envelope, retry-after included
                     │  2xx
         ┌───────────┴──────────────┐
         ▼ buffered                 ▼ streamed
   (4) response/<dialect>.rs   (5) stream/<dialect>.rs
         │                          │
         ├──────────────────────────┼────────────────▶  Error::Malformed
                                                         one payload that does not fit its wire
         │                          │
         └──────────────────────────┴────────────────▶  Error::Upstream { status: None }
                                                         the payload's own refusal inside a 2xx
```

Step 1 never leaves our side and costs no attempt; an expired credential is refused here too
(`Renewal`). Step 2 keeps the transport's phase, which is also the retry judgment. `connect` may be
replaced, and so may a stream whose head did not arrive within the first-byte limit, because a
service acknowledges a stream as soon as it admits the call. Any other failure after the request
left is not, because a service that has the call may already be generating and billing it. A
buffered reply's head only comes with the whole answer, so a late one means it is still being
generated. Step 3 lets the status say only *that* this is a failure. What it is, the wire's
envelope says, so the same `429` is a rate limit for one service and an exhausted quota for
another. Non-`2xx` bodies that are not JSON become `HTTP {status}: <text>`, truncated.

Steps 4 and 5 can go wrong in two ways: the payload does not fit the wire (`Malformed`, including a
body whose framing broke, which the transport noticed but refused to call a network failure), or
the payload says the service refused (`Upstream` with no status to blame).

`is_retryable()`: `Transport` when `retryable`, and `Upstream` with status 429 or 5xx. `Build`,
`Unsupported`, `Malformed`, `Renewal` and in-band `Upstream { status: None }` never retry, even
before a stream's first event. `retry-after` is parsed as delta-seconds or an HTTP date and capped
at one hour.

Two more predicates classify an error for the layers above:

- `needs_renewal()`: `Renewal`, a 401, or a 403 with an AWS credential code (`ExpiredToken`,
  `InvalidAccessKeyId`, `UnrecognizedClientException`).
- `is_context_length_exceeded()`: known codes (`context_length_exceeded`,
  `model_context_window_exceeded`, `prompt_too_long`) or known messages on a 400/413/in-band error.
  The executor uses it to trigger [compaction](../compaction.md) on explicit context rejection.

A refusal about the account is not an error at all but data, in `AccountState.failure` and
`warnings`. What the caller finally gets is the last failure, unchanged.
