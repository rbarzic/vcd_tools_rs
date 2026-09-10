# Current Compatibility Contract

Status: **CHARACTERIZED AT M0**  
Plan: **VCD-RQ-001**  
Scope: behavior of the pre-`OpenedVcd` implementation that later milestones must preserve unless a separately approved/versioned change says otherwise.

## 1. Rust public surface

The crate root currently exports these public items.

### Types

- `VcdError` and `Result<T>`
- `Timescale { magnitude, unit }`
- `Signal { name, id_code, size, type_, scope }`
- `SignalIndex { by_id, by_name }`
- `TimeWindow { start, end }`
- `ChangeValue::{Integer, Float, Text}`
- `TimeValue { signal, time, value }`
- `VcdMeta { timescale, signal_count, start_time, end_time }`
- `TargetValue::{Integer, Text}`
- `TimeValueIter<R>`
- `SignalMismatch`
- `ComparisonOptions`
- `ComparisonResult`
- `JsonComparisonResult`

Public fields are part of the source compatibility boundary. In particular, later compact catalog work cannot replace the public `SignalIndex` field types in place.

### Functions and methods

- signal/header: `read_signals_with_offset`, `read_signals`, `list_signals`, `list_signals_from_file`, `build_target_map`, `build_sizes`, `load_signal_list`;
- parsing/iteration: `tokenize_file`, `time_value_iter_from_body`, `TimeValueIter::new`, `compute_time_bounds`;
- queries: `read_vcd_metadata`, `extract_time_values_from_file`, `find_nth_occurrence`, `count_toggles`, `compare_vcd_files`;
- values: `normalize_change_value`, `parse_target_value`, `format_value_for_signal`, `format_change_value`, `ChangeValue::{normalize,as_integer}`, `TargetValue::normalize`;
- helpers/results: `SignalIndex::build`, `TimeWindow::contains`, `ComparisonOptions::default`, `ComparisonResult::get_summary`, `JsonComparisonResult::from`.

Later milestones keep signatures and return types unless a major/versioned change is explicitly approved. Path-based functions remain explicit temporary-open wrappers rather than acquiring a hidden process-global cache.

### Errors

Current error categories and display strings:

- I/O: `I/O error: ...`;
- missing `$enddefinitions`;
- duplicate full signal name;
- one or multiple missing signals;
- occurrence less than one;
- parser errors prefixed `unexpected parse error:`.

Tests lock deterministic missing-signal ordering, duplicate-name wording, invalid occurrence, CLI error prefix, and malformed-body propagation. Operating-system I/O text is not golden-tested because it is platform-dependent.

## 2. Header and signal semantics

- Header reading consumes through the line containing `$enddefinitions` and reports the resulting stream position as body offset.
- Scope identifiers requiring escaping are sanitized before parser use. The current escaped name is visible in the resolved full name (for example `\\top-scope.sig`).
- Signal declaration order is retained by `Vec<Signal>` and `list`.
- A full signal name is scope components joined with `.`, then reference and optional VCD index text.
- Duplicate full names are rejected even when IDs differ.
- One `IdCode` may map to multiple alias names.
- `SignalIndex` clones complete `Signal` values into public lookup maps.
- Name filtering is case-sensitive substring matching.

Evidence: `tests/query_semantics.rs` list, alias, duplicate-name, and CRLF/special-scope tests; existing `tests/integration_tests.rs` large-header tests.

## 3. Time and metadata semantics

- `TimeWindow.start` and `.end` are inclusive.
- An inverted window (`start > end`) is currently accepted and yields no extracted events; it is not an error in the Rust query API.
- Decreasing timestamps are accepted and emitted in body order. However, a query with `end` stops permanently at the first timestamp greater than `end`, so it can miss a later body command whose timestamp decreases back into the window.
- Metadata start is the first timestamp or change time; changes before a timestamp occur at time zero.
- Metadata end is the last timestamp/change time.
- Empty bodies and no-timestamp dumpvars bodies produce bounds `(0, 0)`.
- Opening/listing does not compute metadata; `read_vcd_metadata` scans the body.

Evidence: `tests/query_semantics.rs` metadata and window tests.

## 4. Value semantics

- Scalar changes are `ChangeValue::Text`, including `0`, `1`, `x`, and `z`.
- Fully known binary vectors up to and including 128 bits become `ChangeValue::Integer(u128)`.
- Wider vectors and any vector containing X/Z remain `ChangeValue::Text` in VCD bit order.
- Real changes become `ChangeValue::Float(f64)`.
- String changes become `ChangeValue::Text`.
- Text normalization lowercases; integer normalization is decimal; float normalization uses Rust string formatting.
- Display formatting for multi-bit integer signals is zero-padded lowercase hexadecimal with `0x`; non-integers use their display text.
- Comparison uses integer equality, exact `f64::to_bits()` for two floats, ASCII-case-insensitive text equality, and normalized-string fallback for unlike variants.
- `ignore_unknown` recognizes only text exactly equal to `x` or `z` ignoring case, not a wider vector containing unknown bits.

Evidence: `tests/query_semantics.rs` value normalization; `tests/integration_tests.rs` value formatting and normalization.

## 5. Extraction semantics

- Body commands are emitted in source order.
- Every same-timestamp change is retained; repeated timestamp commands do not collapse events.
- Body `$comment`, `$dumpvars`, `$dumpoff`, `$dumpon`, and `$dumpall` framing commands are ignored by extraction while contained value changes are emitted normally.
- Only changes whose timestamps are inside the window are emitted.
- No value active before `start` is synthesized.
- A shared-ID change expands to every selected alias in request order.
- Requesting the same name twice produces duplicate events.
- Missing target names are reported in request order before body scanning.
- Empty target list is accepted by the Rust function and returns no events after scanning.
- The convenience Rust/Python function materializes all events; the CLI streams iterator events except pretty rendering stores rows.
- Native CLI extraction emits one aligned row per timestamp at which a selected change occurs and carries the last selected value between emitted rows. It does not seed pre-window values.

Evidence: `tests/query_semantics.rs` alias/multiplicity/order/window tests and `tests/cli_compat.rs` aligned output test.

## 6. Find and toggle semantics

### Find

- Occurrence is one-based; zero returns `InvalidOccurrence`.
- Matching compares normalized value strings.
- Every matching value-change command counts, even if it repeats the same value; it is not transition-only.
- Search stops at the requested occurrence or after `end`/EOF.
- Not found is `Ok(None, signal_size)` in Rust, an error exit in native CLI, and `{found:false,...}` in Python.

### Toggle

- The first event inside the window establishes a baseline and is not a toggle.
- A transition from the value active before `start` is not counted.
- Comparison uses formatted signal values, including width-aware integer formatting.
- Duplicate names collapse in the returned `HashMap`, although duplicate mapped events are processed.
- Empty target list returns an empty map.
- Native CLI preserves request order when rendering the map.

Evidence: `tests/query_semantics.rs` find/toggle tests and `tests/cli_compat.rs` output test.

## 7. Compare semantics

- Both headers are parsed independently.
- `common_signals` are sorted by full name; file-only signal lists retain each file's declaration traversal order.
- `signals_only` filters the sorted common set; requested missing or file-only names are silently absent rather than errors.
- Selected timelines are materialized per signal for each file.
- Each signal begins at textual `x` for the selected window; pre-window state is not loaded.
- At a timestamp, all same-time events from each file are consumed and only each file's final current value is compared once.
- `max_mismatches` is applied independently per signal.
- `ignore_unknown` suppresses a comparison when either current value is text exactly `x` or `z`, ignoring case.
- A self/equivalent comparison passes with zero mismatches.
- Native CLI exits successfully even when comparison reports `passed:false`; mismatch is data, not process failure.

Evidence: `tests/query_semantics.rs` difference/window, ordering/file-only, unknown policy, mismatch limit/filter, and same-timestamp-final-value tests; `tests/cli_compat.rs` JSON/compact/default tests; existing integration compare tests.

## 8. Native CLI contract

Commands:

- `list <VCD> [--filter]`
- `meta <VCD>`
- `extract <VCD> --signal/--signals-file [--start/--end]`
- `toggle <VCD> --signal/--signals-file [--start/--end]`
- `find <VCD> --signal --value [--occurrence|--occurence] [--start/--end]`
- `compare <REFERENCE> <ACTUAL> [options]`

Global options are `--log-level` and global `--pretty`; current Clap configuration accepts `--pretty` before or after the subcommand.

Output:

- list: one name per line or one-column table;
- metadata: fixed field-order TSV/table;
- extract: aligned TSV/table;
- toggle: request-order TSV/table;
- find: two-line TSV/table;
- compare: detailed, JSON, or compact;
- normal query errors: exit 1, empty stdout before rendering, `Error: ...` on stderr;
- compare differences do not cause nonzero exit.

Evidence: `tests/cli_compat.rs` exact/golden assertions for filter, signals files, found/not-found, option placement, all compare formats, mismatch exit behavior, and errors. Help text is intentionally not golden-tested because Clap formatting may vary with dependency patch releases; required command/argument behavior is tested.

## 9. Python contract

Module functions exported by `vcd_tools.__all__`:

- `list_signals(path, filter=None) -> list[str]`
- `metadata(path) -> dict`
- `extract(path, signals, start=None, end=None) -> list[dict]`
- `toggles(path, signals, start=None, end=None) -> dict[str,int]`
- `find(path, signal, value, occurrence=1, start=None, end=None) -> dict`
- `compare(file1, file2, *, max_mismatches=None, signals=None, ignore_unknown=False, start=None, end=None) -> dict`

All Rust `VcdError`s currently become `RuntimeError`. Returned dictionary keys are documented in `src/python.rs`. The pip console entry point is Python `vcd_tools._cli:main`, not the native Rust binary.

### Current Python console compatibility exceptions

The Python console currently diverges from the native CLI in two characterized ways:

1. Native Clap declares `--pretty` global and accepts it before or after the subcommand. Python `argparse` accepts `--pretty` only before the subcommand and rejects `meta file.vcd --pretty` with exit 2.
2. Native aligned extraction carries the last selected value into a later emitted row. Python console alignment only fills values changed at that exact timestamp, leaving other columns empty.

These are compatibility exceptions, not desired new server semantics. M0 locks them with `tests/python/test_cli_compat.py` rather than silently changing user-visible behavior. A later task must explicitly choose whether to preserve the divergence or align the Python console through a documented behavior change and updated tests/reference.

**Documented M0 exception:** automated compiled-extension compatibility tests are not added in this task because the repository has no existing maturin development test harness and ordinary `cargo test` does not import the extension. The focused pure-Python console tests run without an extension by injecting deterministic API results. M7 owns the wheel/import matrix. M2 must at minimum add a configured Python extension smoke job before changing wrapper internals. The signatures, shapes, and exception mapping above remain required compatibility targets.

## 10. `vcd2trace` contract

- Separate binary with positional VCD, `--output`, optional JSON `--signal-path`, and log level.
- Default signal paths are fixed Aldebaran hierarchy strings.
- Override JSON fields are optional and merge individually with defaults.
- All ten required resolved signals must exist before body processing.
- One full body scan maintains latest state and processes rising clock edges.
- Output lines use `PC : 0x........ I : 0x........` and `xNN <= 0x........` forms.

**Documented M0 exception:** no small source-control fixture currently models all ten CPU trace signals and expected architectural output. Migration is optional task RQ-M2-T09 and requires a byte-identical golden fixture before code changes. Until then, `vcd2trace` is a no-change compatibility boundary and existing build/smoke coverage applies.

## 11. Packaging and platforms

- Rust 2024 package version `0.1.7`.
- Library crate types: `rlib` and `cdylib`.
- Optional `python` feature enables PyO3 ABI3 Python 3.8.
- Native binaries: `vcd_tools_rs` and `vcd2trace`.
- PyPI project `vcd-tools` version `0.1.7`; console command uses Python wrapper.
- Release workflow builds Linux x64/ARM64, macOS Intel/ARM64, and Windows x64 archives containing both native binaries.
- Server work must not remove existing Windows artifacts or make Python wheels require Unix-only APIs.

Evidence: `Cargo.toml`, `pyproject.toml`, `.github/workflows/release.yml`; cross-target compile remains a later CI gate because target toolchains are not all guaranteed locally.

## 12. Coverage map and known gaps

| Contract | Evidence |
|---|---|
| Header/list/order/aliases/duplicates | `query_semantics`, existing integration tests |
| Metadata bounds/empty/no timestamp | `query_semantics` |
| Extraction order, multiplicity, bounds, types | `query_semantics` |
| Find/toggle semantics | `query_semantics` |
| Compare mismatch/window behavior | `query_semantics` |
| Native CLI version/list/meta/extract/toggle/find/compare/errors | `cli_compat` |
| Large >100k-signal header and current broad APIs | existing `integration_tests` |
| Python pure-console option placement/alignment | `tests/python/test_cli_compat.py`; documented divergence above |
| Python compiled-extension runtime | documented exception; M2/M7 follow-up |
| `vcd2trace` byte-golden output | documented exception; required before optional migration |
| Cross-platform source compatibility | release workflow contract; later CI gate |
| OS-specific I/O error wording | deliberately not golden-tested |

This document records behavior; it does not declare every behavior ideal. In particular inverted windows, escaped scope spelling, materialization, and compare exit status may only change through an explicit compatibility decision.
