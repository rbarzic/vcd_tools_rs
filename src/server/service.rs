//! Production protocol dispatcher backed by one replaceable `OpenedVcd` generation.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Value, json, to_value};

use crate::opened::OpenedVcd;
use crate::query::{QueryContext, QueryError, QueryLimits};
use crate::server::protocol::{
    CHUNK_BYTES, CHUNK_ROWS, DescribeResult, DistributionCapabilities, FindResult, MetadataResult,
    PingResult, ProtocolError, ProtocolErrorCode, Request, RequestMethod, ResponseFrame,
    SecurityCapabilities, ServerLimits, StreamStats, TimeValueRow, ToggleRow, TogglesResult,
    WireTimescale, WireValue, protocol_error_from_query,
};
use crate::server::runtime::{Dispatcher, OutputSink, RequestContext, RuntimeConfig};
use crate::{TargetValue, TimeWindow, parse_target_value};

const METHODS: [&str; 8] = [
    "ping", "describe", "list", "metadata", "extract", "find", "toggles", "cancel",
];
const SCHEMAS: [&str; 2] = ["signal_name.v1", "time_value.v1"];
pub const HARD_MAX_SIGNALS: usize = 65_536;
pub const HARD_MAX_ROWS: u64 = 16_000_000;
pub const HARD_MAX_RESPONSE_BYTES: u64 = 1_073_741_824;
pub const HARD_MAX_COMMANDS: u64 = 4_000_000_000;

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub max_signals: usize,
    pub max_rows: u64,
    pub max_response_bytes: u64,
    pub max_commands: u64,
}

impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            max_signals: 4_096,
            max_rows: 1_000_000,
            max_response_bytes: 268_435_456,
            max_commands: 1_000_000_000,
        }
    }
}

impl ServiceConfig {
    pub fn validate(&self) -> std::io::Result<()> {
        if self.max_signals == 0
            || self.max_signals > HARD_MAX_SIGNALS
            || self.max_rows == 0
            || self.max_rows > HARD_MAX_ROWS
            || self.max_response_bytes < crate::server::protocol::TERMINAL_FRAME_RESERVE_BYTES
            || self.max_response_bytes > HARD_MAX_RESPONSE_BYTES
            || self.max_commands == 0
            || self.max_commands > HARD_MAX_COMMANDS
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "server query limits are outside supported bounds",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct GenerationSlot {
    path: PathBuf,
    current: Mutex<Arc<OpenedVcd>>,
}

impl GenerationSlot {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProtocolError> {
        let display_path = path.as_ref().to_path_buf();
        let opened = OpenedVcd::open(&display_path).map_err(map_vcd)?;
        let path = opened.configured_path().to_path_buf();
        Ok(Self {
            path,
            current: Mutex::new(Arc::new(opened)),
        })
    }

    /// Return the current complete generation, atomically replacing it between
    /// requests when the configured path no longer identifies that generation.
    pub fn current(&self) -> Result<Arc<OpenedVcd>, ProtocolError> {
        let mut current = self.current.lock().expect("generation slot lock");
        if current.validate_source().is_ok() {
            return Ok(Arc::clone(&current));
        }
        let replacement = Arc::new(OpenedVcd::open(&self.path).map_err(map_vcd)?);
        *current = Arc::clone(&replacement);
        Ok(replacement)
    }
}

pub struct VcdService {
    generation: Arc<GenerationSlot>,
    runtime: RuntimeConfig,
    limits: ServiceConfig,
    started: Instant,
    hooks: ServiceHooks,
}

#[derive(Default)]
struct ServiceHooks {
    before_metadata_publish: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    after_stream_chunk: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl std::fmt::Debug for VcdService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VcdService")
            .field("runtime", &self.runtime)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl VcdService {
    pub fn new(
        path: impl AsRef<Path>,
        runtime: RuntimeConfig,
        limits: ServiceConfig,
    ) -> Result<Self, ProtocolError> {
        runtime.validate().map_err(map_io)?;
        limits.validate().map_err(map_io)?;
        Ok(Self {
            generation: Arc::new(GenerationSlot::open(path)?),
            runtime,
            limits,
            started: Instant::now(),
            hooks: ServiceHooks::default(),
        })
    }

    pub fn generation_slot(&self) -> &Arc<GenerationSlot> {
        &self.generation
    }

    #[doc(hidden)]
    pub fn set_before_metadata_publish_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .hooks
            .before_metadata_publish
            .lock()
            .expect("metadata publish hook") = Some(hook);
    }

    #[doc(hidden)]
    pub fn set_after_stream_chunk_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .hooks
            .after_stream_chunk
            .lock()
            .expect("stream chunk hook") = Some(hook);
    }

    fn before_metadata_publish(&self) {
        if let Some(hook) = self
            .hooks
            .before_metadata_publish
            .lock()
            .expect("metadata publish hook")
            .take()
        {
            hook();
        }
    }

    fn after_stream_chunk(&self) {
        if let Some(hook) = self
            .hooks
            .after_stream_chunk
            .lock()
            .expect("stream chunk hook")
            .take()
        {
            hook();
        }
    }

    fn query_context(
        &self,
        runtime: &RequestContext,
        requested_commands: Option<u64>,
        requested_rows: Option<u64>,
        requested_response: Option<u64>,
    ) -> Result<(QueryContext, u64), ProtocolError> {
        let commands = lower_limit("commands", requested_commands, self.limits.max_commands)?;
        let rows = lower_limit("rows", requested_rows, self.limits.max_rows)?;
        let response = lower_limit(
            "response_bytes",
            requested_response,
            self.limits.max_response_bytes,
        )?;
        if response < crate::server::protocol::TERMINAL_FRAME_RESERVE_BYTES {
            return Err(ProtocolError::bad_request(
                "max_response_bytes must reserve at least 4096 terminal bytes",
            ));
        }
        let limits = QueryLimits::unlimited()
            .with_max_signals(self.limits.max_signals)
            .with_max_rows(rows)
            .with_max_result_bytes(response)
            .with_max_commands(commands);
        Ok((runtime.query_context(limits), response))
    }

    fn current(&self, runtime: &RequestContext) -> Result<Arc<OpenedVcd>, ProtocolError> {
        runtime.check()?;
        let opened = self.generation.current()?;
        runtime.check()?;
        Ok(opened)
    }

    fn dispatch_inner(
        &self,
        request: Request,
        runtime: &RequestContext,
        output: &mut OutputSink,
    ) -> Result<(), ProtocolError> {
        let id = request.id;
        match request.method {
            RequestMethod::Ping => {
                let result = PingResult {
                    protocol: "1".into(),
                    package_version: env!("CARGO_PKG_VERSION").into(),
                    uptime_ms: self.started.elapsed().as_millis().to_string(),
                    ready: self.generation.current().is_ok(),
                };
                send_unary(
                    output,
                    &id,
                    &result,
                    self.limits.max_response_bytes,
                    self.runtime.max_encoded_frame_bytes as u64,
                )
            }
            RequestMethod::Describe => {
                let opened = self.current(runtime)?;
                let result = self.describe(&opened);
                opened.validate_source().map_err(map_vcd)?;
                send_unary(
                    output,
                    &id,
                    &result,
                    self.limits.max_response_bytes,
                    self.runtime.max_encoded_frame_bytes as u64,
                )
            }
            RequestMethod::List(params) => {
                let (query, response_limit) =
                    self.query_context(runtime, None, params.max_rows, params.max_response_bytes)?;
                let opened = self.current(runtime)?;
                let mut stream = StreamOutput::begin(
                    output,
                    id,
                    "signal_name.v1",
                    opened.generation().get(),
                    response_limit,
                    self.runtime.max_encoded_frame_bytes as u64,
                )?;
                let mut rows = Vec::new();
                let max_rows = query.limits().max_rows().unwrap_or(self.limits.max_rows);
                for name in opened.signal_names() {
                    query.check().map_err(map_query)?;
                    if params
                        .filter
                        .as_ref()
                        .is_some_and(|filter| !name.contains(filter))
                    {
                        continue;
                    }
                    if stream.rows + rows.len() as u64 >= max_rows {
                        return Err(limit_error("rows", max_rows, max_rows.saturating_add(1)));
                    }
                    rows.push(Value::String(name.to_string()));
                    if rows.len() == CHUNK_ROWS as usize {
                        stream.send_rows(output, std::mem::take(&mut rows))?;
                        self.after_stream_chunk();
                    }
                }
                if !rows.is_empty() {
                    stream.send_rows(output, rows)?;
                    self.after_stream_chunk();
                }
                opened.validate_source().map_err(map_vcd)?;
                query.check().map_err(map_query)?;
                stream.finish(output, 0)
            }
            RequestMethod::Metadata(params) => {
                let (query, response_limit) =
                    self.query_context(runtime, params.max_commands, None, None)?;
                let opened = self.current(runtime)?;
                let metadata = runtime.with_expensive_scan(|| {
                    opened.metadata_with_context(&query).map_err(map_query)
                })?;
                let result = MetadataResult {
                    signal_count: metadata.signal_count.to_string(),
                    timescale: metadata.timescale.as_ref().map(WireTimescale::from),
                    start_time: metadata.start_time.to_string(),
                    end_time: metadata.end_time.to_string(),
                };
                self.before_metadata_publish();
                query.check().map_err(map_query)?;
                opened.validate_source().map_err(map_vcd)?;
                query.check().map_err(map_query)?;
                send_unary(
                    output,
                    &id,
                    &result,
                    response_limit,
                    self.runtime.max_encoded_frame_bytes as u64,
                )
            }
            RequestMethod::Extract(params) => {
                let (query, response_limit) = self.query_context(
                    runtime,
                    params.max_commands,
                    params.max_rows,
                    params.max_response_bytes,
                )?;
                let opened = self.current(runtime)?;
                let window = TimeWindow {
                    start: params.start,
                    end: params.end,
                };
                let mut events = opened
                    .extract(&params.signals, window, &query)
                    .map_err(map_query)?;
                let mut stream = StreamOutput::begin(
                    output,
                    id,
                    "time_value.v1",
                    opened.generation().get(),
                    response_limit,
                    self.runtime.max_encoded_frame_bytes as u64,
                )?;
                loop {
                    let batch = runtime.with_expensive_scan(|| {
                        let mut rows = Vec::with_capacity(CHUNK_ROWS as usize);
                        while rows.len() < CHUNK_ROWS as usize {
                            match events.next() {
                                Some(Ok(event)) => {
                                    let width = opened
                                        .signal(&event.signal)
                                        .map_or(1, |signal| signal.size());
                                    rows.push(
                                        to_value(TimeValueRow {
                                            signal: event.signal,
                                            time: event.time.to_string(),
                                            width: width.to_string(),
                                            value: WireValue::from(&event.value),
                                        })
                                        .map_err(internal_serialization)?,
                                    );
                                }
                                Some(Err(error)) => return Err(map_query(error)),
                                None => return Ok((rows, true)),
                            }
                        }
                        Ok((rows, false))
                    })?;
                    if !batch.0.is_empty() {
                        stream.send_rows(output, batch.0)?;
                        self.after_stream_chunk();
                    }
                    if batch.1 {
                        break;
                    }
                }
                stream.finish(output, events.commands_processed())
            }
            RequestMethod::Find(params) => {
                let (query, response_limit) = self.query_context(
                    runtime,
                    params.max_commands,
                    None,
                    params.max_response_bytes,
                )?;
                let opened = self.current(runtime)?;
                let target: TargetValue = parse_target_value(&params.value);
                let (event, width) = runtime.with_expensive_scan(|| {
                    opened
                        .find_nth_occurrence(
                            &params.signal,
                            target,
                            params.occurrence,
                            TimeWindow {
                                start: params.start,
                                end: params.end,
                            },
                            &query,
                        )
                        .map_err(map_query)
                })?;
                let result = FindResult {
                    found: event.is_some(),
                    signal: params.signal,
                    time: event.as_ref().map(|event| event.time.to_string()),
                    width: width.to_string(),
                    value: event.as_ref().map(|event| WireValue::from(&event.value)),
                };
                send_unary(
                    output,
                    &id,
                    &result,
                    response_limit,
                    self.runtime.max_encoded_frame_bytes as u64,
                )
            }
            RequestMethod::Toggles(params) => {
                let (query, response_limit) = self.query_context(
                    runtime,
                    params.max_commands,
                    None,
                    params.max_response_bytes,
                )?;
                let opened = self.current(runtime)?;
                let counts = runtime.with_expensive_scan(|| {
                    opened
                        .count_toggles(
                            &params.signals,
                            TimeWindow {
                                start: params.start,
                                end: params.end,
                            },
                            &query,
                        )
                        .map_err(map_query)
                })?;
                let rows = params
                    .signals
                    .into_iter()
                    .map(|signal| ToggleRow {
                        toggles: counts.get(&signal).copied().unwrap_or(0).to_string(),
                        signal,
                    })
                    .collect();
                send_unary(
                    output,
                    &id,
                    &TogglesResult { rows },
                    response_limit,
                    self.runtime.max_encoded_frame_bytes as u64,
                )
            }
            RequestMethod::Cancel(_) => Err(ProtocolError::new(
                ProtocolErrorCode::Internal,
                "cancel must be handled by the connection runtime",
                json!({}),
                false,
            )),
        }
    }

    fn describe(&self, opened: &OpenedVcd) -> DescribeResult {
        DescribeResult {
            protocol: "1".into(),
            generation: opened.generation().get().to_string(),
            signal_count: opened.signal_count().to_string(),
            timescale: opened.timescale().map(WireTimescale::from),
            source_size: opened.identity().len().to_string(),
            immutable_source: true,
            methods: METHODS.into_iter().map(str::to_string).collect(),
            schemas: SCHEMAS.into_iter().map(str::to_string).collect(),
            backends: vec!["stream".into()],
            limits: ServerLimits {
                connections: self.runtime.max_connections.to_string(),
                active_requests_per_connection: self
                    .runtime
                    .max_active_requests_per_connection
                    .to_string(),
                workers: self.runtime.workers.to_string(),
                queued_jobs: self.runtime.queue_depth.to_string(),
                output_chunks_per_connection: self.runtime.output_chunks.to_string(),
                request_line_bytes: self.runtime.request_line_bytes.to_string(),
                request_id_bytes: crate::server::protocol::MAX_REQUEST_ID_BYTES.to_string(),
                signals_per_query: self.limits.max_signals.to_string(),
                response_rows: self.limits.max_rows.to_string(),
                response_bytes: self.limits.max_response_bytes.to_string(),
                commands: self.limits.max_commands.to_string(),
                timeout_ms: self.runtime.max_timeout_ms.to_string(),
                chunk_rows: CHUNK_ROWS.to_string(),
                chunk_bytes: CHUNK_BYTES.to_string(),
                simultaneous_expensive_scans: "1".into(),
            },
            security: SecurityCapabilities {
                transport: "unix".into(),
                socket_mode: "0600".into(),
                peer_uid_check: false,
            },
            distribution: DistributionCapabilities {
                native_serve: true,
                python_console_serve: true,
            },
        }
    }
}

impl Dispatcher for VcdService {
    fn dispatch(
        &self,
        request: Request,
        context: &RequestContext,
        output: &mut OutputSink,
    ) -> Result<(), ProtocolError> {
        self.dispatch_inner(request, context, output)
    }
}

struct StreamOutput {
    id: String,
    next_seq: u32,
    encoded_bytes: u64,
    rows: u64,
    response_limit: u64,
    frame_limit: u64,
    started: Instant,
}

impl StreamOutput {
    fn begin(
        output: &mut OutputSink,
        id: String,
        schema: &str,
        generation: u64,
        response_limit: u64,
        frame_limit: u64,
    ) -> Result<Self, ProtocolError> {
        let frame = ResponseFrame::begin(&id, schema, generation.to_string());
        let bytes = frame_bytes(&frame, frame_limit)?;
        ensure_response_budget(0, bytes, response_limit)?;
        output.send_frame(&frame)?;
        Ok(Self {
            id,
            next_seq: 1,
            encoded_bytes: bytes,
            rows: 0,
            response_limit,
            frame_limit,
            started: Instant::now(),
        })
    }

    fn send_rows(
        &mut self,
        output: &mut OutputSink,
        rows: Vec<Value>,
    ) -> Result<(), ProtocolError> {
        if rows.is_empty() {
            return Ok(());
        }
        self.send_rows_split(output, rows)
    }

    fn send_rows_split(
        &mut self,
        output: &mut OutputSink,
        rows: Vec<Value>,
    ) -> Result<(), ProtocolError> {
        let frame = ResponseFrame::chunk(&self.id, self.next_seq, rows.clone());
        let Some(bytes) = try_frame_bytes(&frame, self.frame_limit)? else {
            if rows.len() == 1 {
                return Err(limit_error(
                    "chunk_bytes",
                    self.frame_limit,
                    self.frame_limit + 1,
                ));
            }
            let midpoint = rows.len() / 2;
            self.send_rows_split(output, rows[..midpoint].to_vec())?;
            return self.send_rows_split(output, rows[midpoint..].to_vec());
        };
        ensure_response_budget(self.encoded_bytes, bytes, self.response_limit)?;
        output.send_frame(&frame)?;
        self.encoded_bytes = self.encoded_bytes.saturating_add(bytes);
        self.rows = self.rows.saturating_add(rows.len() as u64);
        self.next_seq = self.next_seq.checked_add(1).ok_or_else(|| {
            ProtocolError::new(
                ProtocolErrorCode::Internal,
                "response sequence overflow",
                json!({}),
                false,
            )
        })?;
        Ok(())
    }

    fn finish(self, output: &mut OutputSink, commands: u64) -> Result<(), ProtocolError> {
        let stats = StreamStats {
            rows: self.rows.to_string(),
            encoded_bytes: "0".into(),
            elapsed_ms: self.started.elapsed().as_millis().to_string(),
            commands: commands.to_string(),
            backend: "stream".into(),
        };
        let frame = ResponseFrame::end_success(&self.id, self.next_seq, stats, self.encoded_bytes)
            .map_err(internal_serialization)?;
        let bytes = frame_bytes(&frame, self.frame_limit)?;
        if self.encoded_bytes.saturating_add(bytes) > self.response_limit {
            return Err(limit_error(
                "response_bytes",
                self.response_limit,
                self.encoded_bytes.saturating_add(bytes),
            ));
        }
        output.send_frame(&frame)
    }
}

fn send_unary<T: Serialize>(
    output: &mut OutputSink,
    id: &str,
    result: &T,
    response_limit: u64,
    frame_limit: u64,
) -> Result<(), ProtocolError> {
    let frame = ResponseFrame::result(id, result).map_err(internal_serialization)?;
    let bytes = frame_bytes(&frame, frame_limit)?;
    if bytes > response_limit {
        return Err(limit_error("response_bytes", response_limit, bytes));
    }
    output.send_frame(&frame)
}

fn frame_bytes(frame: &ResponseFrame, frame_limit: u64) -> Result<u64, ProtocolError> {
    try_frame_bytes(frame, frame_limit)?
        .ok_or_else(|| limit_error("encoded_frame_bytes", frame_limit, frame_limit + 1))
}

fn try_frame_bytes(frame: &ResponseFrame, frame_limit: u64) -> Result<Option<u64>, ProtocolError> {
    let mut counter = CappedCounter::new(frame_limit as usize);
    match serde_json::to_writer(&mut counter, frame) {
        Ok(()) => Ok(Some(counter.written as u64 + 1)),
        Err(_) if counter.exceeded => Ok(None),
        Err(_) => Err(ProtocolError::new(
            ProtocolErrorCode::Internal,
            "response serialization failed",
            json!({}),
            false,
        )),
    }
}

struct CappedCounter {
    cap: usize,
    written: usize,
    exceeded: bool,
}

impl CappedCounter {
    fn new(line_cap: usize) -> Self {
        Self {
            cap: line_cap.saturating_sub(1),
            written: 0,
            exceeded: false,
        }
    }
}

impl Write for CappedCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let attempted = self.written.saturating_add(buffer.len());
        if attempted > self.cap {
            self.exceeded = true;
            return Err(io::Error::other("encoded frame exceeds cap"));
        }
        self.written = attempted;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn ensure_response_budget(current: u64, next: u64, limit: u64) -> Result<(), ProtocolError> {
    let actual = current
        .saturating_add(next)
        .saturating_add(crate::server::protocol::TERMINAL_FRAME_RESERVE_BYTES);
    if actual > limit {
        Err(limit_error("response_bytes", limit, actual))
    } else {
        Ok(())
    }
}

fn lower_limit(kind: &str, requested: Option<u64>, configured: u64) -> Result<u64, ProtocolError> {
    let value = requested.unwrap_or(configured);
    if value > configured {
        Err(ProtocolError::bad_request(format!(
            "{kind} may only lower the configured maximum"
        )))
    } else {
        Ok(value)
    }
}

fn limit_error(kind: &str, limit: u64, actual: u64) -> ProtocolError {
    ProtocolError::new(
        ProtocolErrorCode::LimitExceeded,
        "query limit exceeded",
        json!({"kind":kind,"limit":limit.to_string(),"actual":actual.to_string()}),
        false,
    )
}

fn map_query(error: QueryError) -> ProtocolError {
    protocol_error_from_query(&error)
}

fn map_vcd(error: crate::VcdError) -> ProtocolError {
    map_query(QueryError::from(error))
}

fn map_io(error: std::io::Error) -> ProtocolError {
    ProtocolError::new(
        ProtocolErrorCode::SourceUnavailable,
        "source unavailable",
        json!({"kind":error.kind().to_string()}),
        true,
    )
}

fn internal_serialization(error: serde_json::Error) -> ProtocolError {
    let _ = error;
    ProtocolError::new(
        ProtocolErrorCode::Internal,
        "response serialization failed",
        json!({}),
        false,
    )
}
