# Internals

Design notes for the modules under `src/`. They are internal modules of the `wish` binary crate,
not a public library: code examples use `crate::` paths and `pub` means crate visibility. The HTTP
surface is described in [the API reference](../api.md) and the config file in
[configuration](../configuration.md).

## Process layout

One process serves HTTP, drives sessions and calls upstreams. `src/main.rs` calls `server::run`,
which reads `--config <file>` (or the user's file, `server/user_dirs.rs`), opens storage and serves
until SIGINT/SIGTERM (on Windows until Ctrl-C, Ctrl-Break, console close or system shutdown; see
`server/console.rs`). Besides the API it serves the web app when one is installed, and a server
without a token checks every request's `Host` and changes' `Origin` first (`server/web.rs`).

```text
HTTP / SSE / CLI
       |
    server/        request decoding, HTTP errors, routes, process bootstrap
       |
    application/   configuration, account/service assembly, session use cases
       |                          |                     |
    executor/                  session/              storage/
    agent operations,          durable state,        SQLite, indexes,
    model execution            queue, history       blobs, snapshots
       |
    client/        model clients and independent account clients; retry policy
       |
    protocol/      upstream request/response rules, authentication and renewal
       |
    transport/     one HTTP attempt and streamed body reading
```

`server/` adapts external requests to application use cases. `application/` assembles services
and coordinates sessions, without depending on HTTP types or server modules. `executor/` owns
agent execution; it does not choose protocol-specific wire behavior. `protocol/` owns those
rules and does not know about application configuration or persistence. `storage/` accepts
records to persist and does not reach back into application or session logic.

The application facades separate session commands from read-only queries and history views.
Peer, MCP and Skill access enforce session permissions before invoking their services. Event
subscriptions return application values; HTTP adapters add SSE framing and response headers.
Login state and account checks live in the application layer; callback listeners remain in
`server/` and upstream OAuth exchanges remain in `protocol/`.

Storage cleanup crosses three boundaries deliberately: `session/resources.rs` recognizes
references carried by messages, `application/storage_cleanup.rs` coordinates idle-session locks,
history pruning and notifications, and `storage/files.rs` measures or deletes selected resources.
`storage/data_dir.rs` owns the persisted path layout; `storage/measurement.rs` attributes database
space. Storage does not decide whether a session or shell execution is still using a resource.
The storage HTTP module only decodes requests and serializes these use-case results.

Account credentials have an independent `AccountRuntime` and `AccountClient`; a search or fetch
service does not need a synthetic model endpoint to reuse an account. Service configuration
selects its upstream protocol, transport policy and account. Application errors are mapped to
HTTP status codes only by `server/error.rs`.

## Protocol layer

- [protocol](protocol.md): the converter model and how the pieces compose into one call.
- [model-use](protocol/model-use.md): conversation wires, compat modes, stream replay rules.
- [token-count](protocol/token-count.md): provider-side input counting.
- [upstream-compaction](protocol/upstream-compaction.md): asking a service to compact history.
- [account-state](protocol/account-state.md): quota and balance readings.
- [model-list](protocol/model-list.md): model catalogs.
- [endpoint](protocol/endpoint.md): targets, auth placement, credentials, building a call.
- [error](protocol/error.md): the error vocabulary and what retries.

## Client and transport

- [client](client.md): one upstream ready to call; the only description of retry, limits and
  the stream-head retry.

## Session engine

- [session](session.md): state, the input queue and its sender, the reader, generations, crash
  settlement.
- [history](history.md): indexed history queries and search.
- [statistics](statistics.md): model call records.

## Executor

- [executor](executor.md): the run loop, run boundaries, output continuation, shutdown.
- [compaction](compaction.md): local standby summaries, upstream compaction, provider-switch
  handoff.

## Storage

- [storage](storage.md): typed SQLite objects and lists, stored kinds, the history index, cache,
  schema version.
- [migration](migration.md): bringing an older data directory and configuration file up to date at
  startup.

## Tools

- [tools](tools.md): shell, history tools, `view_image`, `ask_user` and `web_search`, with their
  application integration.

## MCP

- [mcp](mcp.md): MCP servers reached from a session's shell through `wish mcp`: the client, the
  instances, the bridge.
- [skills](skills.md): skills found and read from a session's shell through `wish skill`.
- [groups](groups.md): groups of sessions and the user, and what a session sees of other sessions
- [folders](folders.md): how the list is arranged in folders, as a file system arranges files
  through `wish session`.
