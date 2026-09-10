# M4 T02–T05 runtime validation

Status: implementation complete; consolidated review pending.

Implemented:

- Unix-only safe listener lifecycle with absolute/existing owner-controlled parent requirement, conservative live/stale connect probe, refusal to unlink non-sockets, stale inode recheck, post-bind `0600`, and created-inode-only cleanup.
- Fixed worker pool and bounded global `sync_channel`, observable counters, immediate queue rejection, RAII active-request cleanup, panic isolation, and one cancellation-aware scan permit.
- Bounded per-connection output channel, one serialized writer, incremental accepted protocol decoder, active-ID registry, partial/multiple frame handling, and transport-error close behavior.
- Cooperative cancellation for queued, permit-waiting, working, and output-blocked requests; same-connection cancel method; disconnect cancellation; finite connection permits; write timeouts prevent unbounded blocking.
- Dispatcher seam only; production query methods remain T06.

Focused validation:

```sh
cargo check --all-targets
cargo test --test protocol_v1 --test server_runtime -- --test-threads=1
```

Results:

- protocol v1: 15 passed;
- Unix runtime: 14 passed;
- total focused: 29 passed, 0 failed;
- host and Windows all-target checks passed.

The runtime tests cover listener owner-private creation permissions and immediate post-bind failure cleanup, stale/live/non-socket/cleanup behavior, shared connection shutdown state and permit release, hard connection limits, partial and multiple frames, inter-request frame interleaving, queue saturation, panic cleanup and worker survival, cancellation while queued/waiting/scanning/output-blocked, deliberate close when a required terminal cannot be queued, exact output lifecycle/sequence, capped serialization at N/N+1 and with an 8 MiB payload, timeout lowering/overflow rejection, disconnect cleanup, bounded scan concurrency, malformed/oversized close behavior, and runtime counters.

Capped serialization writes JSON through a bounded writer before appending the newline; the runtime-observed maximum buffer never exceeds `max_encoded_frame_bytes`.

The final publication-race fix uses atomic no-replace hard-link publication from the unique private temporary socket, then unlinks the temporary name. A deterministic two-binder race proves exactly one winner, `AlreadyExists`/`AddrInUse` for the loser, no hidden temporary paths, and no ability for loser cleanup to unlink the live winner.

Observed focused-test cancellation assertions require work/permit and blocked-output worker release within 250 ms; the 2 ms polling design normally completes substantially below that bound. This is a regression bound, not a production percentile benchmark.

Known later work:

- T06 production service methods must use `RequestContext::with_expensive_scan` so the scan permit is released before potentially blocked output.
- T08/T09 add the listener accept loop, CLI integration, and race-free end-to-end client.
- T10 performs longer socket soak, FD/RSS/thread measurements, native Unix platform checks, and G5 evidence.
