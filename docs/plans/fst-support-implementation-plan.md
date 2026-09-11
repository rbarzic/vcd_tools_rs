# FST Support Implementation Plan

Plan ID: **FST-001**  
Status: **PROPOSED**  
Dashboard: [`../../FST_PLAN.md`](../../FST_PLAN.md)  
Architecture: [`../design/fst-support-architecture.md`](../design/fst-support-architecture.md)  
Semantic contract: [`../design/fst-semantic-contract.md`](../design/fst-semantic-contract.md)

## 1. Tracking rules

- Task IDs are stable and should appear in commits/PRs.
- A task is `DONE` only when acceptance commands/evidence are linked.
- Gate failures stop dependent implementation.
- Existing VCD behavior is the regression oracle.
- External performance datasets never become mandatory ordinary-CI inputs.
- Parser panic/resource findings are correctness issues, not optional polish.

Status values: `PROPOSED`, `BLOCKED`, `READY`, `IN_PROGRESS`, `IN_REVIEW`, `DONE`, `DEFERRED`, `REJECTED`.

Evidence template:

```text
Task:
Status:
Commit/PR:
Changed files:
Validation commands/results:
Fixture hashes:
Performance/resource delta:
Decisions:
Residual risks:
```

## 2. Goals

1. Read normal FST files with pure-Rust `fst-reader` 0.17.
2. Keep every accepted VCD Rust API and semantic unchanged.
3. Add content detection and an additive `OpenedWaveform` facade.
4. Support FST list, metadata, extract, find, toggle, compare, Python, and server queries.
5. Preserve aliases, requested duplicates, values, windows, and generation validation.
6. Use FST native compressed-section filtering; do not build a VCD sidecar for FST.
7. Keep normal open/query memory bounded and never load the full time table.
8. Prevent malformed FST input from terminating the process.
9. Build and test all current native and wheel targets.
10. Record dependency licensing and reproducible fixtures/benchmarks.

## 3. Non-goals

- FST writing or VCD/FST conversion.
- Preserving original compressed/lexical encoding.
- Transparent gzip/archive/container support.
- Incomplete FST plus implicit `.hier` discovery in the first release.
- stdin, URL, or object-store input.
- Sidecar or selective-cache implementation changes.
- TCP/HTTP/gRPC or Windows named pipes.
- Server-side compare or arbitrary client paths.
- Package/command renaming.
- `vcd2trace` FST support.
- Every producer-specific FST attribute/extension.

## 4. Dependency decision candidate

```toml
fst-reader = "0.17"
```

Initial tested version: 0.17.0.

- BSD-3-Clause, pure Rust.
- Dependencies: `lz4_flex`, `miniz_oxide`, `num_enum`, `thiserror`.
- Source checks passed Linux x64/ARM64, Windows x64, macOS Intel/ARM64.
- Distribution must retain the BSD notice/disclaimer.
- Upgrade only after the tiny compatibility corpus passes.

`fst-sys`/GTKWave C bindings are rejected unless a future change record demonstrates a requirement not met by `fst-reader`.

---

# F0 — Contract, dependency, and fixtures

Milestone status: `IN_PROGRESS`
Exit: FST-G0

| ID | Status | Depends | Deliverable |
|---|---|---|---|
| FST-F0-T01 | IN_PROGRESS | — | Add/pin `fst-reader` and record license/dependency evidence |
| FST-F0-T02 | IN_PROGRESS | — | Add deterministic tiny FST fixture generator and provenance |
| FST-F0-T03 | BLOCKED | T02 | Characterize ordering/window/alias/value semantics |
| FST-F0-T04 | BLOCKED | T02 | Corrupt/truncated/deep/oversized/gzip fixture probes |
| FST-F0-T05 | BLOCKED | T02 | Packed-real, real change, generic string, extended state spike |
| FST-F0-T06 | READY | — | Approve semantic decisions in `fst-semantic-contract.md` |
| FST-F0-T07 | BLOCKED | T01-T06 | Gate report |

## FST-F0-T01 — Dependency and license

Acceptance:

- `fst-reader` exact resolved version recorded;
- BSD-3-Clause text included in third-party notices/release artifacts;
- `cargo tree` and SBOM reviewed;
- five-target `cargo check --all-targets` passes after dependency addition;
- no native compiler/runtime dependency introduced.

## FST-F0-T02 — Tiny deterministic fixtures

Required committed fixtures:

- valid FST with nested scopes and aliases;
- scalar 0/1/x/z and extended digital states;
- 32-bit known vector and 129-bit/wide/XZ vector;
- real changes including exact bit patterns;
- generic string values including invalid UTF-8 probe where writer permits;
- repeated timestamp and multiple handles at one timestamp;
- empty/no-event FST;
- equivalent VCD/FST pair;
- deliberately changed equivalent pair;
- malformed/truncated/corrupt cases.

Each binary fixture requires:

- generator source and pinned `fst-writer` dev version or another deterministic tool;
- producer/version/license;
- SHA-256 manifest;
- regeneration command;
- expected semantic JSON/text oracle.

## FST-F0-T03 — Reader semantics

Prove:

- adapter-side suppression of callbacks before start;
- inclusive end behavior;
- callback early termination;
- deterministic same-time order across repeated runs;
- alias handle grouping and request-order expansion;
- no-target fast return;
- callback errors distinguish early success, cancellation, limits, and parser errors.

## FST-F0-T04/T05 — Robustness subset

Inventory every reachable `assert!`, `todo!`, panic, and unchecked allocation in the selected version. Decide supported/rejected constructs. Confirm catch-unwind behavior and whether upstream patching is needed.

## Gate FST-G0 — Parser and semantic subset

Pass only when:

- twelve dashboard decisions are approved;
- dependency/license is accepted;
- deterministic tiny fixtures exist;
- callback order/window/value behavior is executable;
- gzip/incomplete/panic policies are explicit;
- no production FST backend has been added prematurely.

---

# F1 — Format-neutral types and detection

Milestone status: `IN_PROGRESS` on FST-G0
Exit: FST-G1

| ID | Status | Depends | Deliverable |
|---|---|---|---|
| FST-F1-T01 | IN_PROGRESS | G0 | `WaveformFormat`, hint, timescale, metadata, signal ID/kind |
| FST-F1-T02 | IN_PROGRESS | T01 | Extensible `WaveformError` and query mapping |
| FST-F1-T03 | IN_PROGRESS | T01,T02 | Content detection with explicit override |
| FST-F1-T04 | BLOCKED | T01-T03 | `OpenedWaveform` additive enum facade |
| FST-F1-T05 | BLOCKED | T04 | Common catalog/metadata/validation accessors |
| FST-F1-T06 | BLOCKED | T01-T05 | VCD regression and public API review |

Acceptance:

- no public VCD type/function signature changes;
- no third-party FST types in public API;
- auto detection works for correct, mislabeled, and extensionless fixtures;
- explicit mismatch fails deterministically before output;
- empty/ambiguous/truncated detection is bounded;
- VCD characterization, CLI, Python, protocol, and server tests remain green.

## Gate FST-G1 — Additive facade

Pass when the facade is useful without changing VCD behavior and detection is tied to one validated generation.

---

# F2 — `OpenedFst`, catalog, and identity

Milestone status: `BLOCKED` on FST-G1  
Exit: FST-G2

| ID | Status | Depends | Deliverable |
|---|---|---|---|
| FST-F2-T01 | BLOCKED | G1 | `OpenedFst` header/open path without time table |
| FST-F2-T02 | BLOCKED | T01 | Compact hierarchy/scope/declaration catalog |
| FST-F2-T03 | BLOCKED | T02 | Handle aliases, neutral IDs, exact names/types/directions |
| FST-F2-T04 | BLOCKED | T01 | Common fingerprint extraction and `FstFileIdentity` |
| FST-F2-T05 | BLOCKED | T01-T04 | Independent reader and completion validation |
| FST-F2-T06 | BLOCKED | T01-T05 | Metadata/catalog diagnostics and supplied-file benchmark |
| FST-F2-T07 | BLOCKED | T01-T06 | Open-time server resource limits |

Server open limits must cover:

- declarations and unique handles;
- hierarchy depth and scope count;
- individual/total name bytes;
- catalog owned bytes;
- section/index counts and metadata-derived allocation;
- gzip wrapper rejection;
- elapsed/deadline where observable.

Acceptance targets for supplied FST:

- open/header under 50 ms and under 32 MiB RSS before hierarchy retention;
- catalog under 500 ms;
- hierarchy names/counts/hash match recorded evidence;
- metadata available without body scan/time table;
- repeated independent opens/readers agree;
- replacement/truncate/mutation is rejected before and after query;
- actual catalog peak RSS recorded and accepted.

## Gate FST-G2 — Open/catalog/identity

Pass when catalog and metadata are exact, generation behavior matches VCD guarantees, and normal open cannot allocate the full time table.

---

# F3 — Visitor and query parity

Milestone status: `BLOCKED` on FST-G2  
Exit: FST-G3

| ID | Status | Depends | Deliverable |
|---|---|---|---|
| FST-F3-T01 | BLOCKED | G2 | Backend-neutral visitor and VCD adapter |
| FST-F3-T02 | BLOCKED | T01 | FST filtered callback visitor |
| FST-F3-T03 | BLOCKED | T02 | Value conversion and pre-window suppression |
| FST-F3-T04 | BLOCKED | T02,T03 | Cancellation, deadline, row/byte/work limits |
| FST-F3-T05 | BLOCKED | T01-T04 | Generic extract/find/toggle |
| FST-F3-T06 | BLOCKED | T05 | FST timeline provider and comparison |
| FST-F3-T07 | BLOCKED | T06 | Four-format comparison matrix and timescale rejection |
| FST-F3-T08 | BLOCKED | T01-T07 | Panic boundary, malformed and generation-race tests |
| FST-F3-T09 | BLOCKED | T01-T08 | Performance/query evidence on supplied file |

Required semantic tests:

- exact list/catalog and metadata;
- aliases and duplicate requests;
- inclusive boundaries and pre-window suppression;
- repeated/same-time ordering;
- scalar, vectors, X/Z/extended states, real bits, strings;
- find found/absent/occurrence zero/early exit;
- toggle first-event baseline;
- cancellation and every limit N/N+1;
- callback parser errors versus application stop;
- source replacement during callback/read/publication;
- VCD/VCD, FST/FST, VCD/FST, FST/VCD comparison.

Performance evidence on supplied FST:

- one, 10, and 100 selected handles;
- early/middle/late narrow windows;
- full-range clock/high-frequency signals;
- first and repeated open;
- callback cancellation responsiveness;
- peak RSS and output/result hash.

## Gate FST-G3 — Query parity

Pass when accepted semantic results match the equivalent VCD where applicable, format-specific ordering is deterministic, queries are bounded, and malformed FST cannot terminate the process.

---

# F4 — CLI, Python, and server

Milestone status: `BLOCKED` on FST-G3  
Exit: FST-G4

| ID | Status | Depends | Deliverable |
|---|---|---|---|
| FST-F4-T01 | BLOCKED | G3 | CLI handlers use `OpenedWaveform` and neutral help |
| FST-F4-T02 | BLOCKED | T01 | `--format` and compare override flags |
| FST-F4-T03 | BLOCKED | G3 | Existing Python functions autodetect FST |
| FST-F4-T04 | BLOCKED | T03 | Add reusable Python `Waveform` for explicit format |
| FST-F4-T05 | BLOCKED | G3 | Server generation slot/service use `OpenedWaveform` |
| FST-F4-T06 | BLOCKED | T05 | Add `describe.source_format` and supported formats |
| FST-F4-T07 | BLOCKED | T01-T06 | Native and Unix wheel FST E2E tests |
| FST-F4-T08 | BLOCKED | T01-T07 | README, CLI, Python, protocol docs |

Acceptance:

- existing six Python query signatures unchanged;
- VCD CLI/Python/protocol golden tests unchanged;
- native and pip Unix server start on FST and answer all supported methods;
- Windows wheels build/query FST but do not expose Unix serve;
- auto detection and explicit override agree;
- server replacement can switch VCD↔FST between requests without mixing generations;
- `describe` format fields are additive and golden-tested.

## Gate FST-G4 — Surface parity

Pass when every supported user surface handles FST and VCD regression output is byte-identical.

---

# F5 — Hardening and release

Milestone status: `BLOCKED` on FST-G4  
Exit: FST-G5

| ID | Status | Depends | Deliverable |
|---|---|---|---|
| FST-F5-T01 | BLOCKED | G4 | Fuzz detection/open/hierarchy/read/query |
| FST-F5-T02 | BLOCKED | G4 | Corrupt/deep/oversized/compression resource suite |
| FST-F5-T03 | BLOCKED | G4 | Repeated supplied-file benchmark and comparison report |
| FST-F5-T04 | BLOCKED | G4 | Five-target native archive install/smoke |
| FST-F5-T05 | BLOCKED | G4 | Five-target wheel install/query smoke |
| FST-F5-T06 | BLOCKED | T01-T05 | BSD notice, SBOM, dependency/license/vulnerability audit |
| FST-F5-T07 | BLOCKED | T01-T06 | Coordinated pre-publication qualification workflow |
| FST-F5-T08 | BLOCKED | T07 | Version, release notes, tag, publish |

Release qualification must happen before tag publication from the same commit/version. GitHub release and PyPI workflows must not independently publish an unqualified commit.

## Gate FST-G5 — Release

Pass only when:

- fuzz/corrupt/resource tests have no unresolved panic, hang, or excessive allocation;
- all four comparison combinations pass;
- supplied-file evidence is reproducible;
- every claimed archive/wheel is installed and tested;
- license notices/SBOM are complete;
- VCD tests and artifacts remain unchanged;
- FST limitations are explicit.

---

# Validation matrix

## Fast per-change

```sh
cargo fmt --all -- --check
cargo check --all-targets
cargo test --test fst_semantics -- --test-threads=1
cargo test --test query_semantics --test reusable_queries -- --test-threads=1
```

## Full core

```sh
cargo test --all-targets -- --test-threads=1
cargo test --release --all-targets -- --test-threads=1
cargo check --all-targets --target x86_64-pc-windows-msvc
cargo check --all-targets --target aarch64-unknown-linux-gnu
cargo check --all-targets --target x86_64-apple-darwin
cargo check --all-targets --target aarch64-apple-darwin
```

## Python and server

```sh
scripts/test-python-extension.sh
cargo test --test protocol_v1 --test server_runtime --test server_e2e -- --test-threads=1
```

Add FST-specific native/wheel installed-artifact smoke scripts before release.

## Fuzz

```sh
cargo fuzz run waveform_detection
cargo fuzz run fst_open
cargo fuzz run fst_hierarchy
cargo fuzz run fst_query
```

Record seed, duration, execution count, sanitizer, and corpus hash.

---

# Risk register

| ID | Risk | Severity | Mitigation/gate |
|---|---|---|---|
| FST-R01 | Parser panic/todo on uncommon data | Critical | catch boundary, fixtures, upstream patch, F0/F3/F5 |
| FST-R02 | Metadata-derived allocation/compression bomb | Critical | open limits, gzip reject, corrupt/fuzz suite |
| FST-R03 | Pre-start callback violates windows/toggles | Critical | explicit suppression and boundary tests |
| FST-R04 | Same-time ordering differs from VCD | High | format-specific contract, deterministic backend order |
| FST-R05 | Alias/duplicate multiplicity lost | High | handle binding catalog and request-order tests |
| FST-R06 | Generic string bytes corrupt via lossy UTF-8 | High | reject invalid UTF-8 initially |
| FST-R07 | FST work limit misrepresented as commands | Medium | document work units; protocol v2 rename later |
| FST-R08 | Mixed timescale comparison is wrong | Critical | require equal physical tick; no implicit scaling |
| FST-R09 | FST facade breaks public VCD Rust APIs | Critical | additive types only; G1 public API review |
| FST-R10 | Gzip wrapper loads whole file into RAM | Critical | reject/default policy and header probe |
| FST-R11 | Complete time-table accidentally loaded | Critical | forbid normal use; RSS regression test |
| FST-R12 | Binary fixtures lack provenance/license | High | manifest, generator, SHA-256, notices |
| FST-R13 | Publication happens before qualification | High | coordinated pre-tag workflow |
| FST-R14 | Dependency license/build changes later | Medium | pin line, notices/SBOM, upgrade corpus |
| FST-R15 | Cancellation delayed by block decompression | Medium | callback checks, measured/documented upper behavior |

# Completion definition

FST support is complete when:

1. `OpenedVcd` and all VCD-specific APIs remain compatible.
2. `OpenedFst` and `OpenedWaveform` are public, documented, and generation-safe.
3. CLI/Python/server support FST through content detection.
4. List/metadata/extract/find/toggle and required compare combinations pass semantic fixtures.
5. Malformed FST cannot terminate the process.
6. Normal operation never loads the complete time table.
7. Resource limits are enforced during open and queries.
8. Supplied-file benchmarks and hashes are published.
9. Native archives and wheels are installed/smoke-tested on all claimed targets.
10. Licensing/SBOM/notices and release documentation are complete.
