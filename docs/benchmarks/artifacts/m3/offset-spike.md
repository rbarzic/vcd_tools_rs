# M3 snapshot-reader and sparse-offset spike

Tasks: `RQ-M3-T01`, `RQ-M3-T02`  
Repository commit inspected: `aa396a707514c1771d3f63376ae96c50467edb10`  
Repository modifications: **none** (`git status --short` remained empty)  
Recommendation: **PASS WITH REQUIRED GUARDS** for sparse sidecar; no positioned-I/O adapter required for the current reader design.

## Executive result

A fresh `vcd::Parser<BufReader<File>>` can resume at a timestamp checkpoint recorded from `parser.reader().stream_position()` before `next()`. Command kind, IDs, values, timestamps, multiplicity, and order matched the original parser suffix across:

- every timestamp in all applicable semantic fixtures;
- repeated and decreasing timestamps;
- CRLF and unusual `$enddefinitions` formatting;
- comments and dump-control blocks;
- a generated timestamp crossing an 8 KiB buffer boundary;
- five checkpoints over the first 64 MiB of each supplied large VCD body;
- several fresh `BufReader` capacities, deliberately different from the builder.

Two constraints are mandatory:

1. **Only checkpoint timestamps outside `$dump* ... $end` simulation-command blocks.** `vcd` 0.7 carries private `simulation_command` parser state. A deliberately noncanonical timestamp inside `$dumpvars` parses in the original stream but a fresh parser resumed there later reports `UnmatchedEnd`. The index builder must track emitted `Command::Begin`/`Command::End` and never checkpoint while a simulation command is active.
2. **Sparse timestamp lookup is valid only for non-decreasing timestamp files.** The project deliberately accepts decreasing timestamps. The sidecar builder must detect any decrease during its full build scan and mark sparse seeking unavailable for that generation, falling back to body-start streaming. Do not sort such checkpoints or binary-search them.

Repeated timestamps also require retaining/selecting the **earliest checkpoint for an equal timestamp run** so same-time changes are not skipped.

## RQ-M3-T01 — snapshot-reader findings

### Current mechanism

`OpenedVcd` does not clone a mutable file cursor. Each query:

1. retains the caller's configured path as an absolute path without resolving the final symlink;
2. opens a fresh `File`;
3. validates metadata/platform evidence plus complete raw-header and bounded content hashes;
4. seeks the independent handle to `body_offset`;
5. wraps it in an independent `BufReader`/parser;
6. validates both the admitted handle and a fresh configured-path reopen before result publication.

This avoids shared seek offsets and preserves configured-symlink retarget detection. Existing tests cover eight concurrent independent parsers, cursor independence, append/truncate/replacement, configured symlink retargeting, and completion validation.

### Existing cross-target evidence

The clean G2 evidence records successful:

```sh
cargo check --all-targets --target x86_64-unknown-linux-gnu
cargo check --all-targets --target x86_64-pc-windows-msvc
cargo check --all-targets --target aarch64-unknown-linux-gnu
cargo check --all-targets --target x86_64-apple-darwin
cargo check --all-targets --target aarch64-apple-darwin
```

Source: `docs/benchmarks/artifacts/m1/g2-validation.md` and `target-checks.md`.

### Platform assumptions and limitations

- Unix identity additionally uses stable device/inode metadata.
- Stable Rust did not expose the desired by-handle Windows volume/file index APIs for this implementation. Windows/non-Unix validation therefore relies on length, mtime, complete raw-header BLAKE3, beginning/end samples, and optional strict full-content BLAKE3.
- Target checks prove source/type compatibility, not native Windows/macOS filesystem runtime behavior. Native runtime mutation/replacement tests remain a release-CI obligation under CR-002.
- Default bounded fingerprinting is best-effort for adversarial in-place changes in the middle of a body when length and mtime are deliberately preserved. Strict full-content mode covers that case at full-scan cost.
- For sidecar use, acquire an independently validated `OpenedBodyReader`, then seek that reader to the accepted checkpoint. Reopening an unvalidated raw path at an offset would bypass generation guarantees.

**T01 recommendation: PASS.** The current reopen-and-validate design is sufficient; a positioned-I/O reader is not required before server work. Keep native Windows/macOS replacement tests at G8.

## RQ-M3-T02 — executable offset experiment

### Prototype

A standalone temporary Rust crate was created under `/tmp/vcd-offset-spike` with only `vcd = "0.7"`. It did not modify the repository.

Artifacts at execution time:

```text
/tmp/vcd-offset-spike/Cargo.toml
  SHA-256 849fa2986d164216b98b63f0a9821b41b238927d86cd0aca4d6952fda7bb7705
/tmp/vcd-offset-spike/src/main.rs
  SHA-256 ffe7d280b91af88f9d45ebc4a7b81d913850f2a25893037e15dc83e2e1ceaed8
/tmp/vcd-offset-spike-output2.txt
  SHA-256 7a27a9ddb99afc1c8857ecd580aef0aaaa8c3e15b37b0289113dd6f8eec543da
```

Core experiment for each original parser command:

```rust
let logical_safe_offset = parser.reader().stream_position()?;
let command = parser.next();
if let Some(Ok(Command::Timestamp(t))) = command {
    // Also find the first non-parser-whitespace byte between the pre/post
    // positions and assert it is '#'.
    checkpoints.push((command_index, logical_safe_offset, exact_hash_offset, t));
}
```

For each checkpoint, the prototype opened a fresh file, sought to either the logical-safe offset or exact `#` offset, created a new parser, and compared the resulting `Vec<Command>` with the original suffix using `Command::PartialEq`. This compares every command variant's complete value, not only timestamps.

Command used:

```sh
cd /tmp/vcd-offset-spike
cargo run --release -- \
  /home/roba/work/github/vcd_tools_rs \
  /home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/cp1_cp2_payload_006/tb.vcd \
  /home/roba/work/gitlab/icdesign/chips/polaris-hw/sim/ctests/calibrate_rcosc33/tb.vcd
```

### Small-fixture coverage

For each small fixture, the prototype compared:

- complete body parsing from `body_offset`;
- complete suffix from every timestamp's logical-safe offset;
- complete suffix from every timestamp's exact `#` offset;
- fresh buffer capacities `1`, `7`, `64`, `8192`, and `65536` bytes.

Results:

| Fixture | Commands | Timestamp checkpoints | Notes |
|---|---:|---:|---|
| `query_semantics.vcd` | 30 | 6 | vectors, X/Z, real, string, `$dumpvars`, repeated `#10` |
| `query_semantics_changed.vcd` | 30 | 6 | compare variant |
| `control_commands.vcd` | 17 | 3 | comments, dumpoff/dumpon/dumpall |
| `crlf_unusual_header.vcd` | 2 | 1 | CRLF, whitespace around enddefinitions |
| `decreasing_timestamps.vcd` | 8 | 4 | `0,10,5,12` body order retained |
| `dumpvars_no_timestamp.vcd` | 3 | 0 | body-start checkpoint covers it |
| `empty_body.vcd` | 0 | 0 | body-start checkpoint covers it |
| generated buffer fixture | 9 | 3 | repeated `#1`; first timestamp begins around byte 8190 |

All comparisons passed. The generated buffer fixture intentionally had parser whitespace between the logical pre-`next()` offset and exact `#`; both offsets resumed correctly. This proves that `stream_position()` is a **safe parser boundary**, but is not guaranteed to point exactly at `#`.

### Large-file bounded coverage

The prototype scanned one bounded prefix per supplied VCD, not the full files repeatedly:

- prefix span: first 64 MiB after `body_offset`;
- checkpoints: first timestamp at/after relative byte thresholds 0, 16, 32, 48, and 64 MiB;
- compared suffix: 2,048 complete commands per checkpoint;
- fresh reader capacities: 17, 8192, and 65536 bytes;
- separately compared the first 2,048 commands from `body_offset` to cover initial dump blocks.

#### VCD-A — `cp1_cp2_payload_006/tb.vcd`

Observed body offset: `9,512,918`.

| Relative threshold | Safe/exact offset | Timestamp | Compared commands |
|---:|---:|---:|---:|
| 0 | 11,282,895 | 0 | 2,048 |
| 16 MiB | 26,290,157 | 403,533,000,000 | 2,048 |
| 32 MiB | 43,067,464 | 956,856,000,000 | 2,048 |
| 48 MiB | 59,844,608 | 1,357,989,000,000 | 2,048 |
| 64 MiB | 76,621,815 | 1,818,369,272,000 | 2,048 |

#### VCD-B — `calibrate_rcosc33/tb.vcd`

Observed body offset: `9,510,142`.

| Relative threshold | Safe/exact offset | Timestamp | Compared commands |
|---:|---:|---:|---:|
| 0 | 11,281,429 | 0 | 2,048 |
| 16 MiB | 26,290,100 | 285,422,832,000 | 2,048 |
| 32 MiB | 43,070,178 | 362,984,679,000 | 2,048 |
| 48 MiB | 59,845,010 | 437,900,000,000 | 2,048 |
| 64 MiB | 76,623,904 | 509,328,536,000 | 2,048 |

All 30 large-checkpoint suffix comparisons passed, as did body-start comparisons. The scan was bounded to approximately the first 96 MiB after each body start (64 MiB threshold plus enough commands to complete captures), avoiding full-file scans.

### Deliberate parser-state counterexample

`vcd` 0.7 stores `simulation_command: Option<SimulationCommand>` privately. A temporary noncanonical body containing a timestamp before `$end` of `$dumpvars` was accepted by the original forward parser. Resuming fresh at that timestamp failed when the later `$end` was parsed:

```text
ParseError { kind: UnmatchedEnd }
```

This demonstrates that timestamp alone is not a sufficient checkpoint criterion. Builder-side `Begin`/`End` state tracking is required even though ordinary VCD producers place timestamps outside dump blocks.

### Required design changes before sidecar implementation

1. Redefine the stored offset as **parser-safe offset at or before the timestamp**, where any preceding bytes are only parser whitespace. This is exactly what `BufReader::stream_position()` before `next()` provides. If exact `#` offsets remain required, derive them with bounded raw-byte inspection and test both forms; exact offsets have no demonstrated query benefit.
2. Track `Command::Begin`/`Command::End`; never record a timestamp while a simulation command is active.
3. Detect decreasing timestamps during the one full build scan. For that generation, abort/disable sparse index creation and retain streaming fallback. Record a diagnostic reason rather than publishing a misleading sorted index.
4. Coalesce equal timestamps to their earliest safe offset. If a byte threshold is crossed at a later repeated timestamp, use the first offset in that equal-time run.
5. Keep mandatory `(BODY_START, body_offset)` independently of timestamp records.
6. Store and validate the actual parsed `body_offset`; do not rely on historical numbers because the source files can differ by generation.
7. Add the successful small fixtures, buffer-boundary fixture, and the timestamp-inside-dump negative fixture as permanent sidecar-offset tests when M5 starts.

## Recommendation

**PASS WITH REQUIRED GUARDS** (`PASS`, not `ALTERNATIVE` or `DEFER`).

The public `Parser::reader()` plus `BufReader::stream_position()` mechanism is sufficient for sparse resume. A custom byte-level VCD scanner is not warranted. Sparse indexing must be generation-validated and conditionally unavailable for decreasing timestamps, and checkpoint construction must exclude timestamps inside simulation-command blocks and preserve earliest equal-time offsets.

The spike proves parser suffix equivalence and the feasibility mechanism. It does **not** yet prove:

- full-file timestamp monotonicity of either supplied large VCD (only bounded prefixes were intentionally scanned);
- final stride choice, sidecar size, atomic publication, or query speedup;
- native Windows/macOS filesystem replacement behavior;
- future active-value-at-start semantics, which would require snapshots and a new format/flag.
