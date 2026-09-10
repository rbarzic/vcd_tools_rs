# RQ-M3-T04 — bounded scheduler spike

## Recommendation

**PASS for the standard-library thread/channel model; do not add an async runtime for M4.**

A four-worker `std::thread` pool, `std::sync::mpsc::sync_channel` admission/output queues, an atomic cancellation flag, and a cancellation-aware one-scan permit stayed bounded and cancelled all admitted work in at most **1.475 ms** in this spike. At 16 and 64 offered clients, overload was rejected immediately rather than increasing workers or queue storage. Nothing measured here justifies the dependency and implementation complexity of an async runtime.

This recommendation assumes M4 enforces a configured connection/admission cap. Revisit async only if the required operating point becomes hundreds/thousands of mostly idle simultaneous connections, if a thread per admitted connection is required and measured stack/FD cost fails its budget, or if real Unix-socket soak tests show unacceptable fairness or cancellation behavior.

## Prototype

The time-boxed prototype is `/tmp/m3_scheduler_spike.rs` (not a repository file). It uses:

- 4 fixed job workers;
- a bounded global `sync_channel` of 8 jobs (`try_send` gives immediate overload rejection);
- a bounded per-request output `sync_channel` of 2 chunks;
- 64 KiB output chunks;
- one `Mutex<bool> + Condvar` expensive-scan permit (one simulated generation);
- cancellation via `Arc<AtomicBool>`, polled every 1 ms in scan work, every 2 ms while waiting for the scan permit, and every 1 ms while retrying full output;
- `try_send` loops so blocked output remains cancellable and a dropped receiver is detected;
- RAII release of the expensive permit, with `catch_unwind` at the worker/job boundary;
- a newline frame accumulator fed across three arbitrarily split reads;
- `/proc/self/status` sampling every 1 ms for RSS and total process threads.

The expensive permit is deliberately dropped after scan and before output, so a slow reader does not retain the scan permit. `max_scans=1` in every load run confirms the invariant.

The client load is an offered-load/admission test: accepted jobs get simulated reader threads; excess jobs are rejected before allocating a reader. Thus the reported process thread peak includes main, monitor, four server workers, and accepted-client harness readers. The **server job-worker count itself remains four**.

### Storage bounds by construction

- Global pending work: 8 jobs, plus at most 4 jobs held by workers.
- Per output: 2 queued 64 KiB chunks, plus at most one 64 KiB chunk retained by a worker retrying `try_send` (about 192 KiB transient/request).
- Worker count: 4, independent of offered clients.
- Only admitted clients allocate output channels/readers in this harness.

## Environment and exact commands

```text
$ uname -a
Linux onio-ws13 6.17.0-29-generic #29~24.04.1-Ubuntu SMP PREEMPT_DYNAMIC Mon May 11 10:30:58 UTC 2 x86_64 x86_64 x86_64 GNU/Linux

$ rustc --version
rustc 1.93.1 (01f6ddf75 2026-02-11)
```

Build/run command:

```sh
rustc --edition=2021 -O /tmp/m3_scheduler_spike.rs -o /tmp/m3_scheduler_spike && \
/usr/bin/time -v /tmp/m3_scheduler_spike 2>&1 | tee /tmp/m3_scheduler_spike.out
```

Repository cleanliness check:

```sh
git status --short
```

Result: no output; the repository was not modified. Only `/tmp` and this required report artifact were written.

## Results

### Partial framing

```text
FRAME partial chunks=3 frames=["{\"id\":1}", "{\"id\":2}", "partial"] remainder_bytes=0
```

The accumulator correctly retained a partial frame across reads and extracted multiple frames from one read.

### Offered load, queue bounds, memory, and threads

| Offered clients | Accepted | Immediately rejected | Completed | Elapsed ms | Baseline RSS KiB | Peak RSS KiB | Delta KiB | Peak process threads | Max concurrent scans |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 0 | 1 | 106 | 2152 | 2432 | 280 | 7 | 1 |
| 4 | 4 | 0 | 4 | 423 | 2688 | 2988 | 300 | 10 | 1 |
| 16 | 12 | 4 | 12 | 1266 | 3116 | 3292 | 176 | 18 | 1 |
| 64 | 12 | 52 | 12 | 1265 | 3236 | 3252 | 16 | 18 | 1 |

Exact output:

```text
SCALE clients=1 accepted=1 rejected=0 completed=1 elapsed_ms=106 base_rss_kib=2152 peak_rss_kib=2432 rss_delta_kib=280 peak_threads=7 max_scans=1
SCALE clients=4 accepted=4 rejected=0 completed=4 elapsed_ms=423 base_rss_kib=2688 peak_rss_kib=2988 rss_delta_kib=300 peak_threads=10 max_scans=1
SCALE clients=16 accepted=12 rejected=4 completed=12 elapsed_ms=1266 base_rss_kib=3116 peak_rss_kib=3292 rss_delta_kib=176 peak_threads=18 max_scans=1
SCALE clients=64 accepted=12 rejected=52 completed=12 elapsed_ms=1265 base_rss_kib=3236 peak_rss_kib=3252 rss_delta_kib=16 peak_threads=18 max_scans=1
```

Interpretation: admission saturates at no more than workers + queue capacity (12). Exact accepted count can be lower because `try_send` races workers draining the queue; this is expected and was also observed in the explicit saturation case. Neither thread count nor admitted storage grew from 16 to 64 offered clients. RSS deltas are allocator/page-granularity observations, not precise per-request allocation measurements; the structural channel bounds are the stronger evidence.

### Cancellation latency

Single phase-specific cases:

```text
CANCEL phase=scan status=cancel-scan latency_us=116
CANCEL phase=blocked-output status=cancel-output latency_us=361
```

Cancellation under offered load (all admitted work completed cancellation handling before the reported latency):

| Offered | Admitted | Rejected | All-admitted cancellation latency |
|---:|---:|---:|---:|
| 1 | 1 | 0 | 851 us |
| 4 | 4 | 0 | 1475 us |
| 16 | 12 | 4 | 1449 us |
| 64 | 12 | 52 | 1391 us |

```text
CANCEL_LOAD clients=1 accepted=1 rejected=0 all_done_latency_us=851 statuses=["cancel-output"]
CANCEL_LOAD clients=4 accepted=4 rejected=0 all_done_latency_us=1475 statuses=["cancel-output", "cancel-scan", "cancel-wait", "cancel-wait"]
CANCEL_LOAD clients=16 accepted=12 rejected=4 all_done_latency_us=1449 statuses=["cancel-output", "cancel-wait", "cancel-wait", "cancel-scan", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait"]
CANCEL_LOAD clients=64 accepted=12 rejected=52 all_done_latency_us=1391 statuses=["cancel-output", "cancel-wait", "cancel-scan", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait", "cancel-wait"]
```

This covers cancellation while blocked on output, while scanning, and while waiting for the single-scan permit. Latency remained below the configured 2 ms maximum polling interval at all tested loads.

### Disconnect, saturation, panic cleanup, slow reader

```text
EDGE disconnect=disconnect
EDGE saturation accepted=10 rejected=10 global_cap=8 workers=4

thread '<unnamed>' (...) panicked at /tmp/m3_scheduler_spike.rs:29:83:
intentional panic cleanup test
EDGE after_panic=ok
SLOW status=ok chunks=100 elapsed_bound_queue_chunks=2 base_rss_kib=3936 peak_rss_kib=4064 delta_kib=128
```

- Dropping the output receiver was observed as `disconnect` rather than hanging.
- The saturation count is race-dependent, but acceptance remained below the hard maximum of 12 and excess work was rejected.
- The panic text is expected from Rust's default panic hook. `catch_unwind` kept the worker alive, RAII released the scan permit, and the following request completed `ok`.
- A reader delayed 5 ms per chunk consumed all 100 chunks while its queue never exceeded 2 chunks. Observed RSS delta was 128 KiB; the sender can additionally hold one 64 KiB retry chunk outside the queue.
- Full run wall time was 7.10 s with exit status 0.

## Limitations and M4 requirements

1. **Not real socket I/O.** This isolates scheduler/backpressure mechanics with in-memory channels. M4 still needs Unix-socket tests for partial reads/writes, EOF, write errors, FDs, permissions, and shutdown races.
2. **One generation only.** The prototype's single permit represents one generation. Production should key permits by generation while defining a bounded lifecycle for the permit map.
3. **Connection lifecycle not measured.** The test rejects jobs before client reader allocation. Production must separately cap admitted/open connections; idle connections otherwise imply per-connection threads or require polling/async.
4. **Frame size was not adversarially tested.** The accumulator proved partial/multiple-frame behavior but has no prototype maximum. M4 must reject a frame as soon as buffered bytes exceed the frozen protocol limit.
5. **Fairness is not guaranteed by `std::sync::Condvar` or `mpsc`.** No starvation appeared in this short run, but queue-order/fairness and a long soak remain M4-T09/T10 work.
6. **Cancellation is cooperative.** Every engine scan/merge loop and output retry must retain bounded polling intervals. A blocking parser/syscall without timeout would invalidate these latency results.
7. **Panic cleanup is partial evidence.** Permit release and worker survival were shown; production also needs RAII removal from the request registry and one terminal protocol error where possible.
8. **RSS sampling is Linux-specific and coarse.** Internal `/proc` peaks differed from GNU `time`'s run-wide maximum (GNU `time` printed 3128 KiB while the 1 ms monitor observed later values up to 4064 KiB). The table consistently uses the internal sampler; conclusions rely primarily on hard queue/thread bounds.
9. **No statistical latency distribution.** These are deterministic short spike measurements, not p50/p95/p99 soak evidence.

## Gate recommendation

For Gate G4, select **standard-library threads/channels** with these mandatory design constraints:

- fixed worker pool and bounded global `sync_channel`;
- immediate, protocol-defined overload response on `try_send` failure;
- bounded per-connection/request output queues and fixed maximum chunk size;
- cancellation-aware retry/polling for scan-permit wait, scan loops, and output backpressure;
- release expensive-scan permit before potentially blocked output;
- RAII cleanup at a `catch_unwind` request boundary;
- explicit maximum connections, frame bytes, queued jobs, output chunks, and cancellation poll interval;
- keyed one-scan-per-generation permits with bounded cleanup;
- real Unix-socket load/soak validation before G5.

**Async runtime: not justified by this spike.** It should remain a documented fallback, not an M4 dependency.
