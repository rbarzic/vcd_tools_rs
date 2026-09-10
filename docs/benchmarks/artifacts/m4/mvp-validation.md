# M4 server MVP validation

Candidate branch: `design-db`  
Base commit: `0c7f223`

Implemented scope: RQ-M4-T06 through T09. T10 long soak/release hardening remains deferred.

## Focused validation

```sh
cargo test --test protocol_v1 --test server_runtime --test server_e2e -- --test-threads=1
for i in $(seq 1 5); do cargo test --quiet --test server_e2e -- --test-threads=1; done
cargo check --all-targets
cargo check --all-targets --target x86_64-pc-windows-msvc
cargo run --quiet --bin vcd_tools_rs -- serve --help
```

Results:

- protocol: 15 passed;
- bounded runtime: 14 passed;
- real Unix-socket end-to-end: 3 passed, repeated five times;
- host and Windows all-target source/type checks passed;
- native serve help passed.

## Manual native smoke

```sh
cargo build --quiet --bin vcd_tools_rs
dir=$(mktemp -d); chmod 700 "$dir"
./target/debug/vcd_tools_rs serve tests/fixtures/query_semantics.vcd \
  --socket "$dir/server.sock" &
pid=$!
python3 - "$dir/server.sock" <<'PY'
import json, socket, sys, time
for _ in range(100):
    try:
        s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); break
    except OSError: time.sleep(.02)
s.sendall(b'{"v":1,"id":"smoke","method":"ping","params":{}}\n')
print(json.loads(s.makefile().readline()))
s.close()
PY
kill -INT "$pid"; wait "$pid"
test ! -e "$dir/server.sock"
rm -rf "$dir"
```

Observed response:

```json
{"v":1,"id":"smoke","seq":0,"type":"result","ok":true,"result":{"package_version":"0.1.7","protocol":"1","ready":true,"uptime_ms":"50"}}
```

Socket cleanup passed.

## Final reviewer safety pass

Five blocking findings were fixed and covered with deterministic tests:

1. Metadata builders now poll `QueryContext`/command limits while parsing. A cancelled elected build resets the transient cache attempt, wakes waiters to re-elect, and releases the server scan permit. A real-socket barrier test proves the next metadata request completes.
2. Cached metadata performs context and source-generation checks immediately before unary publication. Atomic replacement at a publication barrier returns `STALE_SOURCE`; replacement after the first streamed chunk produces `complete:false`/`STALE_SOURCE`.
3. Runtime and service fields have explicit hard maxima at reasonable multiples of defaults. Every max is accepted and max+1 is rejected; all CLI max+1 cases fail before socket bind.
4. Every post-bind accept-loop exit now sets shutdown, joins accepted connections, cleans the socket, and then returns the original fatal accept error. A real-socket injected-fatal test verifies EOF, cleanup, and error identity.
5. Real-socket tests now cover frame splitting/terminal reserve and exact encoded bytes, connection saturation and event-coordinated disconnect cleanup without sleeps, active metadata cancellation, stale unary/stream behavior, spawned native startup/ping/SIGTERM/cleanup, and invalid pre-bind configuration.

Final focused results:

- opened metadata concurrency/cancellation: 27 passed, 1 diagnostic ignored;
- protocol: 15 passed;
- bounded runtime: 15 passed;
- real Unix-socket/binary E2E: 10 passed;
- host `cargo check --all-targets`: passed;
- changed-file formatting and diff checks: passed.

## Parent-observed release smoke

After the focused suite, the release binary was built and run directly:

```sh
cargo build --release --bin vcd_tools_rs
DIR=$(mktemp -d); chmod 700 "$DIR"
./target/release/vcd_tools_rs serve tests/fixtures/query_semantics.vcd \
  --socket "$DIR/server.sock"
```

A Python Unix-socket client sent `ping` and received:

```json
{"id":"smoke","ok":true,"result":{"package_version":"0.1.7","protocol":"1","ready":true,"uptime_ms":"40"},"seq":0,"type":"result","v":1}
```

`SIGTERM` produced a successful process exit and removed the owned socket. Result: **PASS**.

## Deferred G5/T10 risks

- long-duration soak and p95/p99 latency;
- native macOS listener/runtime behavior;
- peer-credential enforcement (filesystem mode is the v1 boundary when unavailable);
- release archive and wheel packaging checks;
- hostile-client FD/RSS observation over long duration;
- system service integration and forced termination behavior.
