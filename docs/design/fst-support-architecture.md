# FST Support Architecture

Status: **DRAFT**  
Plan: **FST-001**  
Dashboard: [`../../FST_PLAN.md`](../../FST_PLAN.md)

## 1. Design rule

FST support is additive. Do not rename, genericize, or replace these accepted public VCD surfaces:

- `OpenedVcd`
- `Signal`, `SignalIndex`, `SignalRef`
- `VcdMeta`, `VcdError`
- `read_vcd_metadata`, `compare_vcd_files`
- other VCD-named compatibility functions

VCD internals expose `vcd::IdCode`, `vcd::VarType`, a `vcd::Parser`, and VCD ordering assumptions. Mapping FST handles/types into those types would be lossy and would break downstream Rust code.

## 2. Additive facade

```rust
#[non_exhaustive]
pub enum WaveformFormat {
    Vcd,
    Fst,
}

#[non_exhaustive]
pub enum WaveformFormatHint {
    Auto,
    Vcd,
    Fst,
}

#[derive(Clone)]
pub enum OpenedWaveform {
    Vcd(OpenedVcd),
    Fst(OpenedFst),
}
```

Add neutral types without exposing `fst-reader` types:

```rust
pub struct WaveSignalId(u32);

pub enum WaveSignalKind {
    Bits,
    Real,
    String,
}

pub struct WaveTimescale {
    pub exponent10_seconds: i8,
}

pub struct WaveformMeta {
    pub format: WaveformFormat,
    pub timescale: Option<WaveTimescale>,
    pub signal_count: usize,        // declarations, aliases included
    pub unique_signal_count: usize,
    pub start_time: u64,
    pub end_time: u64,
}
```

`OpenedWaveform` provides common catalog, metadata, validation, and query operations. Existing `OpenedVcd` methods remain available for VCD-specific callers.

## 3. Format detection

Detection is content-based:

1. Open and fingerprint one configured source generation.
2. If an explicit hint is `Fst`, require `fst_reader::is_fst_file` and a successful FST open.
3. If an explicit hint is `Vcd`, require a successful VCD header parse.
4. Under `Auto`, probe FST magic/content first, restore/reopen the handle, then parse as FST when true; otherwise attempt a real VCD header parse.
5. If both fail, return a bounded `UnsupportedWaveform` error containing format categories, not unbounded parser text.
6. Extension affects diagnostics/probe ordering only; mislabeled and extensionless valid files remain usable.

Detection, hierarchy/header parsing, and generation identity must refer to the same source generation. Replacement between requests can change format; the server opens the complete replacement before atomically swapping `Arc<OpenedWaveform>`.

## 4. `OpenedFst`

`OpenedFst` owns:

- display and absolute configured paths;
- FST generation identity/fingerprint;
- `FstHeader`-derived start/end/timescale;
- compact declaration/scope/handle catalog;
- open/resource policy;
- no shared mutable `FstReader`;
- no complete time table.

Each query opens and validates an independent handle, constructs `FstReader<BufReader<File>>`, applies an `FstFilter`, performs callback-based processing, and revalidates the admitted handle plus configured path before publishing success.

Opening the supplied FST is cheap. `open_and_read_time_table` is forbidden in normal paths because it consumed about 738 MiB for 92,797,051 timestamps.

## 5. FST hierarchy catalog

```rust
struct FstSignalCatalog {
    declarations: Box<[FstSignalMeta]>,
    by_name: HashMap<Arc<str>, FstDeclarationKey>,
    aliases_by_handle: HashMap<usize, Box<[FstDeclarationKey]>>,
    scopes: Box<[FstScopeNode]>,
}

struct FstSignalMeta {
    handle_index: usize,
    neutral_id: WaveSignalId,
    full_name: Arc<str>,
    width: u32,
    kind: WaveSignalKind,
    fst_type: InternalFstType,
    direction: InternalDirection,
    is_alias: bool,
    scope: Option<FstScopeKey>,
}
```

Rules:

- preserve declaration order;
- preserve exact scope/variable spelling, including spaces before ranges;
- join scope components with `.` only;
- reject duplicate complete names deterministically;
- store one event chain per unique FST handle;
- expand selected aliases and repeated requested names in request order;
- do not map handles to `vcd::IdCode`;
- validate header declaration/handle counts against hierarchy results.

## 6. Common visitor seam

`fst-reader` is callback-based. Do not force it into `OpenedTimeValueIter` using one producer thread per query.

```rust
pub enum VisitControl {
    Continue,
    Stop,
}

pub struct WaveEvent {
    pub signal: Arc<str>,
    pub time: u64,
    pub width: u32,
    pub value: ChangeValue,
}

pub struct ScanStats {
    pub rows: u64,
    pub logical_bytes: u64,
    pub work_units: u64,
}

impl OpenedWaveform {
    pub fn visit_changes<F>(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
        visitor: F,
    ) -> WaveformQueryResult<ScanStats>
    where
        F: FnMut(WaveEvent) -> WaveformQueryResult<VisitControl>;
}
```

Backend adapters:

- VCD loops over the existing reusable extraction iterator.
- FST calls `read_signals`, converts borrowed values immediately, expands bindings, and maps callback stop/errors.
- Find, toggle, comparison collection, CLI rendering, Python collection, and server chunking use the visitor.
- Existing VCD iterator API remains unchanged.

A short implementation spike must confirm visitor integration with current server output/backpressure before committing this seam.

## 7. FST value mapping

| FST callback/hierarchy | Neutral `ChangeValue` |
|---|---|
| one-bit digital bytes | exact `Text`, preserving case/state |
| width 2–128, only `0`/`1` | `Integer(u128)` |
| vector containing other states | exact `Text` |
| width >128 | exact `Text` |
| real | `Float(f64)`; compare via `to_bits()` |
| generic string, valid UTF-8 | `Text` |
| generic string, invalid UTF-8 | typed unsupported/value error |

FST can contain digital states beyond 0/1/x/z, including h/u/w/l/-/?. Preserve them. The semantic contract decides which are treated as unknown by comparison; no silent normalization is allowed.

`FstSignalValue::String` is borrowed bytes. Convert/copy only selected values that are inside the requested window.

## 8. Window and ordering behavior

`FstFilter` selects overlapping compressed sections and may emit an initial value before `filter.start`. The adapter must drop callback events where `time < start`.

FST ordering contract:

1. increasing callback time;
2. deterministic FST callback/handle order within a time;
3. requested alias/duplicate order within one handle.

FST does not retain VCD source-command order. Do not promise cross-format extraction row order. Attach a monotonically increasing backend sequence to callbacks where timeline merging requires stable ordering.

If the reader's same-time handle order proves unstable across repeated/platform reads, buffer one timestamp and sort by handle index before alias expansion.

## 9. Metadata and timescale

FST metadata comes from its header and hierarchy at open:

- `signal_count`: declarations including aliases;
- `unique_signal_count`: unique handle groups;
- `start_time`/`end_time`: header values;
- timescale: `timescale_exponent` as `WaveTimescale`.

The supplied exponent `-13` maps exactly to 100 fs in the existing magnitude/unit representation. Not every exponent necessarily does. The generic model uses exponent form; protocol/legacy adapters either map exactly to SI magnitude/unit or reject unsupported exponents until an additive protocol field is approved.

Do not load the complete FST time table for metadata.

## 10. Cancellation and limits

Check `QueryContext`:

- before opening;
- after header/hierarchy processing;
- on every callback;
- before conversion;
- before alias expansion/delivery;
- after read completion and before source publication.

Callback errors represent cancellation, deadline, row/byte/work limit, early successful stop, or conversion failure and must remain distinguishable from parser errors.

FST has no VCD command count. Protocol v1 retains `max_commands`, but the FST backend interprets it as documented `work_units` based on callbacks and observable setup/section work. It must not claim equivalent VCD command accounting.

Cancellation is callback-granular. A compressed block may finish decompressing before cancellation is observed; this limitation is documented and tested.

## 11. Generation identity

Extract a private common source fingerprint from VCD identity without changing public `FileIdentity`:

```rust
struct SourceFingerprint {
    len: u64,
    modified: Option<SystemTime>,
    platform_identity: PlatformIdentity,
    sample_fingerprint: ContentFingerprint,
    full_content_fingerprint: Option<ContentFingerprint>,
}
```

VCD identity retains body/header fields. FST identity adds a structural/header fingerprint.

For each FST query:

1. validate/open configured path;
2. retain an independent handle for post-read validation;
3. move the query handle into `FstReader`;
4. read selected signals;
5. validate the admitted generation;
6. validate a fresh configured-path reopen;
7. publish only after both checks pass.

`File::try_clone` may share cursor; only inspect/seek the retained handle after FST reading ends.

## 12. Comparison

Implement a timeline provider for FST and then for `OpenedWaveform`.

Initial rules:

- exact complete-name intersection;
- declaration/file-only ordering follows each backend contract;
- equal physical tick required for mixed-format compare;
- mismatched timescales return a deterministic unsupported-timescale error;
- no timestamp rescaling in the first release;
- inclusive windows, no pre-window synthesis, unknown policy, max mismatches, and final same-time value behavior remain unchanged;
- both generations are revalidated immediately before result publication.

All four ordered combinations require tests: VCD/VCD, FST/FST, VCD/FST, and FST/VCD.

## 13. User-surface migration

### Native and Python CLI

Single-input commands accept `INPUT` and default content auto-detection. Add `--format auto|vcd|fst` only when explicit override is implemented. Compare uses separate reference/actual format overrides.

Existing VCD output and errors stay exact. User-facing help changes from “VCD” to “waveform” only when FST paths work.

### Rust

Keep VCD-named APIs VCD-only. Add generic waveform functions and error types.

### Python

Keep the six existing signatures unchanged and accept FST through auto-detection. Add a new `Waveform(path, format="auto")` object for explicit selection and reusable open state. Existing VCD errors remain `RuntimeError`-compatible.

### Server

Replace VCD-only generation ownership with `Arc<OpenedWaveform>`. Add additive `describe` fields:

```json
"source_format":"fst",
"supported_source_formats":["vcd","fst"],
"format_detection":"content"
```

Keep protocol query schemas and methods unchanged. Do not add server compare or arbitrary paths.

### `vcd2trace`

Remain VCD-only until the separate ten-signal byte-golden trace fixture exists.

## 14. Safety policy

`fst-reader` 0.17 contains assertions, panic/todo paths, and allocations based on file metadata. Before release:

- place `catch_unwind` around FST open/hierarchy/read boundaries;
- convert panic to a typed parser/internal error without terminating the server;
- add malformed/truncated/deep/oversized fixtures;
- validate every handle before constructing filters;
- reject whole-file gzip wrappers by default until bounded decompression exists;
- set server open limits for declarations, hierarchy depth, name bytes, catalog bytes, and decompression expansion;
- fuzz detection, open, hierarchy, and selected reads;
- upstream or patch supported-path panics found by tests.

## 15. Dependency and licensing

Proposed dependency: `fst-reader = "0.17"`, initially lock-tested at 0.17.0.

- license: BSD-3-Clause;
- pure Rust dependencies;
- checks passed for Linux x64/ARM64, Windows x64, macOS Intel/ARM64;
- release distributions must include the BSD notice/disclaimer;
- use a dependency/license audit and update SBOM/notices before release.

Do not use `fst-sys`/GTKWave C bindings unless this plan is explicitly revised.
