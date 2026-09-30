# Migration

`src/migration.rs` brings a data directory and its configuration file up to the formats this build
reads. `server::run` calls `migration::run(config_path)` once, before the configuration is parsed or
a database is opened; nothing else refers to the module. Every other module reads the current
formats only and refuses anything else (storage refuses a `wish.sqlite` whose `user_version` it
does not know, serde refuses a configuration field it does not know). There are no compatibility
shims for older shapes anywhere else.

## Versions

The data directory records one number in `format.json`: `{"version": N}`, how many steps it has
been through. Step `i` in `STEPS` takes version `i` to `i + 1`, and `LATEST` is `STEPS.len()`.

| `format.json` | Meaning |
| --- | --- |
| absent, and neither database exists | A new directory: marked `LATEST`, nothing to do |
| absent, a database exists | Written before this module existed: version 0 |
| `N < LATEST` | Steps `N..LATEST` run |
| `N == LATEST` | Nothing to do |
| `N > LATEST` | Written by a newer build: startup fails |

The configuration file has no version of its own: it is migrated with the directory its `data_dir`
names, read from the raw JSON (default `data`).

## Steps

Each step is a file `m<NNNN>_<name>.rs` with a `SUMMARY` and `apply(&mut Data)`, listed in `STEPS`.
`Data` hands it the configuration as a `serde_json::Value` and each database that exists, inside a
transaction. A step is frozen once released:

- It reads and writes the raw formats (the configuration's JSON, the databases through SQL, stored
  records as JSON text) and never the program's current types, which keep changing after it.
- It is repeatable: applied to data it has already changed, it changes nothing more.
- It rewrites everything that depends on what it changes, such as the history index and its FTS
  tables when message JSON changes, and `PRAGMA user_version` when `wish.sqlite`'s schema changes.

| Step | Change |
| --- | --- |
| `m0001_mcp_switch` | A session's MCP switch now only decides what `wish mcp` answers, so `session.tools.mcp` in every session record and `defaults.tools.mcp` are set to `true` |
| `m0002_web_search` | Sessions gain a `web_search` switch: on in `defaults.tools`, off in every existing session record so its tool list and prompt cache stay; a configuration with a ChatGPT (Codex) provider and no search providers gets one borrowing that account |
| `m0003_tool_batches` | A tool batch is deleted once its tools finish; the batches earlier runs left behind are deleted, except the one a session still executing tools names |
| `m0004_compact_storage` | Deleting a session gives its room back: `wish.sqlite` is set to incremental auto-vacuum and its FTS tables are merged, dropping the text of history deleted earlier |
| `m0005_short_outcomes` | A session record's `status.last_operation.outcome` keeps only its short form (`StreamFailed: {reason}`, `ModelStopped: {stop_reason}`); records saved before the server trimmed it held the partial or whole response |
| `m0006_standby_lists` | A standby generation's entry list is deleted when the standby gets a new one without being activated; in each session, the entry lists earlier builds left behind that way - every entry list neither its queue nor one of its generations names - are deleted |

## Running them

1. Print the pending steps to standard error.
2. Copy `management.sqlite` and `wish.sqlite` with `VACUUM INTO`, and the configuration file, to
   `backups/before-migration-<from>-to-<to>-<unix time>/` in the data directory.
3. Open each database and begin a transaction; run every pending step against them and the
   configuration in memory. A failing step rolls everything back: startup fails with the step's
   number, its reason and the copy's location, and nothing has been written.
4. Commit `wish.sqlite`, then `management.sqlite`; replace the configuration file atomically (only
   when a step changed it, keeping its permissions); write `format.json`.
5. `VACUUM` both databases, which returns what the steps deleted and applies a layout a step set
   (such as `auto_vacuum`); a failure here is only printed.

The writes in step 4 cannot be one transaction. A crash between them leaves `format.json` at the old
version, and the next start repeats the steps, which is why steps are repeatable. A failed commit
reports the copy to restore from.

## Adding a step

Add `m<NNNN>_<name>.rs` with its `SUMMARY` and `apply`, append it to `STEPS`, and add a row above.
Test it in wish-test: build an old directory by hand (drop `format.json` and rewrite records in
the old shape), start the new build, and check the result and the copy.
