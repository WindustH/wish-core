# System Architecture

Wish runs as a single daemon that serves HTTP and SSE, drives active sessions, supervises session shell processes, and calls upstream AI services.

## Startup sequence

`src/main.rs` hands off to `server::run` (unless invoked internally with `--blob-worker`, which runs a bounded image/PDF extraction subprocess in `application::blob_preview::worker` and exits).

Before touching configuration or databases, `server::run` handles `--help`, `--version`, and session bridge CLI subcommands (`askuser`, `extension`, `history`, `task`, `blob`, `mcp`, `skill`, `session`). For normal server startup, it proceeds in order:

1. Locate `--config <path>` or create a default user config (`server/user_dirs.rs`).
2. Acquire an exclusive `.wish.lock` inside `data_dir`.
3. Run `migration::run(&config_path)` before parsing typed config structs or opening SQLite.
4. Construct `App` (`application/app.rs`), symlink the binary at `<data_dir>/bin/wish`, create `<config_dir>/skills`, start background maintenance and MCP idle reapers, and bind Axum (`server/http.rs`).
5. Wait for `SIGINT`/`SIGTERM` (or Windows console close/shutdown via `server/console.rs`) and run the ordered shutdown.

## Module layering

`wish-test/wish_test/architecture.py` parses imports across `src/` so higher orchestration never leaks into lower layers and HTTP handlers never bypass application services:

```text
HTTP / SSE / CLI
       │
    server/          request decoding, HTTP status mapping, SSE framing, OAuth listeners, static web
       │
    application/     config validation, service wiring, session lifecycle, previews, cleanup
       ├──────────────────────────┬──────────────────────┐
    executor/                  session/               storage/
    run loop, boundaries,      state machine,         SQLite engine, LRU,
    continuation, compaction   queue, history, stats  blobs, snapshots, management index
       │
    client/          upstream binding, renewal handles, retry loops
       │
    protocol/        LLM wire codecs, Endpoint/AuthScheme/SigV4/OAuth, Error taxonomy
    extensions/      bundled vendor packages (search, fetch, image, video, audio, ASR)
    extension_host/  neutral runner contracts and registries
       │
    transport/       single HTTP attempt, timeouts, body limits, proxy policy
```

Three boundaries matter most in day-to-day changes:

- **HTTP handlers stay thin**: code in `src/server/http/` translates requests and calls `application/`, with no direct filesystem mutations, `rusqlite` queries, storage handles, or `Client` calls.
- **Lower layers stay independent**: `application`, `task`, `storage`, and `protocol` cannot import layers above them.
- **Vendor isolation**: supplier implementations under `extensions/<vendor>/` only implement the extension contract, while `application`, `client`, `protocol`, `tool`, and `extension_host` never branch on vendor names or import vendor modules directly.

## Blocking work and resource pools

Async I/O runs on Tokio workers, while blocking filesystem work, SQLite waits, and CPU-heavy tasks enter through `utils::blocking` so heavy computations cannot starve I/O. It splits admission into two independent semaphore pools—16 permits for disk/database I/O and `max(1, min(CPUs - 1, 8))` permits for CPU work like file diffs or PDF/ZIP parsing. Once started, a blocking task holds its permit until completion, so blocking closures must never wait synchronously on another admitted blocking task.

Service HTTP clients (`ReqwestTransport`) live for the duration of an applied configuration, pooling up to 8 idle connections per origin. Fetch configuration snapshots sit behind `Arc` and swap atomically on config saves so active reads complete against their starting snapshot.

Image thumbnails and single-page PDF extractions (`application/blob_preview/`) run in isolated `--blob-worker` subprocesses (up to 2 concurrent slots, 30 s timeout) feeding a bounded 128 MiB / 128-entry cache. Periodic background maintenance evicts idle media and preview caches after 10 minutes.
