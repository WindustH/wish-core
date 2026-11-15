# Storage & Migrations

Wish stores durable state in two SQLite databases inside `data_dir` (`wish.sqlite` for sessions/history and `management.sqlite` for the service index, folders, and usage metrics) alongside per-session `blobs/` and `shell/` directories.

## Single-writer SQLite engine (`src/storage/`)

`Storage` runs a dedicated SQLite thread behind a 64-slot command queue and a 32 MiB byte-bounded LRU cache serving immutable `Arc<T>` snapshots.

- **Transactions & rehearsals**: Writes execute inside `Storage::transaction(move |tx| ...)` closures on the storage thread, invalidating cached entries only after commit. Read-only transactions (`Storage::rehearse`) run identically and roll back unconditionally without populating the cache.
- **Type safety (`StoredValue::KIND`)**: Stored types in `src/session/persistence.rs` declare fixed `KIND` strings (`wish_core::session::...`); mismatched reads fail with `TypeMismatch`.
- **Lists & position release**: Lists store elements as `(list_key, position)` rows (`schema_version = 4` interns list names in `wish_list_keys`). Queue cancellations shift tail positions (`remove_item`), whereas history pruning zeroes values in place (`release_items`) so entry IDs and history sequences never renumber.
- **History index & vacuuming**: `storage::history_index` maintains filter columns and FTS5/trigram tables in the same transaction as history writes. Databases open in WAL mode (`NORMAL` sync) with incremental `auto_vacuum`; `Storage::shrink` optimizes FTS segments and truncates free trailing pages after session deletion, while `Storage::vacuum` rebuilds after bulk pruning. `Storage::shutdown` performs a `FULL` WAL checkpoint before closing.

## Blobs, linked files, and aliases

`src/storage/blobs/` manages `data_dir/blobs/<session_id>/`:

- Uploaded files are content-addressed by 64-hex SHA-256 ID; model images are normalized into bounded derivative blobs (`<= 2048px`, `<= 2 MiB`) leaving originals intact.
- Linked local files (`blobs/linked.rs`) hash the canonical host path into an independent ID namespace and read directly from the host file without duplicating bytes or deleting external files.
- Library metadata (`blobs/library.rs`, `cleanup.rs`) persists folders, visibility, pin flags, and display names; cleanup protects files referenced by active/standby contexts, queued inputs, and `.media-jobs/*.json` (preserving hidden files when any job file is unreadable).
- Environment aliases (`application/env_aliases.rs`) persist `.env-aliases.json` mapping `$WISH_BLOB_ID_*` and `$WISH_SESSION_ID_*` using monotonic, never-recycled uppercase base-36 suffixes (`0`–`9`, `A`–`Z`, `10`, ...). Cross-session messages translate aliases and share immutable blobs under `blob_publication` synchronization.

## Startup migration pipeline (`src/migration.rs`)

`server::run` runs `migration::run(&config_path)` once under `.wish.lock` before typed config parsing or `Storage::open`. Runtime code handles only the current schema (`schema_version = 4`, `deny_unknown_fields` on config).

`data_dir/format.json` tracks `{"version": N}` against `LATEST = 15`. When `N < LATEST`, Wish snapshots `wish.sqlite`, `management.sqlite` (via `VACUUM INTO`), and `config.json` into `data_dir/backups/`, runs each pending `m<NNNN>_<name>.rs` step idempotently over raw SQL/JSON inside transactions, commits `wish.sqlite` then `management.sqlite`, updates `config.json` if touched, writes `format.json`, and vacuums both databases.

| Step | Change |
| --- | --- |
| `m0001` | Initialize `tools.mcp = true` across defaults and sessions. |
| `m0002` | Add `web_search` switch (enabled in defaults, off on existing sessions) and seed Codex search if unset. |
| `m0003` | Remove completed legacy tool batch records. |
| `m0004` | Enable incremental `auto_vacuum` on `wish.sqlite` and merge FTS5 tables. |
| `m0005` | Trim stored `last_operation.outcome` payloads in session headers. |
| `m0006` | Remove orphaned unreferenced standby generation lists. |
| `m0007` | Add `tools.skills = true` across defaults and sessions. |
| `m0008` | Enable incremental `auto_vacuum` on `management.sqlite`. |
| `m0009` | Add `tools.sessions = true` across defaults and sessions. |
| `m0010` | Move legacy group records into `management.sqlite` tables. |
| `m0011` | Drop relay limits and `groups` config section. |
| `m0012` | Add `folders` table plus `folder` / `pinned` columns and listing indexes. |
| `m0013` | Strip saved `user-agent: claude-cli/...` headers from `anthropic` preset providers. |
| `m0014` | Add `history_search` and `history_search_delivery = "command"` to `defaults.tools`. |
| `m0015` | Drop retired group tables and schedule cleanup of orphaned group blob directories. |
