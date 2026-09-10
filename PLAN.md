# Reusable VCD Query and Server Plan

This file is the tracking dashboard for plan **VCD-RQ-001**. Detailed requirements are split across the linked design and execution documents.

## Objective

Make repeated exploratory queries against one large VCD substantially faster while preserving the existing streaming CLI, Rust API, Python API, query semantics, and supported release targets.

The optimization is a reusable query engine and optional indexes. The Unix-socket server is one client of that engine; keeping a process or file descriptor alive is not treated as the optimization by itself.

## Current decision

**Status: APPROVED / IMPLEMENTATION IN PROGRESS.**

Gate G0 was approved by the project owner. The recommendations below are now the accepted v1 product boundary unless changed through the documented change-control process.

Recommended v1 product boundary:

- One server instance owns one VCD selected at startup.
- The VCD is immutable for the lifetime of an opened generation.
- Existing path-based Rust and Python functions remain compatible.
- `OpenedVcd` caches a compact header/catalog and creates an independent reader for every streaming query.
- Metadata is lazy; opening a VCD must not scan the complete body.
- The first server release uses the streaming backend and bounded responses.
- Sparse sidecar and selective timeline caches are optional backends, enabled only after differential correctness and resource gates pass.
- Server transport is Unix-domain socket only in v1; normal library and CLI builds remain supported on Windows.
- No eager all-signal in-memory event index by default.

## Documents

| Document | Purpose | Status |
|---|---|---|
| [`docs/plans/reusable-query-implementation-plan.md`](docs/plans/reusable-query-implementation-plan.md) | Work breakdown, dependencies, task status, acceptance criteria, validation, risks, and gates | APPROVED; M3 READY |
| [`docs/design/current-compatibility-contract.md`](docs/design/current-compatibility-contract.md) | Characterized Rust, native CLI, Python, trace, packaging, errors, and semantic compatibility boundary | ACCEPTED AT G1 |
| [`docs/design/reusable-query-architecture.md`](docs/design/reusable-query-architecture.md) | Core ownership, data model, semantics, query backends, invalidation, and compatibility | DRAFT |
| [`docs/design/sidecar-format-v1.md`](docs/design/sidecar-format-v1.md) | Sparse timestamp/offset index format and lifecycle | DRAFT; blocked on offset spike |
| [`docs/design/unix-json-protocol-v1.md`](docs/design/unix-json-protocol-v1.md) | Frozen native Unix JSONL protocol, methods, schemas, limits, errors, cancellation, and lifecycle | ACCEPTED AT G4 |
| [`docs/benchmarks/reusable-query-baseline.md`](docs/benchmarks/reusable-query-baseline.md) | Existing measurements, benchmark procedure, targets, and regression policy | M0 BASELINE RECORDED |
| [`docs/release/reusable-query-rollout.md`](docs/release/reusable-query-rollout.md) | Feature staging, packaging, compatibility, rollback, and operational readiness | DRAFT |

## Status vocabulary

- `PROPOSED` — specified but not approved.
- `BLOCKED` — waiting on a decision, experiment, or dependency.
- `READY` — approved and implementable.
- `IN_PROGRESS` — implementation has started.
- `IN_REVIEW` — implementation is complete and under review.
- `DONE` — acceptance criteria and validation evidence are recorded.
- `MVP_AVAILABLE` — the scoped MVP is runnable and focused tests pass; release-hardening gate remains deferred.
- `DEFERRED` — intentionally postponed without rejecting the work.
- `REJECTED` — explicitly removed from scope.

A task is not `DONE` until its acceptance criteria pass and evidence is linked from the implementation plan.

## Milestone tracker

| Milestone | Scope | Status | Entry gate | Exit gate |
|---|---|---|---|---|
| M0 | Decisions, semantic characterization, fresh baselines | DONE | G0 passed | G1 passed |
| M1 | Compact catalog and `OpenedVcd` | DONE | G1 passed | G2 passed |
| M2 | Transport-neutral query engine and compatibility wrappers | DONE | G2 passed | G3 passed |
| M3 | Technical spikes: offsets, snapshot readers, scheduler, serialization | DONE | G3 passed | G4 passed |
| M4 | Experimental bounded Unix-socket server using streaming backend | MVP_AVAILABLE | G4 passed | G5 hardening deferred |
| M5 | Sparse persistent sidecar | BLOCKED | Offset spike approved | G6 |
| M6 | Selective/hot-signal timeline cache | BLOCKED | M2 and workload evidence | G7 |
| M7 | Persistent Python `Vcd`, hardening, release candidate | BLOCKED | M4 plus approved optional backends | G8 |

## Gate tracker

| Gate | Decision | Status | Required evidence |
|---|---|---|---|
| G0 | Approve product boundary and non-goals | PASSED | Decisions D-001 through D-010 approved by project owner |
| G1 | Lock current semantics and baseline | PASSED | Reviewer accepted 92 serial Rust tests, 3 Python tests, compatibility contract, and durable `0.1.7` baselines |
| G2 | Accept compact catalog and snapshot model | PASSED | Reviewer accepted T08, 79.9% RSS reduction, faster native list, concurrency/snapshot tests, and CR-002 all-target checks |
| G3 | Accept query-engine refactor | PASSED | Reviewer accepted 177 debug/release Rust tests, compiled Python compatibility, five target checks, and T09 deferral |
| G4 | Select feasible server and index mechanisms | PASSED | Reviewer accepted safe offsets, SoA events, bounded std scheduler, and frozen native protocol |
| G5 | Ship experimental streaming server | DEFERRED | MVP focused tests pass; long soak, peer credentials, native macOS runtime, and packaging remain |
| G6 | Enable sparse sidecar as opt-in | BLOCKED | Resume correctness, corruption/invalidation, speed and size targets pass |
| G7 | Enable selective cache as opt-in | BLOCKED | Ordering equivalence, byte budget, single-flight and speed targets pass |
| G8 | Publish stable feature | BLOCKED | Full CI, documentation, packaging, fuzzing, rollback and RC evidence pass |

## Decision log

Update the `Status`, `Decision`, and `Evidence` columns; do not silently change an accepted decision.

| ID | Question | Recommended decision | Status | Evidence |
|---|---|---|---|---|
| D-001 | What does one server own? | Exactly one VCD fixed at startup | ACCEPTED | Approved at G0; limits filesystem exposure and cache growth |
| D-002 | May the VCD change? | Immutable generation; detect replacement/mutation and fail or reopen between requests | ACCEPTED | Approved at G0; prevents mixed header/body/index results |
| D-003 | What is cached at open? | Compact header/catalog, timescale, body offset, opened-generation identity; metadata remains lazy | ACCEPTED | Approved at G0; full metadata scan costs 6–26 s |
| D-004 | Initial query backend? | Independent full-stream reader per request | ACCEPTED | Approved at G0; lowest-risk compatibility baseline |
| D-005 | Persistent acceleration? | Sparse sidecar only after parser-offset spike; opt-in initially | ACCEPTED | Approved at G0; avoids unproven resume offsets |
| D-006 | Event cache policy? | Bounded, selective, by `IdCode`, opt-in/lazy; no eager all-signal default | ACCEPTED | Approved at G0; avoids multi-GB RAM growth |
| D-007 | Server interface? | `vcd_tools_rs serve <vcd> --socket <path>` on Unix, protocol v1 over JSON Lines | ACCEPTED | Approved at G0; matches existing CLI distribution |
| D-008 | Network scope? | Unix-domain socket only; owner-only permissions; no TCP/HTTP in v1 | ACCEPTED | Approved at G0; local debugging use case |
| D-009 | Protocol numeric encoding? | Encode time and arbitrary-width values as strings; never depend on JSON integer precision | ACCEPTED | Approved at G0; preserves `u64`/`u128` fidelity |
| D-010 | Server execution model? | Start with bounded standard threads/channels; add async runtime only if spike proves necessary | ACCEPTED | Approved at G0; avoids unnecessary runtime complexity |

## Immediate next actions

1. Commit, tag, and push release `v0.1.8` after native and wheel smoke checks.
2. Keep T10/G5 long soak, peer-credential hardening, and native macOS runtime explicitly deferred.
3. Do not add sidecar/cache/compare/shutdown methods to protocol v1.
4. Publish Unix server mode through both the native binary and Unix PyPI console; Windows remains unsupported for Unix sockets.

## Progress update template

Use this block in the implementation plan for every completed task:

```text
Task: RQ-Mx-Tyy
Status: DONE
Commit/PR:
Changed files:
Validation commands:
Validation result:
Benchmark delta:
Decisions made:
Residual risks/follow-ups:
Completed by/date:
```
