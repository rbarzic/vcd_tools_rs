# Reusable Query Baseline and Performance Gates

Status: **M0 BASELINE RECORDED; FULL OPTIMIZATION MATRIX PENDING IMPLEMENTATION**
Plan: **VCD-RQ-001**

## 1. Baseline status and caveats

M0 rebuilt the current checkout with `cargo build --release` and verified `vcd_tools_rs 0.1.7`. The fresh measurements in §5 are the compatibility/performance baseline for the current implementation. The worktree was dirty only because M0 documentation, fixtures, tests, and benchmark scripts were uncommitted; no `src/` file changed from commit `02decd790615af5ce7f7d21550c00e5ca5098ab5`.

The earlier exploratory measurements used a stale binary reporting `0.1.0`; they remain in §4 only as corroborating repeated-run context and are not the authoritative version baseline.

The Linux page cache was not dropped. Results represent a warm/pressured workstation and the harness records this as `cache_policy=uncontrolled`. Full scans were mostly CPU-bound. To keep the first implementation pass bounded, fresh full metadata and toggle scans were run once per external VCD; the earlier repeated measurements show their variance, but future optimization gate comparisons must take at least three candidate/baseline samples in the same controlled session.

## 2. Environment recorded during M0

- OS: Linux 6.17.0-29-generic x86_64
- CPU: Intel Core i9-12900K, 24 logical CPUs
- RAM: 65,577,852 KiB reported by `/proc/meminfo`
- Rust: `rustc 1.93.1 (01f6ddf75 2026-02-11)`
- Cargo: `cargo 1.93.1 (083ac5135 2025-12-15)`
- Binary: `target/release/vcd_tools_rs`
- Binary/package version: `0.1.7`
- Baseline commit: `02decd790615af5ce7f7d21550c00e5ca5098ab5`
- Timing: GNU `/usr/bin/time`
- Harness: `scripts/bench-vcd.sh`; durable TSV records contain complete source/output hashes and per-run resource metrics
- Filesystem: ext4 mounted from `/dev/nvme0n1p2`
- Storage: non-rotational NVMe; backing device reported as WD_BLACK SN7100 2TB

## 3. Datasets

| Dataset ID | Path | Size | SHA-256 | Signals | Timescale | Time range | Header/body boundary |
|---|---|---:|---|---:|---|---:|---:|
| VCD-A | `/home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/cp1_cp2_payload_006/tb.vcd` | 4,589,561,612 B / 4.27 GiB | `8b5a3791ad8d4232e637fd16edba9618cd0370eb66d3d7e1d780f82531c8f4ac` | 193,730 | 1 fs | 0–65,290,706,000,000 | 9,512,897 B |
| VCD-B | `/home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/calibrate_rcosc33/tb.vcd` | 1,031,850,745 B / 0.96 GiB | `9cdb6e87f05c5a20f30ac88cb73df7488bc6a7a2201a9b1bab415864388624f2` | 193,650 | 1 fs | 0–4,495,146,875,000 | 9,510,121 B |

The body-size/time-span proxy is approximately 70.2 MB per 10^12 fs for VCD-A and 227.4 MB per 10^12 fs for VCD-B. This is not true event density: the current binary does not count body commands without changing the measured work, so event count is recorded as unavailable in M0 rather than estimated misleadingly.

The repository fixture `tests/waveform.vcd` is used for repeatable correctness and large-header tests. Large external VCDs must not become mandatory ordinary-CI inputs.

## 4. Exploratory results

### Separate invocations

| Query | VCD-A runs | Median | VCD-B runs | Median |
|---|---|---:|---|---:|
| `list --filter clk` | 0.45, 0.44, 0.45 s | 0.45 s | 0.44, 0.46, 0.46 s | 0.46 s |
| Early `find`, first occurrence | 0.53, 0.53, 0.54 s | 0.53 s | 0.53, 0.54, 0.53 s | 0.53 s |
| Full one-signal `toggle` | 35.36, 35.32, 35.38 s | 35.36 s | 8.41, 8.43, 8.43 s | 8.43 s |

### Full-scan resources

| Dataset | Operation | Elapsed | User CPU | System CPU | Peak RSS |
|---|---|---:|---:|---:|---:|
| VCD-A | one-signal toggle | 35.36 s median | 34.40 s | 0.86 s | ~425,000 KiB |
| VCD-B | one-signal toggle | 8.43 s median | 8.14 s | 0.28 s | ~425,000 KiB |
| VCD-A | metadata | 26.13 s | 25.26 s | 0.81 s | 407,288 KiB |
| VCD-B | metadata | 6.21 s | 5.94 s | 0.27 s | 407,068 KiB |

### Batching

| Dataset | One scan for 3 signals | 3 separate one-signal scans | Speedup |
|---|---:|---:|---:|
| VCD-A | 35.81 s | 106.08 s | 2.96× |
| VCD-B | 8.51 s | 25.29 s | 2.97× |

Adding two targets to the scan cost only 0.45 s on VCD-A and 0.08 s on VCD-B. Parsing the body, not handling selected signals, is the dominant cost.

## 5. Fresh M0 release baseline

Commands were run through `scripts/bench-vcd.sh` with the external paths from §3. The strengthened `open` operation now emits the complete, non-empty signal catalog, so its checksum covers both parsing and declaration-order output. Complete raw records are checked in under `docs/benchmarks/artifacts/m0/`.

```sh
cargo build --release
./target/release/vcd_tools_rs --version
BENCH_RUNS=5 scripts/bench-vcd.sh "$A" open
BENCH_RUNS=5 scripts/bench-vcd.sh "$B" open
scripts/bench-vcd.sh "$A" list --filter cache_clk
scripts/bench-vcd.sh "$B" list --filter cache_clk
scripts/bench-vcd.sh "$A" find --signal tb.U_ALDEBARAN_MONITOR.cache_clk --value 1 --occurrence 1
scripts/bench-vcd.sh "$B" find --signal tb.U_ALDEBARAN_MONITOR.cache_clk --value 1 --occurrence 1
scripts/bench-vcd.sh "$A" extract --signal tb.U_ALDEBARAN_MONITOR.cache_clk --end 223151152000
scripts/bench-vcd.sh "$B" extract --signal tb.U_ALDEBARAN_MONITOR.cache_clk --end 223151152000
scripts/bench-vcd.sh "$A" compare "$A" --signals-only tb.U_ALDEBARAN_MONITOR.cache_clk --end 223151152000 --output compact
scripts/bench-vcd.sh "$B" compare "$B" --signals-only tb.U_ALDEBARAN_MONITOR.cache_clk --end 223151152000 --output compact
scripts/bench-vcd.sh "$A" toggle --signal tb.U_ALDEBARAN_MONITOR.cache_clk --signal tb.U_ALDEBARAN_MONITOR.clk --signal tb.U_CHIP.clk_fpga_in --end 223151152000
scripts/bench-vcd.sh "$B" toggle --signal tb.U_ALDEBARAN_MONITOR.cache_clk --signal tb.U_ALDEBARAN_MONITOR.clk --signal tb.U_CHIP.clk_fpga_in --end 223151152000
```

The full metadata and unbounded one-signal toggle commands shown below were captured with the previous v1 harness in the same M0 worktree. Their complete output hashes remain valid and are preserved here; the new v2 artifacts add complete dataset fingerprints and cheap-operation coverage.

### Header/catalog open

| Dataset | Elapsed samples | Median | Peak RSS range | Output bytes | Output SHA-256 |
|---|---|---:|---:|---:|---|
| VCD-A | 0.53, 0.55, 0.52, 0.53, 0.53 s | 0.53 s | 420,860–421,448 KiB | 16,622,292 | `5f4ed97876b72f2e96bce53d66817aaa243e9a9017613b57dbde926cb097ec06` |
| VCD-B | 0.50, 0.50, 0.51, 0.51, 0.50 s | 0.50 s | 420,704–421,372 KiB | 16,618,959 | `0bf0ceba4933d5c9f0cb34a45676988c4533b616fcd6ed25bb7861d4ffb5f318` |

### Cheap query and bounded batch baselines

All output hashes below are complete SHA-256 values; each command exited zero.

| Dataset | Operation | Elapsed | Peak RSS | Output bytes | Output SHA-256 |
|---|---|---:|---:|---:|---|
| VCD-A | filtered list | 0.44 s | 421,588 KiB | 33 | `903dd62e7781c84e249b3c304aa6821a9edbf7a18509e88ef63f39ee5a56be7a` |
| VCD-B | filtered list | 0.44 s | 421,288 KiB | 33 | `903dd62e7781c84e249b3c304aa6821a9edbf7a18509e88ef63f39ee5a56be7a` |
| VCD-A | early find | 0.54 s | 425,112 KiB | 53 | `a343865c5580a468cce0a588ff149946ad974d96344237cf0cd6fe421adab90b` |
| VCD-B | early find | 0.51 s | 425,044 KiB | 53 | `a343865c5580a468cce0a588ff149946ad974d96344237cf0cd6fe421adab90b` |
| VCD-A | bounded extract | 0.51 s | 425,248 KiB | 67 | `251ed8e3c25bbcea98c40894a7157f1877b152c1471d557d59586ad9e454605c` |
| VCD-B | bounded extract | 0.53 s | 425,096 KiB | 67 | `251ed8e3c25bbcea98c40894a7157f1877b152c1471d557d59586ad9e454605c` |
| VCD-A | bounded self-compare | 1.26 s | 869,292 KiB | 35 | `abbd813262733922b9c70d8d659e5d20597aafc0fc7923594d89f96cc19b4a28` |
| VCD-B | bounded self-compare | 1.24 s | 869,240 KiB | 35 | `426eec48b81bdb66af9781b0f087b62c6ffc017e2a0338dbfaa962289fcde373` |
| VCD-A | bounded three-signal toggle | 0.54 s | 424,748 KiB | 103 | `4dcba795d971cb1a3e0e62f1674cfae9849565d6387bb9c947ddff670edd16ef` |
| VCD-B | bounded three-signal toggle | 0.51 s | 424,964 KiB | 103 | `4dcba795d971cb1a3e0e62f1674cfae9849565d6387bb9c947ddff670edd16ef` |

### Full scans

| Dataset | Operation | Elapsed | User | System | Peak RSS | Output bytes | Output SHA-256 |
|---|---|---:|---:|---:|---:|---:|---|
| VCD-A | metadata | 26.77 s | 25.82 s | 0.94 s | 407,280 KiB | 67 | `6a702f95f2ec3f4535fce7101b131a38d368300dbe263028c75f1212c3a408ac` |
| VCD-B | metadata | 6.36 s | 6.09 s | 0.26 s | 407,204 KiB | 66 | `b2abfd7c0de1abda1da7e8088ee69b1170c14013205ee02bc738ffc5b95f556d` |
| VCD-A | toggle `tb.U_ALDEBARAN_MONITOR.cache_clk` | 36.58 s | 35.52 s | 1.03 s | 425,364 KiB | 56 | `520b9e99daec66b2d6db455956c0c126d4f4da6bd466983fc789b76f5d62a4ab` |
| VCD-B | toggle `tb.U_ALDEBARAN_MONITOR.cache_clk` | 8.73 s | 8.43 s | 0.29 s | 425,220 KiB | 55 | `5eb722848bda669403c2a250bb9b64ec736f0a9900dfbd7f0bba8e1eec31efd9` |

Every recorded command exited zero. The durable v2 TSV records under `docs/benchmarks/artifacts/m0/` contain exact commands, complete primary and compare-secondary VCD fingerprints, complete stdout/stderr hashes, filesystem/storage information, output bytes, and GNU time/RSS/I/O metrics.

### Deliberately deferred baseline cases

M0 did not repeat every expensive BM-04 through BM-15 combination. Those cases depend on implementations that do not exist yet (reusable open, sparse sidecar, cache, server, and persistent Python iterator), or would add several minutes of duplicate current full scans without changing the initial architectural gate. Before the corresponding optimization gate, run matched repeated baseline/candidate samples for that benchmark ID. Compare and extraction semantics are protected now by small exact fixtures and output checksums; large-file performance samples are added when those paths are modified.

## 6. M1 compact-catalog diagnostic (preview, not G2 acceptance)

The reproducible helper below runs compact-only parsing and legacy compatibility materialization in separate optimized test processes, allowing GNU `time` to report actual process peak RSS rather than only an ownership estimate:

```sh
scripts/measure-catalog-memory.sh "$A"
```

Four alternating warm measurements per mode on VCD-A after the M1 T01–T03 corrective pass:

| Mode | Elapsed range | Peak RSS range | Signals | Estimated retained catalog bytes |
|---|---:|---:|---:|---:|
| Compact only | 0.17–0.20 s | 78,700–78,936 KiB | 193,730 | 36,562,701 (188.73/signal) |
| Legacy compatibility materialization | 0.71–0.77 s | 393,008–393,088 KiB | 193,730 | Not used; includes owned signals and public clone-heavy maps |

The complete eight samples, command template, commit/dirty state, VCD SHA-256, environment, filesystem, and cache policy are stored in [`artifacts/m1/catalog-memory-A.tsv`](artifacts/m1/catalog-memory-A.tsv). The helper accepts `CATALOG_RUNS` and emits the durable TSV schema directly.

This shows the reusable compact representation can reduce peak process RSS by about 80% when consumers avoid legacy materialization. It does **not** pass G2 by itself: existing path APIs still require compatibility conversion and remain slower than the M0 list baseline. T04–T08 must measure real `OpenedVcd` access, resolve or accept the compatibility latency regression, and complete snapshot/concurrency evidence.

## 7. Interpretation

A header-only persistent process can save about 0.45 seconds per request and avoid repeated allocation churn. For full scans that is approximately:

- 1.3% of VCD-A one-signal toggle time;
- 5.3% of VCD-B one-signal toggle time.

The server becomes materially valuable when it:

- batches compatible targets into one scan;
- seeks using a validated sparse index;
- or reuses compact timelines for hot IDs.

An open file descriptor alone is not expected to improve CPU parsing time.

## 8. Authoritative baseline procedure

M0 must run a freshly built checkout:

```sh
cargo clean
cargo build --release
./target/release/vcd_tools_rs --version
cargo test
```

Record for every run:

- commit SHA and dirty status;
- package/binary version;
- `rustc -Vv` and Cargo version;
- optimization profile and features;
- CPU, RAM, OS/kernel;
- filesystem and storage device type;
- source size, signal count, time range, and approximate event density;
- cache state policy: uncontrolled warm, warmed by a defined pass, or cold where safely available;
- exact command;
- wall/user/system time;
- peak RSS;
- bytes read where available;
- exit status and output checksum.

Use at least five measured repetitions after one setup run for sub-second operations. Use at least three for multi-second large scans unless cost is prohibitive and documented.

## 9. Exact exploratory command pattern

```sh
BIN=./target/release/vcd_tools_rs
A=/home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/cp1_cp2_payload_006/tb.vcd
B=/home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/calibrate_rcosc33/tb.vcd
export LC_ALL=C

/usr/bin/time -f \
  'elapsed=%e user=%U sys=%S rss_kb=%M fsin=%I fsout=%O exit=%x' \
  "$BIN" list "$A" --filter clk >/dev/null

/usr/bin/time -f \
  'elapsed=%e user=%U sys=%S rss_kb=%M fsin=%I fsout=%O exit=%x' \
  "$BIN" find "$A" \
    --signal tb.U_ALDEBARAN_MONITOR.cache_clk \
    --value 1 --occurrence 1 >/dev/null

timeout 120s /usr/bin/time -f \
  'elapsed=%e user=%U sys=%S rss_kb=%M fsin=%I fsout=%O exit=%x' \
  "$BIN" toggle "$A" \
    --signal tb.U_ALDEBARAN_MONITOR.cache_clk >/dev/null

timeout 120s /usr/bin/time -f \
  'elapsed=%e user=%U sys=%S rss_kb=%M fsin=%I fsout=%O exit=%x' \
  "$BIN" toggle "$A" \
    --signal tb.U_ALDEBARAN_MONITOR.cache_clk \
    --signal tb.U_ALDEBARAN_MONITOR.clk \
    --signal tb.U_CHIP.clk_fpga_in >/dev/null

timeout 120s /usr/bin/time -f \
  'elapsed=%e user=%U sys=%S rss_kb=%M fsin=%I fsout=%O exit=%x' \
  "$BIN" meta "$A" >/dev/null
```

Run equivalent commands on VCD-B.

## 10. Required benchmark matrix

| ID | Scenario | Variants | Measurements |
|---|---|---|---|
| BM-01 | Header/catalog open | repository fixture, VCD-A/B | wall, allocations, peak/incremental RSS, bytes per signal |
| BM-02 | Reused list/lookup | no filter, common/absent filters | p50/p95, allocations |
| BM-03 | Metadata first/warm | stream, sidecar | wall, scan throughput, bytes read |
| BM-04 | Extract position | early, middle, final 10%, absent | first-row, total wall, commands/bytes scanned |
| BM-05 | Target count | 1, 10, 100, 1000 | throughput, conversion/allocation count |
| BM-06 | Value types | scalar, integer vector, X/Z vector, real, string, wide vector | throughput, memory |
| BM-07 | Toggle/find | early/late/absent and bounded/unbounded end | wall and scanned prefix |
| BM-08 | Sparse sidecar build | several strides | build wall/RSS, index bytes, checkpoints |
| BM-09 | Sparse query | exact boundary and final 10% | speedup, equivalence, bytes skipped |
| BM-10 | Selective cache build/hit | one/hot set/aliases/oversize timeline | build wall, hit latency, retained bytes |
| BM-11 | Concurrent engine | 1/4/16 readers | throughput, p50/p95/p99, RSS |
| BM-12 | Server load | 1/4/16/64 clients | latency, queue, FD/thread count, errors |
| BM-13 | Slow/disconnected client | varying receive rate | memory, worker occupancy, cancellation latency |
| BM-14 | Python iterator | small/large result | time-to-first-row, peak RSS, GIL behavior |
| BM-15 | Existing CLI regression | list/meta/extract/find/toggle/compare/vcd2trace | wall and output checksum |

## 11. Provisional performance gates

These are release gates, not promises. Revisions require measurements and rationale in the implementation plan.

| Target | Gate |
|---|---|
| Compact catalog | At least 40% lower incremental peak RSS for header/catalog work, or explicitly approved exception |
| Reused list | At least 10× faster median than path-based reparse on large-header fixture |
| Streaming backend | No unexplained >10% regression in existing cold one-shot operations |
| Lazy metadata | No more than 10% slower than current metadata full scan before caching |
| Warm metadata | At least 20× faster than full rescan |
| Sparse tail query | At least 5× faster median for representative final-10% query |
| Sidecar size | At most 0.5% of source under selected default stride |
| Sidecar build | At most 15% slower than metadata scan unless justified |
| Hot timeline hit | At least 10× faster than full scan |
| Cache budget | Accounted ownership never exceeds configured limit after admission/eviction; RSS tolerance documented |
| Server resources | No unbounded queue, RSS, FD, worker, or task growth under overload/slow client |
| Cancellation | Bounded latency demonstrated during scan, wait, and output backpressure |
| Python iterator | Memory bounded by configured buffering rather than total result rows |

## 12. Correctness before speed

Every optimized result is differentially compared to the streaming backend for:

- event count, values, aliases, duplicate targets;
- same-timestamp/body ordering;
- inclusive boundaries;
- find occurrence;
- toggle baseline;
- comparison behavior;
- errors and cancellation state.

A faster backend with any unexplained semantic mismatch fails the gate.

## 13. Regression reporting template

```text
Benchmark ID:
Commit/base commit:
Dataset and fingerprint:
Build/features:
Environment:
Command:
Samples:
Baseline median/p95:
Candidate median/p95:
Delta:
Peak RSS delta:
Output/equivalence result:
Interpretation:
Gate result: PASS / FAIL / WAIVED
Waiver owner and rationale:
```
