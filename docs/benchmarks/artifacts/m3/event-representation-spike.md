# RQ-M3-T03 — compact timeline spike

**Date/environment:** 2026-02-20; Linux 6.17.0-29-generic x86_64; `rustc 1.93.1`; 65,577,852 KiB RAM. Page cache was not controlled. Repository sources were not modified; prototypes were written only under `/tmp`.

## Decision

**T03 gate: PASS (representation selected for a future bounded selective-cache implementation).**

Recommend a **per-`IdCode`, structure-of-arrays (SoA) timeline**:

```text
time:    Vec<u64>       // exact VCD timestamp; supports binary search
sequence: Vec<u64>      // monotonically increasing global body-value-change sequence
payload: Vec<[u64; 2]>  // u128 halves, f64 bits, or text-table ID
kind:    Vec<u8>        // scalar 0/1/X/Z, integer, float, text
text table/pool         // exact bytes; intern only with bounded/benefit-aware policy
```

This costs **33 logical allocated bytes/event before text-pool and allocator overhead** (8 + 8 + 16 + 1), versus 48 bytes/event for the architecture draft's natural Rust AoS `CompactEvent { u64, u64, enum }` layout on this target. Keep fixed `u64` sequence: it costs 8 B/event (32% of the 25-B SoA record without sequence), but directly supplies the required stable merge key and avoids variable-length/random-access complexity.

**G7/cache enablement: DEFER.** T03 is evidence for a representation, not cache policy. An implementation must still pass byte-budget accounting, concurrent single-flight/cancellation, differential query tests, and matched speed tests. Cache default remains `Off`. In particular, oversize timelines must be usable only transiently by the initiating request and must not be admitted when their complete accounted ownership exceeds the cache budget.

## Current behavior inspected

- `src/lib.rs:99-103`: `ChangeValue::{Integer(u128), Float(f64), Text(String)}`.
- `src/query/stream.rs:226-338`: selected commands preserve parser/body encounter order, exact `u64` current time, integers for fully known vectors up to 128 bits, and text for wide/XZ; real remains `f64` and string remains text. `SelectedChange` currently has no sequence field.
- `src/query/extract.rs:89-107`: one selected body command is expanded through all requested alias bindings; its value is cloned per pending alias row. Alias names are catalog-side `Arc<str>`.
- `src/query/extract.rs:15-23`: existing result-limit “logical bytes” are transport accounting, not actual cache ownership.
- `docs/design/reusable-query-architecture.md:244-274`: cache keys must be `IdCode`, names must not occur in events, cache merge is `(time, sequence)`, and oversize timelines are not admitted.
- Existing semantics tests require body order at repeated timestamps and requested-name order for aliases.

The current `ChangeValue::Float` comparison already uses `to_bits()` (`src/query/compare.rs:265`), so a cache must store `f64::to_bits`, not a normalized numeric/string form.

## Inputs

```text
4,589,561,612  /home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/cp1_cp2_payload_006/tb.vcd
SHA-256 8b5a3791ad8d4232e637fd16edba9618cd0370eb66d3d7e1d780f82531c8f4ac
1,031,850,745  /home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/calibrate_rcosc33/tb.vcd
SHA-256 9cdb6e87f05c5a20f30ac88cb73df7488bc6a7a2201a9b1bab415864388624f2
23,804,580  tests/waveform.vcd
SHA-256 cd7fc6a9f7291456aff24b87334adaf63b5a4d7738464ae9864809563561031e
751  tests/fixtures/query_semantics.vcd
SHA-256 7a8728cee756af096415f9d914f9e8ba5987198b7d3a48b6436c4ed144359796
```

External clock identifiers found in their declarations were `A"` (VCD-A) and `:"` (VCD-B), both named `cache_clk`. The bounded parser retained only the selected ID but scanned each supplied external VCD once per representation. A's configured cap was 2,000,000 events; the complete file contained only 1,258,570 selected clock events, so the run completed the file. B also completed the file.

## Prototype and representation results

Prototype source/binary: `/tmp/m3_event_spike.rs`, `/tmp/m3_event_spike`. It compared:

1. **AoS/interned Arc:** `Vec<Event { time:u64, sequence:u64, value: enum { Integer(u128), FloatBits(u64), Text(Arc<str>) } }`.
2. **Recommended SoA:** the five-part layout above; scalar 0/1/X/Z are tags and allocate no text.

On this x86-64 target:

```text
sizeof CompactValue-like enum = 32 B
sizeof AoS Event              = 48 B
sizeof Arc<str>               = 16 B
sizeof String                 = 24 B
SoA logical record            = 33 B
```

`cap_bytes` below is vector capacity, explaining its power-of-two slack; RSS reflects touched pages and process overhead.

| Input | Representation | Events | Exact record bytes/event | `cap_bytes` | Peak RSS | Elapsed |
|---|---|---:|---:|---:|---:|---:|
| synthetic mixed, 10M | AoS/Arc | 10,000,000 | 48 | 805,306,368 | 470,416 KiB | 0.51 s |
| synthetic mixed, 10M | SoA | 10,000,000 | 33 | 553,648,128 | 323,920 KiB | 0.25 s |
| VCD-B `cache_clk`, complete | AoS/Arc | 271,975 | 48 | 25,165,824 | 14,416 KiB | 2.97 s |
| VCD-B `cache_clk`, complete | SoA | 271,975 | 33 | 17,301,504 | 10,192 KiB | 3.01 s |
| VCD-A `cache_clk`, complete | AoS/Arc | 1,258,570 | 48 | 100,663,296 | 60,304 KiB | 13.31 s |
| VCD-A `cache_clk`, complete | SoA | 1,258,570 | 33 | 69,206,016 | 41,872 KiB | 12.99 s |

The SoA record is 31.25% smaller than AoS (33 vs 48 B/event). Synthetic RSS was 31.1% lower and VCD-A clock-build RSS was 30.6% lower. Scan time on actual VCDs was effectively parser/I/O dominated, not worsened materially by SoA.

### Oversize-clock risk

The complete supplied VCD-A clock already needs 41,532,810 bytes by record formula (`1,258,570 * 33 = 41,532,810 B`, 39.61 MiB), excluding vector slack, cache entry, text table, allocator, and active-reader `Arc` retention. The 10M synthetic clock-like workload needs 330,000,000 B (314.7 MiB) exact records and reached 323,920 KiB RSS. Thus “one timeline” cannot be assumed small. Builders need incremental checked accounting/reservation, an event/count limit or budget abort path, and no admission when final owned capacity plus pool exceeds budget. Account **capacity**, pool/hash allocations, and retained evicted `Arc`s, not only `len * 33`.

## Fidelity and ordering

The small semantic fixture exercises scalar, known integer, 129-bit wide text, X/Z vector text, real, string, aliases, repeated timestamps, and same-time body order.

SoA fixture scan results:

```text
ID ! scalar/alias: events=6, text_distinct=0
ID " scalar X/Z:   events=6, text_distinct=0
ID # vec4:          events=4, text_distinct=1, text_bytes=5
ID $ wide129:       events=2, text_distinct=2, text_bytes=260
ID % real:          events=2, text_distinct=0
ID & string:        events=2, text_distinct=2, text_bytes=7
```

A k-way heap merge of per-ID fixture timelines by `(time, global_sequence)` exactly reproduced the 20-event source order:

```text
source_events 20 merged_events 20 equal True
time10 [(10, '!', '0'), (11, '"', 'x'), (12, '#', 'b10xz'),
        (14, '%', 'r1.5'), (15, '&', 'srun'), (16, '"', '1')]
alias_id_!_declarations=2 stored_timeline_events 6 expanded_rows 12
```

This demonstrates why timestamp-only merge is insufficient and why per-ID storage shares aliases: the `!` timeline is stored once (6 events) and expanded to 12 logical rows for two declarations/requests.

A separate fidelity assertion stored/reconstructed `u128::MAX` from two `u64` halves; round-tripped three exact f64 bit patterns, including negative zero and NaN payload `0x7ff8000000000042`; and retained text of lengths 4, 1,024, and 12:

```text
u128_roundtrip=true f64_bit_patterns=3 text_lengths=[4, 1024, 12]
```

## Text interning tradeoff

For one million 14-byte strings:

| Cardinality | Representation | Peak RSS | Distinct text bytes |
|---:|---|---:|---:|
| 4 | AoS/Arc interning | 48,404 KiB | 56 |
| 4 | SoA text IDs | 33,940 KiB | 56 |
| 1,000,000 | AoS/Arc interning | 242,368 KiB | 14,000,000 |
| 1,000,000 | SoA text IDs | 223,120 KiB | 14,000,000 |

Interning is strongly appropriate for scalar X/Z and repeated strings/wide states, but a hash interner is expensive for all-unique text. The prototype intentionally used a simple `HashMap<String,u32>` plus `Vec<String>` and therefore duplicates key ownership; those unique-text RSS numbers are an upper-bound warning, not a production pool design. Recommended policy:

- encode scalar 0/1/X/Z directly as tags (zero pool/hash work);
- use a timeline-local text table with exact bytes and integer IDs;
- bound interner bytes/entries and account hash/table capacity;
- consider “intern on second occurrence” or a bounded dedup table so unique long strings do not remain twice;
- never normalize case or text: preserve exact wide/XZ/string bytes.

## Exact commands and selected output

Discovery and fingerprints:

```sh
stat -c '%s %n' "$A" "$B" tests/waveform.vcd tests/fixtures/query_semantics.vcd
sha256sum "$A" "$B" tests/waveform.vcd tests/fixtures/query_semantics.vcd
./target/release/vcd_tools_rs list "$A" --filter cache_clk
./target/release/vcd_tools_rs list "$B" --filter cache_clk
grep -m3 '^\$var .* cache_clk ' "$A"
grep -m3 '^\$var .* cache_clk ' "$B"
```

Build and synthetic/RSS measurements:

```sh
rustc -O /tmp/m3_event_spike.rs -o /tmp/m3_event_spike
for mode in aos soa; do
  /usr/bin/time -f 'TIME mode=%C elapsed=%e user=%U sys=%S maxrss_kib=%M' \
    /tmp/m3_event_spike synth 10000000 "$mode"
done
for card in 4 1000000; do
  for mode in aos soa; do
    /usr/bin/time -f 'TIME elapsed=%e maxrss_kib=%M' \
      /tmp/m3_event_spike synthtext 1000000 "$card" "$mode"
  done
done
```

External supplied VCD clocks:

```sh
B=/home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/calibrate_rcosc33/tb.vcd
A=/home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/cp1_cp2_payload_006/tb.vcd
for mode in aos soa; do
  /usr/bin/time -f 'TIME dataset=B mode=%C elapsed=%e user=%U sys=%S maxrss_kib=%M' \
    /tmp/m3_event_spike "$B" ':"' 100000000 "$mode"
done
for mode in aos soa; do
  /usr/bin/time -f 'TIME dataset=A sample_limit=2000000 mode=%C elapsed=%e user=%U sys=%S maxrss_kib=%M' \
    /tmp/m3_event_spike "$A" 'A"' 2000000 "$mode"
done
```

Fixture type coverage:

```sh
for id in '!' '"' '#' '$' '%' '&'; do
  /tmp/m3_event_spike tests/fixtures/query_semantics.vcd "$id" 1000 soa
done
cargo test --test query_semantics alias_expansion_follows_requested_name_order -- --nocapture
cargo test --test query_semantics extraction_preserves_body_order_and_repeated_timestamp_changes -- --nocapture
```

Both tests passed (`1 passed; 0 failed` each). The merge check was a Python `heapq` merge of fixture per-ID arrays keyed by `(time, sequence)`; output is quoted above. Fidelity executable:

```sh
rustc -O /tmp/m3_fidelity.rs -o /tmp/m3_fidelity
/tmp/m3_fidelity
```

Final repository check:

```sh
git status --short
```

Output was empty before and after the read-only spike (the configured `.pi` artifact is ignored by Git).

## Limitations / follow-up gates

- The `/tmp` scanner is deliberately a bounded measurement prototype, not production VCD parsing; production must use the existing parser and differential tests.
- RSS runs were single samples with uncontrolled page cache. Representation size and ordering/fidelity conclusions are strong; performance acceptance needs repeated matched runs.
- Actual high-frequency evidence covers one clock from each supplied external VCD, plus synthetic 10M events; it is not a workload-distribution study.
- The SoA recommendation does not settle cache eviction, reservations, concurrent builders, cancellation, or promotion threshold.
- Before G7: add cache-vs-stream differential tests (windows, aliases, duplicate requests, repeated timestamps, decreasing timestamp rejection, malformed tails, real bit patterns, wide/XZ/string), exact ownership accounting tests, oversize non-admission tests, and at least three matched performance samples.
