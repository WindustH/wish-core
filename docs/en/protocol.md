# Protocol

`src/protocol/` is the intermediate layer as a whole: it receives a request in our intermediate
representation and the outbound configuration it is aimed at, and converts them into one HTTP call -
method, url, headers and body - and it receives what an upstream answered, an HTTP body or the
records of a stream, and converts them into the corresponding intermediate representation. The parts
it is made of have their own page: [model-use](protocol/model-use.md) for what is said about the
model, [upstream-compaction](protocol/upstream-compaction.md) for the call that asks a service to
stand in for a conversation, [account-state](protocol/account-state.md) and
[model-list](protocol/model-list.md) for what a service says about the account and its models, and
[outbound](protocol/outbound.md) for the target, the auth plan and the material every one of them
is reached with.

Each feature names the protocols it serves with its own enum - `ModelUseProtocol`,
`UpstreamCompactionProtocol`, `AccountStateProtocol`, `ModelListProtocol`, and `AuthProtocol` on
the outbound - and an ask a protocol does not carry is `Error::Unsupported`, never a stringly
refusal.

Every module in it is a converter - one thing in, one thing out - and each function this crate
serves is a composition of a few of them.

```
 what each module converts
 ─────────────────────────

 model_use/request/<dialect>     Request ──────────────────▶ a body, in a Draft
 model_use/response/<dialect>    a reply body ─────────────▶ Response
 model_use/stream/<dialect>      wire records ─▶ StreamEvent ─▶ StreamAccumulator ─▶ Response
 upstream_compaction              UpstreamCompactionRequest ─▶ a body;  a reply ─▶ UpstreamCompaction
 account_state/<service>          a body, or a reply head ──▶ AccountState
 model_list/<dialect>             one page ─────────────────▶ ModelCatalog
 outbound                         a Draft × an Outbound × Credentials ─▶ Call
 outbound/oauth                   a credential ──────────────▶ a new Tokens pair
 outbound/sigv4                   AWS material ──────────────▶ three signed headers
 wire                             the container: Call, Reply, ReplyStream
```

```
 how they compose
 ────────────────

 a conversation call                        bound by client.rs
   Request  ─▶ request/<dialect> ─▶ a Draft ──┐
   Outbound × Credentials ─▶ dispatch ────────┼─▶ Call ─▶ transport ─▶ HTTP
                                              │
   Response ◀─ response/<dialect> ◀─ body  ───┤◀─ Reply          the buffered lane
   Response ◀─ stream/<dialect> ◀─ records ───┘◀─ ReplyStream  the streamed lane

 an account read                            bound by account_state/fetch.rs
   the entry's Outbound × Credentials ─▶ dispatch ─▶ Call ─▶ transport ─▶ HTTP
   AccountState ◀─ account_state/<service> ◀─ the body, or the reply head

 a model list                               bound by model_list/fetch.rs
   the entry's Outbound × Credentials ─▶ dispatch ─▶ Call ─▶ transport ─▶ HTTP
   ModelCatalog ◀─ model_list/<dialect> ◀─ one page

 a token refresh                            when to refresh is the caller's call
   a credential ─▶ outbound/oauth ─▶ a new Tokens pair
```

The two lanes of a conversation call share the request and the reply head, and differ only in what
reads the answer. A compaction rides the same lane a conversation does: its body is rendered by
the upstream-compaction request renderer and sent through the same dispatch, and what comes back
is decoded as one more reply.
An account reading often needs no read of its own - a served call reports one in its reply head.

Every crossing to and from the network goes through the same container: `Call`, `Reply` and
`ReplyStream` (`wire.rs`) are the only values that pass between a converter and a transport, so no
dialect knows HTTP and no transport knows a body. The binders hold both ends - `client.rs` for the
conversation wire, which is also why the retry policy lives there: a `2xx` body is decoded, a
non-`2xx` one is classified into an `Error`, and whether that becomes another attempt is its call -
and each read's `fetch.rs` for its own round trip, one attempt that carries no policy of its own.
The conversation wire's binder has its own page: [client](client.md).
