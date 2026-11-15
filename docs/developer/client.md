# Client & Transport

`src/client.rs` combines a `ModelUseProtocol`, an `Endpoint`, dynamic `Credentials`, and a single-attempt `ReqwestTransport` into `Client<T>`. It implements the executor's `ModelCaller` trait and owns the retry policy for model and account traffic.

## Capabilities and session binding

`src/application/provider.rs` wires optional protocols onto a `Client` at build time:

- `with_token_count` and `with_upstream_compaction` verify compatibility with the chosen `ModelUseProtocol` upfront and fail mismatched requests with `Error::Unsupported`.
- `with_model_list` and `with_account_state` attach catalog and quota readers. Reply-borne account protocols (rate-limit headers or Codex quota frames) parse on every model call.
- `with_code_agent_identity` injects provider-expected client context: `Codex` populates `prompt_cache_key` and `client_metadata` on Responses, `Claude` sets `metadata.user_id` on Messages, and `Copilot` sets `x-initiator` (`user` vs `agent`) plus `copilot-vision-request: true` when images are included.
- `with_session_id` supplies the `{session}` header template value, and `with_attempt_observer` records 1-second streaming throughput samples.

## Subscription credential renewal

Subscription providers (`openai_codex`, `google_antigravity`, `github_copilot`) carry `AuthScheme::Bearer(Some(CredentialRenewal))`. `AccountClient::refresh_credentials` exchanges tokens and returns them to `application`, which persists rotated secrets to `config.json` and rebuilds affected clients.

Active sessions snapshot a shared renewal handle before each logical call (`SessionModel`), picking up freshly renewed tokens between rounds of a long tool loop without disturbing an in-flight attempt. Expired credentials detected in `Endpoint::build_call` fail immediately with `Error::Renewal` before touching the network.

## Retry lanes

`utils::retry` drives retry loops using classifications from `protocol::retry` and `Error::is_retryable()`:

- **Model lane (`RetryPolicy::model`)**: Up to 5 retries with jittered `5n` s backoff (`5–6s`, `10–12s`, `15–18s`, `20–24s`, `25–30s`). For streamed calls, retries cover only connection and stream opening up to the first decoded event—once the caller receives an event, retrying would duplicate output. Streamed Codex compaction retries the whole call since nothing leaves the client until the compaction item finishes.
- **Auxiliary lane (`RetryPolicy::auxiliary`)**: Up to 3 retries (`2 + [0, 1)` s) for read-only calls (`count_tokens`, `get_model_list`, `get_account_state`, `search`, `fetch`), which can safely retry across all transport phases.
- **OAuth / device-code exchanges**: Single attempt, never retried.

Upstream `Retry-After` headers (capped at 1 hour) raise the wait floor. Observers emit `waiting`, `attempting`, and `finished` notifications as transient `upstream_retry` SSE events so reconnected clients can restore retry countdowns.

## Timeouts and deferred stream headers

`ReqwestTransport` bounds each attempt with `connect` (10 s), `first_byte` (60 s), `idle` (60 s, extended to 1200 s for Gemini), and `total` (300 s buffered, 30 minutes streamed after headers arrive).

Because Google GenerateContent, Vertex, and Antigravity streams may reason and bill before flushing HTTP response headers, the server sets `with_deferred_stream_headers` (a 5-minute head deadline). If that deadline expires, it fails as non-retryable `AwaitHeaders` rather than risking duplicate billing.

## Gemini native stream recovery

When a Gemini stream supplies `candidates[0].continuationToken`, `protocol/model_use/recovery.rs` supports in-process stream continuation (up to 32 hops using `RetryPolicy::model()`):

1. The client freezes the original request and credentials, emitting `ModelStream::Checkpoint { revision }` whenever a continuation token arrives.
2. On a `CONTINUATION` boundary or mid-stream interruption with an active checkpoint, the client updates only `continuationToken`, emits `Rewind { revision }` to discard speculative suffixes (uncommitted tool args or unsigned reasoning), and resumes from the checkpoint.
3. Usage stays cumulative within each hop and additive across committed hops while `InputTokens` preserves the initial prompt size.

Checkpoints live purely in memory for the active call. When migrating mid-tool-loop from another provider into Gemini 3+, the GenerateContent renderer adds `skip_thought_signature_validator` only to the first unsigned function call of a turn while leaving genuine Google signatures intact.
