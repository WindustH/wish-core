# Internals

Design notes for the modules under `src/`. They are internal modules of the `wish` binary crate,
not a public library: code examples use `crate::` paths and `pub` means crate visibility. The HTTP
surface is described in [the API reference](../api.md) and the config file in
[configuration](../configuration.md).

## Process layout

One process serves HTTP, drives sessions and calls upstreams. `src/main.rs` calls `server::run`,
which reads `--config <file>`, opens storage and serves until SIGINT/SIGTERM.

```text
                             HTTP / SSE
                                 |
  server/       routes, provider clients, session slots, BTW asks, shutdown
                  |                   |                     |
                  v                   v                     v
  executor/     run loop,           tool/                 session/
                compaction          shell, history,       state machine, queue,
                  |      |          view_image            generations, history
                  |      +----------------------------------> |
                  v                                           v
  executor::model::Client                                  storage/
  (retry via utils/)                                       SQLite + LRU + history index
                  |
                  v
  protocol/     one converter per wire, outbound auth
                  |
                  v
  transport/    one HTTP attempt: reqwest, SSE, AWS event-stream
```

Arrows point from caller to callee. `server/` is the only module that knows about providers,
the config file or the HTTP API. Behavior it adds on top of the engine is marked "server-owned" in
these pages.

## Protocol layer

- [protocol](protocol.md): the converter model and how the pieces compose into one call.
- [model-use](protocol/model-use.md): conversation wires, compat modes, stream replay rules.
- [token-count](protocol/token-count.md): provider-side input counting.
- [upstream-compaction](protocol/upstream-compaction.md): asking a service to compact history.
- [account-state](protocol/account-state.md): quota and balance readings.
- [model-list](protocol/model-list.md): model catalogs.
- [outbound](protocol/outbound.md): targets, auth placement, credentials, dispatch.
- [error](protocol/error.md): the error vocabulary and what retries.

## Client and transport

- [client](client.md): one upstream ready to call; the only description of retry, limits and
  the stream-head retry.

## Session engine

- [session](session.md): state, queue, generations, control handles, crash settlement.
- [history](history.md): indexed history queries and search.
- [statistics](statistics.md): model call records.

## Executor

- [executor](executor.md): the run loop, run boundaries, output continuation, shutdown.
- [compaction](compaction.md): local standby summaries, upstream compaction, provider-switch
  handoff.

## Storage

- [storage](storage.md): typed SQLite objects and lists, cache, schema version.

## Tools

- [tools](tools.md): shell, history tools and `view_image`, with their server integration.
