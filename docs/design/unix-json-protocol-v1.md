# Unix Socket JSON Protocol v1 — Frozen M3 Contract

Status: **ACCEPTED FOR M4 IMPLEMENTATION**
Plan: **VCD-RQ-001**
Decision source: `RQ-M3-T05`

The exact decisions under **Frozen protocol v1** are normative. The diagnosis below records why the earlier draft was changed; references to unresolved draft choices are historical, not current blockers.

## Inherited decisions

- The server is a local debugging facility for repeated queries against exactly one VCD or FST waveform selected at process startup.
- The reusable `OpenedVcd`/query engine is authoritative; transport code must not parse VCD commands or change query semantics.
- Source content is immutable per generation. Admission and successful completion are generation-validated; stale streamed output is incomplete and must be discarded.
- V1 transport is a Unix-domain stream socket using UTF-8 JSON Lines. No TCP, HTTP, gRPC, WebSocket, or Windows named pipes.
- Accepted command shape is native `vcd_tools_rs serve <waveform> --socket <path>` on Unix.
- Existing CLI/Python semantics remain compatibility boundaries: inclusive windows, no synthesized value at `start`, duplicate request multiplicity/order, scalar text, known vectors up to 128 bits as integers, and body order.
- `QueryContext` already supplies cooperative cancellation, deadline, signal/row/logical-result-byte/command limits. Socket encoded-byte limits remain separate.
- `QueryError` supplies stable categories, but transport must refine `QueryError::Vcd(VcdError)` to protocol-specific codes.
- `compare` still materializes selected timelines and accepts a second path in the CLI; it is not safe for the first server surface.
- Sidecar/cache backends do not exist yet and must not be implied as callable v1 methods.

## Diagnosis

The draft has the right transport and lifecycle, but it is not yet a protocol freeze because it leaves four implementation-critical choices open:

1. typed value objects versus a single `value_text` field;
2. event versus aligned extraction layout;
3. list streaming versus unary behavior;
4. exact initial method/capability/error surface.

It also advertises cache methods before a cache exists and says the subcommand-versus-binary choice is unresolved even though G0 accepted the native `serve` subcommand. The packaging decision is now implemented: Unix PyPI wheels expose `vcd_tools_rs serve` through the compiled extension, while Windows wheels do not expose Unix server mode.

## Drift / contradiction check

- **Resolved earlier but still marked open:** `unix-json-protocol-v1.md` says subcommand versus dedicated binary remains to be finalized. D-007 already selected `vcd_tools_rs serve`.
- **Premature methods:** `cache.stats`, `cache.warm`, and `cache.evict` conflict with the current M2 engine and the plan’s cache-off/unimplemented state. Advertising unsupported methods weakens capability negotiation.
- **Unfrozen extraction:** allowing `events` or `aligned` conflicts with the engine seam. Only event streaming is currently semantics-preserving and bounded without another stateful presentation layer.
- **Two byte limits are conflated:** `QueryLimits::max_result_bytes` counts logical in-memory event bytes, not JSON bytes. The server must independently count every encoded frame plus newline.
- **Error taxonomy mismatch:** protocol codes are more specific than `QueryErrorCode::Vcd`; `IO_ERROR` is not meaningfully distinct in the current engine because non-stale `VcdError::Io` becomes `SourceUnavailable`.
- **Window behavior intentionally differs:** Rust compatibility accepts `start > end` as empty, while the server draft says `INVALID_WINDOW`. This is acceptable only as explicit transport validation, not an engine semantic change.
- **Pip/native command behavior:** Unix PyPI wheels and native Unix builds both expose `vcd_tools_rs serve` through the compiled Rust implementation. The query surface auto-detects VCD/FST content and `describe` reports `source_format`; Windows still does not expose Unix server mode.
- **Server compare conflicts with one-file ownership:** compare requires a second source and selected-timeline materialization. It must remain deferred.

## Recommendation

### Gate verdict

**PASS RQ-M3-T05 if the exact decisions below are adopted as the v1 freeze.** No additional framework/product decision is required for M4. If any item marked “frozen” is not accepted, T05 remains blocked because encoder/decoder golden fixtures cannot be written consistently.

## Frozen protocol v1

### 1. Availability and ownership

- One native process owns exactly one VCD fixed at startup.
- Unix only.
- Entry point: `vcd_tools_rs serve <WAVEFORM> --socket <PATH>`.
- No dedicated server binary in v1.
- On non-Unix targets, Unix server modules/dependencies are target-gated; existing library and native binaries continue to build. The `serve` variant may be absent on non-Unix.
- **Distribution freeze:** v1 server mode is supported from source builds, native Linux/macOS archives, and Unix PyPI wheels. The pip-installed `vcd_tools_rs serve` command calls the compiled extension. Windows wheels do not expose `serve`.
- Clients never supply the primary VCD path.

### 2. JSON Lines framing

- One UTF-8 JSON object followed by one `\n` per frame; literal newlines inside strings are invalid JSON and therefore invalid frames.
- Maximum request line: 1,048,576 bytes including the terminating newline.
- Incremental reader rejects an over-limit unterminated line before unbounded allocation.
- Request top-level shape is exactly:

```json
{"v":1,"id":"req-42","method":"extract","params":{}}
```

- `v` is JSON number `1`; `id` and `method` are strings; `params` is an object.
- IDs are non-empty UTF-8 strings of at most 128 bytes and unique among active requests on that connection.
- Unknown top-level fields are ignored for additive envelope evolution. Unknown method-parameter fields are rejected as `BAD_REQUEST` to prevent silent misspellings.
- Request/response field duplicates are rejected where the decoder can observe them; do not intentionally implement last-key-wins semantics.
- Response frames for one request have numeric `seq` starting at 0 and increasing by 1. `seq` is bounded to `u32`; configured row/chunk limits make overflow unreachable.
- Frames for different request IDs may interleave; one connection has a single writer that preserves each request’s order.
- Ordinary response frames always carry the request’s string ID.
- For malformed UTF-8/JSON, oversized framing, or an uncorrelatable duplicate-ID protocol violation, the server may emit one transport error with `"id":null` and then close the connection. It must not reuse the active request’s ID namespace.

### 3. Numeric encoding

Use JSON numbers only for protocol-bounded structural fields: `v` and `seq`. Use booleans for booleans. Encode all waveform values, timestamps, counters, sizes, limits, durations, generations, and statistics as canonical decimal strings.

- `u64`/`u128`: ASCII decimal, no sign, no leading zero except `"0"`.
- Request decimal strings violating that grammar or overflowing the target integer are `BAD_REQUEST`.
- `timeout_ms`, `max_rows`, `max_response_bytes`, `max_commands`, occurrence, width, signal counts, source size, row/byte statistics, and elapsed milliseconds are strings.
- Timescale is structured as `{ "magnitude":"1", "unit":"fs" }`, or `null`; do not expose a presentation-formatted `"1 fs"` as the normative form.

### 4. Signal value schema

Do not use a lone `value_text`. Freeze a tagged object preserving the current `ChangeValue` variant:

```json
{"kind":"integer","text":"10"}
{"kind":"float","text":"1.5","bits":"3ff8000000000000"}
{"kind":"text","text":"10xz"}
```

- Integer `text` is canonical unsigned decimal.
- Float `text` is Rust’s current display spelling. `bits` is exactly 16 lowercase hexadecimal digits containing `f64::to_bits()`. This preserves NaN payload/sign and signed zero even though JSON cannot represent non-finite values. Examples include `"NaN"`, `"inf"`, and `"-inf"` as strings.
- Text is the exact current `ChangeValue::Text` payload, not normalized/lowercased for output.
- No raw-VCD lexical fidelity is promised; this schema preserves the accepted M2 semantic value.
- Every extract/find event includes signal `width` as a decimal string so clients can width-format integer values without reparsing the catalog.

### 5. Initial methods

V1 M4 implements only:

| Method | Response | Frozen status |
|---|---|---|
| `ping` | unary | included |
| `describe` | unary | included |
| `list` | streamed | included |
| `metadata` | unary | included |
| `extract` | streamed | included; events only |
| `find` | unary | included |
| `toggles` | unary | included |
| `cancel` | unary, plus target terminal response | included |

Explicitly deferred/not advertised:

- `compare`
- `cache.stats`, `cache.warm`, `cache.evict`
- `shutdown`
- aligned extraction
- asynchronous job handles
- arbitrary source open/close/path methods

Future optional methods appear only in `describe` when implemented; an absent method returns `UNKNOWN_METHOD`.

### 6. Method schemas

#### `ping`

Params: `{}`.

Result:

```json
{"protocol":"1","package_version":"0.3.0","uptime_ms":"1234","ready":true}
```

#### `describe`

Params: `{}`.

Result includes:

```json
{
  "protocol":"1",
  "generation":"7",
  "source_format":"vcd",
  "signal_count":"193730",
  "timescale":{"magnitude":"1","unit":"fs"},
  "source_size":"4589561612",
  "immutable_source":true,
  "methods":["ping","describe","list","metadata","extract","find","toggles","cancel"],
  "schemas":["signal_name.v1","time_value.v1"],
  "backends":["stream"],
  "limits":{
    "connections":"32",
    "active_requests_per_connection":"8",
    "workers":"4",
    "queued_jobs":"64",
    "output_chunks_per_connection":"8",
    "request_line_bytes":"1048576",
    "request_id_bytes":"128",
    "signals_per_query":"4096",
    "response_rows":"1000000",
    "response_bytes":"268435456",
    "commands":"1000000000",
    "timeout_ms":"120000",
    "chunk_rows":"1024",
    "chunk_bytes":"262144",
    "simultaneous_expensive_scans":"1"
  },
  "security":{
    "transport":"unix",
    "socket_mode":"0600",
    "peer_uid_check":false
  },
  "distribution":{
    "native_serve":true,
    "python_console_serve":true
  }
}
```

Do not return an unrestricted path by default. A display basename may be included only if explicitly configured.

#### `list`

Params:

```json
{"filter":"clk","max_rows":"10000","max_response_bytes":"16777216","timeout_ms":"30000"}
```

All fields optional. `filter` is the accepted case-sensitive substring filter. Response schema is `signal_name.v1`; each chunk’s `rows` is an array of signal-name strings in declaration order. It is always streamed, including an empty result.

#### `metadata`

Params permit only lower per-request `timeout_ms` and `max_commands`. Result:

```json
{
  "signal_count":"193730",
  "timescale":{"magnitude":"1","unit":"fs"},
  "start_time":"0",
  "end_time":"65290706000000"
}
```

The request performs/joins the lazy cancellable scan. No job-state response in v1.

#### `extract`

Params:

```json
{
  "signals":["tb.clk","tb.rst_n"],
  "start":"0",
  "end":"1000000",
  "max_rows":"100000",
  "max_response_bytes":"16777216",
  "max_commands":"10000000",
  "timeout_ms":"30000"
}
```

- `signals` is required, non-empty, and preserves order and duplicates.
- `start`/`end` are optional inclusive `u64` decimal strings.
- `start > end` is a transport-level `INVALID_WINDOW` before engine invocation.
- There is no `layout` parameter in v1.
- Stream schema is `time_value.v1`.
- Each row is:

```json
{
  "signal":"tb.counter[3:0]",
  "time":"1000",
  "width":"4",
  "value":{"kind":"integer","text":"10"}
}
```

Rows preserve accepted body order, alias expansion, and duplicate request multiplicity.

#### `find`

Exact params object:

```json
{
  "signal":"tb.done",
  "value":"1",
  "occurrence":"1",
  "start":"0",
  "end":"1000",
  "max_commands":"10000000",
  "max_response_bytes":"16777216",
  "timeout_ms":"30000"
}
```

- `signal` and `value` are required JSON strings.
- `value` uses existing `parse_target_value` text semantics.
- `occurrence` is an optional decimal `usize` string defaulting to `"1"`; zero is `INVALID_OCCURRENCE`.
- `start` and `end` are optional inclusive decimal `u64` strings; `start > end` is `INVALID_WINDOW`.
- `max_commands`, `max_response_bytes`, and `timeout_ms` are optional decimal strings that may only lower server limits.
- No other fields are accepted.

Result:

```json
{"found":true,"signal":"tb.done","time":"1000","width":"1","value":{"kind":"text","text":"1"}}
```

Not found returns the same keys with `found:false`, `time:null`, `value:null`; width remains present.

#### `toggles`

Exact params object:

```json
{
  "signals":["tb.clk","tb.enable"],
  "start":"0",
  "end":"1000",
  "max_commands":"10000000",
  "max_response_bytes":"16777216",
  "timeout_ms":"30000"
}
```

- `signals` is a required non-empty array of JSON strings; order and duplicates are preserved and the server signal-count limit applies.
- `start` and `end` are optional inclusive decimal `u64` strings; `start > end` is `INVALID_WINDOW`.
- `max_commands`, `max_response_bytes`, and `timeout_ms` are optional decimal strings that may only lower server limits.
- No other fields are accepted.

Result uses an ordered array rather than a JSON object, preserving requested order/duplicates:

```json
{"rows":[{"signal":"tb.clk","toggles":"500"}]}
```

#### `cancel`

Params: `{ "request_id":"req-42" }`. It can target only an active request on the same connection. Result: `{ "found":true }` or false. The target independently terminates with `CANCELLED` unless completion won the race.

### 7. Response lifecycle

Unary success/error remains as drafted:

```json
{"v":1,"id":"x","seq":0,"type":"result","ok":true,"result":{}}
{"v":1,"id":"x","seq":0,"type":"error","ok":false,"error":{"code":"BAD_REQUEST","message":"invalid request","details":{},"retryable":false}}
```

Streaming lifecycle is exactly one `begin`, zero or more `chunk`, and one `end`:

```json
{"v":1,"id":"x","seq":0,"type":"begin","schema":"time_value.v1","generation":"7"}
{"v":1,"id":"x","seq":1,"type":"chunk","rows":[...]}
{"v":1,"id":"x","seq":2,"type":"end","complete":true,"stats":{"rows":"1000","encoded_bytes":"73500","elapsed_ms":"12","commands":"5000","backend":"stream"}}
```

- Chunk caps: 1,024 rows and 262,144 encoded bytes, whichever occurs first.
- `max_response_bytes` counts the UTF-8 bytes of every serialized response frame including each terminating newline.
- Reserve 4,096 bytes from the response budget for a terminal `end` frame. Reject a requested response limit below 4,096 as `BAD_REQUEST`.
- Before sending a chunk, ensure the chunk plus terminal reserve fits. If not, do not send that chunk; send `end` with `complete:false` and `LIMIT_EXCEEDED`.
- `max_rows` is checked before encoding/delivering a row, matching the engine’s no-over-limit-row behavior.
- Actual encoded bytes are authoritative for the socket. Engine logical-result-byte accounting is an independent inner guard and must not be reported as socket bytes.
- Successful `end.stats` always has exactly `rows`, `encoded_bytes`, `elapsed_ms`, `commands`, and `backend`. Numeric fields are decimal strings; non-scanning operations report commands as `"0"`; backend is `"stream"` in M4.
- If any parse/I/O/stale/cancel/deadline/limit/internal error occurs after `begin`, terminate with `end`, `complete:false`, and an error object. Clients must discard prior rows for transactional correctness.
- If the same error occurs before `begin`, emit `type:error`.
- Disconnect cancels all connection-owned requests and emits nothing further.

### 8. Error mapping

Stable error envelope:

```json
{"code":"SIGNAL_NOT_FOUND","message":"one or more signals were not found","details":{},"retryable":false}
```

Frozen mapping:

| Source | Protocol code | Retryable |
|---|---|---|
| framing/JSON/type/decimal/unknown parameter | `BAD_REQUEST` | false |
| `v != 1` | `UNSUPPORTED_VERSION` | false |
| absent method | `UNKNOWN_METHOD` | false |
| reused active ID / invalid ID | `DUPLICATE_REQUEST_ID` or `BAD_REQUEST` | false |
| missing signal(s) | `SIGNAL_NOT_FOUND` | false |
| `start > end` | `INVALID_WINDOW` | false |
| occurrence zero | `INVALID_OCCURRENCE` | false |
| any configured query/socket limit | `LIMIT_EXCEEDED` | false |
| bounded scheduler full | `QUEUE_FULL` | true |
| cancellation | `CANCELLED` | false |
| deadline | `DEADLINE_EXCEEDED` | true |
| generation mismatch | `STALE_SOURCE` | true |
| non-stale `QueryError::SourceUnavailable` | `SOURCE_UNAVAILABLE` | true |
| missing enddefinitions, duplicate VCD declaration, VCD parser failure, other `QueryError::Vcd` | `PARSE_ERROR` | false |
| FST signal-data parser failure or contained FST parser panic | `FST_ERROR` | false |
| panic/invariant/unclassified failure | `INTERNAL` | false |

Remove `IO_ERROR` and `UNSUPPORTED_PLATFORM` from request-level v1 codes: current engine does not distinguish a useful generic I/O category, and unsupported server platforms cannot establish the Unix protocol. VCD failures retain `PARSE_ERROR`; FST parser failures use the format-specific `FST_ERROR` category.

`LIMIT_EXCEEDED.details` includes `kind`, `limit`, and `actual` as strings. Signal errors include a bounded ordered `signals` array. Unsupported version includes `supported_versions:["1"]`. Details and message lengths are bounded.

### 9. Limits and scheduling contract

Freeze these M4 defaults; CLI flags may lower/raise them only within implementation hard maxima:

- connections: 32
- active requests per connection: 8
- workers: 4
- global queued jobs: 64
- per-connection queued output chunks: 8
- request line: 1 MiB
- request ID: 128 bytes
- signals per query: 4,096
- response rows: 1,000,000
- encoded response bytes: 256 MiB
- commands: 1,000,000,000
- timeout: 120,000 ms
- chunk rows: 1,024
- chunk encoded bytes: 256 KiB
- simultaneous expensive scans for the one generation: 1

Per-request values may only lower server-configured limits. Every queue is bounded. Output-queue send uses timed waits (at most the query poll interval) so cancellation/deadline is observable under backpressure. `describe` returns effective limits.

The thread/channel versus async implementation remains T04’s decision; this protocol does not require either. It does require a connection reader to remain active while work executes, one serialized connection writer, and a request registry with cancellation tokens.

### 10. Capability negotiation

- No separate handshake method.
- Every request declares `v:1`.
- `describe` is the capability endpoint and returns exact implemented methods, schemas, backends, and limits.
- Do not advertise future cache/sidecar methods or backends.
- When optional backends later exist, backend choice remains server policy unless a versioned request parameter is explicitly added.
- Unknown response fields are ignored by clients; unknown frame `type` or schema is fatal to that request.
- Incompatible field meaning requires protocol v2.

### 11. Socket security/lifecycle freeze

- Required explicit socket path.
- Parent directory must already exist unless a separately explicit `--create-socket-dir` flag is added.
- Restrictive umask plus post-bind owner-only mode `0600`.
- Refuse to unlink a non-socket path.
- Before treating a socket as stale, attempt connection; a live listener blocks startup. Only an owned socket in an owner-controlled directory may be removed after a failed connect.
- Graceful shutdown stops admission, cancels or waits for in-flight requests within a configured grace period, closes listener, and removes only the inode created by this process.
- Peer-UID equality should be enforced where stable APIs are available; otherwise filesystem permissions are the boundary. `describe.security.peer_uid_check` reports whether the check is active; `describe.security.transport` and `socket_mode` use the exact fields shown above.
- `describe.distribution` reports native/Python-console server availability using the exact booleans shown above.
- No waveform values in default logs.

## Risks

- The default limits are candidates that must survive the T04 scheduler/load spike; if measurements require adjustment before M4, change them through one recorded protocol decision before golden fixtures are declared stable.
- Float `bits` exposes exact engine `f64` representation but not original VCD lexical spelling; documentation must distinguish semantic from lexical fidelity.
- Streaming is not rollback-capable. The `complete:false` contract relies on clients honoring “discard prior chunks”; examples and client fixture must enforce this.
- Complete header/sample generation validation adds fixed work per query. Scheduler benchmarks must include that cost.
- Unix wheel and native-console users may both invoke `serve`; release tests must keep their options and behavior aligned.
- A 120-second default can still reject unusual multi-gigabyte scans; operators can raise the server maximum explicitly, while clients may only lower it.

## Accepted consequential clarifications

1. Server mode is available through the native binary and the pip Python console on Unix; Windows does not expose the Unix server.
2. The value schema is tagged `integer|float|text`, with exact float bits and event width; no `value_text` or aligned-extraction alternative exists in v1.
3. M4 must add golden JSONL fixtures before listener/service integration.
