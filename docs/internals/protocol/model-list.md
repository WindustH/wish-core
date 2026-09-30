# Model list

`src/protocol/model_list/` is the intermediate layer for what models a service says it offers. It
converts one page of a catalog into `ModelListPage`, so the UI can offer a model picker without
knowing which service answered.

```
caller                      model_list                     service
──────────────────────────────────────────────────────────────────────────────
ModelListPage ◀── parse_page() ◀── model_list/<dialect>.rs ◀── one page ◀── HTTP
               ── fetch() + build_page_query() ──▶ source.rs entry × Credentials ──▶ build_call ──▶ HTTP
```

`Client::get_model_list(&ModelListQuery)` calls `fetch` ([client](../client.md#calls)). It makes one
GET with no retry. A non-`2xx` body is decoded by `http_error::decode_provider_envelope`.

## Protocols

| `ModelListProtocol` (id) | Auth | Page query |
| --- | --- | --- |
| `OpenAiModels` (`openai_models`) | bearer, or none with `unauthenticated` | `after` once a gateway returns a cursor |
| `OpenAiCodexModels` (`openai_codex_models`) | bearer + `chatgpt-account-id` | `client_version` only |
| `QwenModels` (`qwen_models`) | bearer | `page_no`, `page_size` |
| `AnthropicModels` (`anthropic_models`) | `x-api-key` + `anthropic-version` | `after_id`, `limit` |
| `GoogleModels` (`google_models`) | `x-goog-api-key` | `pageToken`, `pageSize` |
| `BedrockModels` (`bedrock_models`) | SigV4 | none |

No entry pins a host or a path: `ModelListQuery { base_url, path, cursor, page_size,
unauthenticated }` always supplies both (the Codex preset's `model_list_path` is
`/backend-api/codex/models`). `page_size` defaults to 100 (`ModelListQuery::first`). Every protocol
has an entry; `find_source` is one match over them.

## Shape

```text
ModelListPage { protocol, models: [Model], next_cursor, warnings }
Model        { id, name, owner, created_at, context_window, max_output_tokens }
```

Ids are kept as the service spelled them, except Google resource prefixes: `models/` (Gemini API)
and `publishers/<publisher>/models/` (Vertex) are stripped, because a call passes the bare id back.
`next_cursor` is the service's cursor, except for Qwen, whose next page number is computed from
`page_no`, `page_size` and `total`. What cannot be represented is kept in `warnings`.
