# History queries

`Session::create_history_reader()` returns a cloneable `HistoryReader`. It can be retained while
an executor owns the session, across generation switches, and after dropping the owner.
`HistoryReader::open(storage, session_id)` opens the same read interface without claiming ownership.

```text
UI / search_history tool
          |
     HistoryReader
          |
  SQLite filters + FTS5 -> matching sequences + snippets
          |
  read_history_item / read_history_around -> original records
```

History is independent of active context. Compaction seals generations and replaces context
references; the original message/event history stays searchable. A history record's generation is
the generation in which that fact was recorded, not every later context referencing its message.
Context-only entries, including prepared summaries, are reached through their history events; they
are not added to the conversation transcript by indexing. Queued messages become message history
when consumed; their enqueue events are available earlier.

## Filtering and paging

```rust
use wish_core::session::history::query::{HistoryFilter, HistoryPageRequest, MessageType};
use wish_core::session::statistics::Timestamp;

let reader = session.create_history_reader();
let filter = HistoryFilter {
    message_types: vec![MessageType::User, MessageType::Assistant],
    since: Some(Timestamp(start_ms)),
    until: Some(Timestamp(end_ms)),
    ..Default::default()
};
let page = reader.query_history(filter.clone(), HistoryPageRequest::default())?;
// Pass page.next as the next request's cursor, retaining the same filter and order.
```

Message types are `system`, `developer`, `user`, `assistant`, `reasoning`, `tool_use`, `tool_result`
and `upstream_compaction`. Other filters select message/event kind, event variant name, origin,
generation, model call and tool name. Conditions combine with AND; choices within a type list use
OR. An empty filter includes messages and events.

Times are HistoryRecord timestamps in Unix milliseconds, with inclusive `since` and exclusive
`until`. Every history record has a timestamp. Message creation time on Entry may differ from its history time, for example while waiting in the queue.
Ordering and cursors use sequence numbers, so equal or non-monotonic timestamps do not lose rows.

The first page captures an exclusive history upper bound. Later pages retain that bound and seek
by sequence, in oldest-first or newest-first order. New appends do not displace existing pages.
The limit is caller-controlled, independent of the LRU's internal page size.

## Search and expansion

`search_history(HistorySearch, HistoryFilter)` returns top matches with history references,
message/event type, tool name, snippets and FTS5 scores. `terms` requires all whitespace-separated
terms, using the default Unicode tokenizer; input is quoted as literal text, not FTS operators.
`substring` matches contiguous text through a trigram index, useful for Chinese and paths. Needles
shorter than three characters scan the filtered indexed text and report `used_text_index: false`.
Short substring case folding follows SQLite lower() (ASCII); longer searches use the FTS tokenizer.

Search returns a bounded top result set with `has_more`, not a relevance pagination cursor: new
records can change relevance scores. Narrow filters or increase the requested limit to explore
more hits. `read_history_item(sequence)` loads one original record; `read_history_around(sequence,
before, after)` expands timeline neighbors. These are historical facts, not automatically valid
protocol replay units. Applications that reconstruct model messages must validate tool pairing
and reasoning replay requirements separately.

Text indexes include message text, readable reasoning, and tool argument/result JSON. Image
blocks, reasoning signatures/ciphertext, upstream opaque bodies, and streamed delta payloads are
not indexed. Search-tool arguments/results remain filterable/readable but are excluded from full
text to avoid indexing repeated queries and retrieved snippets. Events provide structural filters;
Finished(Failed) additionally indexes its error text. Metadata is not part of full-text search.

## Metadata and storage

Metadata predicates apply to message metadata: `Exists { path }` and scalar `Equals { path, value }`,
using SQLite JSON paths such as `$.project_id`. Missing differs from explicit null; boolean values
are distinct from numbers. Call `Session::index_history_metadata(path)` for frequently used paths;
it creates a database-wide expression index. Unindexed metadata predicates may scan filtered rows.
Objects and arrays are retained as metadata but are not scalar equality operands.

Secondary tables hold filter columns and extracted text; FTS5 uses that text as external content.
The authoritative messages, events and history remain individual existing storage elements.
Ordinary writes and index updates share one transaction. Stream-event batches retain their batch
commit and project event fields directly, without rereading every new event payload.

Queries load only matching history references; explicit expansion loads original payloads through
existing storage/cache APIs. No read-time backfill is performed. Short substring and unindexed
metadata queries may scan index rows, without loading the full message history.

`Session::rebuild_history_index()` explicitly rebuilds that session's derived rows in one atomic
transaction, reading current-format history in bounded pages.
No query-result cache or separate search service is required. Searches cover committed history;
stream events awaiting the existing persistence batch are not visible yet.
