# Model use

`src/protocol/model_use/` is an intermediate layer, and the home of `ModelUseProtocol`: the enum
of wires a conversation can speak. It receives a request in our intermediate representation and
converts it into the form an upstream API accepts. It receives an upstream response or stream and
converts it into the corresponding intermediate representation.

```
caller                      model_use                    wire
──────────────────────────────────────────────────────────────────────────────
Request      ──render()──▶  request/<dialect>.rs  ──▶  JSON body     ──▶  HTTP
Response     ◀──decode()──  response/<dialect>.rs ◀──  JSON body     ◀──  HTTP
StreamEvent  ◀──feed()────  stream/<dialect>.rs   ◀──  wire records  ◀──  SSE, or AWS
                                                                          event-stream (Bedrock)
```

## Protocols

`request/`, `response/` and `stream/` each hold the same seven dialect modules. Vertex shares
Google GenerateContent's modules.

| `ModelUseProtocol` variant | Dialect module | Mode |
| --- | --- | --- |
| `AnthropicMessages(MessagesApiCompatMode)` | `anthropic_messages` | reasoning extension |
| `OpenAiChat(ChatCompletionApiCompatMode)` | `openai_chat` | reasoning extension and instruction roles |
| `OpenAiResponses(ResponsesApiCompatMode)` | `openai_responses` | reasoning form and deployment |
| `GoogleGenerateContent` | `google_generate_content` | none |
| `GoogleVertexGenerateContent` | `google_generate_content` | none |
| `GoogleInteractions` | `google_interactions` | none |
| `BedrockConverse` | `bedrock_converse` | none |
| `MistralConversations` | `mistral_conversations` | none |

Compat modes name the vendor reasoning extension a reimplemented wire speaks. `Official` is the
default for both:

- `MessagesApiCompatMode`: `Official`, `DeepSeek`, `Zai`, `Kimi`, `Qwen`, `MiniMax`, `Mimo`,
  `TokenHub`.
- `ChatCompletionApiCompatMode`: `Official`, `Compatible`, `DeepSeek`, `Zai`, `KimiK2`, `KimiK3`,
  `Qwen`, `MiniMax`, `Mimo`, `TokenHub`, `Mistral`. `Official` is OpenAI's own endpoint and
  `Compatible` any other plain chat endpoint; both carry no reasoning extension.

A chat mode also decides where instruction messages go. `Official` keeps `system` and `developer`
anywhere. `DeepSeek`, `Zai`, `KimiK2` and `KimiK3` refuse `developer` (DeepSeek and Zhipu verified
live) but take `system` anywhere, so both roles become `system`. The rest take one leading
`system` message: the leading instruction run merges into it, and a later instruction becomes a
`user` message.

`ResponsesApiCompatMode` is `{ reasoning_form, deployment }`:

| `ReasoningForm` | Meaning |
| --- | --- |
| `Plaintext` | send reasoning back as `reasoning_text` content |
| `Ciphertext` (default) | request `reasoning.encrypted_content` and send it back |
| `NoSendBack` | read reasoning, never send it back |

| `ResponsesDeployment` | Meaning |
| --- | --- |
| `Platform` (default) | the platform API |
| `Codex` | the ChatGPT-subscription backend. Streamed calls only, no `max_output_tokens` (both refused locally with `Error::Build`), extra routing headers, quota on the reply |

The Messages wire, in every compat mode, requires `max_output_tokens` and refuses
`ToolChoice::None`.

## Message metadata

Every `Message` variant has `metadata: serde_json::Value` for application data, such as
source identifiers or UI annotations. Use `get_metadata()` and `set_metadata(value)` without
matching the variant. Any JSON value is accepted; `null` means no metadata.

Metadata survives serialization, session storage, and reuse in generation history. Old stored
messages without this field load with `null`; null metadata is omitted when serialized.
Protocol decoders initialize it to null, and request renderers never send it upstream.
When constructing a message in Rust, supply `metadata: Default::default()` or your JSON value.

`Message::Developer` has `fixed: Option<bool>`. `is_fixed_instruction()` is true for `System`
and for `Developer` with `fixed` true or absent. Absent means a legacy stored record.
`normalize_new_input()` sets an absent `fixed` to `false`. The server applies it to messages it
receives over HTTP. The leading fixed messages are the prefix that [compaction](../compaction.md) keeps
verbatim.

`Message::Reasoning.opaque_kind` (`ReasoningOpaqueKind`) records which wire produced a signature or
ciphertext. Renderers replay opaque material only to a matching wire. Records without it keep
their readable text only.

## Request mode

`Request.stream` is part of request construction. Most renderers render the `stream` field and,
where needed, streaming usage options. Google GenerateContent/Vertex and Bedrock renderers ignore
it and keep the body unchanged. `ModelUseProtocol::resolve_request_path` selects their stream
endpoint instead: `:streamGenerateContent?alt=sse` for Google, and `/converse-stream` for Bedrock
when the path ends in `/converse`.

## Interrupted streams

A stream's owner may stop reading without claiming a normal EOF. `EventStream::create_accumulator()`
creates a caller-owned accumulator with the protocol's replay rules and reply-head account data;
`EventStream::abort(self)` drops local reading without flushing framing or decoder buffers.
`StreamAccumulator::finalize(end)` defines the closing policy and returns
`StreamFinalization::{Complete, Incomplete}`:

| End | Protocol result | Context fragment |
| --- | --- | --- |
| `Complete` | Strictly assembled `Response`, or error | Normal response; tools handled by the caller |
| `Interrupted { tools }` | `PartialResponse` | Eligible content with all retained tool calls paired |
| `Failed(error)` | `PartialResponse` with failure diagnostics | Empty; failed output is not accepted |

For interruption, the owner explicitly supplies `ToolExecutionState::NotStarted` or
`MayHaveStarted`. The protocol supplies cancelled results for unstarted calls and unknown results
when execution may have occurred. It never infers execution from output or invokes a tool.
`interrupt(tools)` is the direct interruption-only entry point; `finish()` stays strict.

`PartialResponse` retains the ending reason, delivered blocks with their `ReplayDisposition`,
observed usage/account state and any upstream stop reason. `get_replay_messages()` exposes the fully
paired context fragment; `take_replay_messages()` transfers it without discarding the original
partial data. The session appends that fragment verbatim, without filtering or synthesizing results.
A block's replay eligibility describes its content; the ending policy still suppresses all replay
on failure. A `Complete` that fails strict validation returns an error, never a partial success.

`BlockEnd` closes a block for normal response assembly. `BlockComplete` independently certifies
that the wire finished the payload: the `StreamDecoder` dispatcher supplies it after explicit
block endings or successful terminal records, not synthetic closings on abnormal termination.
Responses items whose status is not `completed` are never certified. Code that drives a dialect
decoder directly does not get the dispatcher's certification, so partial replay must go through
`StreamDecoder`. Any other event producer must uphold the same contract. A valid JSON prefix alone
is never a completed tool invocation.

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

An unattached Google GenerateContent/Vertex signature-only item (`MissingSignedPart`) stays outside
replay to avoid moving its proof to a later part. Replay eligibility is tied to the source protocol
and does not guarantee portability to another provider/model. Excluded content remains in the
partial record for inspection; it is not included in the next request. Normal completion remains
strict, and stream failures retain an empty context fragment.

## Output limit

`StreamAccumulator::finish_output_limit()` closes a stream with an explicit
`MaxOutputLengthExceeded` stop using the same certified-block replay rules. Its `PartialResponse`
has `IncompleteReason::OutputLimit`, distinct from local cancellation or transport failure.
`get_continuation_messages()` projects only text and reasoning; tool calls must be reissued and
signatures bound to discarded tool calls are omitted. Buffered output-limit responses use
`PartialResponse::from_output_limit(response, protocol)` with conservative block-completeness
assumptions. Both return `Error::Build` unless the stop reason is `MaxOutputLengthExceeded`. The
executor uses them for [output continuation](../executor.md#automatic-output-continuation).
