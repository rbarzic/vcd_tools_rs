# FST Support Rollout

Status: **DRAFT**  
Plan: **FST-001**

## Release principles

- Preserve VCD behavior before adding FST surface area.
- Qualify parser robustness before accepting untrusted FST input.
- Build FST as an additive backend, not a rewrite of `OpenedVcd`.
- Keep FST native indexing independent from the VCD sidecar.
- Release from one qualified commit/version; do not use publication workflows as tests.
- Keep rollback possible by disabling FST autodetection/backend selection.

## Stage A — Parser/contract preview

Ships only if useful to developers; not advertised as general support:

- `fst-reader` dependency and notices;
- tiny fixtures and semantic contract;
- `OpenedFst`/catalog behind internal or experimental API;
- malformed/panic guards;
- supplied-file benchmark.

No CLI/Python/server FST claim.

## Stage B — Rust generic API

- `WaveformFormat`, `OpenedWaveform`, generic metadata/catalog/query operations;
- VCD-specific APIs unchanged;
- FST list/metadata/extract/find/toggle;
- equal-timescale comparison matrix;
- format detection and explicit override;
- documented supported FST subset.

## Stage C — CLI, Python, server beta

- Native CLI auto-detects FST.
- Existing Python query functions accept FST without signature changes.
- Add reusable Python `Waveform` object for explicit format/open reuse.
- Native and Unix pip server accept FST and report `source_format`.
- Protocol query schemas remain v1-compatible and gain additive describe fields.
- `vcd2trace` stays VCD-only.

## Stage D — Hardened release

Required before stable FST claim:

- parser fuzz/corrupt/resource gates;
- no reachable supported-input panic/todo path;
- open/query memory caps;
- equivalent VCD/FST comparisons;
- supplied-file benchmark report;
- native archive and wheel install/smoke on every target;
- BSD-3-Clause notice, SBOM, dependency/license/vulnerability checks;
- coordinated qualification before tag.

## Packaging

`fst-reader` is pure Rust and passed source checks for current targets. Required artifact checks:

- Linux x64/ARM64 native archives;
- macOS Intel/ARM64 native archives;
- Windows x64 native archive for non-server queries;
- Linux x64/ARM64 wheels;
- macOS Intel/ARM64 wheels;
- Windows x64 wheel;
- source distribution;
- Unix wheel `vcd_tools_rs serve` with FST;
- Windows wheel import/query with no Unix `serve`.

Publication jobs must depend on a reusable qualification workflow or consume artifacts produced by it. GitHub release and PyPI must not independently publish after different test outcomes.

## Documentation checklist

- supported FST subset and producer caveats;
- exact value/ordering/window mapping;
- content detection and override flags;
- server describe format fields;
- mixed-timescale compare rejection;
- gzip/incomplete FST policy;
- cancellation granularity;
- malformed-input beta/trust limitations if fuzz gate is incomplete;
- dependency license notice;
- supplied benchmark provenance and results.

## Rollback

- Preserve explicit VCD selection and all VCD-specific APIs.
- Disable FST autodetection through a release/config feature if a backend defect appears.
- Server fails FST startup rather than falling back to the wrong parser.
- No FST sidecar or persistent cache is authoritative.
- Existing VCD releases remain installable.

## Release checklist

- [ ] FST-G0 through FST-G4 passed.
- [ ] FST-G5 hardening passed or release clearly marked experimental with owner approval.
- [ ] All VCD compatibility tests green.
- [ ] Tiny FST fixtures have provenance, hashes, and regeneration.
- [ ] Supplied FST hash verified before benchmarks.
- [ ] Parser panic/corrupt/gzip cases bounded.
- [ ] Four compare combinations pass.
- [ ] Native and wheel artifacts installed and smoke-tested.
- [ ] Unix pip server queries FST.
- [ ] Windows FST query API works without Unix server.
- [ ] Notices/SBOM/audit complete.
- [ ] Version and release notes updated.
- [ ] Qualification completed before tag and publish.
