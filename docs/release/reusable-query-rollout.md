# Reusable Query and Server Rollout

Status: **DRAFT**  
Plan: **VCD-RQ-001**

## 1. Release principles

- Correctness and source-generation isolation precede performance.
- Existing one-shot CLI and Python functions remain available throughout rollout.
- Streaming backend is permanent and is the fallback/correctness oracle.
- Sidecars and event caches are disposable optimizations.
- Server, sidecar build, and cache promotion begin opt-in/experimental.
- No release changes v1 sidecar/protocol meaning in place.
- Rollback never requires altering or deleting the source VCD.

## 2. Compatibility matrix

| Surface | Requirement |
|---|---|
| Rust path-based API | Existing signatures and behavior retained in v1 |
| New Rust API | `OpenedVcd` stability declared only after core gate |
| CLI commands | Existing output/exit behavior golden-tested |
| Server | Unix-only in v1; feature/subcommand packaging documented |
| Windows | Existing normal binaries/library continue to build; no named-pipe promise |
| Python module functions | Existing signatures/dicts/exceptions retained |
| Python persistent class | Additive API after Rust core stabilizes |
| Sidecar | Rebuildable cache; v1 format golden-tested; unknown versions untouched |
| Protocol | Experimental v1 before stable declaration; incompatible changes use v2 |
| `vcd2trace` | Output remains byte-compatible or migration is deferred |

## 3. Release stages

### Release A — Core preview

Ships:

- compact internal catalog;
- `OpenedVcd` and independent readers;
- reusable streaming query methods;
- explicit compatibility wrappers;
- characterization and benchmark harness.

Defaults:

- sidecar off;
- event cache off;
- no server packaging required.

Exit evidence:

- Gate G3 passed;
- memory target accepted;
- existing APIs/CLI/Python green;
- no material unexplained one-shot regression.

### Release B — Experimental streaming server

Ships:

- Unix-domain `serve` mode;
- protocol v1 marked experimental;
- streaming backend only or optional backends disabled by default;
- bounded workers/queues/results/deadlines;
- generation validation and cancellation;
- operational documentation.

Defaults:

- local owner-only socket;
- one VCD per process;
- compare/shutdown/cache mutation disabled unless explicitly approved;
- sidecar/cache off.

Exit evidence:

- Gate G5 passed;
- soak and security tests pass;
- Windows normal release unaffected.

### Release C — Optional acceleration

Ships only the backends whose gates pass:

- sparse sidecar `ReadOnly`/`BuildIfMissing` opt-in;
- bounded selective cache `Lazy`/`EagerSelected` opt-in;
- backend/cache statistics through `describe` and logs.

Defaults:

- no automatic sidecar build in the first release;
- cache remains off or conservative explicit opt-in;
- full all-signal index absent.

Exit evidence:

- Gate G6 and/or G7;
- corruption, invalidation, ordering, memory and performance targets pass.

### Release D — Persistent Python API and stable candidate

Ships:

- `vcd_tools.Vcd`;
- `iter_extract` bounded-memory iterator;
- GIL-safe long operations;
- type stubs and examples;
- stable-candidate protocol/sidecar declarations if evidence supports them.

Exit evidence:

- wheel matrix and threaded tests pass;
- full Gate G8 release candidate evidence.

### Release E — Consider default optimization

Only after field feedback:

- consider automatic reuse of existing valid sidecars;
- consider conservative lazy cache default;
- do not automatically build indexes or consume large RAM without explicit policy and documented budgets;
- preserve `Off` controls permanently.

## 4. Feature/configuration controls

Required controls, exact syntax finalized with implementation:

```text
sidecar: off | read-only | build-if-missing | require
cache: off | lazy | eager-selected
cache max bytes / entries / concurrent builders
server workers / queue depth / connections / requests per connection
request bytes / signals / rows / response bytes / timeout
fingerprint: normal | strict
```

Invalid or unsafe combinations fail before binding/opening expensive work.

## 5. Packaging

Current release targets include Linux x64/ARM64, macOS Intel/ARM64, and Windows x64.

Requirements:

- Unix server implementation/dependencies are `cfg(unix)` or separately target-gated;
- Linux/macOS archives document server presence;
- Windows archives retain existing binaries and clearly state server unavailability;
- Python wheel builds do not accidentally require Unix transport on Windows;
- release checks verify archive contents per target;
- protocol/client examples are packaged or linked.

If `serve` is a subcommand, non-Unix behavior is either omitted at compile time or returns a clear unsupported-platform error without importing unavailable APIs. The choice is recorded at Gate G4.

## 6. Observability and support data

Server logs/counters must permit diagnosis without exposing waveform data:

- package/protocol/sidecar versions;
- generation/fingerprint summary;
- method/backend;
- queue/execution duration;
- bytes/commands scanned;
- rows/bytes returned;
- cache hit/miss/build/eviction/accounted bytes;
- sidecar accepted/rejected/build reason;
- cancellation/deadline/limit/stale/error code;
- worker, queue, connection, FD and memory health in diagnostics.

Paths are redacted/configurable. Signal values are not logged by default.

## 7. Operational documentation checklist

Before experimental server release, document:

- immutable VCD requirement;
- one-file ownership;
- socket path, permissions, stale cleanup, and shutdown;
- all finite defaults and how to lower them;
- cancellation and incomplete stream client behavior;
- sidecar location, size, validation, deletion and rebuild;
- cache policy and memory budget;
- source replacement/mutation behavior;
- no TCP/remote security claim;
- troubleshooting and diagnostic collection;
- systemd/user-service example only if requested and tested.

## 8. Rollback

### Core

Existing path-based functions remain available. If `OpenedVcd` has a correctness regression, release a patch restoring wrappers to the known streaming implementation without changing public signatures.

### Server

- stop/omit the server; normal CLI remains functional;
- remove only the owned socket;
- no source VCD changes need reversal.

### Sidecar

- set policy `Off`;
- delete `.vcdidx-v1` files safely;
- streaming backend resumes;
- unknown future sidecars are untouched.

### Cache

- set cache mode `Off` or memory budget zero;
- cache is process-local and disappears on exit;
- active requests retain safe references until completion.

### Protocol/format

- never repurpose a released field/version;
- incompatible fix uses protocol v2 or a new sidecar magic/filename;
- retain read compatibility only when demonstrably safe.

## 9. Release-candidate checklist

- [ ] G1–G5 passed; G6/G7 passed for any included optional backend.
- [ ] `cargo fmt`, clippy, all-target tests, release tests green.
- [ ] Linux/macOS server tests green; Windows normal build green.
- [ ] Python wheel/import/thread/iterator tests green for supported versions.
- [ ] Protocol and sidecar golden fixtures green.
- [ ] Fuzz campaigns completed with recorded duration.
- [ ] Large-file benchmark matrix published with commit/environment.
- [ ] No unexplained >10% existing one-shot regression.
- [ ] Soak shows no unbounded RSS/FD/thread/task/queue growth.
- [ ] Mutation, corruption, interrupted build, slow client and cancellation tests green.
- [ ] Documentation and examples reviewed.
- [ ] Archive contents verified on every target.
- [ ] Rollback controls tested.
- [ ] Known limitations and deferred features listed in release notes.

## 10. Post-release review

Collect:

- typical number and kinds of queries per debugging session;
- signal working-set size and repetition;
- sidecar build/reuse rate and size;
- sparse skip ratio and query speedup;
- cache hit rate, bytes, evictions, oversize timelines;
- cancellation/limit/queue-full frequency;
- stale-source and sidecar-rejection frequency;
- crashes, deadlocks, resource growth, protocol client issues;
- platform/package friction.

Only this evidence can justify enabling automatic sidecar reuse/build or lazy cache defaults.
