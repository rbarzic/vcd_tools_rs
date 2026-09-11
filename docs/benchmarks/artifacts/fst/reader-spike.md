# FST reader spike for `vcd_tools_rs`

Status: read-only planning spike; repository unchanged  
Repository branch/commit inspected: `fst-support` / `98129e1fc3c71e03087383b2e9ba4457dc3223f5`  
Recommended dependency: **`fst-reader = "0.17"`**, initially lock-tested at **0.17.0**

## 1. Executive result

FST support is feasible and is a strong fit for the reusable query/server direction, but it should be added as a separate `OpenedFst` backend behind a format-neutral facade rather than by making `OpenedVcd` parse both formats.

`fst-reader` 0.17.0 provides the important features directly:

- safe-Rust FST parsing with no C runtime dependency;
- cheap metadata/data-section discovery on open;
- hierarchy callbacks with scopes, widths, source types, aliases, and handles;
- signal and inclusive time filtering at the compressed-section level;
- synchronous callback streaming of digital/string bytes and real `f64` values;
- incomplete-FST recovery with an external `.hier` file;
- optional complete time-table loading.

The supplied 60.65 MiB FST opens in about 3 ms with about 4.6 MiB process RSS. Building a retained 594,469-declaration name catalog in the prototype took about 125–140 ms and peaked around 156–164 MiB. A one-signal full-range query took about 0.74 s after catalog construction; a narrow time-window query took about 7–12 ms after catalog construction. This is already indexed by FST data sections: no sidecar should be required for basic FST window seeking.

Important integration constraints:

1. `read_signals` is callback/push based, not an iterator.
2. The first relevant section can emit a frame value with a timestamp **before `filter.start`**. The adapter must suppress pre-window callback rows to preserve existing extraction/toggle/compare semantics.
3. FST callbacks collapse aliases to one handle. Requested alias/duplicate names must be expanded from the catalog in request order.
4. Same-timestamp cross-signal order is FST-decoder order, not VCD source-command order. A format-neutral ordering contract must be decided explicitly.
5. Cancellation is observable at callback boundaries, but one compressed signal chain/block can be decompressed before the next callback. The current VCD command-budget meaning cannot be copied literally.
6. `fst-reader` contains several `assert!`, `todo!`, and `panic!` paths for uncommon/malformed constructs. FST input must be treated as potentially panicking until upstream hardening/fuzz evidence exists.
7. `open_and_read_time_table` is not suitable as the normal open path: this sample has 92,797,051 time entries and consumed about 738 MiB RSS.

## 2. Sample file

```text
path: /home/roba/work/gitlab/taloshdl/socs/magellan-sdf/sim/ctests/gpio_toggle/tb_full.fst
size: 63,598,937 bytes (60.65 MiB)
mtime: 2026-08-24 08:16:18.884345186 +0200
mode: -rw-rw-r--
SHA-256: 3bef0aaa70375223f763265b8cc30d7ddab1b2eee74de7dece8ee774a168b886
```

First 64 bytes:

```text
00000000: 0000 0000 0000 0001 4900 0000 0000 0000
00000010: 0000 0001 d1a9 5962 4069 5714 8b0a bf05
00000020: 4000 0000 0008 0000 0000 0000 0000 01a0
00000030: b400 0000 0000 0912 2500 0000 0000 07b1
```

Header from `FstReader::get_header()`:

```text
start_time: 0
end_time: 2,000,001,000,000
var_count: 594,469 declarations
max_handle: 504,075 unique handles
version: "TalosHDL"
date: "Mon Aug 24 08:03:34 2026\n"
timescale_exponent: -13
```

Exponent `-13` means `10^-13` seconds per tick, representable as **100 fs** in the current `Timescale` model.

### Hierarchy/catalog

```text
scopes: 106,676
  modules: 105,249
  begin scopes: 1,361
  tasks: 64
  functions: 2
maximum scope depth: 14
variables/declarations: 594,469
unique handles: 504,075
aliases: 90,394
attributes observed by the prototype: 0
```

Variable types:

```text
Wire: 580,639
Reg: 11,151
Parameter: 2,428
Real: 134
Integer: 66
Event: 51
```

Width distribution:

```text
1 bit: 587,525
2..32 bits: 5,187
33..128 bits: 191
>128 bits: 1,566
maximum width: 2,049
maximum local-name bytes: 64
```

Source naming should be preserved. The file includes names such as:

```text
tb.U_CHIP.U_TOP_PERIPH.gpio_function_r [95:0]
tb.U_CHIP.U_TOP_PERIPH.gpio_pad_dout [31:0]
```

The space before the range is part of the FST hierarchy name. Silently rewriting it to VCD spelling could cause collisions and should not be done in the core catalog.

Alias example: `tb.U_CHIP.U_TOP_PERIPH.FE_PHC20603_gpio_function_r_7.X` is an alias of handle/index 382983. FST filters operate on unique handles; the adapter must map that one change back to selected declaration names.

## 3. `fst-reader` 0.17.0 assessment

### Package

```text
crate: fst-reader 0.17.0
repository: https://github.com/ekiwi/fst-reader
cached source commit: 92b3b821e88b8058a72a7cd04607a327b8397152
edition: Rust 2024
license: BSD-3-Clause
dependencies: lz4_flex 0.13, miniz_oxide 0.9, num_enum 0.7, thiserror 2
```

The license is compatible with this MIT project, but binary/source distributions must retain the BSD notice and disclaimer.

`cargo search fst-reader` reported 0.17.0 as current. The cached 0.8.7 is not recommended: it is older, uses `flate2`/`tempfile`, lacks later API/fix work, and provides no planning benefit over 0.17. Use the maintained latest line and commit a compatibility fixture before upgrades.

### Core API

```rust
FstReader::open(BufRead + Seek)
FstReader::open_and_read_time_table(...)
FstReader::open_incomplete(fst, hierarchy)
reader.get_header() -> FstHeader
reader.get_time_table() -> Option<&[u64]>
reader.read_hierarchy(callback)
reader.read_signals(&FstFilter, callback)
is_fst_file(&mut Read + Seek)
```

`FstFilter` has inclusive `start`, optional inclusive `end`, and optional included handles.

Values are:

```rust
FstSignalValue::String(&[u8])
FstSignalValue::Real(f64)
```

The “String” variant serves digital vectors, generic strings, and one-bit states. Width/type information must come from the hierarchy catalog.

`FstReader<BufReader<File>>` compiled as both `Send` and `Sync`, but read methods require `&mut self`; do not share one reader behind a mutex. Open an independent reader per query, as with VCD. Open is cheap.

### Filtering and random access

Open records each compressed data section’s file offset and time span. `read_signals` skips sections outside the requested interval and only decompresses selected signal chains within relevant sections. This provides native coarse random access without loading the complete time table.

Do **not** call `open_and_read_time_table` by default. On the supplied file:

```text
time entries: 92,797,051
open + table elapsed: 0.43 s
peak RSS: 738,256 KiB
first/last: 0 / 2,000,001,000,000
```

The table alone is roughly 742 MiB of `u64` payload before normal process overhead.

### Incomplete files

The API can recover missing geometry/hierarchy using an external `.hier` file. Product behavior needs a decision:

- v1 recommendation: normal `.fst` only; return a clear incomplete-file error;
- later option: `--hierarchy <path>` / open option for explicit recovery;
- do not silently guess sibling `.hier` paths in server mode.

## 4. Measured behavior

Temporary prototype:

```text
/tmp/fst-reader-spike
fst-reader dependency: 0.17.0
repository modifications: none
```

### Open and hierarchy

Open-only:

```text
elapsed < 0.01 s
internal open timing around 2–7 ms
peak RSS 4,596 KiB
```

Hierarchy decoder without retaining names:

```text
elapsed 0.05 s
peak RSS 17,288 KiB
```

Open + retained prototype catalog (`Vec<String>` paths and metadata):

```text
open: 3,299 us
hierarchy/catalog: 124,740 us
process elapsed: 0.17 s
peak RSS: 156,028 KiB
```

A production arena/shared-name catalog should be measured separately, but the supplied file establishes a realistic 594k-declaration scale.

### Selected query scans

Every result below includes one open and one hierarchy/catalog build. `read_us` is signal-reading time after those steps.

One clock over unbounded file range:

```text
signal: tb.U_ALDEBARAN_MONITOR.clk
events: 2,431
text events: 2,431
unknown events: 5
first/last callback time: 0 / 12,436,545,700
read_us: 741,276
total elapsed: 0.91 s
peak RSS: 169,528 KiB
```

Signals whose names contain lowercase `gpio`:

```text
selected declarations: 10,643
unique handles: 10,124
events: 56,187
text events: 56,187
unknown-containing events: 28,878
value bytes: 77,324
first/last callback time: 0 / 12,440,511,710
read_us: 737,423
total elapsed: 0.90 s
peak RSS: 166,336 KiB
character totals: '0'=22,045, '1'=10,000, 'x'=34,011, 'z'=11,268
```

Twenty-four selected `.alpha` real declarations had no recorded callbacks in this sample. The hierarchy correctly classified 134 Real declarations, but another fixture is required to validate real changes and exact bit preservation.

Narrow clock windows:

```text
window 0..1,000,000,000: read 11.8 ms, total 0.18 s, one callback
window 6,000,000,000..6,001,000,000: read 12.2 ms, total 0.18 s, one callback at time 0
window 1,900,000,000,000..1,900,001,000,000: read 7.0 ms, total 0.16 s, no callbacks
```

The callback at time 0 for the middle window is significant: FST emits the relevant section’s initial frame to reconstruct state even though it precedes `filter.start`. Existing extraction semantics emit only changes inside the requested window. The backend must drop callback events where `time < start`. If a future API wants “value active at start,” it must be an explicit new semantic mode.

Selecting zero handles still traversed the full unbounded data range in about 0.78 s. Resolve/validate targets first and return immediately for an empty target set where current semantics permit it.

### Early termination

The callback returns `Result`; returning an application error stops reading. This supports `find` early exit, cancellation checks, row/result limits, and backpressure failure. Callback errors are wrapped as `ReadSignalsError::CallbackError` and must remain distinguishable from reader errors.

### Ordering

The reader emits increasing time-table order. At one timestamp, its linked-list/scatter implementation determines cross-handle order, which is not a recoverable VCD source-command order. FST generally stores per-signal value chains rather than original command order.

Recommended contract:

- preserve callback order for FST and requested alias order within one handle;
- document output order as backend-stable, not cross-format-identical;
- for caches/compare, attach a monotonically increasing backend sequence as callbacks arrive;
- tests must lock repeated-time behavior;
- do not promise that equivalent VCD and FST extraction rows have identical cross-signal order.

## 5. Value mapping to existing semantics

Use hierarchy width/type plus callback variant:

| FST input | Existing `ChangeValue` mapping |
|---|---|
| width 1 digital bytes | `Text`, preserving `0/1/x/z/...` |
| width 2..128, all bytes `0/1` | `Integer(u128)` |
| wider vector or any non-`0/1` state | exact `Text` |
| `FstSignalValue::Real` | `Float(f64)`; comparisons use `to_bits()` |
| GenericString / variable-length bytes | UTF-8 text only after an explicit policy |

FST defines additional one-bit states (`x z h u w l - ?`). Preserve exact bytes/case. Current “unknown” helpers only recognize scalar x/z, so tests must decide whether other FST states are unknown for compare/formatting; do not normalize them silently.

`FstSignalValue::String` is bytes. Digital states are ASCII, but arbitrary GenericString UTF-8 validity is not guaranteed by the API. Recommended v1 rule: digital values must be valid known state bytes; GenericString uses strict UTF-8 and returns a typed unsupported/parse error for invalid bytes. Lossy conversion would corrupt query semantics.

## 6. Error and robustness findings

Straight truncation behaved as errors rather than panics in the prototype:

```text
empty: is_fst=false, open UnexpectedEof
100 bytes: is_fst=true, open UnexpectedEof
1 MiB: is_fst=true, open MissingGeometry
half file: is_fst=true, open MissingGeometry
```

However, 0.17.0 is not panic-free for adversarial/uncommon inputs. Source inspection found production paths containing:

- assertions for header length, decompressed length, section metadata, geometry/frame counts, and hierarchy state;
- `todo!` for hierarchy EnvVar, ValueList, Unknown attributes and unknown hierarchy entry types;
- `todo!` for impossible/unsupported float endian and gzip dictionary cases;
- an assertion for a rare packed real case;
- `panic!` for an unexpected gzip-wrapper state;
- unchecked allocation sizes derived from file metadata in several decompress paths.

A whole-file gzip wrapper is decompressed completely into a `Vec<u8>` on open. This can violate server memory limits or permit an allocation bomb.

Required safety gate before enabling server uploads/untrusted paths:

1. add `catch_unwind` at the FST backend boundary so a malformed file cannot kill the server worker/process;
2. convert panic to a typed internal/parse error and discard that reader generation;
3. fuzz open/hierarchy/read-signals with allocation/time limits;
4. upstream or patch the reachable `todo!`/assert paths needed by supported files;
5. reject or explicitly budget gzip-wrapper files until bounded decompression exists;
6. validate every requested handle against the catalog before constructing `FstFilter` (invalid handles panic in bitmask indexing).

This project opens one configured local file, which reduces exposure, but normal library/CLI calls can still receive arbitrary files.

## 7. Platform/build findings

A temporary crate using `fst-reader = "0.17.0"` passed `cargo check` for:

```text
x86_64-unknown-linux-gnu
x86_64-pc-windows-msvc
aarch64-unknown-linux-gnu
x86_64-apple-darwin
aarch64-apple-darwin
```

The dependency stack is pure Rust. No fstlib C library, bindgen, C compiler, or platform shim is required. This fits existing native and PyPI wheel targets substantially better than `fst-sys`.

The crate uses Rust edition 2024, compatible with this repository’s toolchain. It declares no Cargo feature surface and no explicit lower `rust-version`; the project CI toolchain remains authoritative.

## 8. Recommended architecture

### Keep format-specific opened types

```rust
pub enum OpenedWaveform {
    Vcd(OpenedVcd),
    Fst(OpenedFst),
}
```

`OpenedFst` owns:

- configured/display path and generation identity;
- `FstHeader`-derived metadata;
- a compact FST hierarchy catalog keyed by `FstSignalHandle` index;
- declaration name/order, scope arena, width, FST type/direction, and alias bindings;
- no shared mutable `FstReader`;
- no full time table.

Each query opens a new validated `File`, constructs `FstReader` (cheap), runs a filtered callback, then revalidates source identity before successful publication.

Do not change `OpenedVcd` or public `Signal`/`SignalIndex` types to contain FST variants. Preserve those compatibility surfaces. Add format-neutral metadata/signal views at the `OpenedWaveform` facade and keep source-specific details available from `OpenedFst`.

### Detection

Use magic/content probing, not extension alone:

1. explicit `WaveformFormat` override when supplied;
2. `fst_reader::is_fst_file` on a freshly opened handle;
3. otherwise parse as VCD and retain existing error behavior.

An `.fst` extension with invalid content must not be accepted solely by name. Detection must restore the file cursor and generation validation must bind detection/header/catalog to one source generation.

### Query execution seam

`fst-reader` is push/callback based. Preferred core:

```rust
OpenedFst::for_each_change(targets, window, context, callback)
```

The callback converts borrowed bytes immediately, expands selected alias/duplicate bindings in request order, applies row/logical-byte limits, and can stop with a typed query error.

Then choose one of two integration routes in an explicit spike:

A. Refactor the format-neutral engine/service around an event sink/callback. This avoids an extra thread and matches `fst-reader` naturally. Keep existing VCD iterator APIs as compatibility wrappers.

B. Implement `FstTimeValueIter` with a producer thread and a small bounded channel. The producer owns `FstReader`; iterator drop cancels and joins it. This preserves the existing pull interface but creates an extra thread per FST query and requires careful panic/backpressure/join handling.

Recommendation: **A for the long-term format-neutral core**. Spike both before committing because the current server and compare provider consume iterators.

### Built-in indexing

FST already stores data sections with time spans and per-signal compressed chains. Use native `FstFilter`; do not build the VCD sparse sidecar for FST. A future hot-signal cache can sit above both formats, but FST should first be benchmarked without it.

### Metadata

FST start/end/variable count/timescale are available at open; metadata should not trigger a body scan. `var_count` is declaration count, while `max_handle` is unique signal count. The format-neutral metadata schema should expose the existing `signal_count` as declaration count to match `list`, with optional `unique_signal_count` as an additive field.

`FstHeader` does not expose the underlying header’s `time_zero`, scope count, section count, or blackout table. Determine whether time-zero/blackout semantics matter and, if so, request upstream API additions rather than parsing private layout in this project.

## 9. CLI, Python, server, and compare scope

Recommended first release:

- existing `list`, `meta`, `extract`, `find`, and `toggle` auto-detect VCD/FST;
- existing Python module functions accept either format with unchanged signatures;
- server opens either format and reports `format:"fst"|"vcd"` in `describe` as an additive protocol field/version-compatible capability;
- native and pip Unix server paths share the same backend;
- `compare` initially supports same-format files with identical timescales; cross-format compare is a separate decision;
- `vcd2trace` remains VCD-only until a trace golden test covers FST.

Time arguments remain raw waveform ticks. Cross-format compare must reject differing timescales initially. Later normalization to a common physical unit requires checked overflow and a specified rounding rule.

No command should be renamed from `vcd_tools_rs` in the first FST release; package/project renaming is unrelated and breaking.

## 10. Hard unknowns / required decisions

1. **Push vs pull integration:** callback-native core versus bounded producer-thread iterator.
2. **Same-time ordering:** formalize FST callback order and cross-format compare/extract expectations.
3. **Pre-window frame:** suppress for current semantics; decide whether a future active-at-start mode is wanted.
4. **Command budget:** FST has no VCD-command count. Define a format-neutral work budget or document FST `max_commands` as callback/work-unit based. Cancellation cannot interrupt every decompression operation.
5. **Time zero and blackouts:** needed semantics are not exposed publicly by the crate.
6. **GenericString bytes:** strict UTF-8 versus a new byte-valued `ChangeValue` variant.
7. **Additional FST states:** unknown handling for h/u/w/l/-/? in compare and display.
8. **Panic/allocation hardening:** dependency behavior on uncommon attributes, rare packed real data, gzip wrappers, and malformed lengths.
9. **Incomplete FST:** whether to expose explicit `.hier` recovery.
10. **Cross-format compare:** reject initially or normalize timescales and ordering.
11. **Naming:** preserve exact FST names with spaces/ranges versus optional compatibility lookup spelling.
12. **Metadata naming:** declaration count versus unique handle count.

## 11. Implementation plan with tracking

### F0 — contract and dependency gate

- `FST-F0-T01`: add `fst-reader = "0.17"`; record BSD notice obligations.
- `FST-F0-T02`: generate/check in a tiny FST fixture using `fst-writer` only as a dev tool. Cover alias, duplicate requested name, 1/32/129-bit vectors, x/z and extended states, real changes, string/invalid UTF-8 policy, repeated timestamp, empty signal, and multiple data sections.
- `FST-F0-T03`: characterize callback ordering, pre-window frame, callback early abort, start/end inclusivity, malformed/truncated behavior, and gzip wrapper.
- `FST-F0-T04`: decide the 12 unknowns in section 10.
- Gate `FST-G0`: no production backend until semantic decisions and fixture pass.

### F1 — `OpenedFst` and catalog

- `FST-F1-T01`: format detection bound to one opened generation.
- `FST-F1-T02`: compact scope/declaration/handle/alias/type catalog.
- `FST-F1-T03`: FST timescale/header metadata mapping without full time table.
- `FST-F1-T04`: independent reader admission/completion validation.
- `FST-F1-T05`: memory/open/list benchmarks on tiny fixture and supplied 60.65 MiB sample.
- Gate `FST-G1`: list/catalog/metadata exact; no body scan for metadata; memory budget accepted.

### F2 — query backend

- `FST-F2-T01`: implement callback-native selected-change decoder with pre-window suppression.
- `FST-F2-T02`: value mapping and alias/duplicate expansion.
- `FST-F2-T03`: cancellation/deadline/row/byte/work limits and callback-error mapping.
- `FST-F2-T04`: extraction, find early exit, and toggle baseline semantics.
- `FST-F2-T05`: timeline provider/compare support for same-format/equal-timescale inputs.
- `FST-F2-T06`: panic boundary and malformed/allocation tests.
- Gate `FST-G2`: differential semantic suite and bounded server behavior pass.

### F3 — format-neutral facade and user surfaces

- `FST-F3-T01`: `OpenedWaveform` auto-detection and common metadata/signal/query views.
- `FST-F3-T02`: CLI list/meta/extract/find/toggle accept FST; update help from “VCD path” to “waveform path.”
- `FST-F3-T03`: Python functions accept FST with compiled-extension tests.
- `FST-F3-T04`: server accepts FST and advertises format/capabilities.
- `FST-F3-T05`: same-format compare and explicit mismatch errors for unsupported cross-format/timescale cases.
- `FST-F3-T06`: keep strict VCD-only functions/API behavior available where names promise VCD.
- Gate `FST-G3`: VCD regression suite green and FST native/Python/server E2E green.

### F4 — release

- `FST-F4-T01`: all current Rust targets and native/PyPI wheel builds.
- `FST-F4-T02`: Linux wheel `serve` smoke using FST fixture.
- `FST-F4-T03`: supplied sample benchmark report: open/list, one/10k signals, early/middle/late windows, cancellation, peak RSS.
- `FST-F4-T04`: dependency license notices and README/CLI/Python/protocol documentation.
- `FST-F4-T05`: fuzz/corpus run or explicit beta limitation for trusted files only.
- Gate `FST-G4`: release only after VCD behavior remains unchanged and panic/security limitations are documented.

Suggested statuses at plan creation:

```text
F0: READY
F1-F4: BLOCKED
FST-G0..G4: OPEN/BLOCKED
```

## 12. Acceptance targets

- Sample open + metadata: under 50 ms and under 32 MiB RSS without hierarchy catalog.
- Sample hierarchy/catalog: under 500 ms; peak RSS recorded and accepted before optimization.
- Narrow selected-signal query after catalog: under 100 ms for representative early/middle/late windows on the sample.
- Unbounded one-signal query: no material regression versus the measured ~0.74 s read time.
- Never load the complete FST time table in normal open/query paths.
- Alias/duplicate request multiplicity and current window/toggle/find behavior match the accepted contracts.
- VCD tests/output remain unchanged.
- Malformed FST cannot terminate the server process.
- FST query queues/results remain bounded.
- Linux/macOS native and Unix PyPI `serve` work; Windows query APIs build even though Unix server does not.

## 13. Exact spike commands

Prototype build:

```sh
mkdir -p /tmp/fst-reader-spike/src
# Cargo.toml: fst-reader = "0.17.0"
cd /tmp/fst-reader-spike
cargo build --release
```

File identity:

```sh
stat --printf='path=%n\nsize=%s\nmtime=%y\nmode=%A\n' "$F"
sha256sum "$F"
xxd -l 64 "$F"
```

Representative measurements:

```sh
/usr/bin/time -f 'elapsed=%e user=%U sys=%S rss_kib=%M in=%I' \
  target/release/open "$F"
/usr/bin/time -f 'elapsed=%e user=%U sys=%S rss_kib=%M in=%I' \
  target/release/fst-reader-spike "$F" inspect
/usr/bin/time -f 'elapsed=%e user=%U sys=%S rss_kib=%M in=%I' \
  target/release/fst-reader-spike "$F" scan tb.U_ALDEBARAN_MONITOR.clk
/usr/bin/time -f 'elapsed=%e user=%U sys=%S rss_kib=%M in=%I' \
  target/release/fst-reader-spike "$F" scan \
  tb.U_ALDEBARAN_MONITOR.clk 6000000000 6001000000
/usr/bin/time -f 'elapsed=%e user=%U sys=%S rss_kib=%M in=%I' \
  target/release/fst-reader-spike "$F" table
```

Cross-target source checks:

```sh
for target in \
  x86_64-unknown-linux-gnu \
  x86_64-pc-windows-msvc \
  aarch64-unknown-linux-gnu \
  x86_64-apple-darwin \
  aarch64-apple-darwin
do
  cargo check --target "$target"
done
```

Malformed probes were run on empty, 100-byte, 1 MiB, and half-file truncations. Repository status remained clean throughout the spike.

## 14. Final recommendation

Proceed with FST support using `fst-reader` 0.17, but make the first implementation milestone a semantic/robustness adapter, not a direct substitution into `OpenedVcd`.

The sample demonstrates excellent native FST indexing and practical selected-query performance. The largest risks are not basic parsing speed; they are callback integration, pre-window frame semantics, malformed-input panic paths, work-limit/cancellation definition, and cross-format ordering. Resolve those in F0/F2 while preserving the existing VCD backend unchanged.
