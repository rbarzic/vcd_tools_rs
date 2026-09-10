# M2 targeted-conversion evidence

Status: RQ-M2-T02 review evidence; G3 remains open.

Raw artifact: [`targeted-conversion.tsv`](targeted-conversion.tsv)

## Reproduction

```sh
TARGETED_RUNS=5 \
TARGETED_TOTAL_SIGNALS=128 \
TARGETED_CYCLES=1000 \
  scripts/measure-targeted-conversion.sh \
  > docs/benchmarks/artifacts/m2/targeted-conversion.tsv
```

The script builds the optimized library test executable and invokes the ignored
`query::stream::tests::probe_targeted_conversion` diagnostic for 1, 10, and 100
selected signals. Each invocation deterministically generates a temporary VCD
with:

- 128 scalar signals;
- 1,000 timestamps;
- one change per signal per timestamp;
- 128,000 value changes;
- 129,000 parsed commands including timestamps.

The raw TSV records the exact command template, generator parameters, Rust/host
information, base commit, dirty status, external GNU time/RSS, internal decoder
elapsed time, command count, and conversion count. The worktree is intentionally
recorded as dirty because T02 had not yet been committed.

## Results

| Selected IDs | Expected/actual conversions | Median internal decode | Range |
|---:|---:|---:|---:|
| 1 | 1,000 / 1,000 | 4,578 µs | 4,031–8,151 µs |
| 10 | 10,000 / 10,000 | 4,624 µs | 4,565–8,778 µs |
| 100 | 100,000 / 100,000 | 12,580 µs | 9,690–17,916 µs |

Every run parsed the same 129,000 commands. Conversion count was exactly
`selected IDs × 1,000` and never included any of the unrelated IDs. Moving from
1 to 10 selected IDs had effectively unchanged median decode time. Selecting
100 IDs increased median time by about 2.75× while performing 100× as many
conversions; this is expected selected-value work rather than a scan-time or
membership regression.

This diagnostic measures conversion counts and elapsed/RSS process metrics. It
does **not** instrument allocator calls or bytes allocated, so it makes no
allocator-specific claim. T02's deterministic conversion-counter tests are the
source of truth for selected-only conversion behavior.
