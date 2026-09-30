# Storage

`storage` keeps typed objects and paged lists in SQLite. Callers work with Rust values; SQL, JSON
encoding, caching and invalidation stay inside the module. Reads return immutable `Arc<T>`
snapshots. Writes return `Result`, so failed persistence is never disguised as a successful
in-memory mutation.

```text
Storage::transaction / rehearse        ReadList<T>
 Transaction: objects, lists             len / get / read_page / read_all / pages
            \                           /
             +------ command channel ---+
                         |
                single storage thread
                 transaction + LRU
                         |
                 one SQLite connection
            objects + list headers + individual rows
```

```rust
let storage = Storage::open("wish.sqlite", StorageOptions::default())?;
let messages = ListId("messages".into());
let id = messages.clone();
storage.transaction(move |tx| -> Result<(), StorageError> {
    tx.create_object("settings", &settings)?;
    tx.create_list::<Message>(&id)?;
    tx.append_item(&id, &message)?;
    Ok(())
})?;
let page = storage.open_list::<Message>(&messages).read_page(0, 128)?;
```

Every write goes through `Storage::transaction`, which runs an owned `Send + 'static` closure on the
storage thread; use `move` to capture data. The closure's writes commit together, and an error
rolls back the entire operation. Cache entries are invalidated only after commit; uncommitted
values are never published. Use the supplied `Transaction` inside the closure: calling a storage
handle there returns `NestedOperation` instead of waiting on itself. A panicking closure is caught,
rolled back and reported as `OperationPanicked`. `Storage::rehearse` runs a closure the same way
and then rolls it back, so its result tells what the edit would do. History pruning's dry run uses
it, and so do the session's own reads (building a request, a context snapshot), which therefore
cannot change anything. The cache never takes a
value written inside a transaction, so a rollback leaves nothing to evict.

`Storage::open_list` returns a `ReadList<T>`, a lazy read-only handle: opening one loads nothing,
and each read is a transaction of its own. Lists change only inside a transaction, so the domain
objects that own them keep their invariants.

## Stored kinds

A value is stored under a kind, and read back only as the kind it was written as; opening an
object or list as another type returns `TypeMismatch`. The kind is `StoredValue::KIND`, which each
stored type declares. The kind is part of the database format: renaming or moving a type does not
change it, and changing a kind needs a [migration](migration.md). The session declares its eight
stored types in one place, `src/session/persistence.rs`, with the strings they have always had -
the paths the types had when their values were first written, from the time the engine was a
`wish_core` library:

| Type | Kind |
| --- | --- |
| `SessionRecord` | `wish_core::session::persistence::SessionRecord` |
| `Entry` | `wish_core::session::history::entry::Entry` |
| `EntryId` | `wish_core::session::history::entry::EntryId` |
| `SessionEvent` | `wish_core::session::history::event::SessionEvent` |
| `HistoryRecord` | `wish_core::session::history::HistoryRecord` |
| `Generation` | `wish_core::session::context::generation::Generation` |
| `ModelCallRecord` | `wish_core::session::statistics::ModelCallRecord` |
| `ToolExecution` | `wish_core::session::machine::state::ToolExecution` |

Only a stored type's own kind is stored; the types nested in it are kept as their serde field and
variant names, which are part of the format too.

## Lists

Lists store each element as its own row under `(list, position)`. Reads use indexed position
ranges, not OFFSET. Cache pages contain 128 elements; `read_page(start, limit)` accepts any
positive limit and reads across cache pages, returning at most the remaining items. `Page::end` is
the position after the range read and `Page::next` the next position, if any. A list whose
positions have been released (below) returns fewer items than its range, so continue from `end`,
not from the number of items. Pagination sees the list at each read; callers needing a fixed view
should retain the initial length.

Whole lists are read without handling pages: `ReadList::read_all` reads a list that fits in memory
in one transaction, and `ReadList::pages(start)` streams a long one, a page per transaction. Inside
a transaction, `Transaction::read_from(list, start)` reads to the end the list has then, and
`Transaction::for_each_item` visits it a page at a time, handing the visitor the transaction too.

The cache retains decoded objects, individual items and pages within an approximate byte budget
(`StorageOptions::cache_capacity_bytes`, default 32 MiB). Cache eviction never affects durable
data. `Storage::open_in_memory` gives the same engine without a file.

`Transaction::append_items` appends a batch using one prepared INSERT and one list-length update,
returning its starting position. Elements remain separate rows. An empty batch returns the current
length. Propagate transaction errors to roll back the batch.

A few operations change existing rows. `Transaction::set_item` replaces one element.
`Transaction::remove_item` deletes one position and shifts every later element down by one, which
is O(n) in the tail; the session queue uses it to cancel pending input.
`Transaction::release_items` releases positions instead: each keeps its row with an empty value, so
the list's length and every other position stay as they are, `get_item` returns `None` for it and
reads leave it out. JSON values are never empty, so the empty value cannot be mistaken for one.
History pruning uses it, so entry IDs and history sequences keep their meaning.
`Transaction::delete_list` deletes one list with its items and history index; a finished tool batch
goes this way, and so does a standby generation's list when the standby is given a new one.
`Transaction::delete_namespace(key)` deletes an object and every list, item and history-index row
under `key/`; `Session::delete` uses it.

## The history index

`storage::history_index` keeps the index of session history: a row per history record with the
fields history is filtered by, and the text it is searched by, in FTS5 tables built over that text.
The session layer extracts the fields ([history](history.md)); the SQL, including what deleting a
list or namespace and measuring a namespace do to the index, stays in that module, so the rest of
storage knows nothing of history.

Nullable history fields use partial indexes that omit NULL keys. A delete from the history index
only records a delete in the FTS tables, whose text stays until their segments merge, so both ways
of giving room back merge them first ('optimize').

## Threads, schema and durability

Open one Storage per database, then clone its handle for other tasks. A dedicated thread owns the
connection and an ordinary byte-bounded LRU cache. Neither is protected by a shared mutex; callers
communicate through channels. SQLite uses WAL and NORMAL synchronization. APIs synchronously
wait for replies; different sessions may still await model/tool work concurrently. Dropping the
last handle closes the channel and joins the worker. No external-writer cache checks or revision
conflict protocol are maintained.

Storage creates schema version 4 in an empty database and opens existing version-4 databases;
any other version is rejected with `SchemaVersion`. Storage carries no migration code of its own: a
database from an older schema is brought up to date by [migration](migration.md) before storage
opens it. Version 4 interns list names in `wish_list_keys`; item and history indexes store integer
keys while list names, positions and history IDs stay unchanged.

Databases are created in incremental auto-vacuum mode. `Storage::shrink` merges the search index,
then cuts the free pages off the end of the file and truncates the WAL; its cost follows what was
freed and the index, and deleting a session uses it. `Storage::vacuum` rebuilds the whole file
instead, which also moves an older database into incremental mode; history pruning uses it. Other
storage work waits until either finishes. Opening storage never vacuums a live database
automatically.

`Storage::shutdown()` waits for earlier storage commands, performs a FULL WAL checkpoint and closes
the connection for all cloned handles. Commands ahead of shutdown finish; later commands return
`Closed`. Errors, `CheckpointBusy` when the checkpoint could not complete among them, are reported,
and the worker still closes. Stop producers and await runners first; the server does this last in
its [shutdown sequence](executor.md#shutdown). Dropping handles is cleanup, not an error-reporting
substitute for explicit shutdown. Live model deltas are never stored, so there is nothing of theirs
to synchronize.

NORMAL avoids a WAL sync for each commit. Recent committed transactions may be lost on power
loss or OS crash; transactions remain atomic. Unfinished live model output is lost on process
termination. Writes wait for their SQL commit; there is no background write-behind cache.
