# Unix Socket JSON Protocol v1

Status: **DRAFT**  
Plan: **VCD-RQ-001**  
Transport: Unix-domain stream socket, UTF-8 JSON Lines

## 1. Server ownership and invocation

Recommended interface:

```sh
vcd_tools_rs serve simulation.vcd \
  --socket /run/user/$UID/vcd-tools.sock \
  --workers 4 \
  --queue-depth 64 \
  --max-request-bytes 1048576 \
  --max-response-bytes 268435456
```

One process serves exactly one VCD selected at startup. Clients cannot choose the primary path. This bounds filesystem exposure and cache cardinality.

`serve` is available only on Unix in v1. Unix-specific implementation and dependencies must be target-gated so existing Windows library and binaries continue to compile. Whether packaging uses a subcommand or dedicated binary is finalized at Gate G0; protocol behavior is independent of that choice.

## 2. Framing

- Each frame is one UTF-8 JSON object followed by `\n`.
- Embedded newlines are JSON escapes, not literal frame separators.
- Protocol version is required: `"v":1`.
- Request IDs are non-empty strings and unique among active requests on a connection.
- Server frames include the originating request ID and monotonically increasing per-request `seq` starting at zero.
- Frames for different IDs may interleave.
- Frames for one ID are ordered.
- The server reads incrementally and rejects an unterminated frame once the configured byte limit is exceeded; it must not allocate an unbounded line first.
- Default maximum request frame is 1 MiB.

## 3. Numeric and value fidelity

JSON numbers cannot safely represent every `u64`/`u128` in common clients. Protocol v1 therefore uses:

- timestamps: decimal strings;
- counts that may exceed safe integer range: decimal strings;
- integer signal values: decimal or display strings, accompanied by a kind when needed;
- real values: canonical strings; non-finite values must never be emitted as invalid JSON numbers;
- textual/vector X/Z/string values: strings.

Example event:

```json
{
  "signal":"tb.counter[127:0]",
  "time":"65290706000000",
  "value":{"kind":"integer","text":"340282366920938463463374607431768211455"}
}
```

A simpler `value_text` field may be used if v1 intentionally exposes only the existing normalized/display representation. The schema choice must be frozen with golden tests before Gate G5.

## 4. Requests

General request:

```json
{
  "v":1,
  "id":"req-42",
  "method":"extract",
  "params":{}
}
```

Unknown top-level fields are ignored for forward-compatible additions. Missing required fields, wrong types, duplicate active IDs, or oversized frames return stable errors.

### Initial methods

| Method | Response | Notes |
|---|---|---|
| `ping` | unary | Liveness, protocol and server version |
| `describe` | unary | Source generation, capabilities, configured limits |
| `list` | streaming or bounded unary | Signal substring filter |
| `metadata` | unary | May trigger one lazy scan/index build |
| `extract` | streaming | Time/value events or aligned rows, schema declared in `begin` |
| `find` | unary | Nth matching occurrence |
| `toggles` | unary | Bounded signal set |
| `cancel` | unary plus target termination | Cancels request owned by same connection |
| `cache.stats` | unary | No signal values |
| `cache.warm` | unary/operation stream | Optional and permission/config gated |
| `cache.evict` | unary | Optional and permission/config gated |

`compare` is deferred from the first server milestone unless its second-file access policy and response bounds pass security/design review. The server must not accept arbitrary paths merely to mirror the current CLI.

`shutdown` is disabled by default. If enabled explicitly, it is restricted to the socket owner/authorized peer.

## 5. Common parameters

Time window:

```json
{"start":"1000","end":"2000"}
```

Both are optional decimal `u64` strings and inclusive. `start > end` returns `INVALID_WINDOW`.

Per-request limits may lower but never raise server maxima:

```json
{
  "max_rows":"100000",
  "max_response_bytes":"16777216",
  "timeout_ms":"30000"
}
```

Signal lists preserve request order and current duplicate behavior unless the method explicitly documents otherwise.

## 6. Response frames

### 6.1 Unary success

```json
{
  "v":1,
  "id":"req-42",
  "seq":0,
  "type":"result",
  "ok":true,
  "result":{}
}
```

A unary request emits exactly one terminal `result` or `error` frame.

### 6.2 Streaming success

```json
{"v":1,"id":"req-42","seq":0,"type":"begin","schema":"time_value.v1"}
{"v":1,"id":"req-42","seq":1,"type":"chunk","rows":[...]}
{"v":1,"id":"req-42","seq":2,"type":"end","complete":true,
 "stats":{"rows":"1000","bytes":"73500","elapsed_ms":"12","backend":"stream"}}
```

- `begin` declares schema before data.
- `chunk` contains a bounded array.
- `end` is terminal.
- Initial chunk target: at most 1,024 rows and approximately 256 KiB encoded data, bounded by both constraints.
- Statistics never include signal values.
- Backend may be `stream`, `sparse`, `cache`, or `mixed`.

### 6.3 Incomplete stream

If an error occurs after `begin`, the terminal frame is:

```json
{
  "v":1,
  "id":"req-42",
  "seq":7,
  "type":"end",
  "complete":false,
  "error":{"code":"STALE_SOURCE","message":"source changed during query","retryable":true}
}
```

Clients must discard or explicitly mark all preceding chunks from an incomplete stream.

## 7. Errors

Error before streaming begins:

```json
{
  "v":1,
  "id":"req-42",
  "seq":0,
  "type":"error",
  "ok":false,
  "error":{
    "code":"SIGNAL_NOT_FOUND",
    "message":"one or more signals were not found",
    "details":{"signals":["tb.missing"]},
    "retryable":false
  }
}
```

Stable v1 codes:

- `BAD_REQUEST`
- `UNSUPPORTED_VERSION`
- `UNKNOWN_METHOD`
- `DUPLICATE_REQUEST_ID`
- `SIGNAL_NOT_FOUND`
- `INVALID_WINDOW`
- `INVALID_OCCURRENCE`
- `LIMIT_EXCEEDED`
- `QUEUE_FULL`
- `CANCELLED`
- `DEADLINE_EXCEEDED`
- `STALE_SOURCE`
- `SOURCE_UNAVAILABLE`
- `PARSE_ERROR`
- `IO_ERROR`
- `UNSUPPORTED_PLATFORM`
- `INTERNAL`

Default messages do not expose Rust backtraces, unrestricted paths, environment data, or source values. Detailed server logs remain local and redact values.

## 8. Method sketches

### `ping`

Request:

```json
{"v":1,"id":"1","method":"ping","params":{}}
```

Result includes protocol version, package version, uptime, and readiness.

### `describe`

Result includes:

- display filename subject to redaction policy;
- generation ID;
- signal count;
- timescale;
- source size;
- sidecar/cache capability and current state;
- immutable-source contract;
- configured server limits;
- supported methods/schemas.

### `list`

Parameters:

```json
{"filter":"clk","max_rows":"10000"}
```

Large lists stream chunks. Signal order remains declaration order.

### `metadata`

No required parameters. If exact body bounds are absent, behavior is selected by server policy:

- perform one cancellable scan;
- wait for/build sidecar;
- return operation state only if a future asynchronous job mode is versioned.

V1 default is a cancellable request that completes with exact metadata or an error.

### `extract`

Parameters:

```json
{
  "signals":["tb.clk","tb.rst_n"],
  "start":"0",
  "end":"1000000",
  "layout":"events",
  "max_rows":"100000"
}
```

`layout` values:

- `events`: one signal/time/value event preserving body and alias expansion order;
- `aligned`: optional schema matching current CLI aligned rows, only if semantics and memory-bounded streaming are fully specified.

Initial implementation should prefer `events` because it maps directly to the engine iterator.

### `find`

Parameters contain one signal, target text, occurrence >= 1, and optional window. Target parsing follows the current Rust function.

### `toggles`

Parameters contain a bounded signal array and optional window. Result order follows request order even if internal result is a map.

### `cancel`

```json
{"v":1,"id":"cancel-1","method":"cancel","params":{"request_id":"req-42"}}
```

The cancellation request returns whether an active target owned by this connection was found. The target independently ends with `CANCELLED` unless it already completed.

## 9. Scheduling and backpressure

Required architecture:

- one incremental frame reader per connection;
- one bounded output queue per connection;
- one bounded global work queue;
- fixed worker count or another execution model approved by Gate G4;
- active-request registry scoped by connection and request ID;
- maximum concurrent requests per connection;
- maximum connections and open file descriptors;
- at most the configured number of expensive scans per generation, initially one;
- cache-hit/indexed work may have a separate concurrency limit.

Queue saturation returns `QUEUE_FULL`; it never creates unbounded tasks or threads.

Workers deliver chunks through bounded queues. A slow client propagates backpressure, but cancellation/deadline remains observable using timed or cancellable sends. No global/catalog/cache lock is held while waiting for socket output.

Disconnect cancels all active requests on that connection and drops queued output. Slow or disconnected clients must not retain a scanner indefinitely.

Worker panics are caught at the request boundary where Rust permits; the request returns `INTERNAL`, building reservations are cleared, and the listener remains alive.

## 10. Limits

Every deployment configures finite defaults for:

- connections;
- active requests per connection;
- global queue depth;
- worker count;
- request frame bytes;
- signal count per request;
- rows per response;
- encoded response bytes;
- chunk rows/bytes;
- command/scan budget or deadline;
- cache bytes/entries/builders;
- sidecar builders;
- optional compare secondary files.

Limits are returned by `describe`. Exceeding them returns `LIMIT_EXCEEDED`, `QUEUE_FULL`, or `DEADLINE_EXCEEDED` as appropriate.

## 11. Socket lifecycle and security

- Default socket mode is owner-only (`0600`) under a restrictive umask.
- Parent directory is not created unless explicitly requested.
- A non-socket path is never unlinked.
- Startup distinguishes a live listener from a stale owned socket before cleanup.
- Shutdown stops admission, handles in-flight work according to configured grace period, closes listener, and removes only the socket created by this process.
- Crash recovery is documented; stale socket removal remains conservative.
- Peer credentials may restrict UID where supported; portability fallback relies on directory/socket permissions.
- The server does not accept an arbitrary primary VCD path.
- Logs do not emit waveform values by default.
- TCP/non-loopback exposure is out of scope.

## 12. Compatibility and negotiation

- Version mismatch returns `UNSUPPORTED_VERSION` with supported versions.
- V1 readers ignore documented optional unknown fields but reject unknown frame types or required capabilities.
- `describe` advertises optional methods/backends.
- Existing v1 field meaning is never changed; incompatible changes require v2.
- Golden request/response fixtures are checked into tests.

## 13. Required tests

1. Partial frames and multiple frames per read.
2. Unterminated/oversized lines without unbounded allocation.
3. Invalid UTF-8 and malformed JSON.
4. Unsupported version/method and duplicate IDs.
5. Numeric overflow and invalid decimal strings.
6. `u64`, `u128`, X/Z, wide vector, string, real, and non-finite-real fidelity.
7. Unary and streamed success/error lifecycle.
8. Per-request ordering with interleaved IDs.
9. Row/byte/deadline/signal limits.
10. Cancellation before run, during scan, while waiting, and during backpressure.
11. Disconnect and slow reader.
12. Queue saturation and bounded workers/resources.
13. Concurrent clients and one-scan policy.
14. Stale source during unary and streamed response.
15. Stale/live socket startup, permissions, graceful shutdown, crash cleanup.
16. Non-Unix build remains green.
17. Load/soak test for RSS, FD, queue, and thread/task leaks.

## 14. Protocol release gate

Protocol v1 cannot be called stable until:

- schemas and error codes are frozen with golden fixtures;
- numeric/value fidelity is demonstrated;
- every queue and result is bounded;
- cancellation works during scan and output backpressure;
- malformed clients cannot crash or grow the server without limit;
- socket ownership and cleanup tests pass;
- Windows release targets remain unaffected;
- documentation includes client examples and incomplete-stream handling.
