# FST Baseline and Benchmark Plan

Status: **PARTIAL READ-ONLY BASELINE**  
Plan: **FST-001**  
Detailed evidence: [`artifacts/fst/reader-spike.md`](artifacts/fst/reader-spike.md)

## 1. Dataset

```text
path: /home/roba/work/gitlab/taloshdl/socs/magellan-sdf/sim/ctests/gpio_toggle/tb_full.fst
size: 63,598,937 bytes (60.65 MiB)
mtime: 2026-08-24 08:16:18.884345186 +0200
SHA-256: 3bef0aaa70375223f763265b8cc30d7ddab1b2eee74de7dece8ee774a168b886
producer/version: TalosHDL
```

The path is local and not portable. Before release evidence, record a stable internal artifact reference or owner-approved provenance statement. If an equivalent VCD exists, record its path and SHA-256; otherwise this FST can measure performance but not cross-format correctness.

## 2. Structure

| Metric | Value |
|---|---:|
| Start/end ticks | 0 / 2,000,001,000,000 |
| Tick duration | 100 fs |
| Declarations | 594,469 |
| Unique handles | 504,075 |
| Aliases | 90,394 |
| Scopes | 106,676 |
| Maximum depth | 14 |
| One-bit declarations | 587,525 |
| Width 2–32 | 5,187 |
| Width 33–128 | 191 |
| Width >128 | 1,566 |
| Maximum width | 2,049 |
| Real declarations | 134 |

## 3. Initial measurements

Environment: Linux x86-64, Rust 1.93.1, uncontrolled page cache. Prototype used `fst-reader` 0.17.0 from `/tmp`; repository source was unchanged.

| Operation | Elapsed/read time | Peak RSS | Notes |
|---|---:|---:|---|
| Open/header only | ~3 ms | ~4.6 MiB | No full time table |
| Hierarchy decode, no retention | ~50 ms | ~17 MiB | 594k declarations visited |
| Open + retained prototype catalog | 125–140 ms catalog; ~0.17 s process | 156–164 MiB | Prototype `Vec<String>`, not optimized arena |
| One clock, full range | ~0.74 s read; ~0.91 s total | ~170 MiB | 2,431 callbacks |
| 10,643 `gpio` declarations | ~0.74 s read; ~0.90 s total | ~166 MiB | 10,124 handles, 56,187 callbacks |
| Clock window 0–1e9 | ~11.8 ms read | included in ~0.18 s total | one callback |
| Clock window 6e9–6.001e9 | ~12.2 ms read | included in ~0.18 s total | emitted a pre-window time-zero frame |
| Clock window 1.9e12–1.900001e12 | ~7 ms read | included in ~0.16 s total | no callbacks |
| Open plus complete time table | ~0.43 s | ~738 MiB | 92,797,051 timestamps; prohibited normal path |

The narrow-window evidence confirms FST native section indexing. A VCD-style sidecar is unnecessary for basic FST seeking.

## 4. Important observed behavior

- An overlapping section may emit a frame before requested `start`; adapter must discard it.
- Aliases collapse to one handle; expansion belongs in the project catalog.
- Selecting no handles still scanned the full range in the prototype; empty targets should return early.
- Callback error stops reading and can implement find/cancellation/limits.
- Callback same-time order is not VCD source-command order.
- Full time-table loading is unacceptable for normal operation.
- Straight truncation returned errors, but source review found panic/todo/assert paths requiring hardening.

## 5. Provisional targets

Targets are gates, not promises. Revise only with evidence.

| Benchmark | Target |
|---|---|
| Supplied FST header/open | <50 ms, <32 MiB RSS before hierarchy catalog |
| Supplied hierarchy/catalog | <500 ms; peak RSS recorded and accepted |
| Reused list/filter after open | <50 ms median for representative filter |
| Narrow one-signal window after catalog | <100 ms early/middle/late |
| Full-range selected clock | no material regression from ~0.74 s read |
| Metadata after open | <5 ms; no body scan/time table |
| Normal open/query | never load complete time table |
| VCD regression | no unexplained >10% regression; outputs identical |
| Malformed/resource cases | bounded failure without process panic |

## 6. Required benchmark matrix

| ID | Area | Cases | Measures |
|---|---|---|---|
| FST-B01 | Open | cold/warm, 5 runs | elapsed, CPU, RSS, section/header counts |
| FST-B02 | Catalog | retained arena/shared names | elapsed, RSS, bytes/declaration |
| FST-B03 | List | full, common/absent filters | elapsed, RSS, output hash |
| FST-B04 | Metadata | first/repeated | latency, scan/table usage |
| FST-B05 | Extract target count | 1/10/100 handles | callbacks, rows, latency, RSS, hash |
| FST-B06 | Extract window | early/middle/late/full | latency, sections/work, hash |
| FST-B07 | Values | scalar/vector/wide/XZ/real/string | fidelity and timing |
| FST-B08 | Find | early/late/absent | latency, callbacks/work |
| FST-B09 | Toggle | low/high-frequency | latency, count hash |
| FST-B10 | Compare | all four format combinations | elapsed, RSS, result hash |
| FST-B11 | Reuse | 1/10/100 sequential queries | total and percentile summary |
| FST-B12 | Concurrency | 1/2/4 workers | throughput, latency, RSS, threads |
| FST-B13 | Server | native and Unix wheel | startup, describe, list/extract, cancel |
| FST-B14 | Robustness | truncation/corrupt/deep/gzip/limits | failure time and RSS |

## 7. Reproducibility requirements

Every final artifact records:

- commit and clean/dirty state;
- package and `fst-reader` versions;
- dataset size, SHA-256, producer/provenance;
- OS, CPU, RAM, storage/filesystem;
- Rust/build profile/features;
- exact command;
- cache policy;
- samples and summary calculation;
- output/result hash;
- exit/error category;
- peak RSS and elapsed/user/system time.

Do not check the 61 MiB external sample into the repository without owner/license approval. CI uses tiny committed fixtures; scheduled/manual benchmarks may use the external sample after verifying its hash.

## 8. Release evidence

Before release:

- run at least five matched samples for sub-second operations;
- compare outputs against tiny semantic or equivalent VCD/FST oracles;
- install and test native archives and wheels rather than only testing source builds;
- include FST native and pip-server smoke;
- report malformed/fuzz resource results;
- verify no complete-time-table allocation in normal paths.
