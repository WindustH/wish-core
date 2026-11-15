# Configuration

Wish reads all server settings from `config.json`. Most sections can be edited visually in the web app under **Settings**; any edits you make directly to the file are validated strictly on load (unknown keys are rejected immediately so typos never slip by silently).

When running `wish` without `--config`, the config lives in your user config folder and defaults to `127.0.0.1:8790`. When running `wish --config path/to/config.json`, an omitted `listen` field defaults to `127.0.0.1:9780`.

## Minimal example

Here is a working starting point (`config.example.json` in the repo):

```json
{
  "listen": "127.0.0.1:9780",
  "data_dir": "data",
  "bearer_token_env": "WISH_HTTP_TOKEN",
  "accounts": {
    "openai": {
      "name": "OpenAI",
      "preset": "openai",
      "api_key_env": "OPENAI_API_KEY"
    }
  },
  "providers": {
    "openai": {
      "credential_id": "openai",
      "preset": "openai",
      "protocol": "openai_responses",
      "base_url": "https://api.openai.com",
      "path": "/v1/responses",
      "auth": "bearer",
      "token_count": "openai_responses",
      "compaction": "openai_responses",
      "model_list": "openai_models",
      "model_list_path": "/v1/models"
    }
  },
  "defaults": {
    "provider": "openai",
    "model": "gpt-5.4",
    "run": { "tools": "Parallel" },
    "compaction": {
      "trigger_tokens": 200000,
      "target_tokens": 80000,
      "segment_tokens": 20000
    }
  }
}
```

## Top-level structure

| Key | Default | What it controls | When changes apply |
| --- | --- | --- | --- |
| `listen` | `127.0.0.1:9780` (`:8790` auto-created) | Listen IP and port | Restart |
| `data_dir` | `"data"` | Databases, session files, shell logs, backups | Restart |
| `bearer_token_env` | `null` | Env var holding the Bearer token for `/api/*` | Restart |
| `web_dir` | `null` | Custom static web directory (defaults to `web/` beside `wish`) | Restart |
| `allowed_hosts` | `[]` | Extra `host[:port]` names allowed when no Bearer token is set | Restart |
| `accounts` | `{}` | Shared credentials and subscription logins | Immediate |
| `providers` | `{}` | Model providers | Immediate |
| `defaults` | See below | Model, cwd, instructions, compaction, and tool defaults for new sessions | New sessions |
| `proxy` | `{"mode": "environment"}` | Outbound proxy mode and credentials | Immediate |
| `shell` | Platform default | Shell executable and flags for session commands | Next command |
| `mcp` | `{"servers": {}}` | MCP servers (`stdio` or Streamable HTTP) | Next check / start |
| `skills` | `{"dirs": [], "disabled": []}` | Extra `SKILL.md` roots and disabled skill names | Immediate |
| `search`, `fetch` | `{"order": [], "providers": {}}` | Web search and webpage reader services | Immediate |
| `image_generation`, `video_generation`, `audio_generation`, `audio_transcription` | `{"order": [], "providers": {}}` | Image, video, audio, and speech-to-text services | Immediate |
| `usage` | `{"stream_sample_limit": 20000}` | Max 1s stream speed samples kept (`null` for unlimited) | Immediate |

## Accounts

Store credentials once under `accounts.<id>` so model providers, search, fetch, and media tools can share them via `credential_id`.

An account takes a human label (`name`), an optional `preset`, and credentials—either `api_key` / `api_key_env`, OAuth fields (`refresh_token`, `expires_at`), or multi-field `credentials` / `credentials_env` maps (used by AWS Bedrock, Jimeng, or Google Antigravity). `proxy_enabled` (default `true`) controls whether sign-in and token refresh use the configured proxy.

Three presets support interactive subscription login without copying raw keys:
- **ChatGPT (`openai_codex`)**: PKCE OAuth (`localhost:1455` callback, or paste the redirect URL if running remotely).
- **Google Antigravity (`google_antigravity`)**: Google OAuth (`localhost:51121` callback, automatically discovers your Cloud Code project ID).
- **GitHub Copilot (`github_copilot`)**: GitHub device code flow; Wish renews short-lived Copilot tokens in the background.

## Model providers

Each key under `providers.<id>` configures a model provider.

| Field | Purpose |
| --- | --- |
| `display_name` | Friendly label shown in model pickers. |
| `preset` | Built-in preset ID (enables preset-specific token refresh and quota endpoints). |
| `credential_id` | References a shared account under `accounts`. |
| `enabled` / `proxy_enabled` | Booleans (both default to `true`). |
| `protocol` | Wire dialect (see supported protocols below). |
| `base_url` / `path` | Endpoint URL and path (e.g. `https://api.openai.com` + `/v1/responses`). |
| `auth` | `bearer` (default), `anthropic_key`, `google_key`, `sigv4`, or `none`. |
| `api_key` / `api_key_env` / `credentials` / `credentials_env` | Inline or env-backed auth when not referencing `credential_id`. |
| `headers` | Custom HTTP headers; supports `${ENV_VAR}` expansion. |
| `models` | Per-model metadata (`context_window`, `max_output_tokens`, `vision`, `reasoning`). |
| `model_list`, `model_list_path`, `model_list_base_url` | Optional model catalog adapter (`openai_models`, `anthropic_models`, `google_models`, `google_antigravity_models`, `bedrock_models`, `mistral_models`, `magpie_models`). |
| `token_count` | Optional pre-flight count API (`openai_responses`, `anthropic_messages`, `google_generate_content`). |
| `compaction` | Optional upstream compaction (`openai_responses`, `openai_responses_streamed`). |
| `account_state`, `account_state_base_url` | Optional quota/balance protocol (`codex_usage`, `copilot_usage`, `google_antigravity_quota`, `zai_quota`, `minimax_coding_plan_quota`, `deepseek_balance`, `openrouter_credits`, `siliconflow_user`, `moonshot_balance`). |

### Protocols and presets

Wish supports 28 wire protocol IDs across OpenAI Chat (`openai_chat`, `compatible_chat`, `mistral_chat`, plus `deepseek_chat`, `zai_chat`, `kimi_k2_chat`, `kimi_k3_chat`, `qwen_chat`, `minimax_chat`, `mimo_chat`, `tokenhub_chat`), OpenAI Responses (`openai_responses`, `plaintext_responses`, `codex_responses`), Anthropic Messages (`anthropic_messages`, plus `deepseek_messages`, `zai_messages`, `kimi_messages`, `qwen_messages`, `minimax_messages`, `mimo_messages`, `tokenhub_messages`), Google (`google_generate_content`, `google_vertex_generate_content`, `google_antigravity_generate_content`, `google_interactions`), AWS Bedrock (`bedrock_converse`), and Mistral Conversations (`mistral_conversations`).

77 built-in presets cover global providers, Chinese providers and coding plans, routers/aggregators (including OpenRouter, OpenCode, and the **Magpie** gateway whose `magpie_models` endpoint assigns protocols per model automatically), and local servers (Ollama, LM Studio, vLLM, oMLX, MLX-LM).

### Per-model metadata

Declaring models under `providers.<id>.models.<model_id>` populates context gauges, reasoning effort selectors, and image capability flags in the UI:

```json
{
  "gpt-5.4": {
    "context_window": 1000000,
    "max_output_tokens": 128000,
    "vision": true,
    "reasoning": {
      "levels": ["none", "low", "medium", "high", "xhigh"],
      "default": "high"
    }
  }
}
```

## Session defaults (`defaults`)

New sessions copy their starting settings from `defaults`:

- **Model & prompt**: `provider`, `model`, `cwd` (defaults to the server's starting directory), `stream` (`true`), `instructions` (`""`), `reasoning` (`null`), `max_output_tokens` (`null`).
- **Execution policy (`defaults.run`)**: `tools` (`"Parallel"` by default, or `"Serial"`) and `background_tools_on_input` (`true`). When you send new input, foreground shell commands get up to one second to finish and return their results inline; commands still running then continue in the background. Explicit background requests, interruption, and hard timeouts remain immediate.
- **Compaction (`defaults.compaction`)**: Set `trigger_tokens`, `target_tokens` (`< trigger_tokens`), `segment_tokens`, and optional fallback `estimator` (`bytes_per_token: 4.0`, `image_tokens: 2048`). Leave unset (`null`) to disable automatic compaction.

### Tools and delivery modes (`defaults.tools`)

Each capability has an enable toggle and, for CLI-capable tools, a delivery mode (`"command"` for `wish <subcommand>` inside the shell, or `"tool"` for a direct model function tool). If `shell` is turned off, enabled capabilities automatically fall back to direct tool delivery.

| Capability | Default enabled | Delivery field | Default delivery |
| --- | --- | --- | --- |
| `shell` | `true` | `task_delivery` | `"command"` |
| `blob` | `true` | `blob_delivery` | `"command"` |
| `history_search` | `true` | `history_search_delivery` | `"command"` |
| `ask_user` | `true` | `ask_user_delivery` | `"command"` |
| `web_search` | `true` | `web_search_delivery` | `"command"` |
| `web_fetch` | `true` | `web_fetch_delivery` | `"command"` |
| `sessions` | `true` | `sessions_delivery` | `"command"` |
| `mcp`, `skills` | `true` | — | Shell commands (`wish mcp`, `wish skill`) |
| `image_generate`, `video_generate`, `audio_generate`, `audio_transcribe` | `false` | `*_delivery` | `"tool"` |

## Proxy and shell

- **`proxy`**: `mode` can be `"environment"` (reads `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY`), `"manual"` (`url` plus optional `username` and `password`), or `"direct"`. Individual providers, accounts, extension services, and HTTP MCP servers can opt out via `proxy_enabled: false`.
- **`shell`**: Set `program` to an absolute path (empty picks the OS default) and optionally override `args` (omitted picks defaults for that shell family, such as `["-lc"]` for `bash`/`zsh`). Each session shell receives `$WISH_TMPDIR` and `$WISH_BLOB_*` / `$WISH_SESSION_ID_*` aliases automatically.

## MCP and skills

Configure MCP servers under `mcp.servers.<id>`:
- **Local (`transport: "stdio"`)**: `command`, `args`, `env`, optional `cwd`.
- **Remote (`transport: "http"`)**: Streamable HTTP `url`, `headers` (`${ENV_VAR}` supported), optional `auth_provider` to reuse a provider's key, and `proxy_enabled`.
- **Lifecycle**: `enabled` (`true`), `scope` (`"session"` per-session or `"shared"`), `idle_timeout` (`1800`s, `0` keeps alive), and `timeout` (`300`s).

For **Skills**, `wish skill` scans `SKILL.md` directories in priority order: `<session_cwd>/.agents/skills`, `<config_dir>/skills`, and `skills.dirs` (supports `~/`). Add names to `skills.disabled` to hide specific skills globally.

## Extension services

`search`, `fetch`, `image_generation`, `video_generation`, `audio_generation`, and `audio_transcription` share a uniform layout: an `order` list controlling priority/fallback, and a `providers` map where each entry specifies `preset`, optional `credential_id` (from `accounts`), `enabled`, `proxy_enabled`, optional `base_url`, plus `model` and `parameters` for media/ASR presets.

You can browse every supported preset and its fields in the web UI under **Settings → Extension tools**, or query `/api/*-presets`.

## Secrets and data directory

Keep secrets in environment variables (`bearer_token_env`, `api_key_env`, `credentials_env`, or `${ENV_VAR}` in headers). When reading `/api/config`, Wish masks stored secrets as `"<redacted>"`; sending `"<redacted>"` back preserves the saved values on disk.

Inside `data_dir`, Wish keeps `wish.sqlite`, `management.sqlite`, `blobs/<session_id>/`, `shell/<session_id>/`, `bin/wish`, `backups/`, and `format.json`.
