# Session Engine

`src/session/` manages session state, the durable input queue, context generations, permanent history, FTS5/trigram indexing, and call statistics. The executor mutates `&mut Session` across turns while `storage/` provides single-writer SQLite persistence.

## Ownership model

```text
Session (single live owner per Storage)
  ├── header (SessionRecord)     config, metadata, phase, active/standby generation pointers, queue_head
  ├── SessionSender              concurrent durable enqueue, move/cancel, context snapshots (cannot mutate header)
  └── SessionReader              cloneable read-only views and history/search queries
```

Only one live `Session` owner may open a given session ID per `Storage` (`AlreadyOpen` otherwise). Reads (`Session::read`) run inside rolled-back `Storage::rehearse` transactions so they stay isolated from writes.

Messages (`Entry`), events (`SessionEvent`), history rows (`HistoryRecord`), and call records (`ModelCallRecord`) live as individual items in paged lists rather than inside the session header. Because `SessionSender` cannot modify the owner's in-memory header, `SessionSender::transact` rejects any header mutation and rolls back.

## Durable queue and side snapshots

`SessionSender::enqueue_message` stores `User`, `System`, or `Developer` entries and appends queue references atomically without interrupting an active run. Until the executor consumes the pending queue at a stable boundary (`InputsConsumed`), pending entries can be reordered (`move_queued_input`) or cancelled (`cancel_queued_input`).

`SessionSender::build_context_snapshot()` reads the active generation up to the last committed step (excluding unfinished tool batches and pending queue entries) in one read transaction, powering stateless `BTW` side questions while runs remain active.

## Context generations and crash settlement

History records every event permanently in ascending sequence order, while `Generation` headers (`active` and `standby`) hold lazy lists of entry IDs representing what the model currently sees.

- `prepare_standby_generation` and `activate_standby_generation` swap contexts atomically while keeping entries appended during preparation. Unused replaced standby lists are deleted right away.
- `defer_instruction_config` swaps runtime config while leaving the active prompt prefix intact until the next generation boundary, emitting a system notice in the meantime.
- If a process crashes mid-flight, the session loads in its active phase and refuses to run until `settle_interrupted()` closes out unfinished work without re-executing external side effects:
  - `CallingModel` settles as `Interrupted`.
  - `ExecutingTools` marks started calls `Unknown` (`ToolOutcomeUnknown`), cancels unstarted ones, or finishes `Interrupted` if none started.
  - `Compacting` restores its prior phase and settles under the same rules.
  - Any leftover `Running` `ModelCallRecord` closes as `Interrupted` or `Failed` in the same transaction.

## History indexing, queries, and pruning

`session::history` extracts filter columns and searchable text into `storage::history_index` inside the same SQLite transaction as message/event appends. Indexing covers message text, readable reasoning, and tool inputs/outputs (skipping search-tool payloads to avoid self-indexing noise, along with images, ciphertexts, and live stream deltas).

- `query_history` pages chronologically by sequence cursor with scalar JSON-path filters (`$.field`).
- `search_history` runs `terms` mode (quoted FTS5 Unicode tokens) or `substring` mode (trigram index, falling back to filtered SQLite `lower()` scan for needles shorter than 3 characters).
- `prune_history` releases unused message/event positions outside active/standby contexts and pending queues without renumbering entry IDs or history sequences, returning unreferenced file paths for cleanup.

## Call statistics (`ModelCallRecord`)

Each `ModelCallRecord` tracks one logical invocation (`Conversation`, `CompactionSummary`, `UpstreamCompaction`, or `CompactionTranslation`), folding transport retries and output continuations into the same record. Only `Conversation` calls populate compaction triggers and fallback estimator calibration, and `last_request_input_tokens` records only the final completed physical request rather than summing across continuation segments. `application` mirrors completed calls into `management.sqlite` (`calls`) and samples 1-second streaming throughput into `stream_samples`.
