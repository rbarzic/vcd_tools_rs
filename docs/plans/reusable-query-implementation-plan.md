# Reusable Query, Index, and Server Implementation Plan

Plan ID: **VCD-RQ-001**
Status: **APPROVED — IMPLEMENTATION IN PROGRESS**
Dashboard: [`../../PLAN.md`](../../PLAN.md)
Architecture: [`../design/reusable-query-architecture.md`](../design/reusable-query-architecture.md)

## 1. How to use this plan

- Task IDs are stable and must appear in commits/PRs where practical.
- Update task status in this document and milestone status in `PLAN.md`.
- A task becomes `DONE` only when its acceptance criteria pass and evidence is recorded.
- If scope changes, add a decision-log entry; do not silently reinterpret completed tasks.
- Tasks may be parallel only when dependencies allow and mutation ownership is clear.
- Gate failures stop dependent work. Do not build around a failed correctness spike.
- Performance targets may be revised only with benchmark evidence and owner approval.

Status values: `PROPOSED`, `BLOCKED`, `READY`, `IN_PROGRESS`, `IN_REVIEW`, `DONE`, `DEFERRED`, `REJECTED`.

### Task evidence template

```text
Task:
Status:
Commit/PR:
Changed files:
Validation commands and results:
Benchmark/equivalence evidence:
Decisions:
Residual risks/follow-ups:
Completed by/date:
```

## 2. Scope summary

### Goals

1. Reuse a parsed compact header/catalog across queries.
2. Preserve current Rust, CLI, Python, and waveform semantics.
3. Separate query execution from rendering/transport.
4. Support safe independent concurrent readers for one immutable VCD generation.
5. Add a bounded local Unix-socket service for repeated queries.
6. Accelerate late-window requests with an optional sparse sidecar if offset-resume proves correct.
7. Accelerate repeated hot-signal requests with an optional bounded timeline cache.
8. Detect source replacement/mutation and never silently mix generations.
9. Keep the streaming backend and opt-out controls permanently available.
10. Preserve all current release targets; server transport is Unix-only in v1.

### Non-goals

- TCP, HTTP, gRPC, WebSocket, remote service, or distributed cache;
- Windows named pipes in v1;
- following a growing/mutable VCD;
- arbitrary client-selected primary paths;
- eager all-signal in-memory event indexing by default;
- SQL/general query language;
- changing inclusive windows, extraction-at-start, toggle baseline, or comparison boundary semantics;
- removing existing path-based or module-level Python functions;
- redesigning `vcd2trace` output;
- making a sidecar authoritative or required for ordinary VCD reading.

## 3. Workstream dependency overview

```text
M0 Decisions + baselines
        │
        ▼
M1 Compact catalog + OpenedVcd
        │
        ▼
M2 Query engine + compatibility
        │
        ├───────────────┐
        ▼               ▼
M3 Technical spikes   M4 Streaming server
        │               │
        ├──────┐        │
        ▼      ▼        │
M5 Sidecar   M6 Cache   │
        └──────┴────────┘
               │
               ▼
M7 Python + hardening + release
```

M4 may begin after the required scheduler/protocol spikes in M3; it does not wait for sidecar/cache implementation. M5 and M6 are optional acceleration backends behind the M2 interface.

---

# M0 — Decisions, semantic lock, and authoritative baseline

Milestone status: `DONE`
Entry: Gate G0 passed
Exit: Gate G1 passed

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M0-T01 | DONE | — | Approve/amend D-001 through D-010 in `PLAN.md` |
| RQ-M0-T02 | DONE | T01 | Inventory public Rust, CLI, Python, and `vcd2trace` contracts |
| RQ-M0-T03 | DONE | T02 | Add focused semantic fixtures |
| RQ-M0-T04 | DONE | T03 | Add characterization and golden compatibility tests |
| RQ-M0-T05 | DONE | T02 | Build benchmark harness and scripts |
| RQ-M0-T06 | DONE | T05 | Capture fresh baseline from current checkout |
| RQ-M0-T07 | DONE | T04,T06 | Approve G1 semantic/performance baseline after review |

## RQ-M0-T01 — Approve product boundary

Expected files:

- `PLAN.md`
- design documents as amended

Acceptance criteria:

- one-file server ownership is accepted or replaced with a documented alternative;
- immutable-generation behavior is explicit;
- cache/index policies and server transport scope are explicit;
- protocol integer/value encoding is selected;
- compatibility and platform boundaries are accepted;
- unresolved choices are assigned owners and due gates.

Validation: document review; no code.

## RQ-M0-T02 — Contract inventory

Record:

- public types/functions and error variants in `src/lib.rs`;
- CLI commands, stdout/stderr, exit behavior, global flag placement;
- Python signatures, dictionary keys/types, exception class/string;
- compare semantics and materialization;
- `vcd2trace` output and signal-path behavior;
- release targets and package contents.

Acceptance: every compatibility requirement maps to a test or an explicit documented exception.

## RQ-M0-T03 — Semantic fixtures

Add small hand-authored VCDs covering:

- aliases sharing one ID;
- duplicate requested targets;
- multiple changes at one timestamp;
- events before, at, inside, and after both boundaries;
- `$dumpvars` before first timestamp;
- empty/no-change/no-timestamp body;
- scalar 0/1/X/Z;
- known vector <=128 bits;
- X/Z vector and vector >128 bits;
- real and string changes;
- repeated/decreasing timestamps if parser accepts them;
- CRLF, whitespace, comments, dump-control commands;
- duplicate declared names;
- malformed body after valid events;
- `$enddefinitions` formatting variations.

Expected files: `tests/fixtures/*.vcd`.

## RQ-M0-T04 — Characterization suite

Expected files:

- `tests/query_semantics.rs`
- `tests/cli_compat.rs`
- `tests/python/` when Python test harness is available
- trace golden fixture if needed

Acceptance:

- current implementation passes before refactor;
- tests cover event multiplicity, body order, alias/request order, normalization, bounds, errors, metadata, compare, CLI formats, and Python shapes;
- output checks use exact/golden comparison where stable.

Validation:

```sh
cargo test
cargo test --test query_semantics --test cli_compat
```

## RQ-M0-T05 — Benchmark harness

Expected files:

- `benches/query.rs` or a deliberately selected benchmark tool
- `scripts/bench-vcd.sh`
- `scripts/bench-rss.sh`
- updates to benchmark document

Requirements:

- avoid embedding external absolute paths in required tests;
- accept dataset paths through arguments/environment;
- record metadata described in the benchmark document;
- checksum output to catch accidental semantic changes;
- support open/list/meta/extract/find/toggle/compare and batched signals;
- separate fast microbenchmarks from large external runs.

## RQ-M0-T06 — Fresh baseline

Rebuild current checkout; confirm binary version matches package. Capture BM-01 through applicable baseline cases on repository fixtures and both supplied VCDs.

Acceptance:

- exact commands and raw summarized results recorded;
- stale `0.1.0` binary no longer used;
- warm/cold policy documented;
- peak RSS and scan CPU recorded;
- no benchmark is accepted without output equivalence/checksum.

## Gate G1 — Semantic and baseline lock

Current status: **PASSED**.

Reviewer decision:

- no remaining M0 issues found;
- RQ-M0-T02 through T06 accepted as `DONE`;
- complete serial Rust suite: 92 passed, 0 failed;
- focused Python console suite: 3 passed;
- durable benchmark records and compatibility claims accepted;
- default-parallel SIGKILL accepted as a documented resource constraint rather than a semantic failure.

Completed evidence:

- `docs/design/current-compatibility-contract.md` inventories the public Rust, native CLI, Python, `vcd2trace`, packaging, error, and semantic contracts, with explicit Python/trace test exceptions assigned to later tasks.
- Eleven focused fixtures plus 21 semantic tests cover aliases, duplicate requests, body/same-time order, exact/inverted/decreasing windows and current early-stop behavior, comments and dump-control commands, dumpvars/no timestamp, empty body, scalar/X/Z, known/unknown/wide vectors, real/string values, CRLF/special scopes, duplicate names, malformed bodies, find, toggle, and expanded compare policy/order cases.
- Fourteen native CLI tests lock version, filtered/full list, metadata, `--pretty` before/after, aligned extraction, signals files, toggle/find found/not-found behavior, compare JSON/compact/default shapes and successful mismatch exit, and errors.
- Three pure-Python console tests lock the documented current divergences for post-subcommand `--pretty` and extraction carry-forward; compiled extension runtime remains an explicit M2/M7 exception.
- `scripts/bench-vcd.sh` and `scripts/bench-rss.sh` fingerprint every primary/secondary input, record complete output/error hashes, optionally verify expected hashes, persist artifacts, and capture version/commit/environment/filesystem/storage/GNU time/RSS/I/O for open/list/meta/extract/find/toggle/compare.
- A fresh `0.1.7` baseline with full hashes, dataset fingerprints, meaningful catalog output, and cheap list/find/extract/compare/batched cases for both supplied VCDs is durably recorded under `docs/benchmarks/artifacts/m0/`; existing full meta/toggle evidence remains documented.
- `cargo check --all-targets`, 38 focused Rust/Python characterization tests, and the complete serial Rust suite (92 tests: 57 existing, 21 semantic, 14 native CLI) pass.

Resource constraints and accepted exceptions:

- Default parallel `cargo test --all-targets` was killed by SIGKILL while existing large-fixture tests concurrently constructed several ~400 MiB indexes. This is a test-runner resource constraint, not a semantic failure. The review-required serial command passes the complete suite. CI should explicitly set one test thread for this fixture until M1 reduces catalog memory or tests are otherwise serialized.
- Project-wide `cargo fmt --all -- --check` still exposes formatting drift in pre-existing production files, and `cargo clippy --all-targets -- -D warnings` reports pre-existing style lints in `src/lib.rs`. All newly added Rust tests pass direct `rustfmt --check`, and `cargo check --all-targets` passes; unrelated source cleanup remains out of M0 scope.
- Compiled Python-extension runtime and byte-golden `vcd2trace` coverage remain explicit later-milestone exceptions.

Pass only when:

- current code passes characterization tests;
- baseline is from the current release build;
- all intended semantic changes are removed, separately approved, or versioned;
- performance budgets are accepted.

---

# M1 — Compact catalog and `OpenedVcd`

Milestone status: `DONE`
Entry: Gate G1 passed
Exit: Gate G2 passed

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M1-T01 | DONE | G1 | Define `FileIdentity`, `GenerationId`, `OpenOptions` |
| RQ-M1-T02 | DONE | G1 | Implement compact scope/name/signal catalog |
| RQ-M1-T03 | DONE | T02 | Consume the parsed vcd 0.7 header tree into the compact catalog |
| RQ-M1-T04 | DONE | T01,T03 | Implement `OpenedVcd::open` and accessors |
| RQ-M1-T05 | DONE | T04 | Implement independent generation-validated readers |
| RQ-M1-T06 | DONE | T04 | Add borrowed `SignalRef` and owned compatibility conversion |
| RQ-M1-T07 | DONE | T04,T05 | Add lazy metadata single-flight state |
| RQ-M1-T08 | DONE | T02-T07 | Concurrency, memory, lookup, and compatibility evidence |

## RQ-M1-T01 — Identity and options

Expected files:

- `src/opened/mod.rs`
- `src/opened/identity.rs`
- `src/error.rs` if errors are split

Requirements:

- identity from opened handles, with target-specific fields gated;
- generation ID stable within one `OpenedVcd`;
- file length, mtime, device/inode/file-ID where available;
- header hash and optional sample/full hash policy;
- options default to streaming, no eager sidecar/cache;
- no body scan during open.

## RQ-M1-T02 — Compact catalog

Expected files: `src/catalog.rs`.

Acceptance:

- names stored once and shared with name lookup;
- aliases store `SignalKey`, not full `Signal` clones;
- scope arena eliminates per-signal cloned scope vectors internally;
- declaration order preserved;
- checked key limits;
- duplicate declaration behavior unchanged;
- estimated owned bytes exposed in tests/diagnostics.

## RQ-M1-T03 — Consuming header-to-catalog conversion

Expected files: `src/header.rs`, reduced `src/lib.rs`.

Acceptance:

- sanitizer behavior remains characterized;
- vcd 0.7 parses the header once, after which its materialized `Header`/`ScopeItem` tree is moved into and consumed by compact catalog construction;
- parser produces exact body offset and timescale without a body scan;
- fixtures and large-header integration tests pass.

Implementation seam: vcd 0.7 exposes `Parser::parse_header` returning a complete `Header`; it does not expose declaration callbacks for true parser-direct catalog construction. Replacing that parser is outside T03. The accepted implementation must consume the tree and release processed nodes rather than borrowing and retaining the full tree while duplicating it.

## RQ-M1-T04 — `OpenedVcd`

Required API responsibilities are in the architecture document.

Acceptance:

- cheap clone through `Arc`;
- compile-time `Send + Sync` assertion;
- path/display identity accessible without exposing unrestricted server paths;
- signals/list lookups avoid owned catalog conversion;
- `open()` latency remains header-bound.

## RQ-M1-T05 — Independent readers

Implementation baseline:

1. open a fresh file handle per query;
2. fingerprint the opened handle;
3. compare it to generation identity;
4. seek that independent handle;
5. validate at completion.

Spike positioned I/O only if needed. Do not rely on `File::try_clone()` independent cursor behavior.

Acceptance tests:

- many concurrent readers produce identical output;
- atomic path replacement is either rejected or creates a later generation, never mixed;
- append/truncate during query returns stale/incomplete according to contract;
- handles do not share cursor position.

## RQ-M1-T06 — Compatibility views

- add borrowed `SignalRef`/iterator;
- materialize current `Signal` and `SignalIndex` only for old APIs;
- preserve public field meaning and declaration order;
- benchmark conversion cost separately from reusable lookup.

## RQ-M1-T07 — Lazy metadata state

State machine:

```text
Absent -> Building -> Ready
                  \-> Failed(retry policy)
```

Acceptance:

- opening/listing does not scan body;
- concurrent metadata calls run at most one scan;
- panic/failure cannot strand `Building`, and all waiters are awakened deterministically;
- immutable ready results and cached failures are shared; explicit retry starts at most one new attempt;
- exact old metadata semantics preserved;
- transport-neutral wait cancellation remains assigned to M2 `QueryContext`, avoiding a conflicting metadata-only public API.

## M1 first-slice evidence (RQ-M1-T01 through T03)

Current status: **ACCEPTED**.

Reviewer decision:

- RQ-M1-T01, T02, and T03 accepted;
- same-handle identity, bounded/strict fingerprints, extensible options, public error compatibility, consuming conversion, and durable RSS evidence verified;
- CR-001 accepted as the formal vcd 0.7 implementation-seam change;
- G2 remains open for T04–T08.

Implementation evidence:

- `src/opened/identity.rs` adds opaque, handle-derived `FileIdentity`, target-gated Unix device/inode evidence, process-local `GenerationId`, and BLAKE3 header/content fingerprints. Windows stable `std` does not expose a non-experimental by-handle file ID, so that target is bound by metadata plus complete-header and content fingerprints; native file-ID support remains additive. Default identity work hashes the complete header plus bounded 64 KiB beginning/end samples; strict full-content hashing is explicit and opt-in.
- `OpenOptions` has private fields, is `#[non_exhaustive]`, and exposes constructor/builder/accessor methods so later sidecar/cache options can be additive. `FingerprintPolicy` is also non-exhaustive.
- `FileIdentity` construction is crate-private. `src/header.rs` provides the provenance-safe seam that reads the header, computes body offset, samples/hashes, and restores cursor position using the same opened handle.
- `src/catalog.rs` stores declaration-order `SignalMeta` records, one shared full-name allocation, numeric `SignalKey` alias lists, and a parent-linked scope arena. Checked `u32` capacity failure maps through the existing `VcdError::Parse` boundary; no public error variant was added.
- vcd 0.7 necessarily materializes a `Header` tree. T03 now moves its items out, drops remaining header metadata, and consumes `ScopeItem`/`Var` nodes while constructing the compact catalog; it does not claim unavailable parser-direct declaration callbacks.
- `read_signals*` preserves public `Signal`/`SignalIndex` return types through a consuming compatibility conversion. Compact lookup maps are released before public clone-heavy maps are built to limit overlap.
- Sixteen focused non-ignored unit tests cover the compatible capacity-error boundary, `Send + Sync` foundations, extensible options, bounded/strict fingerprints, same-handle identity, offset validation, tail-change detection, generation uniqueness, compact names/aliases/nested scopes, consuming compatibility conversion, sanitizer behavior, exact body offset, and memory-estimate accounting.
- Complete serial suite passes: 108 passed, 0 failed, 2 ignored release diagnostics (16 new unit + 92 accepted M0 tests).
- Focused release-mode library and compatibility tests pass; measured diagnostic results are listed below.
- `cargo check --all-targets` passes without warnings. New modules pass `rustfmt --check`; shell syntax and `git diff --check` pass.

Memory/performance observation:

- `scripts/measure-catalog-memory.sh` runs isolated release-test processes for compact-only and compatibility materialization, accepts a configurable sample count, and emits commit, input SHA-256, environment, cache policy, command, GNU time, peak RSS, and probe counts.
- Four alternating warm samples per mode on VCD-A are durably stored in `docs/benchmarks/artifacts/m1/catalog-memory-A.tsv`.
- Compact-only parsing measured 78,700–78,936 KiB peak RSS and 0.17–0.20 s elapsed. The compact catalog retained 193,730 signals with an estimated 36,562,701 owned bytes (188.73 bytes/signal).
- Compatibility materialization measured 393,008–393,088 KiB peak RSS and 0.71–0.77 s elapsed. This is actual process peak RSS, not only estimated ownership.
- The compatibility `list` path remains slower than the M0 0.52–0.55 s baseline because it builds the compact catalog and then materializes clone-heavy public maps. This cold compatibility regression remains a known blocker for G2/T08, not an accepted final result.
- Compact-only actual peak RSS is about 80% below compatibility materialization on VCD-A, demonstrating the reusable representation's benefit once T04 consumers avoid compatibility conversion.

Scope remains limited to T01–T03 in the accepted first slice; no metadata scan, sidecar, cache, or server behavior was added.

## M1 second-slice evidence (RQ-M1-T04 through T06)

Current status: **ACCEPTED**.

Reviewer decision:

- RQ-M1-T04, T05, and T06 accepted;
- default open remains bounded/header-only;
- configured-path reopen, symlink retargeting, complete-header validation, independent cursors, completion validation, Arc ownership, and SignalRef semantics verified;
- G2 remains open for T07/T08.

Implementation evidence:

- `OpenedVcd` is a public cheap-clone `Arc` handle with private immutable inner state: caller/display path, absolute configured path preserving the final symlink, diagnostic canonical target, generation, file identity, body offset, timescale, compact catalog, and open options. Compile-time tests require `OpenedVcd` and `OpenedBodyReader` to be `Send + Sync`.
- Default `open` uses the same-handle identified-header seam and performs header parsing plus bounded beginning/end samples only. Opening the intentionally malformed-body fixture succeeds, proving body commands are not parsed during open. Strict full-content hashing remains explicit opt-in.
- Direct borrowed APIs preserve declaration order and expose signal count, names, lookup, aliases, size/type/ID, borrowed scope components, timescale, and catalog diagnostics without exposing `SignalKey` or internal maps. Legacy `Signal`/`SignalIndex` materialization remains explicit and behavior-compatible.
- `list_signals_from_file` now uses the compact opened catalog directly without a hidden cache. Other path APIs retain their established temporary-open behavior.
- Each `body_reader` reopens the absolute configured path without resolving its final symlink, then validates opened-handle metadata/platform identity, the complete raw header, and bounded samples (plus full content only in strict mode) before seeking its independent handle to the stored body boundary. No `File::try_clone` cursor sharing is used.
- `OpenedBodyReader` implements `Read + BufRead + Seek`, carries its generation, and requires explicit completion validation. `body_parser` is the M2 parser seam. Completion validation rechecks both the admitted handle and a fresh reopen of the configured path while preserving the admitted reader's logical cursor, so final-symlink retargeting is rejected.
- Tests cover cheap clone/shared inner, stable absolute configured paths, direct order/filter/lookup/alias/scope, owned compatibility, no body parse, independent cursors, eight concurrent equivalent parsers, atomic replacement, Unix configured-symlink retarget rejection at admission and completion, append, truncate, sampled in-place mutation, complete >64 KiB header validation outside bounded samples, mutation after admission, cursor restoration after rejection, default/strict policy, and parser body start.
- Complete serial debug suite passes with 125 non-ignored tests and 3 release diagnostics ignored. Focused release `opened`, semantic, and CLI suites pass. `cargo check --all-targets`, `cargo doc --no-deps`, new-module formatting, shell syntax, and `git diff --check` pass.

Measured direct reusable access — **preview only, not G2 acceptance**:

- Dirty-worktree VCD-A preview evidence is in `docs/benchmarks/artifacts/m1/catalog-memory-A.tsv` with four alternating samples per compact/opened/compatibility mode.
- `OpenedVcd` plus a complete borrowed name pass and lookup measured 0.17–0.18 s and 78,644–79,080 KiB peak RSS versus 0.67–0.68 s and 392,864–393,108 KiB for legacy compatibility materialization.
- After one open, 100 complete borrowed name passes took 8,067–10,873 µs total and 100,000 lookups took 1,261–1,352 µs total. Output allocation/serialization is intentionally excluded from these catalog-access timings.

Remaining before G2: T07 review and T08 final cross-target/concurrency/memory/compatibility review. T08 must capture at least five samples per mode from a clean committed tree with verified input hashes; the current four-sample artifact remains preview evidence. Installed-target `cargo check --lib` covers target-gated source; native CI remains responsible for release linking and Windows native file-handle behavior.

## M1 third-slice evidence (RQ-M1-T07)

Current status: **ACCEPTED**.

Reviewer decision:

- RQ-M1-T07 accepted;
- laziness, single-flight scanning, completion validation, shared results, attempt-stable failure/retry, panic recovery, waiter acknowledgements, and bounded history verified;
- report-only test improvement: T08 should replace retry-path timing sleeps with an explicit test hook/counter;
- G2 remains open for T08.

Implementation evidence:

- `OpenedVcdInner` owns a mutex/condition-variable metadata state machine with monotonic attempt epochs: `Absent -> Building(attempt, registered waiters) -> Ready(Arc<VcdMeta>)` or `Failed(attempt, cached error, pending acknowledgements)`.
- `OpenedVcd::metadata` performs no body work until first use, returns one shared immutable `Arc<VcdMeta>`, and coalesces concurrent callers behind one scan.
- Metadata scans use the independent validated body parser, preserve the existing `compute_time_bounds` implementation through an in-place helper, and run completion validation before publishing `Ready`.
- Every caller that observes `Building` registers against that exact attempt before sleeping. Failure/panic retains one terminal outcome until all registered waiters acknowledge it; `retry_metadata` cannot erase or supersede that outcome and concurrent retries coalesce behind the next attempt. Only one terminal attempt is retained, so history is bounded.
- The RAII build guard publishes a shared `metadata builder panicked` failure for the active attempt and notifies all waiters during unwinding, so panic cannot strand `Building`.
- M2 retains ownership of cancellable waits through `QueryContext`; no conflicting metadata-only cancellation surface was introduced. M2 must decrement attempt acknowledgement through a cancellation guard if it allows a registered waiter to leave before consuming an outcome.
- Deterministic hooks test zero scans at open, sixteen concurrent callers/one scan/shared pointer, cached malformed-body failure, a retry racing two deliberately blocked failed-attempt waiters, a panicked builder with two actual blocked waiters followed by recovery, mutation and atomic replacement immediately before publication, exact empty/no-timestamp/decreasing/body-at-zero semantics, and public lazy/shared behavior.
- Post-race-fix validation passes: complete serial debug suite 133 passed with 3 release diagnostics ignored; release library 37 passed with 3 ignored; release opened/semantic/CLI suites 39 passed; `cargo check --all-targets`, changed-file formatting, shell syntax, Markdown links, and `git diff --check` pass.

Scope remains T07 only. G2 remains open for T08 clean-tree performance, available cross-target builds, and final gate review.

## M1 final-gate evidence preparation (RQ-M1-T08)

Current status: **ACCEPTED / G2 PASSED**.

Completed evidence:

- The two metadata retry race tests no longer depend on `sleep` or `JoinHandle::is_finished`. A test-only retry-waiter counter records the exact condition-variable wait; both failure and panic tests wait for that state before releasing registered attempt waiters. Production synchronization is unchanged. Both tests also passed 20 consecutive focused repetitions.
- BLAKE3 uses its `pure` feature so target checking does not require a target-native assembler. This selects the crate's portable implementation without changing BLAKE3 digest semantics or the persisted in-memory fingerprint shape.
- `cargo check --lib` passes for the host and all four installed release targets: `x86_64-pc-windows-msvc`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`. These are preliminary source/type checks; T08 must rerun `cargo check --all-targets` from the clean candidate commit. Per CR-002, native release linking/archive construction remains a G8/release-CI responsibility. Commands and portability findings are durably recorded in `docs/benchmarks/artifacts/m1/target-checks.md`.
- The Windows target exposed that `MetadataExt::volume_serial_number` and `file_index` are still unstable on the pinned stable toolchain. The implementation now uses Unix device/inode only where stable `std` exposes them. Windows and other targets use file length/mtime plus complete-header and bounded-content fingerprints; stronger native file IDs remain an additive hardening option.
- Complete serial debug and release suites each pass 133 non-ignored Rust tests with 3 diagnostics ignored. Three Python console tests pass. `cargo doc --no-deps`, changed-file `rustfmt --check`, shell syntax, and `git diff --check` pass. Project-wide `cargo fmt --all -- --check` still reports only the accepted pre-existing formatting drift in untouched legacy files documented at G1; T08 does not widen scope to reformat them.
- Concurrency/snapshot evidence from T04–T07 covers eight concurrent independent parsers, one metadata scan across sixteen concurrent callers, immutable shared results, configured-symlink retargeting, atomic replacement, append/truncate/in-place mutation, complete-header mutation outside bounded samples, and pre-publication validation.
- Compatibility behavior remains protected by the accepted M0 semantic, CLI, Python-console, and integration suites. The reusable list path avoids legacy `SignalIndex` materialization; final clean-tree path-list measurements remain required to disposition the previous cold compatibility latency regression.

Final clean candidate evidence:

- Code candidate commit: `d420347707b5c3a3e8e56582e82d64a98f332ac1`.
- `cargo check --all-targets` passed from a clean tree for Linux x64, Linux ARM64, Windows x64 MSVC, macOS Intel, and macOS ARM64. Per CR-002, native linking/packaging remains G8 release-CI evidence.
- Complete debug and release suites each passed 133 Rust tests with 3 diagnostics ignored; 3 Python console tests and `cargo doc --no-deps` passed.
- `docs/benchmarks/artifacts/m1/catalog-memory-A-g2.tsv` contains five alternating samples per mode with `git_dirty=false`, `evidence_status=g2_candidate`, exact commit, and verified VCD-A SHA-256.
- `OpenedVcd` median peak RSS is 78,804 KiB versus 392,708 KiB for legacy materialization, a 79.9% reduction that exceeds the 40% target.
- Borrowed traversal/lookup medians are 9,260 µs for 100 complete catalog traversals and 1,246 µs for 100,000 name lookups after open.
- `docs/benchmarks/artifacts/m1/path-list-A-g2.tsv` contains five zero-status samples with the accepted M0 output hash verified on every run.
- Native filtered list improved from the accepted ~0.45 s baseline to a 0.16 s median and from 421,588 KiB to approximately 79,208 KiB median peak RSS after routing the CLI through the compact path.
- Full commands/results and the CR-002 scope are summarized in `docs/benchmarks/artifacts/m1/g2-validation.md` and `docs/benchmarks/reusable-query-baseline.md`.

Reviewer gate decision:

- RQ-M1-T08: `DONE`;
- Gate G2: `PASSED`;
- Milestone M1: `DONE`;
- M2 may proceed;
- non-blocking correction applied: borrowed traversal median is 9,260 µs;
- CR-002 G8 native link/archive/wheel/runtime obligations remain mandatory.

## Gate G2 — Core representation and snapshot

Current status: **PASSED**.

Pass when:

- characterization suite remains green;
- catalog lookup is no slower materially than current lookup;
- provisional 40% memory-reduction target passes or is explicitly revised;
- independent-reader/snapshot tests pass;
- `OpenedVcd` has no shared parser mutex;
- all current release targets pass `cargo check --all-targets` from the candidate commit; native release linking, archives, and runtime platform behavior remain G8/release-CI evidence per CR-002.

---

# M2 — Transport-neutral query engine and compatibility wrappers

Milestone status: `IN_PROGRESS`
Entry: Gate G2 passed
Exit: Gate G3

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M2-T01 | DONE | G2 | Query context, limits, cancellation, internal error model |
| RQ-M2-T02 | DONE | T01 | Streaming backend with target check before conversion |
| RQ-M2-T03 | READY | T02 | Reusable extraction iterator |
| RQ-M2-T04 | BLOCKED | T03 | Reusable find and toggle |
| RQ-M2-T05 | BLOCKED | T03 | Timeline-provider interface and comparison adaptation |
| RQ-M2-T06 | BLOCKED | T03-T05 | Path-based compatibility wrappers |
| RQ-M2-T07 | BLOCKED | T06 | CLI uses one `OpenedVcd` per input |
| RQ-M2-T08 | BLOCKED | T06 | Python module functions use temporary `OpenedVcd` |
| RQ-M2-T09 | BLOCKED | T06 | Evaluate `vcd2trace` migration |
| RQ-M2-T10 | BLOCKED | T01-T09 | Differential backend/compatibility evidence |

## RQ-M2-T01 — Query context

Status: **ACCEPTED**.

Reviewer verified public extensibility, no `VcdError` break, cancellation/deadline ordering, attempt-stable metadata waiter acknowledgements, elected-builder post-publication checks, typed stale-generation caching/classification, timeout overflow behavior, deterministic controlled tests, and preserved source chains.

Acceptance:

- transport-neutral cancellation token and finite/unlimited limits;
- deadline and command/result budgets;
- checks at defined command/timestamp/allocation/emission/wait points;
- old wrappers can select legacy-unlimited behavior;
- typed errors map cleanly to legacy strings and later protocol codes.

Implementation status: **IN REVIEW**.

Evidence:

- `src/query/mod.rs` adds cheap-clone `CancellationToken`, extensible `QueryContext`/`QueryLimits`, stable `QueryErrorCode`/`QueryLimitKind`, structured `QueryError`, and `QueryResult` without changing `VcdError`.
- Limits cover signals, rows, encoded/result bytes, commands/scan budget, and absolute/relative deadlines. `QueryContext::legacy_unlimited` is the explicit compatibility policy.
- Cancellation uses one shared atomic flag; ordinary checks are lock-free. Metadata waits use bounded condition-variable timeouts so cancellation/deadline is observed without a transport dependency.
- `OpenedVcd::metadata_with_context` checks cancellation before work, uses cancellation-safe RAII waiter registration, and preserves existing `metadata()` behavior. Cancelling a waiter does not cancel the shared builder; once elected, a metadata builder completes and publishes its cache outcome for other callers, then performs one final context check before returning to the elected caller.
- Generation mismatch uses a crate-private typed `io::Error` payload detected through downcast, while preserving the exact legacy display text and avoiding a new `VcdError` variant. `CachedMetadataError` has a dedicated `GenerationMismatch` case and reconstructs the same typed payload, so elected builders, attempt-registered waiters, and later cached contextual callers all retain `STALE_SOURCE`; similarly worded ordinary I/O remains `SOURCE_UNAVAILABLE`.
- Overflowing relative deadlines such as `Duration::MAX` are documented and treated as effectively unlimited rather than immediately expired.
- Deadline waiter tests use a shared test-controlled expiration flag after explicit waiter registration; they do not depend on scheduler timing or wall-clock sleeps.
- Structured query errors distinguish cancellation, deadline, specific limits, stale source, queue full, source unavailable, VCD failures, and internal failures with stable transport-facing codes and error sources where applicable.
- Unit/public integration coverage includes token propagation, all builders/accessors, cancellation precedence, deadline and overflow, every limit, display/code/source mapping, real stale-source mutation, attempt-stable cached stale-source classification for elected builder/two waiters/later caller, cancellation before work, cancellation/deadline while registered, waiter acknowledgement cleanup, retry recovery, shared-builder success, and elected-builder cancellation/deadline after Ready/Failed publication with cached outcomes retained.
- Complete serial debug and release suites each pass 153 non-ignored Rust tests with 3 release diagnostics ignored; 3 Python console tests and `cargo doc --no-deps` pass.
- `cargo check --all-targets` passes for the host plus Windows x64 MSVC, Linux ARM64, macOS Intel, and macOS ARM64 targets under CR-002.
- Changed-file `rustfmt --check`, `git diff --check`, and Markdown link validation pass.

Remaining for review: fresh-context reviewer acceptance. T02 remains blocked until T01 is accepted.

## RQ-M2-T02 — Optimized stream decoder

Status: **ACCEPTED**.

Reviewer verified cancellation/deadline precedence at timestamp and periodic checkpoints, exact command budgeting, catalog-owned requested-name sharing with duplicate multiplicity, selected-ID-first conversion, and reproducible 1/10/100-target conversion/timing evidence.

Status: **IN_REVIEW**.

Implementation evidence:

- `src/query/stream.rs` adds a crate-private `SelectedChangeStream` over `OpenedVcd::body_parser`, forming the transport-neutral T03 seam without prematurely exposing raw backend details publicly.
- Target resolution enforces `max_signals`, preserves request index/name/size plus alias and duplicate bindings by `IdCode`, reports missing names deterministically, and completes before reader admission.
- Scalar/vector/real/string commands inspect ID membership and time-window inclusion before `ChangeValue` conversion. One selected body command is converted once even when aliases/duplicate requests share its ID.
- Command count includes timestamps, irrelevant changes, dump/control commands, and parser errors. Limit N permits exactly N commands; command N+1 returns `LimitExceeded(Commands, limit=N, actual=N+1)`.
- Context is checked before resolution/admission, at every timestamp, every 4,096 commands in non-timestamp irrelevant runs, before selected conversion, and before/after successful completion validation.
- EOF and deliberate `end` early-stop validate the opened generation before reporting successful terminal completion. Parser errors, cancellation, deadline, and limits terminate without claiming successful completion.
- Thirteen deterministic tests cover irrelevant vector/string/real/scalar conversion avoidance, out-of-window avoidance, exact normalization/body/alias order versus legacy extraction, duplicate Arc-backed binding multiplicity, exact command/signal limits, timestamp/interval/pre-conversion cancellation, cancellation/deadline versus budget collisions, controlled deadline, EOF/end mutation, malformed body, and decreasing-timestamp early-stop equivalence.
- `scripts/measure-targeted-conversion.sh` and `docs/benchmarks/artifacts/m2/` provide the required release diagnostic for 1/10/100 selected IDs among 128 densely changing signals over 1,000 timestamps. All 15 runs parsed 129,000 commands and converted exactly `selected IDs × 1,000` values. Median internal decode was 4,578/4,624/12,580 µs respectively. This is conversion-count/timing evidence only; allocator calls/bytes were not measured and are not claimed.

Corrective review evidence:

- Timestamp and 4,096-command periodic cancellation/deadline checkpoints now run before a colliding N+1 command-budget failure, preserving cancellation-first semantics. Away from those checkpoints, exact budget enforcement is unchanged.
- Duplicate requests clone the catalog-owned `Arc<str>` rather than allocating a new requested-name string. Tests verify pointer identity with both duplicate bindings and the catalog entry while preserving duplicate binding multiplicity.
- Final post-correction validation passed once in debug and once in release: 165 non-ignored Rust tests passed with 4 release diagnostics ignored in each profile; 3 Python console tests passed; all five CR-002 target checks, changed-file formatting, shell syntax, Markdown links, and `git diff --check` passed.

T03 handoff: expand each `SelectedChange` through `SelectedChangeStream::bindings(id_code)` in binding order. `TargetBinding` provides request index, shared catalog-backed requested name, and signal width. Apply row/result-byte limits while expanding, not in the raw decoder.

Refactor command handling to inspect command ID before `ChangeValue` conversion.

Acceptance:

- unselected vectors/strings are not formatted or allocated into `ChangeValue`; parser-internal ownership is outside this decoder's control and allocator behavior is not claimed without allocator instrumentation;
- selected normalization unchanged;
- cancellation frequency bounded;
- benchmark covers 1/10/100 targets among dense unrelated changes with exact conversion counts and timing.

## RQ-M2-T03 — Extraction iterator

Acceptance:

- streams rather than collects;
- preserves body order, same-time changes, aliases, duplicate targets, and inclusive bounds;
- owns all state required to outlive method call safely;
- stale completion is detectable;
- server can chunk it without CLI/Python dependencies.

## RQ-M2-T04 — Find and toggle

Acceptance:

- occurrence zero/error behavior unchanged;
- find stops early when possible;
- toggle baseline semantics unchanged;
- value equality avoids formatting only where proven equivalent;
- missing and empty target behavior characterized.

## RQ-M2-T05 — Timeline provider and compare

Introduce a backend-neutral source capable of streaming or providing cached timelines. Do not require compare in the first server milestone.

Acceptance:

- existing compare result is unchanged;
- mixed backend tests pass;
- multi-ID ordering uses stable body sequence;
- memory behavior is documented; remaining full materialization is explicitly tracked.

## RQ-M2-T06 — Compatibility wrappers

Existing path functions create a temporary `OpenedVcd` and call reusable methods. No hidden process cache.

Acceptance: signatures/return types/errors remain compatible and old tests pass unchanged.

## RQ-M2-T07 — CLI migration

- open each input once per invocation;
- keep rendering in `src/main.rs` or a presentation module;
- preserve TSV/table output and exit codes;
- avoid pretty-mode regressions; optionally track existing materialization separately.

## RQ-M2-T08 — Python module compatibility

Route old functions through temporary objects. Persistent class is M7.

Acceptance: existing Python signatures, keys, values, and exceptions pass tests.

## RQ-M2-T09 — `vcd2trace`

Migrate only if it reduces duplication without delaying gates. Output must be byte-identical. Otherwise mark `DEFERRED` with rationale.

## Gate G3 — Query engine compatibility

Pass when:

- old/new differential tests agree for every semantic fixture;
- CLI golden and Python compatibility suites pass;
- cancellation/limit tests pass;
- targeted-conversion benchmark shows no material regression;
- all-target builds remain green.

---

# M3 — Required technical spikes and design freeze

Milestone status: `BLOCKED` on G3
Exit: Gate G4

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M3-T01 | BLOCKED | G3 | Confirm independent snapshot-reader behavior across targets |
| RQ-M3-T02 | BLOCKED | G3 | Prove parser-safe timestamp offsets/resume |
| RQ-M3-T03 | BLOCKED | G3 | Prototype compact event bytes/order fidelity |
| RQ-M3-T04 | BLOCKED | G3 | Prototype bounded server scheduling/cancellation/backpressure |
| RQ-M3-T05 | BLOCKED | G3 | Freeze protocol value/numeric schemas and limits |
| RQ-M3-T06 | BLOCKED | T01-T05 | Gate report selecting mechanisms or deferring features |

## RQ-M3-T01 — Snapshot-reader spike

Compare per-query reopen-and-fingerprint with positioned-I/O adapter where needed. Test atomic replacement and cursor independence on Linux/macOS/Windows library targets.

## RQ-M3-T02 — Offset/resume spike

Follow `sidecar-format-v1.md`. This is a hard correctness gate. Sample the supplied large VCDs but keep deterministic small fixtures in CI.

Outcomes:

- `PASS`: parser logical offsets are safe; proceed to M5;
- `ALTERNATIVE`: implement dedicated checkpoint scanner and re-test;
- `DEFER`: sidecar removed from v1; streaming/cache/server proceed;
- never approximate offsets from raw file cursor.

## RQ-M3-T03 — Compact timeline spike

On representative signals measure:

- bytes/event and build peak RSS;
- global sequence overhead;
- scalar/integer/wide/XZ/real/string encoding;
- text interning benefit;
- alias sharing;
- stable k-way merge output;
- an oversize/high-frequency clock timeline.

Do not implement cache policy until representation is accepted.

## RQ-M3-T04 — Scheduler spike

Prototype standard-library bounded threads/channels:

- partial frame reader;
- global bounded jobs;
- per-connection bounded output;
- cancellation during scan and blocked output;
- one expensive scan per generation;
- slow/disconnected clients.

Adopt async runtime only if measurements show the simpler model cannot meet target concurrency/fairness/cancellation.

## RQ-M3-T05 — Protocol schema freeze

Resolve:

- time/count/value string encoding;
- `extract` event schema;
- non-finite real representation;
- unknown optional fields;
- per-method and global limits;
- incomplete stream semantics;
- whether `compare` is deferred;
- subcommand versus dedicated binary.

Add golden JSON fixtures before server implementation.

## Gate G4 — Mechanism selection

Required decisions:

- approved independent reader;
- sidecar proceed/alternative/defer;
- timeline representation proceed/defer;
- thread/channel or async scheduler;
- frozen experimental protocol v1 schema and limits.

---

# M4 — Experimental bounded Unix-socket server, streaming backend

Milestone status: `BLOCKED` on G4
Exit: Gate G5

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M4-T01 | BLOCKED | G4 | Protocol types/decoder/encoder and golden tests |
| RQ-M4-T02 | BLOCKED | T01 | Safe listener lifecycle and permissions |
| RQ-M4-T03 | BLOCKED | T01 | Bounded scheduler and request registry |
| RQ-M4-T04 | BLOCKED | T02,T03 | Connection reader/writer and backpressure |
| RQ-M4-T05 | BLOCKED | T03,T04 | Cancellation/deadline/disconnect handling |
| RQ-M4-T06 | BLOCKED | T01,T05 | `ping`, `describe`, `list`, `metadata`, `extract`, `find`, `toggles` |
| RQ-M4-T07 | BLOCKED | T06 | Generation invalidation/reopen behavior |
| RQ-M4-T08 | BLOCKED | T02-T07 | CLI/binary integration and configuration validation |
| RQ-M4-T09 | BLOCKED | T08 | Race-free test client and end-to-end suite |
| RQ-M4-T10 | BLOCKED | T08,T09 | Load, soak, leak, security, and portability evidence |

## RQ-M4-T01 — Protocol implementation

Expected files:

- `src/server/protocol.rs`
- `tests/fixtures/protocol-v1/*.jsonl`

Acceptance:

- bounded incremental frame decoding;
- version, ID, sequence, error and stream lifecycle;
- precise numeric/value representation;
- no unrestricted paths/backtraces in errors;
- malformed/fuzzed input cannot panic.

## RQ-M4-T02 — Listener lifecycle

Expected files: `src/server/listener.rs`.

Acceptance:

- Unix-only target gating;
- owner-only defaults;
- refuses non-socket unlink;
- distinguishes stale/live socket conservatively;
- removes only its owned socket on graceful shutdown;
- Windows current binaries/library still compile.

## RQ-M4-T03 — Scheduler

Acceptance:

- finite connections, requests/connection, workers, queues, expensive scans;
- saturation returns `QUEUE_FULL`;
- no lock held through scan/socket write;
- panic boundary cleans request/build state;
- queue/stat counters observable.

## RQ-M4-T04 — Connections/backpressure

Acceptance:

- frames for requests may interleave; per-ID order remains stable;
- partial/multiple frames handled;
- bounded output chunks/queue;
- slow reader does not grow memory without limit.

## RQ-M4-T05 — Cancellation

Acceptance:

- cancellation before scheduling, during scan, while waiting, and during blocked output;
- disconnect cancels owned requests;
- target terminal state unambiguous;
- cancellation latency benchmarked.

## RQ-M4-T06 — Service methods

Initial server uses streaming backend; optional backends are capability-advertised later.

Every method validates limits, source generation, signal count, time window, and result budget. `compare`, cache mutation, and shutdown remain disabled/deferred unless approved.

## RQ-M4-T07 — Generation handling

Acceptance:

- path replacement between requests opens a new complete generation or returns unavailable;
- old in-flight request remains on old handle;
- mutation during unary discards result;
- mutation during stream terminates incomplete/stale;
- cache/index never cross generations.

## RQ-M4-T08 — Command integration

Recommended command:

```sh
vcd_tools_rs serve <VCD> --socket <PATH> ...
```

Acceptance:

- `--help` documents every finite limit and immutable-file contract;
- invalid settings fail before binding;
- server can be excluded/unsupported cleanly on non-Unix;
- no regression to existing command parsing.

## RQ-M4-T09 — End-to-end tests

Test helper starts server, waits on readiness without arbitrary sleeps, sends requests, cancels, and shuts down. Tests own unique temporary socket paths.

## RQ-M4-T10 — Soak/security/portability

Cover malformed clients, oversized frames, slow readers, disconnect, queue overload, stale sockets, permissions, graceful shutdown, source replacement, and many clients. Record RSS, FDs, threads/tasks, queue depth, and p50/p95/p99.

## Gate G5 — Experimental server

Pass when:

- protocol and operations tests pass;
- resources/results are bounded;
- cancellation and backpressure are proven;
- no source-generation mixing;
- Linux/macOS server tests and Windows non-server builds pass;
- soak shows no unexplained growth;
- docs and rollback are present.

---

# M5 — Sparse persistent sidecar

Milestone status: `BLOCKED` on M3 offset spike
Exit: Gate G6

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M5-T01 | BLOCKED | M3-T02 PASS | Freeze binary v1 layout and golden fixture |
| RQ-M5-T02 | BLOCKED | T01 | Checked decoder/encoder and fuzz target |
| RQ-M5-T03 | BLOCKED | T02 | Builder with exact metadata/checkpoints |
| RQ-M5-T04 | BLOCKED | M1-T01,T03 | Fingerprint and stale/corrupt validation |
| RQ-M5-T05 | BLOCKED | T03,T04 | Atomic publish and inter-process single builder |
| RQ-M5-T06 | BLOCKED | T03 | Sparse-seek streaming backend |
| RQ-M5-T07 | BLOCKED | T06 | Differential/property equivalence suite |
| RQ-M5-T08 | BLOCKED | T02,T05,T07 | Corruption, mutation, interruption, fuzz evidence |
| RQ-M5-T09 | BLOCKED | T06-T08 | Stride/size/build/query benchmark and policy decision |
| RQ-M5-T10 | BLOCKED | G6 | Expose opt-in CLI/server policy and documentation |

Key acceptance criteria are normative in `sidecar-format-v1.md`.

Additional requirements:

- build cancellation leaves no accepted partial file;
- index failure falls back unless policy is `Require`;
- exact metadata can populate `OpenedVcd` lazy state;
- sidecar can benefit normal CLI/Python opened objects, not only server;
- decoder performs bounded allocation with checked arithmetic;
- unknown versions remain untouched.

## Gate G6 — Sidecar opt-in

Pass when exact backend equivalence, mutation/corruption safety, atomic publication, fuzzing, size target, and tail-query speed target pass. Otherwise sidecar remains experimental/deferred; server streaming behavior remains functional.

---

# M6 — Bounded selective timeline cache

Milestone status: `BLOCKED` on M2 and M3 representation spike
Exit: Gate G7

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M6-T01 | BLOCKED | M3-T03 PASS | Compact timeline with global sequence |
| RQ-M6-T02 | BLOCKED | T01 | Binary-search slicing and stable multi-ID merge |
| RQ-M6-T03 | BLOCKED | T01 | Byte-accounted LRU/cache statistics |
| RQ-M6-T04 | BLOCKED | T03 | Per-ID single-flight and batched loader |
| RQ-M6-T05 | BLOCKED | T04,M2-T01 | Cancellation-safe reservations/waiters |
| RQ-M6-T06 | BLOCKED | T03-T05 | Off/Lazy/EagerSelected policies, warm/evict |
| RQ-M6-T07 | BLOCKED | T02,T06 | Integrate extract/find/toggle and optional compare |
| RQ-M6-T08 | BLOCKED | T07 | Differential, concurrency, eviction, oversize tests |
| RQ-M6-T09 | BLOCKED | T08 | Hit/build/RSS/alias/order benchmarks |
| RQ-M6-T10 | BLOCKED | G7 | Expose opt-in server/OpenedVcd controls and docs |

Acceptance details:

- key by `IdCode`; aliases share storage;
- store `(time, sequence, compact value)` without names;
- stable k-way merge reproduces body order;
- allocated capacity, interned text ownership, and metadata included in accounting;
- cache ownership never exceeds budget after admission/eviction;
- active `Arc` users survive eviction;
- oversize timeline is not admitted;
- overlap coalesces and disjoint build concurrency is bounded;
- failure/cancel/panic always clears reservations and wakes waiters;
- no eager all-signal default.

## Gate G7 — Cache policy

Select `Off`, `Lazy`, or another default only from workload evidence. Initial stable default remains `Off` unless retained-memory and promotion behavior are demonstrably safe. Hot-hit target is at least 10× over full scan.

---

# M7 — Persistent Python API, hardening, CI, and rollout

Milestone status: `BLOCKED` on G5 and selected optional backends
Exit: Gate G8

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M7-T01 | BLOCKED | G3 | PyO3 `Vcd` class backed by `OpenedVcd` |
| RQ-M7-T02 | BLOCKED | T01 | Release GIL during scans/waits |
| RQ-M7-T03 | BLOCKED | T01,T02 | Bounded-memory `iter_extract` |
| RQ-M7-T04 | BLOCKED | T01-T03 | Python exports, stubs, compatibility/docs/tests |
| RQ-M7-T05 | BLOCKED | G5,G6/G7 as included | Fuzz malformed VCD/sidecar/protocol |
| RQ-M7-T06 | BLOCKED | G5 | Structured logs and operational counters |
| RQ-M7-T07 | BLOCKED | T01-T06 | Full documentation and examples |
| RQ-M7-T08 | BLOCKED | T01-T07 | CI matrix and cross-target/package checks |
| RQ-M7-T09 | BLOCKED | T08 | Final benchmark matrix and release-candidate soak |
| RQ-M7-T10 | BLOCKED | T09 | Experimental/stable release and feedback review |

## RQ-M7-T01 through T04 — Python

Target API:

```python
with vcd_tools.Vcd("simulation.vcd") as vcd:
    names = vcd.list_signals(filter="clk")
    meta = vcd.metadata()
    rows = vcd.extract(["tb.clk"], start=0, end=1000)
    for row in vcd.iter_extract(["tb.clk"], start=0):
        ...
```

Acceptance:

- object reuses catalog/index state;
- old module functions remain;
- list-returning `extract` remains compatible;
- iterator memory is bounded by buffering;
- long Rust work releases GIL;
- drop/close/lifetime and second-thread behavior tested;
- operations after close fail deterministically;
- Python 3.8 ABI3 through supported versions pass wheel smoke tests;
- type stubs added.

## RQ-M7-T05 — Fuzzing

Targets:

- VCD/header/body parser boundary inputs where feasible;
- sidecar decoder and offset equivalence;
- protocol decoder;
- query backend equivalence on generated valid subsets.

Bound input size/time to detect excessive allocation as well as panic.

## RQ-M7-T06 — Observability

Log/counter fields:

- request ID, generation, method, backend;
- queue and execution duration;
- commands/bytes scanned where measurable;
- rows/bytes emitted;
- cancellation/deadline/limit/stale result;
- sidecar state/build/reject reason;
- cache hit/miss/build/eviction/accounted bytes.

Do not log waveform values or unrestricted paths by default.

## RQ-M7-T08 — CI

Required jobs:

- format and clippy with warnings denied;
- Rust tests/all targets;
- Linux/macOS server integration;
- Windows current binary/library build;
- Python ABI3 wheel/import smoke matrix;
- golden sidecar/protocol compatibility;
- scheduled large external/performance job if infrastructure permits;
- fuzz smoke in CI and longer scheduled runs.

## Gate G8 — Stable completion

The project is complete only when:

1. `OpenedVcd` is public, reusable, thread-safe, and documented.
2. Existing Rust, CLI, Python, and trace behavior passes compatibility suites.
3. Catalog memory target is accepted.
4. Every shipped optimized backend is exactly equivalent to streaming semantics.
5. Sidecars, if shipped, are atomic, checked, invalidated, fuzzed, and disposable.
6. Cache, if shipped, obeys byte limits and ordering under concurrency/cancellation.
7. Server has finite workers, queues, request/result sizes, and deadlines.
8. Cancellation works during scan, waits, and output backpressure.
9. Mutation/replacement never mixes generations.
10. Python persistent and iterative APIs are safe and documented.
11. Current release targets and packages remain valid.
12. Final benchmarks meet gates or contain explicit approved waivers.
13. Rollback by disabling server/sidecar/cache is tested.

---

# 4. Validation command matrix

## Fast checks

```sh
cargo fmt --all -- --check
cargo check --all-targets
cargo test
```

## Full Rust checks

```sh
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo test --release
cargo test --features python
```

Python-feature linking may require a dedicated configured CI job; it must not be silently skipped.

## Python

```sh
python -m venv .venv
. .venv/bin/activate
python -m pip install -U pip maturin pytest
maturin develop --features python
python -m pytest tests/python
```

## Server

```sh
cargo test --test server_protocol -- --nocapture
cargo test --test server_concurrency -- --nocapture
cargo test --test server_invalidation -- --nocapture
cargo run --release -- serve tests/waveform.vcd \
  --socket /tmp/vcd-tools-test.sock \
  --workers 4 --queue-depth 16
```

Smoke request:

```sh
printf '%s\n' '{"v":1,"id":"1","method":"metadata","params":{}}' \
  | socat - UNIX-CONNECT:/tmp/vcd-tools-test.sock
```

## Fuzz

```sh
cargo fuzz run sidecar_decode -- -max_total_time=300
cargo fuzz run index_equivalence -- -max_total_time=300
cargo fuzz run protocol_decode -- -max_total_time=300
```

## Benchmarks

Follow [`../benchmarks/reusable-query-baseline.md`](../benchmarks/reusable-query-baseline.md). Never compare debug and release builds or omit output-equivalence checks.

---

# 5. Risk register

| ID | Risk | Severity | Mitigation/gate | Status |
|---|---|---|---|---|
| R-01 | Parser checkpoint invalid due to buffering/state | Critical | M3-T02 hard spike; differential tests; defer if unproven | OPEN |
| R-02 | Seeking misses required prior value state | High | Preserve current event-only/baseline semantics; no synthesized state | OPEN |
| R-03 | Per-ID cache loses same-time body order | Critical | Store global sequence and stable k-way merge | OPEN |
| R-04 | Header/body/index refer to different file generations | Critical | Open-handle identity, generation binding, pre/post validation | OPEN |
| R-05 | Current public `SignalIndex` change breaks users | High | Keep public type; internal compact catalog plus conversion | OPEN |
| R-06 | `open()` accidentally performs full metadata scan | High | Lazy metadata test and latency benchmark | OPEN |
| R-07 | Cache exceeds actual memory budget | High | Capacity-aware accounting, oversize rejection, RSS stress | OPEN |
| R-08 | Loader/build state deadlocks after cancel/panic | Critical | RAII guards, guaranteed wakeups, fault injection | OPEN |
| R-09 | Slow client exhausts workers/memory | Critical | Bounded queues, deadlines, cancellable sends | OPEN |
| R-10 | Arbitrary filesystem access through server | Critical | One configured VCD; defer/restrict compare paths | OPEN |
| R-11 | Unix server breaks Windows builds/releases | High | `cfg(unix)`, CI cross-target checks, packaging rules | OPEN |
| R-12 | JSON loses `u64`/`u128`/float fidelity | High | String encoding and golden cross-language tests | OPEN |
| R-13 | Sidecar stale/corrupt acceptance | Critical | Multi-field identity, hashes/checksums, bounded decoder, fallback | OPEN |
| R-14 | Interrupted sidecar publishes partial data | Critical | temp + sync + atomic rename + lease | OPEN |
| R-15 | Large ordinary CI becomes slow/flaky | Medium | tiny deterministic fixtures; external scheduled benchmark jobs | OPEN |
| R-16 | Python iterator/GIL lifetime bug | High | Arc ownership, close/drop/thread tests, bounded iterator | OPEN |
| R-17 | Async runtime adds packaging complexity | Medium | thread/channel spike first; evidence required for runtime | OPEN |
| R-18 | Existing compare/pretty/Python materialization remains large | Medium | Explicit limits/server deferral; track separate improvements | OPEN |
| R-19 | VCD mutates after stream chunks consumed | High | terminal incomplete/stale contract; immutable-source docs | OPEN |
| R-20 | Benchmark conclusions use stale binary | High | M0 fresh release build and version check | CLOSED at G1 |

# 6. Change-control records

## CR-001 — Replace parser-direct catalog population with consuming conversion

| Field | Record |
|---|---|
| Change ID | `CR-001` |
| Owner/approval | Supervising implementation authority; approved after M1 reviewer finding |
| Affected tasks | `RQ-M1-T03`, G2 memory evidence |
| Old requirement | Populate the compact catalog directly during header parsing, without a complete intermediate header tree |
| Technical constraint | `vcd` 0.7 exposes `Parser::parse_header()` returning a fully materialized `Header`; it provides no declaration callback/streaming header-builder seam |
| Approved replacement | Parse the required `Header`, move `Header.items` out, drop remaining header metadata, and consume owned `ScopeItem`, nested scope, and `Var` nodes while constructing the compact catalog |
| Evidence | Consuming implementation in `src/header.rs`/`src/catalog.rs`; compatibility tests; durable isolated RSS measurements in `docs/benchmarks/artifacts/m1/catalog-memory-A.tsv` |
| Peak-memory implication | The complete parser header necessarily exists initially, but processed tree nodes are not retained through a borrowed second representation; actual compact-only peak RSS, not an ownership estimate alone, remains the G2 evidence |
| Compatibility impact | None; public `Signal`, `SignalIndex`, and path APIs retain their accepted M0 behavior |
| Rollback | Revert the compact parser/catalog modules and restore the M0 header implementation |
| Status | `APPROVED` |

## CR-002 — Define G2 portability as all-target source/type compatibility

| Field | Record |
|---|---|
| Change ID | `CR-002` |
| Owner/approval | Supervising implementation authority; approved after T08 portability review |
| Affected tasks | `RQ-M1-T08`, Gate G2, Gate G8 |
| Old requirement | “All current targets compile,” which could be read as requiring native linking/archive builds for every release platform at G2 |
| Constraint | The development host can install Rust standard libraries and run `cargo check`, but cannot natively link MSVC/macOS binaries or reproduce the release runners/cross toolchains |
| Approved replacement | G2 requires `cargo check --all-targets` for the host and every installed release target from the clean candidate commit. Native/cross release linking, archive construction, wheel checks, and platform runtime semantics remain mandatory at G8 in the existing release CI matrix |
| Evidence | `docs/benchmarks/artifacts/m1/target-checks.md` plus final T08 clean-commit checks |
| Compatibility impact | None; this changes gate timing/evidence, not supported targets or production behavior |
| Rollback | Restore native link requirement to G2 and keep M2 blocked until release-runner evidence exists |
| Status | `APPROVED` |

Future changes must append another record using this template:

```text
Change ID: CR-xxx
Date/owner:
Affected tasks/gates/docs:
Old decision/target:
New decision/target:
Evidence:
Compatibility impact:
Rollback impact:
Approval:
```
