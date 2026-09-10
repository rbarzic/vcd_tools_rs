# Reusable Query Baseline and Performance Gates

Status: **PARTIAL BASELINE**  
Plan: **VCD-RQ-001**

## 1. Important caveat

The exploratory measurements used `./target/release/vcd_tools_rs`, which reported version `0.1.0`, while the reviewed checkout declares version `0.1.7` in `Cargo.toml`. These measurements establish scale and likely bottlenecks, but M0 must rebuild the current checkout and record an authoritative baseline before optimization work.

The Linux page cache was not cleared. Results represent a real warm/pressured workstation rather than a controlled cold-cache laboratory run. Full scans were nevertheless stable and mostly CPU-bound.

## 2. Environment recorded during evaluation

- OS: Linux 6.17 x86_64
- CPU: Intel Core i9-12900K, 24 logical CPUs
- RAM: 62 GiB
- Binary: `target/release/vcd_tools_rs`
- Reported binary version: `0.1.0`
- Checkout version: `0.1.7`
- Output directed to `/dev/null` where applicable
- Timing: GNU `/usr/bin/time`

## 3. Datasets

| Dataset ID | Path | Size | Signals | Timescale | Time range | Header/body boundary |
|---|---|---:|---:|---|---:|---:|
| VCD-A | `/home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/cp1_cp2_payload_006/tb.vcd` | 4,589,561,612 B / 4.27 GiB | 193,730 | 1 fs | 0–65,290,706,000,000 | 9,512,897 B |
| VCD-B | `/home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/calibrate_rcosc33/tb.vcd` | 1,031,850,745 B / 0.96 GiB | 193,650 | 1 fs | 0–4,495,146,875,000 | 9,510,121 B |

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

## 5. Interpretation

A header-only persistent process can save about 0.45 seconds per request and avoid repeated allocation churn. For full scans that is approximately:

- 1.3% of VCD-A one-signal toggle time;
- 5.3% of VCD-B one-signal toggle time.

The server becomes materially valuable when it:

- batches compatible targets into one scan;
- seeks using a validated sparse index;
- or reuses compact timelines for hot IDs.

An open file descriptor alone is not expected to improve CPU parsing time.

## 6. Authoritative baseline procedure

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

## 7. Exact exploratory command pattern

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

## 8. Required benchmark matrix

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

## 9. Provisional performance gates

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

## 10. Correctness before speed

Every optimized result is differentially compared to the streaming backend for:

- event count, values, aliases, duplicate targets;
- same-timestamp/body ordering;
- inclusive boundaries;
- find occurrence;
- toggle baseline;
- comparison behavior;
- errors and cancellation state.

A faster backend with any unexplained semantic mismatch fails the gate.

## 11. Regression reporting template

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
