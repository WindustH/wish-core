# Image generation

Implemented 2026-10-29. This is a text-to-image tool, independent of the chat model.
Reference-image editing and masks are not exposed by this version.

## Using it

1. In Settings → Image generation, add a service and link its account. Model IDs,
   base URLs and provider-specific generation parameters are editable.
2. Enable `Image Generate` in the session's tools (or in new-session defaults).
   It can be delivered as a direct `image_generate` tool or `wish imagegen` command.
3. The tool accepts `prompt` and an optional configured `provider` ID. Without an
   ID, it selects the first enabled service in `image_generation.order`.
4. Returned `images` contain `id`, `mime_type`, `byte_count`, and canonical `path`.
   Images are in the session's existing blob directory, not temporary CDN links.
   Tool details load them through the authenticated blob endpoint. The model gets
   file references, not base64 image contents. It can use `view_image` if it needs
   visual inspection, or shell commands to process the returned paths.

Example (inside a session shell):

```sh
wish imagegen --json '{"prompt":"A watercolor mountain landscape","provider":"openai_images"}'
```

Example service configuration (credentials remain in `accounts`):

```json
{
  "image_generation": {
    "order": ["openai_images"],
    "providers": {
      "openai_images": {
        "preset": "openai_images",
        "credential_id": "my-openai-account",
        "model": "gpt-image-2.5-sunburst",
        "enabled": true,
        "proxy_enabled": true,
        "parameters": {"quality": "medium", "size": "1024x1024"}
      }
    }
  }
}
```

The preset catalog is available at `GET /api/image-presets`. The shared account
picker derives missing platform types from that catalog. Existing model-account
platforms are reused. Removing an account includes image services in the existing
cascade confirmation.

## Provider research and adapters

Each row links its primary protocol documentation. These are protocol integrations,
not a promise that every model is available to every account. Model lifecycle,
regional access and paid quotas remain provider-controlled.

| Service | Implemented wire path / response |
| --- | --- |
| [OpenAI API](https://developers.openai.com/api/docs/guides/image-generation) | `POST /v1/images/generations`; `data[].b64_json` or `url` |
| [ChatGPT / Codex](https://learn.chatgpt.com/docs/image-generation) | Codex account's `images/generations`; base64 images; separate subscription authentication |
| [Google Gemini](https://ai.google.dev/api/generate-content) | `models/{model}:generateContent`; candidate image parts |
| [Google Interactions / Nano Banana](https://ai.google.dev/gemini-api/docs/image-generation) | `interactions`; image blocks in outputs/steps |
| [xAI Grok Imagine](https://docs.x.ai/developers/model-capabilities/images/generation) | Images API, synchronous response; deferred mode is not enabled |
| [Together AI](https://docs.together.ai/docs/inference/images/overview) | Images API, `response_format: base64` |
| [SiliconFlow](https://docs.siliconflow.cn/docs/userguide/capabilities/images) | Images API; `images[].url` response |
| [Volcengine Seedream / Ark](https://docs.volcengine.com/docs/ark/image-generation-tutorial?lang=en) | Ark `/api/v3/images/generations`; separate [LAS endpoint preset](https://docs.volcengine.com/docs/lake-ai-service/image-generation-seed-ream-series-models?lang=en) |
| [Black Forest Labs](https://docs.bfl.ai/quick_start/generating_images) | `x-key`; submit once, poll supplied URL, download result sample |
| [fal.ai](https://fal.ai/docs/documentation/model-apis/inference/queue) | `Authorization: Key`; queue submission, status, result, image download |
| [Replicate](https://replicate.com/docs/reference/http) | Official-model predictions endpoint, poll `urls.get`, download output images; arbitrary community-version schemas are not advertised |
| [Stability AI](https://docs.stability.ai/api-reference) | Multipart Stable Image generation, JSON image response |
| [Alibaba Wan](https://www.alibabacloud.com/help/en/model-studio/wan-image-generation-and-editing-api-reference) | Synchronous multimodal generation; separate Beijing/Singapore workspace accounts, `workspace_id` endpoint expansion |

The backend stores the authoritative presets in `resources/image-presets.json`.
`parameters` are native provider fields, not falsely normalized into one universal
schema. The application owns prompt and model fields. Counts are capped at eight.
Unsupported parameters produce a provider error; images are never simulated.

## ChatGPT subscription channel

Official product documentation establishes subscription image generation in Codex;
that is different from an OpenAI API key. The implementation was also checked
against OpenAI's Codex source: `codex-rs/codex-api/src/endpoint/images.rs`,
`codex-api/src/images.rs`, and `ext/image-generation/src/{backend,tool}.rs`.
The inspected implementation uses `images/generations`, `gpt-image-2`, the active
provider's authentication, and an image-turn identifier. Wish therefore uses its
existing `openai_codex` account runtime, Bearer access token and
`chatgpt-account-id`, with the Codex base URL. Account renewal remains in the shared
account lifecycle; this feature never creates a second login store.

This is a Codex integration surface, not an assurance that the public OpenAI API
accepts subscription tokens. Wish refuses a subscription account on ordinary API
presets and refuses ordinary API accounts on the Codex preset. Real subscription
access still needs validation with an entitled account. Tests use synthetic tokens.

## Boundaries and failure semantics

- `protocol/image_generation`: request encoding, response decoding and credential
  placement. No storage and no retry loop.
- `application/image_generation`: immutable service configuration snapshots, pooled
  transports, bounded concurrency (two active jobs), polling, downloads and blobs.
- `tool/image_generate`: schema, cancellation, model-facing artifact report.
- The capability catalog and existing session bridge provide both delivery routes.

A paid submission is sent once. There is no automatic resubmission or provider
fallback after an ambiguous failure. Cancellation stops local waiting; it cannot
promise cancellation of an accepted upstream job. Polling does not resubmit it.
The total job deadline is ten minutes. Poll URLs must stay on the configured origin
(except BFL's own HTTPS regional origins). CDN downloads and their redirects never
carry account credentials. Response bodies and image counts/sizes are bounded;
base64 decoding uses the bounded CPU pool. The session's existing blob reference
scanner retains the generated IDs mentioned in the persisted tool result.

## Verification

`wish-test/tests/test_image_generation.py` drives a real Wish server through both
its shell bridge and direct model tool execution. Local HTTP fixtures cover every
preset, async polling, credential placement, credential-free downloads, blob
contents/paths, rejection of foreign polling origins, tool disabling and absence
of automatic resubmission. No unit tests or billable upstream calls are added.
The frontend's existing accounts browser regression covers mobile/desktop image
settings and nested account creation. These checks establish integration behavior;
they do not establish live billing, entitlement or model availability.

## Provider discovery and routing

`image_generate`, `web_search` and `web_fetch` accept `{"action":"list_providers"}`
without execution arguments. The result contains live configured IDs, display names, models,
local credential readiness and `default_order`. It excludes credentials and connection details.
CLI equivalents are `wish imagegen --list-providers`, `wish websearch --list-providers`
and `wish webfetch --list-providers`. Discovery observes the session capability switch.

For execution, an optional `provider` pins the configured ID, including an enabled service
outside the default order. A pinned failure never selects a different service. Without it,
search/fetch try enabled providers in configuration order; image generation submits only to
the first enabled provider, since an ambiguous paid request cannot safely fail over. Fetch
cursors keep their original snapshot and reject a mismatched explicit provider.
