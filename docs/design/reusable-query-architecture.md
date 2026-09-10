# Reusable Query Architecture

Status: **DRAFT**  
Plan: **VCD-RQ-001**  
Tracker: [`../../PLAN.md`](../../PLAN.md)

## 1. Purpose

This document defines the target architecture for reusing VCD parsing work across repeated queries. It is transport-neutral: the existing CLI, Python bindings, and proposed Unix-socket server must use the same query engine.

The core principle is:

> A long-lived process is useful only when it retains validated, query-relevant state. An open socket or open file descriptor does not avoid parsing the VCD body.

## 2. Existing behavior and constraints

The current implementation in `src/lib.rs`:

- parses and sanitizes the header for each public path-based query;
- constructs `Vec<Signal>` plus `SignalIndex::by_name` and `SignalIndex::by_id` with cloned signal data;
- starts each body query at `body_offset`;
- applies `TimeWindow.start` as a filter rather than as a seek target;
- converts value-change commands before checking whether their ID is requested;
- owns a forward-only `vcd::Parser` in each `TimeValueIter`;
- materializes all extraction rows in the Rust/Python convenience API;
- mixes query orchestration and output rendering in the CLI.

Consequences measured on the example files:

- header/list work costs about 0.45 seconds and approximately 415 MiB process peak RSS;
- full scans cost about 8.4 seconds for 0.96 GiB and 35.4 seconds for 4.27 GiB;
- batching three signals into one scan is about 2.97 times faster than three scans.

## 3. Architectural boundaries

```text
CLI / Python / Unix socket
          │
          ▼
Transport-neutral request and result types
          │
          ▼
OpenedVcd + QueryEngine
  ├── Compact SignalCatalog
  ├── Generation/FileIdentity
  ├── StreamingBackend (required)
  ├── SparseSidecarBackend (optional)
  └── SelectiveTimelineCache (optional)
          │
          ▼
VCD parser and independent source readers
```

Rules:

1. Transport code must not parse VCD commands directly.
2. Query methods must not print or serialize output.
3. The engine must work with only the streaming backend.
4. Optional indexes may accelerate a query but may not change its result.
5. Compatibility wrappers remain explicit; there is no hidden global cache.
6. No request may share a forward-only parser with another request.

## 4. Semantic compatibility contract

The characterization suite must lock down these behaviors before refactoring:

1. `start` and `end` are inclusive.
2. Extraction emits changes inside the window only; it does not synthesize the value active at `start`.
3. Toggle counting uses the first in-window event as its baseline and does not count a transition from before `start`.
4. Comparison begins selected timelines at textual `x` at the window boundary.
5. Every change at the same timestamp is retained in body order.
6. Fully known binary vectors of at most 128 bits become `ChangeValue::Integer`.
7. Wider vectors and vectors containing X/Z remain text.
8. Scalars remain text, including `"0"`, `"1"`, `"x"`, and `"z"`.
9. Real comparison retains existing exact behavior, including bit-level equality where currently used.
10. One `IdCode` may have multiple signal aliases.
11. A selected alias receives an event for each body change to the shared ID.
12. Duplicate requested aliases/names preserve current multiplicity and ordering unless a separately approved API change rejects them.
13. Duplicate declared full names remain an error.
14. Missing signal errors remain deterministic.
15. A body with no timestamp/change yields metadata bounds `(0, 0)`.
16. Existing CLI TSV/table layouts and Python dictionary keys remain compatible.
17. Parse errors after partial iteration remain observable; server streams mark incomplete output.

A cached per-ID representation must retain a global body sequence number or equivalent stable ordering key. Merging timelines solely by timestamp is insufficient because multiple changes can occur at the same timestamp.

## 5. Core types

The exact API remains subject to normal Rust design review, but implementation must preserve the responsibilities below.

```rust
#[derive(Clone)]
pub struct OpenedVcd {
    inner: Arc<OpenedVcdInner>,
}

struct OpenedVcdInner {
    path: PathBuf,
    generation: GenerationId,
    identity: FileIdentity,
    body_offset: u64,
    timescale: Option<Timescale>,
    catalog: SignalCatalog,
    metadata: SingleFlight<VcdMeta>,
    sidecar: SidecarManager,
    cache: SelectiveTimelineCache,
}

#[non_exhaustive]
pub struct OpenOptions {
    fingerprint_policy: FingerprintPolicy,
    // Sidecar/cache controls are added through backward-compatible builders
    // only after their independent gates pass.
}

impl OpenOptions {
    pub fn new() -> Self;
    pub fn with_fingerprint_policy(self, policy: FingerprintPolicy) -> Self;
    pub fn fingerprint_policy(&self) -> FingerprintPolicy;
}
```

Required properties:

- `OpenedVcd` is cheap to clone and is `Send + Sync`.
- `open()` parses only the header and validates source identity; it does not scan the body.
- Metadata is lazy or supplied by an explicit index build.
- The object never stores one mutable body parser.
- Read-only catalog and ready index structures are shared using `Arc` or immutable ownership.
- Building states use single-flight coordination and cancellation-safe cleanup.

### 5.1 Independent snapshot readers

Do not assume `File::try_clone()` provides an independent seek cursor; duplicated handles may share file position.

The initial safe strategy is:

1. open the configured path for each query;
2. obtain identity from the opened handle, not only from the path;
3. verify that handle identity matches the `OpenedVcd` generation;
4. seek the independent handle to the required offset;
5. parse using that handle;
6. validate source identity again at query completion according to policy.

The configured path is converted to an absolute path at open time so later working-directory changes cannot redirect it, but its final symlink is deliberately not canonicalized. Every admission and completion reopen uses that absolute configured path; a retargeted symlink therefore fails generation validation. A canonical target may be retained for diagnostics only and must never become the reopen path.

Opening then inspecting the handle avoids a path replacement race: if replacement happened before open, identity differs; if it happens after open, the query retains the already opened object. Completion validates both the admitted handle and a fresh configured-path reopen. A later optimization may use positioned-I/O adapters, but it requires cross-platform tests.

## 6. Compact signal catalog

The existing public `Signal` and `SignalIndex` types remain compatible. The new engine uses an internal representation that avoids cloning complete signals into every lookup structure.

```rust
type SignalKey = u32;
type ScopeKey = u32;

struct SignalCatalog {
    signals: Box<[SignalMeta]>,        // declaration order
    by_name: HashMap<Arc<str>, SignalKey>,
    aliases_by_id: HashMap<IdCode, Box<[SignalKey]>>,
    scopes: Box<[ScopeNode]>,
}

struct SignalMeta {
    id_code: IdCode,
    full_name: Arc<str>,
    size: u32,
    var_type: VarType,
    scope: Option<ScopeKey>,
}

struct ScopeNode {
    component: Arc<str>,
    parent: Option<ScopeKey>,
}
```

Requirements:

- full names are stored once and shared with lookup keys;
- alias maps contain numeric keys, not cloned `Signal` values;
- scope hierarchy is interned/parent-linked rather than cloned into each signal;
- signal width is read directly from metadata; queries do not construct `HashMap<String, u32>`;
- declaration order is retained;
- conversion to `SignalKey` is checked;
- borrowed `SignalRef` supports the reusable API;
- existing owned `Signal` values are materialized only for compatibility calls;
- use `Vec` initially for aliases; add `smallvec` only if measurements justify it.

Target: at least 40% reduction in incremental peak RSS attributable to header/catalog construction, unless Gate G2 records a revised measured target.

## 7. Query context and limits

All reusable methods accept an internal or public context equivalent to:

```rust
pub struct QueryContext {
    pub cancellation: CancellationToken,
    pub limits: QueryLimits,
}

pub struct QueryLimits {
    pub max_signals: usize,
    pub max_rows: Option<u64>,
    pub max_result_bytes: Option<u64>,
    pub max_commands: Option<u64>,
    pub deadline: Option<Instant>,
}
```

Compatibility wrappers use an uncancelled context with legacy-unbounded result behavior. The server always supplies finite limits.

Cancellation/deadline checks occur:

- before reader creation and generation validation;
- at timestamps;
- at least every configurable number of commands, initially 4,096;
- before selected-value conversion/allocation;
- before result/chunk delivery;
- while waiting for metadata, cache, sidecar, scheduler, or output capacity.

## 8. Query backends

### 8.1 Streaming backend — required

The baseline backend opens an independent reader, seeks to `body_offset`, and scans forward. It must first inspect the command ID and convert a scalar/vector/real/string value only when that ID is selected.

This backend is always available and is the correctness oracle for optional backends.

### 8.2 Sparse sidecar backend — optional

A sidecar maps selected timestamp commands to parser-safe byte offsets. For a request with `start`, it seeks to the greatest checkpoint not later than `start`, parses the timestamp command, and continues normally.

It does not store prior signal state. This is compatible only because current extraction/toggle/find/windowed-compare semantics do not require synthesizing the pre-window value. Details are in [`sidecar-format-v1.md`](sidecar-format-v1.md).

### 8.3 Selective timeline cache — optional

Cache keys are `IdCode`, not names, so aliases share one timeline.

```rust
struct CompactEvent {
    time: u64,
    sequence: u64,
    value: CompactValue,
}

enum CompactValue {
    Integer(u128),
    FloatBits(u64),
    Text(Arc<str>),
}
```

Requirements:

- stable `sequence` preserves same-time body ordering across IDs;
- names are not stored in events;
- cache hits use binary search by time and a stable k-way merge by `(time, sequence)`;
- cache ownership is byte-accounted and bounded;
- active readers retain timelines through `Arc` after eviction;
- per-ID loading is single-flight;
- overlapping requested IDs are batched into one scan where practical;
- build concurrency is bounded;
- failed, cancelled, or panicking loaders clear reservations and wake waiters;
- a timeline larger than the complete cache budget is usable by the initiating request but is not admitted;
- no eager all-signal build occurs by default.

Cache modes:

- `Off`
- `Lazy` — promote after a measured/configured request threshold
- `EagerSelected` — warm only explicitly named IDs

The default is selected at Gate G7. Until then, `Off` is the safe release default.

## 9. Metadata

`OpenedVcd::open` must not compute body bounds. Metadata state is:

```text
Absent -> Building -> Ready(Arc<VcdMeta>)
                  \-> Failed(cached error; explicit retry)
```

A full streaming metadata scan and sidecar construction both compute exact start/end bounds. If either produces metadata for the same generation, it may satisfy the shared state. `OpenedVcd::metadata` returns an `Arc<VcdMeta>` so clones and concurrent callers share the immutable ready result.

Only one metadata/index builder runs per generation. A caller that observes `Building` registers against that attempt before sleeping. Success is immutable and remains reusable. Failure or panic publishes one terminal error plus the number of registered waiters; an explicit retry cannot replace that outcome until those waiters have acknowledged it. This bounded acknowledged-waiter design retains at most one terminal attempt and prevents a racing retry from making a prior waiter observe a newer attempt. A cancellation-safe waiter-registration guard is added with the M2 `QueryContext`; M1 deliberately does not introduce a metadata-specific cancellation API. The M1 builder guard converts panic into the attempt's shared failure and wakes all waiters.

## 10. File identity and generation semantics

`FileIdentity` contains, where available:

- file length;
- nanosecond-resolution modification time;
- Unix device and inode, or Windows volume serial number and file index;
- parsed body offset;
- BLAKE3 of the complete header;
- a default bounded fingerprint of the first and last 64 KiB;
- optional strict full-file BLAKE3, which is explicitly opt-in because it scans the body.

Identity fields are opaque/read-only and constructors remain crate-private. Header bytes, body offset, metadata, bounded samples, and optional strict hash must all derive from the same opened handle. Every later admission and completion validation recomputes the complete raw byte region `0..body_offset` and compares it with the stored header fingerprint under both bounded and strict policies; beginning/end samples are not a substitute for complete-header validation.

Rules:

1. Catalog, metadata, sidecar, and timelines belong to exactly one generation.
2. A server request is admitted only against a validated active generation.
3. Atomic replacement between requests creates a new generation after successful open.
4. In-flight requests retain the old reader/object and cannot silently switch generations.
5. Append, truncate, and in-place mutation are unsupported during a query and produce a stale/incomplete result when detected.
6. Unary results are discarded if completion validation reports stale input.
7. Streamed results terminate with `complete:false` and `STALE_SOURCE`; clients must discard prior chunks.
8. Nothing cached transfers across generations unless complete identity matches.
9. Initial v1 does not incrementally follow a growing VCD.

## 11. Compatibility strategy

Existing functions remain and become wrappers around a temporary `OpenedVcd`:

```text
path function
  -> OpenedVcd::open(path)
  -> corresponding reusable method
  -> collect/materialize only where legacy return type requires it
```

Compatibility requirements:

- no signature change to current public path functions in v1;
- public `Signal` and `SignalIndex` remain available;
- error wording remains stable where tests or clients depend on it;
- CLI stdout, stderr, and exit status receive golden tests;
- Python module-level signatures and dictionary shapes remain stable;
- no hidden global opened-file cache;
- `vcd2trace` migration is optional and must produce byte-identical output.

## 12. Python API

Add a persistent class after the Rust engine stabilizes:

```python
with vcd_tools.Vcd("simulation.vcd") as vcd:
    vcd.list_signals(filter="clk")
    vcd.metadata()
    vcd.toggles(["tb.clk"])
    for row in vcd.iter_extract(["tb.clk"], start=1000):
        ...
```

- Existing module functions create temporary `OpenedVcd` objects.
- `Vcd.extract` preserves list materialization.
- `Vcd.iter_extract` provides bounded-memory iteration.
- Long scans and waits release the GIL.
- The iterator owns the Rust generation through `Arc`.
- Operations after `close()` fail deterministically.
- Python 3.8 ABI3 support remains.

## 13. Concurrency and scheduling

- Independent streaming readers may run concurrently, but the server limits expensive scans.
- Initial default: one active full-body scan per VCD generation; configurable only after throughput benchmarks.
- Indexed/cache-hit reads may execute concurrently.
- Requests known before a scan starts may be coalesced by selected ID.
- Requests arriving after the scanner passed earlier body data cannot join that scan.
- No catalog/cache/global scheduler lock is held while parsing or delivering output.
- All queues are bounded.

## 14. Error model

Add structured internal errors while preserving compatibility display strings. Required categories include:

- invalid window/occurrence/request;
- missing signal(s);
- duplicate declaration;
- parse error;
- I/O error;
- cancelled/deadline exceeded;
- query/result limit exceeded;
- stale source/source unavailable;
- corrupt/unsupported sidecar;
- queue full;
- unsupported platform/version;
- internal invariant failure.

Transport maps these to stable protocol codes; Rust callers retain typed errors.

## 15. Non-goals

- remote networking, HTTP, TCP, gRPC, or authentication;
- Windows named pipes in v1;
- mutable/live VCD tailing;
- eager all-signal event loading by default;
- SQL or a general query language;
- changing extraction/toggle/compare window semantics;
- daemon-only indexes that cannot be reused by the regular CLI;
- forcing `compare` into the first server milestone if bounded two-file behavior is not ready.

## 16. Required proof points

Architecture implementation cannot pass its gates without evidence for:

1. old/new semantic equivalence;
2. independent readers bound to one file generation;
3. substantial catalog memory reduction;
4. no material cold-path regression;
5. offset-resume correctness before sidecar use;
6. stable merge ordering for cached timelines;
7. cancellation-safe single-flight state;
8. bounded server queues/results/resources;
9. Windows builds unaffected by Unix-only transport;
10. deterministic mutation and stale-cache behavior.
