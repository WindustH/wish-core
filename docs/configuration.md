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
- [MCP servers](#mcp-servers)
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
  "mcp": {
    "servers": {
      "context7": {"transport": "stdio", "command": "npx", "args": ["-y", "@upstash/context7-mcp"]},
      "zai-search": {"transport": "http", "url": "https://open.bigmodel.cn/api/mcp/web_search_prime/mcp",
                     "auth_provider": "zai", "scope": "shared"}
    }
  },
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
| `mcp` | `{"servers": {}}` | MCP servers sessions can call; see [MCP servers](#mcp-servers) |
| `search` | `{"order": [], "providers": {}}` | The services the `web_search` tool asks; see [Web search](#web-search) |

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
| `account_state_base_url` | `null` | Host the account reading is asked on, for a service with regional twins (`https://open.bigmodel.cn` for a Zhipu plan). Without it, the protocol's own host; the provider's `base_url` is never used for it |
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
| `tools` | `{"shell": true, "ask_user": true, "mcp": true, "web_search": true}` | The optional tools new sessions get: `shell` runs commands in the working directory, `ask_user` lets the model ask you questions, `mcp` lets it call [MCP servers](#mcp-servers) from the shell, `web_search` lets it search the web through the [search providers](#web-search). Each session can switch them later. A new session gets `web_search` only while some search provider can answer |
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
permissions and environment. On Windows the agent is told that commands run on
Windows, and output that is not UTF-8 is read in the system's OEM code page, the
one cmd.exe writes in (GBK on Chinese Windows). A changed shell applies from the next command of
every session that follows the global setting. A session can also have its
own shell, set in the web app or with
[`PUT /api/sessions/{id}/shell`](api.md#put-apisessionsidshell).

## MCP servers

`mcp.servers` lists the MCP servers sessions can use, by name. Sessions reach
them from their shell with the `wish mcp` command, which the agent instructions
of every session with a shell describe: the model runs
`wish mcp list`, `wish mcp describe <server>/<tool>` and
`wish mcp call <server>/<tool> '<json>'`, and reads the answers as command
output. The servers and their tools never appear in the model's tool list, so
adding a server, removing one or a server changing its tools does not
invalidate a conversation's prompt cache. MCP needs the shell: without it a
session has no way to run the command.

A session's shell finds `wish` through a directory Wish puts first on its
`PATH`: `<data_dir>/bin`, which holds a `wish` link to the running program, on
Linux and macOS, and the program's own directory on Windows, where creating a
link takes a privilege ordinary users lack. On Windows the agent is told to pass
`wish mcp call` its arguments on standard input, since cmd.exe cannot quote JSON
reliably.

A session's `tools.mcp` switch decides only what the command answers. With it
off, every `wish mcp` command says MCP is disabled for the session and that you
can enable it in the session's settings; the model's request is the same either
way, so switching it keeps the cache too.

| Field | Default | Description |
| --- | --- | --- |
| `transport` | required | `stdio` for a local program, `http` for a remote server (Streamable HTTP) |
| `enabled` | `true` | A disabled server is kept in the file but not offered |
| `command`, `args` | | stdio: the program and its arguments. A bare name is looked up on `PATH` |
| `env` | `{}` | stdio: variables added to Wish's environment for the program |
| `cwd` | | stdio: where the program starts. Absent, a session's instance starts in the session's working directory and a shared one in `defaults.cwd` |
| `url` | | http: the server's endpoint |
| `headers` | `{}` | http: headers sent with every request |
| `auth_provider` | | http: a provider ID whose key is sent as `Authorization: Bearer`, for a server that comes with a subscription the provider already has (Zhipu's search MCP with a GLM Coding Plan key, for example) |
| `proxy_enabled` | `true` | http: use the [proxy](#proxy) |
| `scope` | `"session"` | `session` runs one instance per session; `shared` has every session use one |
| `idle_timeout` | `1800` | Seconds without a call after which an instance is closed; the next call starts it again. `0` keeps it open |
| `timeout` | `300` | Seconds a call may go without an answer or a progress report. Past it the call is abandoned and the server is told to stop |

A server runs once per session by default, started the first time that session
calls it. That is how MCP servers are written to be used (one client each),
so it is right whether or not a server keeps state, such as a browser it
drives. Use `scope: "shared"` only for servers you know keep none, where one
instance saves starting a program per session. A local server runs in a
process group of its own, and closing it ends everything it started.

The tools a server offers are listed when it first connects and shared by all
of its instances, so `wish mcp list` in a new session does not start anything.
Server names may use letters, digits, `-` and `_`. For an `http` server, the
transport sets `Accept`, `Content-Type`, `Mcp-Session-Id`,
`MCP-Protocol-Version` and `Last-Event-ID` itself, and `auth_provider` excludes
an `Authorization` header.

## Web search

`search.providers` lists the services the `web_search` tool asks, by ID, and
`search.order` the order it asks them in. When one cannot answer - its quota is
spent, its key refused, its service down, or it takes longer than 60 seconds -
the next is asked, and a search fails only when every one has. A provider left
out of `order` is never asked.

The model sees one `web_search` tool whichever provider answers. Its
description and parameters never name a service, so changing the providers or
their order does not change the model's request or its prompt cache.

```json
"search": {
  "order": ["chatgpt", "tavily"],
  "providers": {
    "chatgpt": {"preset": "openai_codex_search", "auth_provider": "openai_codex"},
    "tavily": {"preset": "tavily", "api_key_env": "WISH_SEARCH_TAVILY_KEY"}
  }
}
```

A provider is one of two kinds:

- **Borrowed.** Search that comes with a subscription uses a model provider's
  account: `auth_provider` names it. The search uses that provider's
  credentials and proxy setting as they stand at the moment of the search, so
  a renewed ChatGPT sign-in applies at once. A borrowed provider takes no key of
  its own, and only the model presets its preset lists can lend it their
  account. While that model provider is disabled or missing its credentials,
  the search provider is skipped.
- **Own key.** Any other search service takes its own key, like a model
  provider: `api_key`, or `api_key_env` naming an environment variable.

| Field | Default | Description |
| --- | --- | --- |
| `preset` | required | A search preset ID (below); it names the protocol and the service's address |
| `enabled` | `true` | A disabled provider is kept in the file but never asked |
| `auth_provider` | | Borrowed: the model provider whose account the search uses |
| `api_key`, `api_key_env` | | Own key: the key, or the environment variable that holds it |
| `base_url` | the service's | Where the service is: required for a self-hosted one, otherwise only to reach another address |
| `proxy_enabled` | `true` | Own key: use the [proxy](#proxy). A borrowed provider follows its model provider |
| `headers` | `{}` | Headers added to every search request |

Search presets:

| Preset | Kind | Service |
| --- | --- | --- |
| `openai_codex_search` | Borrowed from `openai_codex` | ChatGPT's search, the one Codex uses: `POST {base_url}/alpha/search` with the model provider's base URL |
| `kimi_code_search` | Borrowed from `kimi_code` | Kimi Code's search, the one Kimi's coding CLI uses: `POST {base_url}/search`. Kimi expects clients to identify themselves truthfully and may refuse ones it does not know |
| `minimax_coding_plan_search` | Borrowed from `minimax_token_cn`, `minimax_token_global` | MiniMax Token Plan's search, on the host of the plan's region. Ten results at most, no filters |
| `tavily_keyless` | No key | [Tavily](https://docs.tavily.com/documentation/keyless) without an account: free and rate-limited by Tavily |
| `tavily` | Own key | [Tavily](https://docs.tavily.com) with a key (`tvly-…`) |
| `exa` | Own key | [Exa](https://exa.ai/docs/reference/search); the snippet is Exa's highlights |
| `perplexity_search` | Own key | [Perplexity's Search API](https://docs.perplexity.ai/docs/search/quickstart) |
| `brave` | Own key | [Brave Search](https://api-dashboard.search.brave.com/app/documentation/web-search/get-started) |
| `serper` | Own key | Google results through [Serper](https://serper.dev) |
| `jina` | Own key | [Jina](https://jina.ai/reader)'s search; it cannot limit results by date |
| `searxng` | No key, `base_url` required | A self-hosted [SearXNG](https://docs.searxng.org/dev/search_api.html). Its JSON output must be on (`search.formats: [html, json]` in settings.yml), and its limiter, if on, allows API requests only a few times an hour: turn it off or let Wish's address through |
| `bocha` | Own key | [博查 Bocha](https://open.bochaai.com), a mainland China service |
| `metaso` | Own key | [秘塔 Metaso](https://metaso.cn/search-api/playground), a mainland China service; no filters |

Services without domain filters of their own get `site:` operators in the query
where their engines honour them (Brave, Serper, SearXNG), and every service's
results are filtered by domain again after the reply.

The results the model reads are the pages found - title, URL, site, date and a
short snippet - with a note that they come from the web and are not verified.
What a service cannot filter itself (a blocked domain, for example) is
filtered after its reply, and a filter that cannot be applied at all is
mentioned in the result.

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
`headers` value, every MCP server's `env` and `headers` value, and the proxy
password with `"<redacted>"`. Sending that
placeholder back keeps the stored value. The file on disk holds the real
values, so protect it accordingly (for example `chmod 600`).

## Applying changes

| Change | Takes effect |
| --- | --- |
| Providers, proxy | On the next model call. Calls in progress finish with the old settings |
| Shell | On the next command |
| MCP servers | On the next call. An instance whose server changed, or would now be reached with another key or proxy, is closed and started again |
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
| `shell/<session>/` | Captured output of every shell command, and in `mcp/` the images and other files MCP tools returned |
| `bin/` | A link to the Wish program, first on sessions' shell `PATH` so `wish mcp` runs the same build |
| `format.json` | The version of the stored formats, `{"version": N}`, which a newer release migrates from |
| `backups/` | Copies of the databases and the configuration file taken before each migration |

Deleting a session removes its rows and both of its directories, and the database shrinks by what they took. Back up the
whole directory while Wish is stopped, or copy the databases with
`sqlite3 wish.sqlite ".backup backup.sqlite"` while it runs. See
[deployment](deployment.md#backups-and-upgrades).
