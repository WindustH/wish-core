# Token counting

`protocol::token_count` maps an existing model-use `Request` to a provider's input-count API.
`TokenCount` reports `input_tokens` and optional `cached_input_tokens`. These are provider counts,
not generation usage, billing totals, or a guarantee that the model will accept the request.

```text
Request -> token_count/request -> count endpoint
                                      |
TokenCount <- token_count/response <-- JSON
```

Enable the capability explicitly on a client:

```rust
let client = client.with_token_count(TokenCountProtocol::OpenAiResponses)?;
let request = session.build_request()?;
let count = client.count_tokens(&request).await?;
```

| Count protocol | Endpoint relative to the configured generation path | Request |
| --- | --- | --- |
| OpenAiResponses | append `/input_tokens` | model, input, tools, tool choice, reasoning |
| AnthropicMessages | append `/count_tokens` | model, system, messages, tools, tool choice, thinking |
| GoogleGenerateContent | replace `:generateContent` or `:streamGenerateContent` with `:countTokens` | nested `generateContentRequest` with `models/<id>`, system and tools |

Model-use renderers supply the input conversion, preserving images and replayable reasoning and
omitting application metadata. Counting always uses a buffered response, regardless of
`Request.stream`. OpenAI and Claude omit output caps; Claude counting does not require one.
Google's nested request retains its generation configuration. Prompt-cache keys are not forwarded
to OpenAI counting; Claude's content cache markers are retained. Only Google fills
`cached_input_tokens` (from `cachedContentTokenCount`).

Only platform Responses, official Messages, and Gemini GenerateContent pairings are currently
supported (`TokenCountProtocol::validate_model_use`). Chat Completions, Codex, Vertex, Google
Interactions, Bedrock, Mistral Conversations and vendor-specific Messages modes are not implicitly
mapped to these endpoints. A Google path without `:generateContent` is `Error::Build`. A compatible provider must explicitly expose the selected
count API. `get_token_count_protocol()` reports the client's configuration, not a network probe.
Missing configuration and incompatible pairings return `Unsupported`; missing or invalid counts
return `Malformed` rather than an invented zero. Network errors use the client's normal retry
and error handling. The client never falls back to an estimate and never counts per turn on its
own.

[Compaction](../compaction.md#counting) is the only engine user: it counts candidate contexts
through `ModelCaller::count_tokens` when an endpoint is configured and estimates otherwise. Count
against the same model and request that will be used for generation; counts are not additive across
messages and may differ from later usage.
Claude specifically describes its count as an estimate.

References: [OpenAI count API](https://developers.openai.com/api/reference/resources/responses/subresources/input_tokens/methods/count),
[OpenAI SDK request schema](https://github.com/openai/openai-python/blob/main/src/openai/types/responses/input_token_count_params.py),
[Claude token counting](https://platform.claude.com/docs/en/build-with-claude/token-counting),
[Gemini countTokens](https://ai.google.dev/api/tokens).
