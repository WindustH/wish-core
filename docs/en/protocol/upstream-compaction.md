# Upstream compaction

`src/protocol/upstream_compaction/` is the intermediate layer for asking a service to stand in for a
conversation that grew too long: it receives the history as our own messages and converts it into
the form the upstream API accepts, and it receives what the service handed back and converts it into
the items the next call starts from. Those items carry the service's own opaque payload, which
travels back untouched from then on. Nothing here reads it, and what becomes of the messages it
replaced is the caller's decision.

The protocol kinds are named by `UpstreamCompactionProtocol` when a client is built - `OpenAiResponses`
for the suffix call, `OpenAiResponsesStreamed` for the deployment that compacts on its own stream -
and a pairing this wire cannot serve is refused at build time. The responses wire is the only wire
with this call, and it takes it in two forms. The platform API
has an endpoint of its own, told what the call is by the path it goes to. The Codex deployment takes
it on the streamed call it always serves, told by a `{"type": "compaction_trigger"}` item appended
to `input[]` and by the purpose the call declares beside the body. Either way the reply has to carry
exactly one `compaction` item: a reply without one, or with more than one, fails rather than reading
as an empty compaction.

```
caller                     compaction                          service
────────────────────────────────────────────────────────────────────────────────────
UpstreamCompactionRequest ──render()──▶ request/openai_responses.rs ──▶ JSON body ──▶ HTTP
   platform                     {model, input[]}                    ──▶  POST /responses/compact
   Codex                        {model, store:false, stream:true,   ──▶  POST /responses
                                 input[] + compaction_trigger}
UpstreamCompaction        ◀──decode()── response/openai_responses.rs ◀── JSON body ◀── HTTP
                  ◀──feed()──── model_use/stream/<dialect>.rs ◀── records ◀── SSE
```

The compacted history then travels as one message in the conversation, `Message::UpstreamCompaction`, and
every wire without this call refuses the conversation rather than quietly sending the rest of it.
