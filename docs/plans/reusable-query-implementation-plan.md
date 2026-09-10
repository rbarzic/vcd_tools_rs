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

Milestone status: `IN_PROGRESS`  
Entry: Gate G0 passed  
Exit: Gate G1

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M0-T01 | DONE | — | Approve/amend D-001 through D-010 in `PLAN.md` |
| RQ-M0-T02 | READY | T01 | Inventory public Rust, CLI, Python, and `vcd2trace` contracts |
| RQ-M0-T03 | BLOCKED | T02 | Add focused semantic fixtures |
| RQ-M0-T04 | BLOCKED | T03 | Add characterization and golden compatibility tests |
| RQ-M0-T05 | READY | T02 | Build benchmark harness and scripts |
| RQ-M0-T06 | BLOCKED | T05 | Capture fresh baseline from current checkout |
| RQ-M0-T07 | BLOCKED | T04,T06 | Approve G1 semantic/performance baseline |

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

Pass only when:

- current code passes characterization tests;
- baseline is from the current release build;
- all intended semantic changes are removed, separately approved, or versioned;
- performance budgets are accepted.

---

# M1 — Compact catalog and `OpenedVcd`

Milestone status: `BLOCKED` on G1  
Exit: Gate G2

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M1-T01 | BLOCKED | G1 | Define `FileIdentity`, `GenerationId`, `OpenOptions` |
| RQ-M1-T02 | BLOCKED | G1 | Implement compact scope/name/signal catalog |
| RQ-M1-T03 | BLOCKED | T02 | Refactor header parsing to populate catalog directly |
| RQ-M1-T04 | BLOCKED | T01,T03 | Implement `OpenedVcd::open` and accessors |
| RQ-M1-T05 | BLOCKED | T04 | Implement independent generation-validated readers |
| RQ-M1-T06 | BLOCKED | T04 | Add borrowed `SignalRef` and owned compatibility conversion |
| RQ-M1-T07 | BLOCKED | T04,T05 | Add lazy metadata single-flight state |
| RQ-M1-T08 | BLOCKED | T02-T07 | Concurrency, memory, lookup, and compatibility evidence |

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

## RQ-M1-T03 — Header parser refactor

Expected files: `src/header.rs`, reduced `src/lib.rs`.

Acceptance:

- sanitizer behavior remains characterized;
- parser produces body offset, timescale, and compact catalog in one header pass;
- no body scan;
- fixtures and large-header integration tests pass.

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
- cancellation/panic/failure cannot strand `Building`;
- waiters are awakened deterministically;
- exact old metadata semantics preserved.

## Gate G2 — Core representation and snapshot

Pass when:

- characterization suite remains green;
- catalog lookup is no slower materially than current lookup;
- provisional 40% memory-reduction target passes or is explicitly revised;
- independent-reader/snapshot tests pass;
- `OpenedVcd` has no shared parser mutex;
- all current targets compile.

---

# M2 — Transport-neutral query engine and compatibility wrappers

Milestone status: `BLOCKED` on G2  
Exit: Gate G3

| ID | Status | Depends on | Deliverable |
|---|---|---|---|
| RQ-M2-T01 | BLOCKED | G2 | Query context, limits, cancellation, internal error model |
| RQ-M2-T02 | BLOCKED | T01 | Streaming backend with target check before conversion |
| RQ-M2-T03 | BLOCKED | T02 | Reusable extraction iterator |
| RQ-M2-T04 | BLOCKED | T03 | Reusable find and toggle |
| RQ-M2-T05 | BLOCKED | T03 | Timeline-provider interface and comparison adaptation |
| RQ-M2-T06 | BLOCKED | T03-T05 | Path-based compatibility wrappers |
| RQ-M2-T07 | BLOCKED | T06 | CLI uses one `OpenedVcd` per input |
| RQ-M2-T08 | BLOCKED | T06 | Python module functions use temporary `OpenedVcd` |
| RQ-M2-T09 | BLOCKED | T06 | Evaluate `vcd2trace` migration |
| RQ-M2-T10 | BLOCKED | T01-T09 | Differential backend/compatibility evidence |

## RQ-M2-T01 — Query context

Acceptance:

- transport-neutral cancellation token and finite/unlimited limits;
- deadline and command/result budgets;
- checks at defined command/timestamp/allocation/emission/wait points;
- old wrappers can select legacy-unlimited behavior;
- typed errors map cleanly to legacy strings and later protocol codes.

## RQ-M2-T02 — Optimized stream decoder

Refactor command handling to inspect command ID before `ChangeValue` conversion.

Acceptance:

- unselected vectors/strings are not formatted or allocated;
- selected normalization unchanged;
- cancellation frequency bounded;
- benchmark covers 1/10/100 targets among dense unrelated changes.

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
| R-20 | Benchmark conclusions use stale binary | High | M0 fresh release build and version check | OPEN |

# 6. Change-control records

When a gate or target changes, append:

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
