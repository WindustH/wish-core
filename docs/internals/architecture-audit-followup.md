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
