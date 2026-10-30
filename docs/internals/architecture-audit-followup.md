# Backend architecture audit follow-up

The baseline is `4356f5e`. This change preserves the on-disk format and existing tool/configuration
field names. Verification uses the separate `wish-test` executable/HTTP integration suites; inline
Rust unit tests were removed at the owner's request. No `cargo test` gate is introduced.

## Boundaries and ownership

- `capability.rs` defines stable built-in identities, tool-name aliases and command discovery.
  Application dispatch, permission checks, advertisement, CLI help selection and history indexing
  resolve these identities. `application/capabilities.rs` defines the persisted switch fields once;
  optional patches and conversions derive from that declaration. Existing serde defaults remain.
- `tool/contract.rs` owns the execution port; `session/tool_input.rs` owns durable input arrival
  semantics. `utils/control.rs` owns cancellation. The executor retains compatibility re-exports
  for engine fixtures, but tool implementations no longer import executor internals.
- The session's `SessionTools` explicitly owns its lazily initialized `TaskManager`. Shell adapters
  receive shared runtime handles and have no `Deref` magic. `task` has no session/executor/tool
  dependency. Its foreground result separates captured output, edits and failure; rendering and
  conversion into model outcomes live in `tool/task_output.rs`. Task payload JSON remains stable
  for existing CLI and UI consumers; this is not a wholesale task wire-schema rewrite.
- Storage list queries return `RecordPage` and `ConversationRow`/`ConversationEntry`. HTTP query
  deserialization, page bounds and API JSON projection live in application. Persisted session index
  payloads remain opaque records, keeping storage independent of application descriptor types.
- Session creation is an application use case. Initial metadata, history and queued first input
  now commit with the session header in one transaction. Opening/placement errors attempt cleanup
  and report cleanup failures. Cross-database creation is still not a distributed transaction;
  abrupt process death between engine storage and management indexing remains a separate recovery
  concern.
- `App` internals and `SessionSlot` fields are restricted to application. The application exposes
  the stable model-reply interface used by HTTP SSE. Image materialization moved from executor
  to application model projection; binary MCP file persistence moved to storage.

## Runtime work

- Cold session opening uses per-ID locks; global map locks cover lookup/insertion only. Failed
  opening locks are weakly retained and pruned. Existence checks for attachment access and group
  membership do not initialize session tools.
- Folder, group and peer use cases dispatch synchronous database work through bounded IO admission.
  Shell discovery, question index updates, user-input touches and MCP binary writes do likewise.
- Status reads use the last committed queue-size snapshot. The writer publication lock covers the
  transaction and subsequent atomic update; readers never take that lock. Failed transactions do
  not publish, and late older transactions cannot overwrite newer counts.
- Search/fetch account branches use the same account client. The shared auxiliary-call helper
  preserves retry policy and the **per-attempt** 60-second deadline; it does not impose a new
  60-second deadline across all retries. HTTP connection reuse remains unchanged.
- Retired MCP connections are tracked and awaited during shutdown. Management DB lock poisoning
  returns a repository error instead of panicking subsequent callers. Other locks are not blindly
  recovered: hiding poison can expose partially changed application state.

## Contracts

- Missing tasks return 404 with actionable lookup guidance; closed/interrupted tasks return
  conflict, IO failures return internal errors, unknown outcomes retain `details.kind=unknown`
  for CLI exit 3. Invalid `If-Match` header text returns 400. CLI refusal handling no longer relies
  on the exact English string `not found`.
- Ordinary session/list/group pages accept 1–1000 items. Context inspection retains its 1–100
  bound. Tests requesting all history now follow pagination.
- Context-inspector message names use explicit stable spellings instead of `Debug`. Existing
  PascalCase inspector values and snake_case history filters are retained for compatibility.
- Credential persistence and periodic renewal live in `application/credentials.rs`; login status
  is an enum with unchanged serialized spelling. Preset loading/renewal lookup is centralized.
- The root `crate::Error` alias is removed; imports explicitly name the protocol error module.
  Search/fetch adapters share their result DTOs. SSE/configuration serialization errors propagate
  rather than panicking or returning empty content.

## Recommendations deliberately not applied mechanically

Provider ports in executor are valid dependency inversion. Session reuse of protocol IR is
intentional: the serialized IR is part of storage format 14, and incompatible changes require a
migration. Renaming every getter, renaming historical enum values, or adding a domain crate merely
to move types would increase churn without fixing a demonstrated defect.

Application read models and UI projections are legitimate application outputs; they do not need
to move into HTTP handlers. Existing extensible event payloads are retained rather than pretending
that a wrapper enum makes every nested JSON field type-safe. Task result payloads and persisted
index records likewise retain their current shapes. Broad event/schema redesign, and algorithmic
changes to ordered-list deletion, require separate compatibility and workload evidence.

Synchronous session state-machine transactions and event callbacks still exist. The changes above
remove the identified application hot paths; they do not claim every engine operation is async.
No throughput improvement is claimed without a controlled load benchmark.

## Validation and CI

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, a build, and
`python3 ../wish-test/tools/check_architecture.py` are CI gates. The dependency guard prevents task from reaching
back into session/executor/tool and storage from reaching application/server/session/executor.

The local `wish-test` checkout has no configured Git remote. CI therefore does not invent a remote
URL: set `WISH_TEST_REPOSITORY` or dispatch with `test_repository` and a compatible `test_ref` to
run the external suites in CI. Locally run:

```sh
cd ../wish-test
python3 -m unittest tests.test_wish_server tests.test_wish_core_history tests.test_wish_core_shell tests.test_wish_core_agent -q
```

The fixture update in wish-test must accompany this backend change because its embedded module
list now includes the neutral capability catalog and shell runtime access is explicit.

Local validation on 2026-10-27: formatting, dependency checks, zero-warning Clippy and release
build passed. The combined external run executed 177 cases: 176 passed; the new initialization
rollback case initially omitted the required `run` configuration. After correcting that test
request, its isolated rerun passed. No backend source changes followed that combined run.

## Resource-control review (2026-10-31)

This follow-up starts from `743af5b`. The supplied static review identified useful risks, but
several conclusions omitted an outer boundary or assumed a different state-machine meaning.
The stored format, historical enum spellings, HTTP routes and tool argument fields stay stable.

### Confirmed defects and changes

- **Repeated context rejection:** one automatic recovery is allowed until a conversation call
  makes progress. A second explicit context-length refusal remains the run's recorded failure;
  it cannot trigger another paid compaction. A later explicit run can try again. Upstream
  compaction already checked its resulting size against the configured target; the missing
  protection concerned inaccurate budgets or a provider that still refused that result.
- **Remote token counting:** standby planning counts its first complete span once, then probes
  boundaries with locally calibrated estimates. Local trimming searches complete, protocol-valid
  candidates with at most eight count calls, independent of the number of boundaries. Under a
  tight probe budget it may retain a more conservative tail. A configured counter's failure
  still fails the compaction; estimates do not silently override that failure or authorize an
  over-budget provider-counted cutover. No wall-clock performance gain is claimed without a
  workload benchmark.
- **Draft upload admission:** a dedicated draft inventory lock serializes quota changes and
  adoption. It is acquired before session creation admission, so an adoption waiting for a scan
  cannot hold the creation lock. The inventory is reconciled on startup/first use and hourly;
  ordinary uploads inspect only their own directory. Successful adoption removes that pending
  entry. A failed or cancelled write invalidates the inventory before another upload is admitted.
  Scans and filesystem work run on bounded blocking IO, and their owned guard survives cancellation.
- **Instruction observations:** an open session shares one instruction state and history cursor
  across model wrappers and generations. It replays only application events after that cursor,
  including observations committed by another writer. Reopening replays durable history once.
  Set/remove events and A -> B -> A changes retain the existing context-delta semantics; a new
  generation alone does not reset the state or repeat tool definitions.
- **Credential renewal:** failures use monotonic exponential backoff from two minutes up to one
  hour. The failure budget survives unrelated configuration saves and is reset by editing the
  account or signing in again. This does not permanently disable an account or expose credentials
  in an API status. Failed configuration persistence is backed off too.
- **Stream recovery:** `InputTokens` is emitted once, when the initial input count becomes known,
  rather than after every subsequent frame. Checkpoint rewind and cumulative usage remain intact.
- **Parallel tools:** at most eight calls are admitted at once. `ToolStarted` is recorded at
  admission, before calling the implementation. A crash with twenty calls leaves eight potentially
  effective calls and twelve unstarted calls, rather than marking the entire queue started.
  Ordinary cancellation still awaits and records each implementation's explicit outcome; the
  original report's `Unknown` claim applies to abandoned/crashed work, not every normal Ctrl+C.
- **Open-session cache:** stable, unreferenced sessions expire after fifteen idle minutes; above
  128 cached slots, eligible entries are retired in least-recently-used order. Runs, live event
  subscribers, mutable session owners, background tools, running processes and independent task
  manager owners pin their resources. A weakly promoted retired owner is reused instead of opening
  a second state machine. History, blobs and task captures stay on disk. These are cache limits,
  not permission to terminate active work; pinned entries can exceed 128. Storage already shares
  its SQLite worker across sessions, so each slot did not own a separate database connection.
- **Ordered-list deletion:** two range updates stage the suffix above its previous length before
  shifting it down. This preserves both the unique position key and `CHECK(position>=0)` without
  relying on SQLite's update order. Cache invalidations still cover the affected suffix, so the
  overall operation is not O(1). Rollback and warmed-page reads are regression-tested.

### Boundaries and naming

- Application media-job recovery is `media_status`; signed HTTP blob access is `media_grant`.
  Fetch snapshots and their cursor reader live in `application/fetch/reader.rs`.
- Image conversion guidance is assembled by `tool/view_image`; `utils/image_format` only detects
  formats. Image request DTOs and generic operation reports/failures live in `extensions/shared`,
  with compatibility re-exports from existing extension entry points. The tool dependency guard
  now rejects extension implementation imports and permits the neutral shared contracts.
- MCP's caller-resolution helper is `authenticate_bridge_caller`. Skill access already used
  `session_roots`, contrary to the report.
- Historical Chinese instruction titles are retained because they are durable observation keys
  and compatibility metadata. Presentation/localization belongs to the existing UI projection;
  changing persisted titles merely for English consistency would manufacture context deltas.

### Claims that did not justify the proposed fix

- Image execution is already wrapped in a **600-second deadline** in
  `application/image_generation.rs`, covering submission, polling and materialization. A timeout
  after submission is an unknown paid outcome, never permission to resubmit. The inner polling
  loop alone did not prove an unbounded production operation; no redundant timeout was added.
- Download MIME is derived from a bounded content prefix and fixed MIME literals, not untrusted
  upload metadata. Header parsing now has an octet-stream fallback for robustness, but the
  reported remote panic path did not exist in the inspected implementation.
- A normally completed run enters `Idle`. `Suspended` records a failed, interrupted or otherwise
  abnormal outcome and requires explicit resume. Renaming it `CompletedRun` or `AwaitingInput`
  would hide that distinction and change durable contracts without a demonstrated benefit.

### Regression entry points

The tests remain in the separate `wish-test` repository; no inline Rust tests or production test
switches were added. `test_wish_core_review` builds copied application source into an independent
executable and exercises renewal backoff/reset, draft/creation lock ordering, quota accounting,
cache eviction/reopening and incremental instruction observations. The agent fixture covers
repeated context refusal, bounded counting, queued-tool crash semantics and ordered-list rollback.
The Google fixture asserts a single initial-input event across checkpoint recovery. Existing HTTP,
engine/history and process-tree suites cover API compatibility and lifecycle behavior.

```sh
cargo fmt --check
CARGO_INCREMENTAL=0 cargo clippy --offline --all-targets -- -D warnings
python3 ../wish-test/tools/check_architecture.py
cd ../wish-test
CARGO_INCREMENTAL=0 python3 -m unittest tests.test_wish_core_review tests.test_wish_core_agent tests.test_wish_core_google -q
CARGO_INCREMENTAL=0 python3 -m unittest tests.test_wish_server tests.test_wish_core_history tests.test_wish_core_shell tests.test_wish_architecture -q
```

Local validation: formatting, zero-warning Clippy and dependency checks passed. The three
external executable fixtures passed (engine contracts, Google recovery, and application resource
guards). The broad HTTP/history/process run executed 191 Python cases: 181 passed and 10 failed.
Re-running those ten cases against the unchanged baseline reproduced eight failures (legacy
extension commands/tool advertisement and a preset-count assertion). The other two tests waited
for `running=false` before accepted input had drained; their explicit queue-draining waits were
corrected and their isolated rerun passed. The eight reproduced baseline failures remain outside
this resource-control change; the broad suite is not reported as entirely green.
