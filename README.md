# Wish

The `wish-core` package builds a single `wish` executable with its HTTP server and engine in one process. Providers, the agent executor,
SQLite storage and built-in tools run in the same process.

```text
HTTP / SSE
    |
    +-- providers ------> core Client ------> upstream
    |
    +-- sessions -------> Session + executor
    |                         |       |
    |                         |       +-- shell / view_image / search_history
    |                         v
    +-- history / calls ---> SQLite + LRU + search index
```

## Run

```sh
cp config.example.json config.json
export WISH_HTTP_TOKEN='your-local-api-token'
export OPENAI_API_KEY='your-provider-key'
cargo run --release -- --config config.json
```

`cargo build --release` produces `target/release/wish` directly from this repository. No second daemon is needed. `--help` and `--version` are available.
Configuration is JSON. `/api/config` hot-updates providers and defaults with revision checking; startup settings require restart. Relative `data_dir`
paths resolve against the process working directory. Provider IDs and model IDs are separate.

By default the listener is `127.0.0.1:9780`. Set `bearer_token_env` to the environment
variable containing the inbound token; omit it for unauthenticated access. All `/api`
routes share this boundary. `/health` and `/version` are public. Shell runs with the
server's OS permissions, in the session's absolute `cwd`; it is opt-in per session.
There is no user/tenant permission model or shell sandbox.

## Example session

```sh
curl http://127.0.0.1:9780/api/sessions \
  -H "Authorization: Bearer $WISH_HTTP_TOKEN" -H 'Content-Type: application/json' \
  -d '{"provider":"openai","cwd":"/tmp","shell":true,"metadata":{"project":"demo"},
       "config":{"model":"YOUR_MODEL_ID","stream":true,"tools":[],"run":{"tools":"Serial"}}}'
```

Use the returned `session.id` below:

```sh
curl http://127.0.0.1:9780/api/sessions/SESSION_ID/messages \
  -H "Authorization: Bearer $WISH_HTTP_TOKEN" -H 'Content-Type: application/json' \
  -d '{"User":{"content":[{"Text":{"text":"Inspect the current directory"}}]}}'
curl -X POST http://127.0.0.1:9780/api/sessions/SESSION_ID/run \
  -H "Authorization: Bearer $WISH_HTTP_TOKEN"
curl -N http://127.0.0.1:9780/api/sessions/SESSION_ID/events \
  -H "Authorization: Bearer $WISH_HTTP_TOKEN"
```

The protocol `messages` endpoint only enqueues. The WebUI `input` endpoint queues and schedules execution. `run` explicitly resumes the session and returns `202`.
The task continues if an HTTP connection disconnects. `interrupt` requests cancellation;
wait for `operation_finished` or `status.running=false` before changing config or restarting.
A second simultaneous run/compact returns `409`.

`search_history` is installed for every session; `shell` is installed when enabled.
Creation takes an empty `config.tools`; the server supplies the real specifications.
Subsequent config updates accept these installed tools and refresh their specifications.
Other tool names have no executor and are rejected.

SIGINT/SIGTERM stops new operations, closes SSE streams, cancels active executions,
waits for core to persist accepted partial output and finish tool cleanup, stops remaining
background shell processes, and closes storage. On restart sessions load lazily and do not
resume automatically. After abrupt process termination, core may retain an unfinished
execution state; this server reports it and refuses to replay its effects automatically.

See [API](docs/server/api.md) for routes, payloads, paging and stream semantics.

Validation lives outside the crate: from `../wish-test`, run
`python3 -m unittest tests.test_wish_server -q`. It uses a local mock provider and exercises
real HTTP, shell execution, interruption, persistence and graceful process shutdown.
