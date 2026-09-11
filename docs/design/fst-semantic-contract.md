# FST Semantic Contract

Status: **PROPOSED — REQUIRED FOR FST-G0**  
Plan: **FST-001**

This document classifies existing VCD semantics for FST support. Implementation may not silently reinterpret a `PRESERVE` rule. `NORMALIZE`, `REJECT`, and `FORMAT-SPECIFIC` rules require the stated behavior and tests.

## 1. Catalog and names

| Existing behavior | FST rule | Classification |
|---|---|---|
| declaration order retained | retain FST hierarchy declaration callback order | PRESERVE |
| full names use `.` between scopes | join FST scopes with `.` | NORMALIZE |
| VCD reference/index spelling preserved | preserve exact FST variable name, including spaces/ranges | FORMAT-SPECIFIC |
| duplicate complete names error | deterministic duplicate-name error | PRESERVE |
| one backend ID may have aliases | one FST handle maps to all declaration aliases | PRESERVE |
| duplicate requested names create duplicate output rows | retain request multiplicity | PRESERVE |
| substring filter is case-sensitive | same | PRESERVE |

Optional alternative spelling lookup is not part of the initial release because it can create collisions. Output always uses the exact catalog name.

## 2. Time and windows

| Existing behavior | FST rule | Classification |
|---|---|---|
| start/end inclusive | adapter applies inclusive bounds | PRESERVE |
| no value active before `start` is emitted | suppress FST section-initial callbacks with `time < start` | PRESERVE |
| inverted window returns no Rust events in legacy APIs | generic surface follows approved existing behavior; server may reject as `INVALID_WINDOW` | PRESERVE |
| toggle first in-window event establishes baseline | same | PRESERVE |
| metadata start/end | use FST header values after consistency checks | NORMALIZE |
| raw ticks are input/output units | same | PRESERVE |

FST time-zero/blackout data not exposed by `fst-reader` is unsupported until an upstream API or explicit parser extension exists.

## 3. Values

| FST value | Output mapping | Classification |
|---|---|---|
| width-1 digital state | exact one-byte `Text` | PRESERVE |
| 2–128 bits, all `0`/`1` | `Integer(u128)` | NORMALIZE to existing model |
| vector with any other state | exact `Text` | PRESERVE |
| vector wider than 128 bits | exact `Text` | PRESERVE |
| real | `Float(f64)` with exact `to_bits()` comparison/wire bits | PRESERVE |
| generic string, valid UTF-8 | `Text` | PRESERVE |
| generic string, invalid UTF-8 | typed unsupported/value error | REJECT |

Digital bytes are not lowercased. States beyond VCD 0/1/x/z, including h/u/w/l/-/?, remain exact. FST-G0 must decide which scalar states count as “unknown” for compare; until then only existing x/z behavior is guaranteed.

Original FST lexical compression/encoding is not exposed.

## 4. Ordering

VCD preserves body command order. FST does not retain an equivalent source-command stream.

FST extraction ordering is:

1. increasing callback timestamp;
2. deterministic backend callback/handle order at equal timestamp;
3. requested alias/duplicate order for one handle.

Classification: **FORMAT-SPECIFIC**.

Requirements:

- repeated reads of the same FST must produce identical order;
- supported platforms must agree, or the adapter buffers one timestamp and sorts by handle index;
- a backend sequence is attached as callbacks arrive for stable timeline merging;
- cross-format tests compare semantic values/results, not global extraction row order at equal timestamps.

Comparison remains based on each signal’s final value at a timestamp, so cross-signal callback order does not change mismatch semantics.

## 5. Find and toggle

- Find occurrence is one-based; zero is the existing invalid-occurrence error.
- Every matching in-window callback counts, even if the value repeats.
- Callback early-stop is a successful private control path, not a parser failure.
- Toggle compares width-aware accepted semantic values.
- The first in-window callback is not a toggle.
- A pre-window reconstruction callback must not seed the toggle baseline.

Classification: **PRESERVE**.

## 6. Metadata

Generic metadata:

```text
format              fst
signal_count        declaration count, aliases included
unique_signal_count unique handle groups
start_time          FST header start
end_time            FST header end
timescale           exponent10_seconds and exact legacy mapping when possible
```

For the supplied sample:

```text
signal_count: 594469
unique_signal_count: 504075
start_time: 0
end_time: 2000001000000
timescale: -13 exponent = 100 fs/tick
```

Do not load the complete FST time table to answer metadata.

## 7. Cancellation and work limits

- Cancellation/deadline checks occur before open, after hierarchy, every callback, before conversion, before delivery, and after read.
- Cancellation cannot necessarily interrupt one compressed-block decompression between callbacks. This is a documented backend limitation.
- Row and logical-byte limits retain exact N/N+1 behavior.
- `max_commands` is interpreted as backend work units for FST, not as VCD parser commands. Statistics must not claim the counts are cross-format equivalent.
- Callback errors preserve cancellation/limit/early-stop versus parser-error categories.

Classification: cancellation/row/bytes **PRESERVE**; command accounting **FORMAT-SPECIFIC**.

## 8. Comparison

Initial comparison support:

| Pair | Rule |
|---|---|
| VCD/VCD | unchanged |
| FST/FST | supported after query parity |
| VCD/FST | supported only when physical tick duration is equal |
| FST/VCD | same rule and symmetric mismatch semantics |

Mismatched timescales return a deterministic unsupported-timescale error. No implicit scaling, rounding, or overflow-prone conversion is allowed initially.

Signal intersection uses exact full names. The equivalent fixture must intentionally use matching names. Format-specific names without an exact peer are file-only signals.

Unknown handling initially retains current scalar x/z behavior. Any extension to h/u/w/l/-/? requires a separately approved semantic change and tests.

## 9. Format detection and errors

- Auto-detection uses content, not extension.
- Explicit format mismatch fails before output.
- Empty/truncated/ambiguous content returns a stable unable-to-detect or format-specific parse error.
- Existing VCD-specific Rust functions and errors remain VCD-only.
- New generic APIs use an extensible waveform error with detection, FST parse, unsupported feature, resource limit, stale source, cancellation, and I/O categories.
- Existing Python functions continue raising `RuntimeError`-compatible errors.

## 10. Incomplete and wrapped FST

Initial policy:

- incomplete FST requiring external `.hier`: REJECT with a clear unsupported/incomplete error;
- automatic sibling `.hier` discovery: REJECT;
- whole-file gzip-wrapped FST: REJECT by default until bounded decompression is available;
- explicit future hierarchy/gzip options: DEFERRED.

## 11. Panic and malformed input

Malformed FST must not terminate the process or server worker.

- catch parser panic at each backend boundary;
- return a bounded typed error;
- discard the failed reader/generation;
- cap open-time hierarchy counts, depth, names, catalog ownership, and decompression expansion for server use;
- keep corrupt fixtures and fuzz regressions.

A parser panic or uncontrolled allocation is a release blocker even if ordinary trusted fixtures pass.

## 12. Required FST-G0 decisions

- Unknown classification for h/u/w/l/-/? states.
- Gzip wrapper: confirm reject-by-default.
- Generic string invalid UTF-8: confirm rejection.
- Same-time callback order: preserve reader order if repeatable, otherwise sort by handle.
- Work-unit accounting definition.
- Supported variable/scope/attribute subset, including packed-real behavior.
- Exact timescale exponent range accepted by protocol adapters.
