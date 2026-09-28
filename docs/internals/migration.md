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

## Running them

1. Print the pending steps to standard error.
2. Copy `management.sqlite` and `wish.sqlite` with `VACUUM INTO`, and the configuration file, to
   `backups/before-migration-<from>-to-<to>-<unix time>/` in the data directory.
3. Open each database and begin a transaction; run every pending step against them and the
   configuration in memory. A failing step rolls everything back: startup fails with the step's
   number, its reason and the copy's location, and nothing has been written.
4. Commit `wish.sqlite`, then `management.sqlite`; replace the configuration file atomically (only
   when a step changed it, keeping its permissions); write `format.json`.

The writes in step 4 cannot be one transaction. A crash between them leaves `format.json` at the old
version, and the next start repeats the steps, which is why steps are repeatable. A failed commit
reports the copy to restore from.

## Adding a step

Add `m<NNNN>_<name>.rs` with its `SUMMARY` and `apply`, append it to `STEPS`, and add a row above.
Test it in wish-test: build an old directory by hand (drop `format.json` and rewrite records in
the old shape), start the new build, and check the result and the copy.
