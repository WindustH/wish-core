# Upstream compaction

`src/protocol/upstream_compaction/` is the intermediate layer for asking a service to stand in for a
conversation that grew too long. It converts the history, as our own messages, into the form the
upstream API accepts. It converts what the service handed back into the items the next call starts
from. Those items carry the service's own opaque payload, which travels back untouched from then
on. Nothing here reads it. What becomes of the replaced messages is decided by
[compaction](../compaction.md#upstream-compaction).

`UpstreamCompactionProtocol` names the protocol kind when a client is built (`with_upstream_compaction`):

| Variant | Pairing accepted | Call |
| --- | --- | --- |
| `OpenAiResponses` | `OpenAiResponses` with `ResponsesDeployment::Platform` | buffered `POST <configured path>/compact` |
| `OpenAiResponsesStreamed` | `OpenAiResponses` with `ResponsesDeployment::Codex` | the streamed call the deployment always serves |

Any other pairing is refused at build time with `Error::Unsupported`, including `OpenAiResponses`
on a Codex deployment, which compacts only on its stream.

The responses wire is the only wire with this call. The Codex deployment is told by a
`{"type": "compaction_trigger"}` item appended to `input[]`, by `x-codex-beta-features:
remote_compaction_v2` and `x-codex-turn-metadata: {"request_kind":"compaction"}`, and by the
code-agent identity body fields ([client](../client.md#build-time-axes)).

```
caller                     compaction                          service
────────────────────────────────────────────────────────────────────────────────────
UpstreamCompactionRequest ──render()──▶ request/openai_responses.rs ──▶ JSON body ──▶ HTTP
   platform                     {model, input[]}                    ──▶  POST <path>/compact
   Codex                        {model, store:false, stream:true,   ──▶  POST <path>
                                 input[] + compaction_trigger}
UpstreamCompaction        ◀──decode()── response/openai_responses.rs ◀── JSON body ◀── HTTP
                  ◀──feed()──── model_use/stream/openai_responses.rs ◀── records ◀── SSE
```

`UpstreamCompactionRequest` is `{ model, conversation }`. `UpstreamCompaction` is
`{ conversation, usage, account_state, warnings }`, where `conversation` holds the compaction item
plus whatever the service echoed beside it.

How many `compaction` items a reply may carry depends on the lane:

- Streamed Codex: exactly one, or `Error::Malformed`. This lane reports no warnings.
- Buffered platform: the decoder accepts any number. Items it cannot represent become `warnings`,
  and `status: "failed"` becomes `Error::Upstream`.
- The executor: rejects a reply with none.

Both lanes retry whole attempts ([client](../client.md#retry)).

The compacted history then travels as one message in the conversation, `Message::UpstreamCompaction`.
Every other wire's renderer refuses such a conversation with `Error::Build` rather than quietly
sending the rest of it.
