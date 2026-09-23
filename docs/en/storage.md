# Storage

`storage` provides typed object and list handles over SQLite. Callers work with Rust values;
SQL, JSON encoding, caching and invalidation stay inside the module. Reads return immutable
`Arc<T>` snapshots. Writes return `Result`, so failed persistence is never disguised as a
successful in-memory mutation.

```text
StoredObject<T>                 StoredList<T> / ReadList<T>
 load / create / save             get / read_page / append / append_items / set
            \                       /
             +-- command channel -----+
                         |
                single storage thread
                 transaction + LRU
                         |
                 one SQLite connection
            objects + list headers + individual rows
```

```rust
let storage = Storage::open("wish.sqlite", StorageOptions::default())?;
let settings = storage.open_object::<serde_json::Value>("settings");
settings.create(&serde_json::json!({"enabled": true}))?;
let snapshot = settings.load()?;
settings.save(&serde_json::json!({"enabled": false}))?;

let messages = storage.create_list::<Message>("messages")?;
messages.append(&message)?;
let page = messages.read_page(0, 128)?;
```

`StoredObject::save` replaces the value. `Storage::transaction` runs an owned `Send + 'static`
closure on the storage thread; use `move` to capture data. Related writes commit together, and
errors roll back the entire operation. Cache entries are invalidated only after commit; uncommitted
values are never published. Use the supplied `Transaction` inside the closure: calling another
storage handle there returns `NestedOperation` instead of waiting on itself.

Lists store each element as its own row under `(list, position)`. Reads use indexed position
ranges, not OFFSET. Cache pages contain 128 elements; `read_page(start, limit)` accepts any positive
limit and reads across cache pages, returning at most the remaining items. `Page::next` is the next position. Pagination sees the
list at each read; callers needing a fixed view should retain the initial length. The cache
retains decoded objects, individual items and pages, with a configurable approximate byte budget.
Cache eviction never affects durable data.

Open one Storage per database, then clone its handle for other tasks. A dedicated thread owns the
connection and an ordinary byte-bounded LRU cache. Neither is protected by a shared mutex; callers
communicate through channels. SQLite uses WAL and NORMAL synchronization. APIs synchronously
wait for replies; different sessions may still await model/tool work concurrently. Dropping the
last handle closes the channel and joins the worker. No external-writer cache checks or revision
conflict protocol are maintained.

Storage creates schema version 4 and transactionally upgrades version 3. Other existing
versions are rejected. Version 4 interns list names in `wish_list_keys`; item and history
indexes store integer keys while public list IDs, positions and history IDs stay unchanged.
The upgrade preserves BLOBs, FTS row IDs and custom metadata indexes. Back up before upgrading;
a version-3 binary cannot open an upgraded database, so rollback requires the pre-upgrade backup.

Nullable history fields use partial indexes that omit NULL keys. Freed pages are reused by
SQLite; reclaiming filesystem space requires explicit maintenance VACUUM. Opening storage
never vacuums a live database automatically. Stored engine type names retain the historical
`wish_core::` prefix after the move to the `wish` executable, so existing sessions remain
readable. Type names still reject opening an object/list as the wrong type.

`StoredList::append_items` and `Transaction::append_items` append a batch using one prepared
INSERT and one list-length update, returning its starting position. Elements remain separate rows.
An empty batch returns the current length. Propagate transaction errors to roll back the batch.

`Storage::flush()` waits for earlier storage commands and performs a FULL WAL checkpoint;
it reports checkpoint/SQL errors. Live model deltas are ephemeral and are not flushed.
`Storage::shutdown()` performs the same synchronization and explicitly closes the connection
for all cloned handles. Commands ahead of shutdown finish; later commands return `Closed`.
Shutdown errors are reported, and the worker still closes. Stop producers and await runners first.
Dropping handles is cleanup, not an error-reporting substitute for explicit shutdown.

NORMAL avoids a WAL sync for each commit. Recent committed transactions may be lost on power
loss or OS crash; transactions remain atomic. Unfinished live model output is lost on process termination. Ordinary object/list writes still wait for their SQL commit;
there is no general background write-behind cache.
