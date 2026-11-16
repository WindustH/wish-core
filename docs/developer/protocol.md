# Protocol Layer

`src/protocol/` is a pure translation layer between Wish's intermediate representation (`Request`, `Response`, `StreamEvent`, `Message`, `Error`) and upstream HTTP/SSE formats. Network execution stops at `attempt::{Call, Reply, ReplyStream}`, keeping `protocol/` independent of HTTP servers, configs, and persistence.

## Conversation wires (`model_use`)

`src/protocol/model_use/` maps `Request` → JSON `Draft` (`request/`), JSON body → `Response` (`response/`), and stream frames → `StreamEvent` → `StreamAccumulator` → `Response` (`stream/`):

| `ModelUseProtocol` | Dialect | Key behaviors |
| --- | --- | --- |
| `AnthropicMessages(MessagesApiCompatMode)` | `anthropic_messages` | Modes: `Official`, `DeepSeek`, `Zai`, `Kimi`, `Qwen`, `MiniMax`, `Mimo`, `TokenHub`. Requires `max_output_tokens`; refuses `ToolChoice::None`. Instruction-only conversations project the last instruction as a `user` turn. |
| `OpenAiChat(ChatCompletionApiCompatMode)` | `openai_chat` | Modes: `Official`, `Compatible`, `DeepSeek`, `Zai`, `KimiK2`, `KimiK3`, `Qwen`, `MiniMax`, `Mimo`, `TokenHub`, `Mistral`. Controls reasoning fields and instruction folding (`DeepSeek`/`Zai`/`KimiK2`/`KimiK3` fold `developer` into `system`; other compat modes merge leading instructions into `system` and later ones into `user`). |
| `OpenAiResponses(ResponsesApiMode)` | `openai_responses` | `ReasoningForm` (`Ciphertext` vs `Plaintext`) × `ResponsesDeployment` (`Platform` vs `Codex`, where `Codex` mandates streaming and rejects `max_output_tokens`). |
| `GoogleGenerateContent` / `Vertex` / `Antigravity` | `google_generate_content` | Streaming routes to `:streamGenerateContent?alt=sse`. Antigravity wraps Cloud Code envelopes and Claude thinking budgets. |
| `GoogleInteractions` | `google_interactions` | Google Interactions format. |
| `BedrockConverse` | `bedrock_converse` | Bedrock Converse JSON and binary `event-stream` (`/converse-stream`). |
| `MistralConversations` | `mistral_conversations` | Mistral Conversations format. |

Every `Message` carries an optional `metadata` JSON value that stays local to Wish (decoders initialize it to `null`, renderers omit it). `Message::Developer` adds `fixed: Option<bool>`; `normalize_new_input()` sets omitted `fixed` to `false` on HTTP ingestion so only explicitly fixed instructions survive compaction and context clears. `Message::Reasoning.opaque_kind` tags encrypted signatures/ciphertexts by origin wire so they replay only on a matching protocol family.

The Google contents wire is also used by Antigravity's Gemini, Claude and GPT-OSS models, whose signatures are not interchangeable. Sessions store `Reasoning.source_model` from the originating call. For older entries, request construction recovers it from `model_call_id` without rewriting history, including context inspection, compaction candidates and forks. Google request rendering omits reasoning from a known different model; native signatures remain attached to their original thought, answer or tool-call parts. Resource-name prefixes and the catalog's reasoning-mode suffixes normalize to the same logical model, while distinct models remain separate. Imported messages without a known source retain the existing wire-kind compatibility rule.

Gemini can finish a streamed answer with a signed, empty text part. The decoder preserves that part even if more text follows, and the renderer restores the empty part for older replies stored with a trailing signature-only message. It never merges that proof into the preceding answer.

Antigravity's Claude deployments additionally require a signature on every replayed thinking block. Their request projection omits unsigned reasoning and reasoning with foreign or untyped signatures, including Gemini summaries whose signature belongs to a separate answer or tool-call part. Normal messages and tool results remain in the request. When foreign tool history has no native proof, Gemini 3+ uses its documented import marker on the first function call of each step; Claude and GPT-OSS do not receive that marker.

## Interrupted streams and output limits

`EventStream::create_accumulator()` tracks protocol-level completion signals (`BlockComplete` vs syntactic `BlockEnd`) so `StreamAccumulator::finalize` decides safely what can enter context when a stream stops early:

- **`Complete`**: Assembles a strictly validated `Response` or returns an error.
- **`Interrupted { tools }`**: Keeps received text and transparent plaintext reasoning (`OpenAiChat` plaintext dialects, `MistralConversations`, plaintext `AnthropicMessages` compat, `OpenAiResponses(Plaintext)`), while discarding incomplete tool calls and incomplete opaque reasoning (Responses ciphertext, native Anthropic, Bedrock, Google). Retained tool calls are paired immediately with `Cancelled` or `Unknown` results based on `ToolExecutionState`.
- **`Failed`**: Produces an empty context fragment.
- **`finish_output_limit()`**: Keeps completed text/reasoning on `MaxOutputLengthExceeded` while dropping partial tool calls and attached signatures so the executor can request continuation.

## Endpoints, auth, and errors

`src/protocol/endpoint/` pairs `Draft` with `Endpoint` and `Credentials` in `build_call`:

- Supports `AuthScheme::None`, `Bearer(Option<CredentialRenewal>)`, `Header(&'static str)`, `SigV4`, and `GitHubToken`.
- Checks token `expires_at` before sending (`Error::Renewal`), expands percent-encoded URL/path placeholders (`api_key`, `workspace_id`, `team_id`, `organization`, `project`, `account_id`, `region`—never AWS secrets), applies `{session}` headers, merges overrides case-insensitively, and signs `SigV4` last.

OAuth helpers (`codex_oauth`, `google_oauth`, `copilot_oauth`, and `antigravity` project discovery) run single non-retried HTTP exchanges and hand raw `Tokens` back to `application`.

`src/protocol/error.rs` classifies errors cleanly: `Build`, `Unsupported`, `Malformed`, and `Renewal` never retry; `Upstream` retries only on `429` and `5xx` (in-band `2xx` refusals never retry); `Transport` carries the transport layer's retry verdict. `is_context_length_exceeded()` detects context-window rejections so the executor can trigger emergency compaction.

## Auxiliary protocols

- **`token_count`**: Buffered input counting for `OpenAiResponses`, `AnthropicMessages`, and `GoogleGenerateContent`.
- **`upstream_compaction`**: Buffered `/compact` on platform `OpenAiResponses` and streamed `compaction_trigger` on `Codex` (`OpenAiResponsesStreamed`), returning opaque `Message::UpstreamCompaction` items rejected by other dialects.
- **`model_list`**: Paged catalog readers (`openai_models`, `openai_codex_models`, `qwen_models`, `anthropic_models`, `google_models`, `google_antigravity_models`, `bedrock_models`, `github_copilot_models`), normalizing Google resource prefixes.
- **`account_state`**: Header/body quota parsers (`headers.rs`, `codex.rs`) plus standalone account balance/quota readers, preserving exact decimal strings and mapping in-band account refusals to `AccountState.failure`.
