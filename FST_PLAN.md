# FST Support Plan

Plan ID: **FST-001**  
Status: **PROPOSED**  
Branch: **`fst-support`**

## Objective

Add read/query support for FST waveform files while preserving all existing VCD Rust APIs, CLI output, Python signatures, server behavior, and VCD semantics.

The implementation will add an `OpenedFst` backend and an additive `OpenedWaveform` facade. It will not turn `OpenedVcd` into a generic type or expose third-party FST parser types publicly.

## Supplied planning dataset

| Field | Value |
|---|---|
| Path | `/home/roba/work/gitlab/taloshdl/socs/magellan-sdf/sim/ctests/gpio_toggle/tb_full.fst` |
| Size | 63,598,937 bytes / 60.65 MiB |
| SHA-256 | `3bef0aaa70375223f763265b8cc30d7ddab1b2eee74de7dece8ee774a168b886` |
| Producer/version | TalosHDL; FST header version `TalosHDL` |
| Time range | 0–2,000,001,000,000 ticks |
| Timescale | 100 fs/tick (`10^-13` seconds) |
| Declarations | 594,469 |
| Unique handles | 504,075 |
| Aliases | 90,394 |

This external file is performance evidence, not a committed correctness fixture. A tiny generated FST fixture and regeneration recipe are required before implementation can pass Gate FST-G0.

## Measured feasibility

Using `fst-reader` 0.17.0:

- open/header discovery: about 3 ms and 4.6 MiB process RSS;
- retained prototype hierarchy/catalog: about 125–140 ms and 156–164 MiB RSS;
- full-range one-signal read after catalog: about 0.74 s;
- narrow selected-signal window after catalog: about 7–12 ms;
- full time-table loading: about 738 MiB RSS — prohibited in normal operation.

FST already indexes compressed data sections by time and signal. The VCD sparse sidecar is not needed for ordinary FST window seeking.

## Proposed product boundary

- Existing `OpenedVcd`, VCD-named Rust functions, types, errors, and outputs remain unchanged.
- Add `OpenedFst` and `OpenedWaveform::{Vcd,Fst}`.
- Native CLI and existing Python query functions auto-detect VCD/FST by content.
- Add an explicit format override only through additive CLI flags and a new generic Python/Rust API; do not alter existing Python signatures.
- Server accepts either VCD or FST at startup and adds `source_format` to `describe`.
- First FST support covers list, metadata, extract, find, toggle, serve, and comparison under the approved comparison rules.
- `vcd2trace`, FST writing/conversion, compressed wrappers, remote inputs, sidecar/cache redesign, and producer-specific extensions are excluded.

## Documents

| Document | Purpose | Status |
|---|---|---|
| [`docs/plans/fst-support-implementation-plan.md`](docs/plans/fst-support-implementation-plan.md) | Work packages, stable task IDs, dependencies, gates, validation, and risks | PROPOSED |
| [`docs/design/fst-support-architecture.md`](docs/design/fst-support-architecture.md) | Additive backend/facade architecture and integration seams | DRAFT |
| [`docs/design/fst-semantic-contract.md`](docs/design/fst-semantic-contract.md) | Exact VCD-to-FST semantic mapping and unsupported cases | PROPOSED |
| [`docs/benchmarks/fst-baseline.md`](docs/benchmarks/fst-baseline.md) | Supplied-file evidence, benchmark matrix, and targets | BASELINE PARTIAL |
| [`docs/benchmarks/artifacts/fst/reader-spike.md`](docs/benchmarks/artifacts/fst/reader-spike.md) | Full read-only parser spike report | RECORDED |
| [`docs/release/fst-rollout.md`](docs/release/fst-rollout.md) | Packaging, testing, licensing, staged release, and rollback | DRAFT |

## Status vocabulary

- `PROPOSED` — specified but not approved.
- `BLOCKED` — waiting on a gate or dependency.
- `READY` — approved and implementable.
- `IN_PROGRESS` — implementation is active.
- `IN_REVIEW` — implementation awaits review.
- `DONE` — acceptance criteria and evidence are complete.
- `DEFERRED` — deliberately postponed.
- `REJECTED` — removed from scope.

## Milestone tracker

| Milestone | Scope | Status | Exit gate |
|---|---|---|---|
| F0 | Dependency, fixtures, and semantic decisions | READY | FST-G0 |
| F1 | Format-neutral types and detection | BLOCKED | FST-G1 |
| F2 | `OpenedFst`, hierarchy catalog, identity | BLOCKED | FST-G2 |
| F3 | FST query visitor and operations | BLOCKED | FST-G3 |
| F4 | CLI, Python, compare, and server | BLOCKED | FST-G4 |
| F5 | Hardening, packaging, and release | BLOCKED | FST-G5 |

## Gate tracker

| Gate | Decision | Status | Required evidence |
|---|---|---|---|
| FST-G0 | Approve parser and semantic subset | OPEN | Tiny fixtures, semantic decisions, panic/resource policy, license review |
| FST-G1 | Accept additive waveform facade | BLOCKED | VCD APIs/tests unchanged; content detection exact |
| FST-G2 | Accept FST open/catalog/identity | BLOCKED | Catalog/metadata/generation correctness and memory target |
| FST-G3 | Accept FST query parity | BLOCKED | Extract/find/toggle/compare semantics, cancellation, malformed input |
| FST-G4 | Accept user-facing FST support | BLOCKED | Native/Python/server E2E and exact VCD regression suite |
| FST-G5 | Release FST support | BLOCKED | Fuzzing, five-target artifacts, license notices, reproducible benchmarks |

## Decisions requiring owner approval at FST-G0

1. Provisionally use `fst-reader = "0.17"`, lock-tested at 0.17.0.
2. Keep `OpenedVcd` unchanged; add `OpenedFst` and `OpenedWaveform`.
3. Use a synchronous visitor as the format-neutral streaming seam rather than a producer thread per FST query.
4. Suppress FST callbacks before `window.start` to preserve current extraction semantics.
5. Preserve FST callback order for different handles at one timestamp; expand aliases/duplicate requests in request order. Do not claim equality with VCD command order.
6. Preserve exact FST digital state bytes. Map known 2–128 bit binary values to `u128`; keep wider/unknown values as text; keep exact `f64` bits.
7. Reject invalid UTF-8 generic-string values initially.
8. Reject or explicitly opt into whole-file gzip-wrapped FST; do not decompress it without a memory limit.
9. Treat protocol `max_commands` as documented backend work units for FST until protocol v2 can name it more accurately.
10. Require equal physical timescales for initial VCD/FST comparison; no implicit rescaling.
11. Keep incomplete FST plus external `.hier` recovery and `vcd2trace` FST support deferred.
12. Catch parser panics at the backend boundary and require fuzz/corrupt-input evidence before release.

## Immediate next actions

1. Review and approve or amend the twelve FST-G0 decisions.
2. Add `fst-reader = "0.17"` and record its BSD-3-Clause notice obligations.
3. Generate deterministic tiny valid/equivalent/corrupt FST fixtures with provenance and hashes.
4. Characterize packed-real, generic-string, extended digital states, same-time order, gzip wrapper, and malformed-input behavior.
5. Do not migrate CLI, Python, or server code until FST-G0 and FST-G1 pass.
