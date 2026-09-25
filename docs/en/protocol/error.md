# Error

`src/protocol/error.rs` is the one vocabulary every layer above an attempt matches on: `Build` is a
request we will not render, `Unsupported` is a feature asked of a protocol that does not carry it,
`Malformed` is a payload that does not fit its wire, `Upstream` is a
failure the service reported, and `Transport` is a failure of the attempt itself - its
`TransportFailure` carries the retry judgment - while `Renewal` is a credential that had already
expired when the call was joined, so nothing was sent. Nothing is judged
twice: the transport reports the attempt and whether another one is safe, a dialect reports what the
payload said, and `client.rs` - the only place that sees all of it - names the kind and spends the
retry policy.

One call, with every place an error can leave it:

```
  the axes ──(0) an ask off them ───────────────────▶  Error::Unsupported
  a client     a feature the protocol does not carry    never worth a retry
     │
  Request  ──┐
  Endpoint ──┴─(1) render, resolve, build ───────────▶  Error::Build
                                                         we cannot render this request
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

Step 1 never leaves our side and costs no attempt - an expired credential is refused here too
(`Renewal`). Step 2 keeps the transport's phase, which is also
the retry judgment: `connect` may be replaced, and so may a stream whose head did not arrive within
the first-byte limit, because a service acknowledges a stream as soon as it admits the call. Any
other failure after the request left is not, because a service that has the call may already be
generating and billing it - a buffered reply's head only comes with the whole answer, so a late
one means it is still being generated. Step 3 lets the status say only *that* this is a failure - what it is,
the wire's envelope says, so the same `429` is a rate limit for one service and an exhausted quota
for another. Steps 4 and 5 can go wrong in two ways: the payload does not fit the wire (`Malformed`,
including a body whose framing broke, which the transport noticed but refused to call a network
failure), or the payload says the service refused (`Upstream`, with no status to blame).

A refusal about the account is not an error at all but data, in `AccountState.failure` and
`warnings`. What the caller finally gets is the last failure, unchanged.
