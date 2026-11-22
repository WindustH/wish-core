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
| `ssh` | Disabled | Optional public SSH port, host identity and user public-key allowlist. Requires loopback HTTP. See [deployment](deployment.md#public-ssh-access-from-a-device-local-wish-web). | Restart to change settings; allowlist checked on login |
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
- **Google Antigravity (`google_antigravity`)**: Google OAuth (`localhost:51121` callback, automatically enrolls the account and discovers a Cloud Code project ID when supplied by Google; consumer accounts do not require a project).
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

### Incremental model uploads

Configure these optional fields under a single model provider. A session keeps one volatile
checkpoint. A subsequent turn can reuse it after its previous response has entered local
history. Wish verifies the provider instance, credentials, generation, model parameters,
tools and ordered projected messages. Reopening a session after a server restart sends a
complete first request. The durable conversation stays usable independently of the provider.

| `incremental` value | Supported endpoint and behavior |
| --- | --- |
| `responses_websocket` | Streaming OpenAI Responses and ChatGPT Codex. Sends `response.create` on a session-owned WebSocket with `store: false`. The official Codex endpoint selects this mode by default. Idle sockets close after two minutes. |
| `responses` | HTTP Responses deployments with documented stored-response chaining, including OpenAI and Zhipu's Responses endpoint. Sets `store: true` and sends `previous_response_id` with subsequent inputs. |
| `openai_conversations` | OpenAI platform Responses with a durable conversation object. Creates the object at the sibling `/conversations` endpoint and attaches each generation using `conversation`. Items persist without the response chain’s 30-day TTL. |
| `ark_responses` | Volcengine Ark Responses session cache. Enables `caching` on creation and continuation. The model and account must support explicit caching. |
| `anthropic_threads` | Official Anthropic Messages with the message threads beta. Creation sends tools and system instructions. Continuation sends the new user or tool-result turn with the previous message ID. |
| `google_interactions` | Gemini Interactions. Stores interactions, resends generation controls and tools, and chains with `previous_interaction_id`. |
| `mistral_conversations` | Mistral Conversations. Appends inputs to the stored conversation's endpoint. An ambiguous append failure ends the attempt. |
| `disabled` | Sends full model requests, including on the official Codex deployment. |

Stored responses, uploaded files, and explicit prefix caches use the provider's retention
and billing rules. Enable each feature on the specific endpoint that implements it. A
Chat Completions endpoint with automatic token caching still receives full message history.
Antigravity's generation recovery tokens apply within a stream. DeepSeek documents its
Responses endpoint as stateless. For DeepSeek vision models, select file reuse to avoid
resending historical image bytes. [DeepSeek Responses API](https://api-docs.deepseek.com/guides/responses_api/).
[DeepSeek vision](https://api-docs.deepseek.com/guides/vision/).

For example, an official Anthropic provider can set `"incremental": "anthropic_threads"`.
An Ark Responses provider can set `"incremental": "ark_responses"`. Keep the existing
model protocol, endpoint, credentials and model selection when adding these fields.
The account, region and model must have access to the upstream feature.
[Anthropic message threads](https://platform.claude.com/docs/en/build-with-claude/message-threads).
[OpenAI WebSocket mode](https://developers.openai.com/api/docs/guides/websocket-mode).

Use `resources.files` to upload images and reuse file IDs on subsequent generation calls.
Both the `protocol` and `path` are required. Paths append to the configured `base_url`,
including any API version already present there.

| File protocol | Generation wire | Example upload path |
| --- | --- | --- |
| `openai` | OpenAI platform Responses | `/v1/files` |
| `deepseek` | DeepSeek Chat, Messages or Responses | `/files` |
| `anthropic` | Anthropic Messages | `/v1/files` |
| `kimi` | Kimi vision Chat | `/v1/files` |
| `gemini` | Gemini GenerateContent or Interactions | `/upload/v1beta/files` |

For example, a DeepSeek model provider with base URL `https://api.deepseek.com` can set
`"resources": {"files": {"protocol": "deepseek", "path": "/files"}}`.
Identical image bytes share a file reference within the same authenticated session.
OpenAI and DeepSeek uploads request a one-day expiration. Other uploads follow the
provider's file lifecycle. Local reference eviction does not delete a provider file.

Use `resources.prefix` with `{"protocol": "gemini", "path": "/v1beta/cachedContents"}`,
`vertex` with the project's cachedContents collection, or `ark` with its Responses path.
Wish creates a cache for a stable prefix of at least 4096 serialized bytes, subject to the
provider's token threshold. Google receives the inherited system instructions and tools in
the cache creation request. Later requests carry the returned name and the remaining contents.
Ark creates a non-streaming prefix and references its ID. A session continuation takes precedence
when a previous response is available.

For an image already held in provider-accessible object storage, set
`resources.image_locations` to an object mapping the SHA-256 digest of the admitted image bytes
to its URI. Vertex accepts `gs://`, Bedrock Converse accepts `s3://`, and Qwen Chat accepts
`oss://` on models and accounts that support the respective storage service. The operator
provisions object permissions. Wish sends the URI in the model request.

Set `WISH_MODEL_TRANSFER_LOG=1` on the server to log generation and resource request body
sizes. `image_base64_bytes` counts encoded image bytes expanded into the serialized model
body. These counts exclude TLS, HTTP and WebSocket framing.

## Session defaults (`defaults`)

New sessions copy their starting settings from `defaults`:

- **Model & prompt**: `provider`, `model`, `cwd` (defaults to the server's starting directory), `stream` (`true`), `instructions` (`""`), `reasoning` (`null`), `max_output_tokens` (`null`).
- **Execution policy (`defaults.run`)**: `tools` (`"Parallel"` by default, or `"Serial"`) and `background_tools_on_input` (`true`). When you send new input, foreground shell commands get up to one second to finish and return their results inline; commands still running then continue in the background. Explicit background requests, interruption, and hard timeouts remain immediate.
- **Compaction (`defaults.compaction`)**: Set `trigger_tokens`, `target_tokens` (`< trigger_tokens`), `segment_tokens`, and optional fallback `estimator` (`bytes_per_token: 4.0`, `image_tokens: 2048`). Leave unset (`null`) to disable automatic compaction.

If a session's working directory disappears, Wish checks it when reopening the session, before model turns, and before starting shell commands. It switches the session to the current `defaults.cwd`, saves that change, and tells the model which directory disappeared and where subsequent commands will run. It does not recreate the missing directory. If the default is also unavailable, execution reports an error; history remains accessible and you can choose an existing directory in session settings.

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

- **`proxy`**: `mode` can be `"environment"` (reads `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`/`NO_PROXY`), `"manual"` (`url` plus optional `username` and `password`), or `"direct"`. Manual proxy URLs support HTTP, HTTPS, SOCKS5 (local DNS) and SOCKS5H (proxy DNS). Individual providers, accounts, extension services, and HTTP MCP servers can opt out via `proxy_enabled: false`.
- **`shell`**: Set `program` to an absolute path (empty picks the OS default) and optionally override `args` (omitted picks defaults for that shell family, such as `["-lc"]` for `bash`/`zsh`). Each session shell receives `$WISH_TMPDIR` and `$WISH_BLOB_*` / `$WISH_SESSION_ID_*` aliases automatically.

`shell.env` is a map of preset environment variables for new commands, layered over the server process's inherited environment. Values are passed literally, including spaces and newlines; `$NAME` is not expanded. Names must match `[A-Za-z_][A-Za-z0-9_]*`; `WISH_*` names are reserved. Removing a preset restores the inherited value, while an empty string sets an empty value. Existing processes retain their environment.

Set `shell.use_network_proxy` to `true` to follow network settings (default: `false`). Manual mode sets `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and their lowercase forms, including URL-encoded proxy credentials. Environment mode inherits the server's proxy variables; direct mode removes those six inherited variables. `NO_PROXY`/`no_proxy` remain inherited. Explicit `shell.env` entries take priority over these generated values. Changes to either shell or network settings apply to the next command without restarting Wish.

```json
"shell": {
  "program": "",
  "args": null,
  "env": { "LANG": "en_US.UTF-8", "PROJECT_MODE": "development" },
  "use_network_proxy": true
}
```

A session's `shell_command` override supports the same fields and replaces the global Shell configuration. Its proxy-following option still tracks network settings. Configuration and session APIs mask nonempty environment values as `<redacted>`; sending those placeholders back preserves the stored values. Model environment instructions list variable names only, never their configured values.

## MCP and skills

Configure MCP servers under `mcp.servers.<id>`:
- **Local (`transport: "stdio"`)**: `command`, `args`, `env`, optional `cwd`.
- **Remote (`transport: "http"`)**: Streamable HTTP `url`, `headers` (`${ENV_VAR}` supported), optional `auth_provider` to reuse a provider's key, and `proxy_enabled`.
- **Lifecycle**: `enabled` (`true`), `scope` (`"session"` per-session or `"shared"`), `idle_timeout` (`1800`s, `0` keeps alive), and `timeout` (`300`s).

For **Skills**, `wish skill` scans `SKILL.md` directories in priority order: `<session_cwd>/.agents/skills`, `<config_dir>/skills`, and `skills.dirs` (supports `~/`). Add names to `skills.disabled` to hide specific skills globally.

## Extension services

`search`, `fetch`, `image_generation`, `video_generation`, `audio_generation`, and `audio_transcription` share a uniform layout: an `order` list controlling priority/fallback, and a `providers` map where each entry specifies `preset`, optional `credential_id` (from `accounts`), `enabled`, `proxy_enabled`, optional `base_url`, plus `model` and `parameters` for media/ASR presets.

You can browse every supported preset and its fields in the web UI under **Settings → Extension tools**, or query `/api/*-presets`.

Image and media catalog entries may include `service: {id, name}` to group model templates under one service in the UI. For example, xAI video models share one service, while regional endpoints and different account types remain separate. The underlying template `id` still selects its wire contract; existing `preset` values and durable media jobs remain valid. Selecting another model template changes both `preset` and `model`, retaining the configured account and endpoint override.

## Secrets and data directory

Keep secrets in environment variables (`bearer_token_env`, `api_key_env`, `credentials_env`, or `${ENV_VAR}` in headers). When reading `/api/config`, Wish masks stored secrets as `"<redacted>"`; sending `"<redacted>"` back preserves the saved values on disk.

Inside `data_dir`, Wish keeps `wish.sqlite`, `management.sqlite`, `blobs/<session_id>/`, `shell/<session_id>/`, `bin/wish`, `backups/`, and `format.json`.

## Interactive terminal in the Web sidebar

Opening **Terminal** automatically connects a real PTY in the session's working directory. **Background tasks** is a separate tab for model-started commands. With persistence off, Ctrl-D exits the current shell; while the terminal page remains open, Wish starts another shell. Detaching or exiting a multiplexer leaves it disconnected until you reconnect. Repeated startup failures stop automatic retries and show an error.

**Settings → Terminal** controls the interactive terminal independently of the model command runner:

```json
"terminal": {
  "shell": "",
  "persistence": "none",
  "per_session": false,
  "session_name": "",
  "font_size": 13,
  "colors": {}
}
```

- `shell`: absolute executable path, or empty for the service user's native shell (`$SHELL` / account database on Unix; `COMSPEC` on Windows). Unix shells load their login profile and interactive startup files. Environment variables, proxy settings and Wish aliases still follow the session's Shell settings.
- `persistence`: `none`, `tmux` or `zellij`. Install the chosen multiplexer on the server. With persistence off, switching tabs, sessions or pages keeps the terminal running; exiting or refreshing the frontend closes it and its jobs. Persistent terminals only detach their multiplexer client. Browser disconnection allows 30 seconds for reconnection before the PTY is closed. A started PTY that never gets a connection or polling activity also expires after 30 seconds.
- `per_session`: defaults to `false`. Multiplexers attach to an **existing** session, optionally selected by `session_name`; missing sessions are not created implicitly. Without a name, tmux selects its existing session and zellij attaches to the first existing session. Set `true` to create/reuse an isolated multiplexer session for each Wish session, ignoring `session_name`.
- Persistent sessions retain their shell, environment and jobs when the Web page detaches. Existing multiplexer sessions do not change shell when settings change. Their lifetime is managed by the multiplexer and host service manager; stopping the host or killing its process group can still end them.
- `font_size`: 8–32 pixels. The default **Wish** palette follows light/dark appearance. `colors` overrides individual xterm theme colors using `#RRGGBB`: `background`, `foreground`, `cursor`, `selectionBackground`, the eight ANSI colors, and their `brightBlack` … `brightWhite` variants.

Shell/persistence changes apply to new connections; font and palette changes apply immediately after saving. Commands run as the Wish service user and are separate from the model transcript. Multiple browser clients share one session PTY's input and dimensions.

Input, output, resize and latency probes use a dedicated WebSocket. An authenticated POST exchanges credentials for a 30-second, single-use, session-scoped ticket; the long-lived bearer token is never placed in the WebSocket URL. The server retains the latest 1 MiB of PTY bytes. Reconnecting beyond that retention resets the display with a notice. A stalled renderer cannot accumulate unbounded output; uncertain input is never replayed automatically.

Terminal profiles should preserve the emulator-provided `TERM`. Forcing `TERM=xterm-kitty` in Wish can break Neovim RGB colors and make Yazi choose unsupported Kitty placeholders. The Web terminal supports direct Kitty image transfers and regular placements, Sixel and iTerm inline images. Direct Yazi sessions prefer Kitty; its theme editor imports JSON and `.itermcolors` files.
