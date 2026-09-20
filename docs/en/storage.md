# Storage

`storage` provides typed object and list handles over SQLite. Callers work with Rust values;
SQL, JSON encoding, caching and invalidation stay inside the module. Reads return immutable
`Arc<T>` snapshots. Writes return `Result`, so failed persistence is never disguised as a
successful in-memory mutation.

```text
StoredObject<T>                 StoredList<T> / ReadList<T>
 load / create / save             get / read_page / append / set
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
ranges, not OFFSET. Logical pages contain 128 elements; `read_page(start, limit)` accepts 1–128,
including reads across a page boundary. `Page::next` is the next position. Pagination sees the
list at each read; callers needing a fixed view should retain the initial length. The cache
retains decoded objects, individual items and pages, with a configurable approximate byte budget.
Cache eviction never affects durable data.

Open one Storage per database, then clone its handle for other tasks. A dedicated thread owns the
connection and an ordinary byte-bounded LRU cache. Neither is protected by a shared mutex; callers
communicate through channels. SQLite uses WAL and FULL synchronous durability. APIs synchronously
wait for replies; different sessions may still await model/tool work concurrently. Dropping the
last handle closes the channel and joins the worker. No external-writer cache checks or revision
conflict protocol are maintained.

Schema version 2 removes the old revision column. Version 1 databases upgrade transactionally
without rewriting message/history data. Type names still reject opening an object/list as the
wrong Rust type.
