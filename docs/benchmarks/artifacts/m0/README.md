# M0 benchmark artifacts

These TSV files are raw `scripts/bench-vcd.sh` v2 records captured from the release build on the two external datasets documented in `../../reusable-query-baseline.md`.

- `A-*`: `cp1_cp2_payload_006/tb.vcd`
- `B-*`: `calibrate_rcosc33/tb.vcd`
- `open`: five complete signal-catalog output runs
- `list`, `find`, `extract`, `compare`, `batched`: one bounded/cheap run

Each TSV contains the exact command, complete primary and secondary input SHA-256, complete stdout/stderr SHA-256, output sizes, environment, filesystem/storage, timing, RSS, I/O, and exit status. The large command stdout files are intentionally not checked in; their complete hashes are the durable equivalence artifacts.

Full unbounded metadata and one-signal toggle evidence predates the v2 raw artifacts and is preserved with complete hashes in the baseline document.
