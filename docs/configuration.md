# Configuration

Wish reads one JSON file, given on the command line:

```sh
wish --config /path/to/config.json
```

`wish --help` prints usage and `wish --version` the version. Any startup
problem (a missing file, invalid JSON, an unknown field, an unset environment
variable, a port in use) prints an error and exits with status 1.

The web app edits the same file through `PUT /api/config`, so it must stay
writable by the Wish process, and so must its directory (saves are written to
a temporary file and renamed into place). Most changes apply immediately; see
[what needs a restart](#applying-changes).

- [A complete example](#a-complete-example)
- [Top-level fields](#top-level-fields)
- [Providers](#providers)
- [Session defaults](#session-defaults)
- [Proxy](#proxy)
- [Shell](#shell)
- [Secrets](#secrets)
- [Applying changes](#applying-changes)
- [Data directory](#data-directory)

## A complete example

```json
{
  "listen": "127.0.0.1:9780",
  "data_dir": "/home/me/.local/share/wish",
  "bearer_token_env": "WISH_HTTP_TOKEN",
  "providers": {
    "openai": {
      "display_name": "OpenAI",
      "protocol": "openai_responses",
      "base_url": "https://api.openai.com",
      "path": "/v1/responses",
      "api_key_env": "OPENAI_API_KEY",
      "token_count": "openai_responses",
      "compaction": "openai_responses",
      "model_list": "openai_models",
      "model_list_path": "/v1/models"
    },
    "deepseek": {
      "preset": "deepseek",
      "protocol": "deepseek_chat",
      "base_url": "https://api.deepseek.com",
      "path": "/chat/completions",
      "api_key_env": "DEEPSEEK_API_KEY",
      "model_list": "openai_models",
      "model_list_path": "/models"
    }
  },
  "proxy": {"mode": "environment"},
  "shell": {"program": "/usr/bin/zsh"},
  "defaults": {
    "provider": "openai",
    "model": "gpt-5",
    "cwd": "/home/me/projects",
    "reasoning": {"effort": "medium"},
    "compaction": {"trigger_tokens": 200000, "target_tokens": 80000, "segment_tokens": 20000}
  }
}
```

Every field is optional; `{}` is a valid configuration that listens on
`127.0.0.1:9780`, stores data in `./data` and has no providers yet. Unknown
fields are rejected everywhere, so typos fail loudly.

## Top-level fields

| Field | Default | Description |
| --- | --- | --- |
| `listen` | `"127.0.0.1:9780"` | Address and port to listen on, as `IP:port` (`[::1]:9780` for IPv6). Host names are not accepted |
| `data_dir` | `"data"` | Where sessions and attachments are stored. A relative path is resolved against the directory Wish is started from, so prefer an absolute path |
| `bearer_token_env` | `null` | Name of an environment variable holding the API access token. When set, every `/api` request needs `Authorization: Bearer <token>`, and web pages on other origins may call the API (they still need the token). The variable must be set and non-empty at startup |
| `providers` | `{}` | Model providers by ID; see [Providers](#providers) |
| `defaults` | | Defaults for new sessions; see [Session defaults](#session-defaults) |
| `proxy` | environment | Outbound proxy; see [Proxy](#proxy) |
| `shell` | platform shell | The shell commands run in; see [Shell](#shell) |

## Providers

Each entry in `providers` connects Wish to one model service. The key is the
provider ID that sessions refer to, so keep it stable once sessions use it.

The easiest way to add a provider is the web app, which offers presets that
fill in the connection details. The preset catalog is also available from
[`GET /api/provider-presets`](api.md#get-apiprovider-presets). A hand-written
entry needs at least `protocol`, `base_url` and `path`.

| Field | Default | Description |
| --- | --- | --- |
| `protocol` | required | Wire protocol, see [below](#protocols) |
| `base_url` | required | `http://` or `https://` origin, optionally with a path prefix. Credential names such as `{region}` or `{workspace_id}` are replaced by their values |
| `path` | required | Request path appended to `base_url`. `{model}` is replaced by the model ID, and credential names as in `base_url` |
| `display_name` | `null` | Name shown in the web app |
| `preset` | `null` | ID of the preset this entry came from. It adds the preset's branding and default request headers, and enables ChatGPT sign-in for `openai_codex` |
| `enabled` | `true` | `false` keeps the entry but refuses new calls through it |
| `auth` | `"bearer"` | How requests are authenticated, see [below](#authentication) |
| `api_key` / `api_key_env` | `null` | The API key, or the name of an environment variable holding it; see [Secrets](#secrets) |
| `credentials` / `credentials_env` | `{}` | Extra credentials by name, or the environment variables holding them |
| `headers` | `{}` | Extra request headers. They override preset headers. `{session}` in a value becomes the Wish session ID, which some services use for caching |
| `proxy_enabled` | `true` | `false` always connects directly, ignoring the [proxy](#proxy) |
| `models` | `{}` | Per-model settings by model ID, see [below](#per-model-settings) |
| `model_list` | `null` | Catalog protocol, used for the model picker |
| `model_list_path` | `null` | Catalog path, required with `model_list` |
| `model_list_base_url` | `null` | Catalog origin when it differs from `base_url` |
| `token_count` | `null` | Token-counting protocol, for exact counts during compaction |
| `compaction` | `null` | Upstream compaction protocol, to let the provider compact the context itself |
| `account_state` | `null` | Protocol for reading balance or quota |
| `refresh_token`, `expires_at` | `null` | Written by ChatGPT sign-in; not meant to be edited by hand |

### Protocols

`protocol` selects the provider's native API. Vendor variants handle each
service's reasoning format and quirks, including which message roles it accepts:
only `openai_chat` sends the `developer` role, which other chat services refuse.

| Family | Values |
| --- | --- |
| OpenAI Chat Completions | `openai_chat` (OpenAI itself), `compatible_chat` (any other OpenAI-compatible service or local server), `deepseek_chat`, `zai_chat`, `kimi_k2_chat`, `kimi_k3_chat`, `qwen_chat`, `minimax_chat`, `mimo_chat`, `tokenhub_chat`, `mistral_chat` |
| OpenAI Responses | `openai_responses`, `plaintext_responses` (services that return readable reasoning), `codex_responses` (ChatGPT subscription) |
| Anthropic Messages | `anthropic_messages`, `deepseek_messages`, `zai_messages`, `kimi_messages`, `qwen_messages`, `minimax_messages`, `mimo_messages`, `tokenhub_messages` |
| Google | `google_generate_content` (Gemini API), `google_vertex_generate_content`, `google_interactions` |
| Others | `bedrock_converse` (AWS Bedrock), `mistral_conversations` |

Optional capabilities, each of which must match the protocol:

| Field | Values |
| --- | --- |
| `token_count` | `openai_responses` (with `openai_responses` or `plaintext_responses`), `anthropic_messages` (with `anthropic_messages`), `google_generate_content` (with `google_generate_content`) |
| `compaction` | `openai_responses` (with `openai_responses` or `plaintext_responses`), `openai_responses_streamed` (with `codex_responses`) |
| `model_list` | `openai_models`, `openai_codex_models`, `qwen_models`, `anthropic_models`, `google_models`, `bedrock_models` |
| `account_state` | `deepseek_user_balance`, `kimi_open_balance`, `kimi_code_companion_usage`, `zai_coding_plan_monitor`, `minimax_token_plan_remains`, `minimax_account_balance`, `siliconflow_balance`, `openrouter_key_quota`, `openrouter_credits`, `hf_whoami_billing`, `qwen_workspace_quota`, `openai_codex_usage` |

A mismatched pairing is refused when the configuration is loaded or saved.

### Authentication

| `auth` | Sends |
| --- | --- |
| `bearer` | `Authorization: Bearer <api_key>` |
| `anthropic_key` | `x-api-key: <api_key>` |
| `google_key` | `x-goog-api-key: <api_key>` |
| `sig_v4` | AWS Signature V4, from the `region`, `access_key_id`, `secret_access_key` and optional `session_token` credentials |
| `none` | Nothing (local servers such as Ollama) |

Credential names are `region`, `access_key_id`, `secret_access_key`,
`session_token`, `account_id`, `workspace_id`, `team_id`, `organization` and
`project`.

### ChatGPT sign-in

A provider with `"preset": "openai_codex"` uses a ChatGPT subscription instead
of an API key. Add it in the web app and choose **Sign in with ChatGPT**. Wish
stores the resulting tokens in this file and renews them in the background.
The sign-in opens a temporary callback on `127.0.0.1:1455`, so the browser
must run on the same machine as Wish; otherwise, paste the final redirect URL
back into the web app. The flow is described in the
[API reference](api.md#chatgpt-sign-in).

### Per-model settings

`models` maps a model ID to an object of settings. Wish itself reads
`input_modalities`: when it is present and lacks `"image"`, images are replaced
with a short text notice naming the file. The web app also stores
`context_window_tokens`, `max_output_tokens`, `supports_reasoning`,
`reasoning_efforts` and `default_reasoning_effort` here to drive its model
editor, context gauge and reasoning picker.

```json
"models": {
  "deepseek-chat": {"context_window_tokens": 128000, "input_modalities": ["text"]}
}
```

## Session defaults

`defaults` is what the web app offers when you start a new chat. Existing
sessions keep their own settings.

| Field | Default | Description |
| --- | --- | --- |
| `provider` | `""` | Default provider ID; must exist in `providers` |
| `model` | `""` | Default model ID |
| `cwd` | Wish's start directory | Default working directory, an absolute path |
| `shell` | `true` | Give new sessions shell access |
| `stream` | `true` | Stream model output |
| `instructions` | `""` | Extra instructions the web app adds to each new session |
| `reasoning` | `null` | `{"enabled": bool, "effort": "low", "summary": "Auto"}`. Accepted effort values depend on the provider |
| `max_output_tokens` | `null` | Output limit per model call |
| `compaction` | `null` | Automatic compaction, see below. Without it, sessions never compact |

### Compaction

```json
"compaction": {"trigger_tokens": 200000, "target_tokens": 80000, "segment_tokens": 20000}
```

| Field | Description |
| --- | --- |
| `trigger_tokens` | Compact once a request's input reaches this size |
| `target_tokens` | Size to bring the context down to; must be below `trigger_tokens` |
| `segment_tokens` | Size of the chunks that are summarized ahead of time |
| `estimator` | Optional `{"bytes_per_token": 4.0, "image_tokens": 2048}` for providers without token counting |

Pick `trigger_tokens` comfortably below the model's context window, leaving
room for the answer. Providers with a `compaction` protocol compact on their
side; others use summaries that Wish prepares in the background. The complete
history is kept either way. Individual sessions can change these values in the
web app's session settings. Details are in
[compaction internals](internals/compaction.md).

## Proxy

`proxy` controls how Wish reaches providers.

| Field | Description |
| --- | --- |
| `mode` | `environment` (default): use `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` from Wish's environment. `manual`: use `url`. `direct`: no proxy |
| `url` | `http://` or `https://` proxy address, required for `manual` |
| `username`, `password` | Optional basic authentication. Keep credentials here rather than in `url` |

Providers with `"proxy_enabled": false` always connect directly.

## Shell

`shell` selects the program that runs the agent's commands.

| Field | Description |
| --- | --- |
| `program` | Absolute path to the shell. Empty uses `/bin/sh` on Unix and `%COMSPEC%` (cmd.exe) on Windows |
| `args` | Arguments placed before the command text. `null` (default) chooses by shell: `-lc` for bash and zsh, `-l -c` for fish, `-NoLogo -NoProfile -NonInteractive -Command` for PowerShell, `/D /S /C` for cmd, `-c` otherwise |

Commands run in the session's working directory with Wish's own user,
permissions and environment. A changed shell applies from the next command of
every session that follows the global setting. A session can also have its
own shell, set in the web app or with
[`PUT /api/sessions/{id}/shell`](api.md#put-apisessionsidshell).

## Secrets

Provider keys and credentials can be written directly (`api_key`,
`credentials`) or referenced by environment variable (`api_key_env`,
`credentials_env`); a variable takes precedence over a direct value. In the web
app, typing `${NAME}` into a key field saves it as a variable reference.

Environment variables are read from Wish's own environment. It is fixed when
the process starts, so export new variables and restart Wish before saving a
configuration that refers to them. A reference to a missing or empty variable
is refused (for disabled providers it is not read).

`GET /api/config` replaces `api_key`, `refresh_token`, every `credentials` and
`headers` value, and the proxy password with `"<redacted>"`. Sending that
placeholder back keeps the stored value. The file on disk holds the real
values, so protect it accordingly (for example `chmod 600`).

## Applying changes

| Change | Takes effect |
| --- | --- |
| Providers, proxy | On the next model call. Calls in progress finish with the old settings |
| Shell | On the next command |
| Defaults | For the next new session |
| `listen`, `data_dir`, `bearer_token_env` | After a restart. The API refuses to change them |
| Values of environment variables | After a restart |

Saves through the API check a revision, so two editors cannot overwrite each
other. Editing the file by hand while Wish runs makes the running revision
stale: restart Wish after a manual edit, before saving from the web app.

## Data directory

| Path | Contents |
| --- | --- |
| `wish.sqlite` | Sessions: messages, events, context generations, queue, model calls and the search index |
| `management.sqlite` | Session list, per-provider usage records and streaming-speed samples |
| `blobs/<session>/` | Uploaded attachments and images the agent viewed |
| `shell/<session>/` | Captured output of every shell command |

Deleting a session removes its rows and both of its directories. Back up the
whole directory while Wish is stopped, or copy the databases with
`sqlite3 wish.sqlite ".backup backup.sqlite"` while it runs. See
[deployment](deployment.md#backups-and-upgrades).
