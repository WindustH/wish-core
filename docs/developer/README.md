# Developer Notes

`wish-core` is a single Rust 2024 binary crate (`wish`). These notes cover how the engine is layered, how sessions and tools execute, and which invariants must hold when changing serialized types or schemas.

For installation, `config.json`, or HTTP API usage, see the [User Guides](../user/README.md).

## Building and checking

Standard Cargo commands build and lint the crate:

```sh
cargo fmt --check
cargo clippy --all-targets
cargo build --release
```

`wish-core` keeps no inline unit tests inside `src/`; integration tests, protocol conformance checks, and import-boundary validation live in the sibling `wish-test` repository:

```sh
python3 ../wish-test/tools/check_architecture.py
```

Serialized types under `src/session/` and `src/protocol/` are part of the on-disk database format. Any breaking change to stored fields, enum variants, or SQLite schemas requires a matching step under `src/migration/`.

## Architecture index

- [System Architecture](architecture.md) — Startup sequence, module layering, HTTP delegation boundaries, and blocking-work admission.
- [Client & Transport](client.md) — `Client<T>`, subscription token renewal, retry lanes, timeouts, and Gemini stream recovery.
- [Protocol Layer](protocol.md) — `ModelUseProtocol` dialects, interrupted stream replay rules, `Endpoint`, `Error`, and auxiliary protocols.
- [Session Engine](session.md) — Single-owner `Session` model, durable queue, context generations, crash settlement, history search, and call records.
- [Executor & Compaction](executor.md) — Run loop, selection boundaries, cancellation scopes, output continuation, and local/upstream compaction.
- [Storage & Migrations](storage-and-migration.md) — Single-writer SQLite engine, `StoredValue::KIND`, blobs/aliases, and startup format migrations.
- [Tools & Extensions](tools-and-extensions.md) — Capability delivery, shell/task supervision, supplier extension isolation, MCP, skills, peers, and folders.
