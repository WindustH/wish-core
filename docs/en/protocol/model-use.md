# Model use

`src/protocol/model_use/` is an intermediate layer, and the home of `ModelUseProtocol` - the enum
of wires a conversation can speak, one variant per protocol kind with the compat mode that says
which vendor's reasoning extension rides it: it receives a request in our intermediate
representation and converts it into the form an upstream API accepts, and it receives an upstream
response or stream and converts it into the corresponding intermediate representation.

```
caller                      model_use                    wire
──────────────────────────────────────────────────────────────────────────────
Request      ──render()──▶  request/<dialect>.rs  ──▶  JSON body     ──▶  HTTP
Response     ◀──decode()──  response/<dialect>.rs ◀──  JSON body     ◀──  HTTP
StreamEvent  ◀──feed()────  stream/<dialect>.rs   ◀──  wire records  ◀──  SSE
```


## Message metadata

Every `Message` variant has `metadata: serde_json::Value` for application data, such as
source identifiers or UI annotations. Use `get_metadata()` and `set_metadata(value)` without
matching the variant. Any JSON value is accepted; `null` means no metadata.

Metadata survives serialization, session storage, and reuse in generation history. Old stored
messages without this field load with `null`; null metadata is omitted when serialized.
Protocol decoders initialize it to null, and request renderers never send it upstream.
When constructing a message in Rust, supply `metadata: Default::default()` or your JSON value.

## Request mode

`Request.stream` is part of request construction. All model-use renderers read it directly.
Google GenerateContent/Vertex and Bedrock keep the body unchanged and select the stream endpoint;
the other dialects render the appropriate `stream` field and, where needed, streaming usage options.

## Interrupted streams

A stream's owner may stop reading without claiming a normal EOF. `EventStream::create_accumulator()`
creates a caller-owned accumulator with the protocol's replay rules and reply-head account data;
`EventStream::abort(self)` drops local reading without flushing framing or decoder buffers.
`StreamAccumulator::finalize(end)` defines the closing policy:

| End | Protocol result | Context fragment |
| --- | --- | --- |
| `Complete` | Strictly assembled `Response`, or error | Normal response; tools handled by the caller |
| `Interrupted { tools }` | `PartialResponse` | Eligible content with all retained tool calls paired |
| `Failed(error)` | `PartialResponse` with failure diagnostics | Empty; failed output is not accepted |

For interruption, the owner explicitly supplies `ToolExecutionState::NotStarted` or
`MayHaveStarted`. The protocol supplies cancelled results for unstarted calls and unknown results
when execution may have occurred. It never infers execution from output or invokes a tool.
`interrupt(tools)` is the direct interruption-only entry point; `finish()` stays strict.

`PartialResponse` retains the ending reason, delivered blocks and their replay eligibility,
observed usage/account state and any upstream stop reason. `get_replay_messages()` exposes the fully
paired context fragment; `take_replay_messages()` transfers it without discarding the original
partial data. Agent code appends that fragment verbatim, without filtering or synthesizing results.
A block's replay eligibility describes its content; the ending policy still suppresses all replay
on failure. A `Complete` that fails strict validation returns an error, never a partial success.

`BlockEnd` closes a block for normal response assembly. `BlockComplete` independently certifies
that the wire finished the payload: the `StreamDecoder` dispatcher supplies it after explicit
block endings or successful terminal records, not synthetic closings on abnormal termination.
Consumers using a dialect decoder directly do not receive the dispatcher's certification and
should use `StreamDecoder` for partial replay. Custom event producers must uphold this contract.
A valid JSON prefix alone is never a completed tool invocation.

Interruption uses these rules:

| Content | Incomplete block |
| --- | --- |
| Ordinary text | Replay the received prefix |
| Transparent reasoning: plaintext is the replay representation | Replay the received plaintext prefix |
| Opaque reasoning: replay depends on a signature/encrypted payload | Exclude from replay until complete |
| Tool call | Exclude from replay until complete |

Visible summaries do not make opaque reasoning transparent. `ReasoningDisplayDelta` only fills
`Message::Reasoning.display`; `ReasoningDelta` fills replayable `plaintext`. Responses decodes
`reasoning_summary_text.delta` and `reasoning_text.delta` separately, matching its buffered reader.

Plaintext replay applies to supported Chat dialects, Mistral Conversations, plaintext Anthropic
compatibility dialects and Responses with `ReasoningForm::Plaintext`. Blocks carrying opaque
material still require completion. Responses ciphertext mode, native Anthropic, Bedrock and Google
require their complete opaque replay material even if readable text arrived earlier. A completed
opaque block need not have visible text. `ReasoningForm::NoSendBack`, official OpenAI Chat and
unknown protocol configurations do not replay reasoning from partial results.

An unattached Google signature-only item stays outside replay to avoid moving its proof to a
later part. Replay eligibility is tied to the source protocol and does not guarantee portability
to another provider/model. Excluded content remains in the partial record for inspection; it is
not included in the next request. Normal completion remains strict, and stream failures retain
an empty context fragment.
