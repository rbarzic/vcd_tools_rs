//! Bounded Unix connection scheduler and cancellation-aware output runtime.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;

use crate::query::CancellationToken;
use crate::server::protocol::{
    CancelResult, JsonLineAccumulator, ProtocolError, ProtocolErrorCode, Request, RequestMethod,
    ResponseFrame, decode_request_with_active, encode_json_line,
};

const POLL_INTERVAL: Duration = Duration::from_millis(2);
const HARD_MAX_TIMEOUT_MS: u64 = 31_536_000_000; // one year

#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub max_connections: usize,
    pub max_active_requests_per_connection: usize,
    pub workers: usize,
    pub queue_depth: usize,
    pub output_chunks: usize,
    pub request_line_bytes: usize,
    pub max_encoded_frame_bytes: usize,
    pub max_timeout_ms: u64,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_connections: 32,
            max_active_requests_per_connection: 8,
            workers: 4,
            queue_depth: 64,
            output_chunks: 8,
            request_line_bytes: crate::server::protocol::MAX_REQUEST_LINE_BYTES,
            max_encoded_frame_bytes: crate::server::protocol::CHUNK_BYTES as usize,
            max_timeout_ms: 120_000,
        }
    }
}

impl RuntimeConfig {
    pub fn validate(&self) -> io::Result<()> {
        if self.max_connections == 0
            || self.max_active_requests_per_connection == 0
            || self.workers == 0
            || self.queue_depth == 0
            || self.output_chunks == 0
            || self.request_line_bytes == 0
            || self.max_encoded_frame_bytes
                < crate::server::protocol::TERMINAL_FRAME_RESERVE_BYTES as usize
            || self.max_timeout_ms == 0
            || self.max_timeout_ms > HARD_MAX_TIMEOUT_MS
            || Instant::now()
                .checked_add(Duration::from_millis(self.max_timeout_ms))
                .is_none()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "server runtime limits must be non-zero",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct RuntimeStats {
    queued: AtomicUsize,
    active: AtomicUsize,
    completed: AtomicUsize,
    rejected: AtomicUsize,
    panicked: AtomicUsize,
    connections: AtomicUsize,
    output_blocked: AtomicUsize,
    forced_disconnects: AtomicUsize,
    max_frame_buffer_bytes: AtomicUsize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStatsSnapshot {
    pub queued: usize,
    pub active: usize,
    pub completed: usize,
    pub rejected: usize,
    pub panicked: usize,
    pub connections: usize,
    pub output_blocked: usize,
    pub forced_disconnects: usize,
    pub max_frame_buffer_bytes: usize,
}

impl RuntimeStats {
    pub fn snapshot(&self) -> RuntimeStatsSnapshot {
        RuntimeStatsSnapshot {
            queued: self.queued.load(Ordering::Acquire),
            active: self.active.load(Ordering::Acquire),
            completed: self.completed.load(Ordering::Acquire),
            rejected: self.rejected.load(Ordering::Acquire),
            panicked: self.panicked.load(Ordering::Acquire),
            connections: self.connections.load(Ordering::Acquire),
            output_blocked: self.output_blocked.load(Ordering::Acquire),
            forced_disconnects: self.forced_disconnects.load(Ordering::Acquire),
            max_frame_buffer_bytes: self.max_frame_buffer_bytes.load(Ordering::Acquire),
        }
    }
}

#[derive(Debug)]
struct ActiveRequest {
    cancellation: CancellationToken,
}

#[derive(Debug)]
pub struct ConnectionState {
    active: Mutex<HashMap<String, ActiveRequest>>,
    disconnected: Arc<AtomicBool>,
    max_active: usize,
}

impl ConnectionState {
    fn new(max_active: usize, disconnected: Arc<AtomicBool>) -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
            disconnected,
            max_active,
        }
    }

    pub fn is_active(&self, id: &str) -> bool {
        self.active
            .lock()
            .expect("active request lock")
            .contains_key(id)
    }

    fn register(self: &Arc<Self>, id: &str) -> Result<CancellationToken, ProtocolError> {
        let mut active = self.active.lock().expect("active request lock");
        if active.contains_key(id) {
            return Err(ProtocolError::duplicate_request_id(id));
        }
        if active.len() >= self.max_active {
            return Err(ProtocolError::new(
                ProtocolErrorCode::LimitExceeded,
                "active requests per connection limit exceeded",
                json!({"kind":"active_requests","limit":self.max_active.to_string(),"actual":(active.len()+1).to_string()}),
                false,
            ));
        }
        let cancellation = CancellationToken::new();
        active.insert(
            id.to_string(),
            ActiveRequest {
                cancellation: cancellation.clone(),
            },
        );
        Ok(cancellation)
    }

    pub fn cancel(&self, id: &str) -> bool {
        let active = self.active.lock().expect("active request lock");
        if let Some(request) = active.get(id) {
            request.cancellation.cancel();
            true
        } else {
            false
        }
    }

    fn remove(&self, id: &str) {
        self.active.lock().expect("active request lock").remove(id);
    }

    pub fn disconnect(&self) {
        self.disconnected.store(true, Ordering::Release);
        for request in self.active.lock().expect("active request lock").values() {
            request.cancellation.cancel();
        }
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }

    pub fn active_count(&self) -> usize {
        self.active.lock().expect("active request lock").len()
    }
}

struct ActiveGuard {
    connection: Arc<ConnectionState>,
    id: String,
    stats: Arc<RuntimeStats>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.connection.remove(&self.id);
        self.stats.active.fetch_sub(1, Ordering::AcqRel);
        self.stats.completed.fetch_add(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
struct ScanGate {
    held: Mutex<bool>,
    changed: Condvar,
}

impl ScanGate {
    fn new() -> Self {
        Self {
            held: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn acquire(self: &Arc<Self>, context: &RequestContext) -> Result<ScanGuard, ProtocolError> {
        let mut held = self.held.lock().expect("scan gate lock");
        while *held {
            context.check()?;
            let (next, _) = self
                .changed
                .wait_timeout(held, POLL_INTERVAL)
                .expect("scan gate wait");
            held = next;
        }
        context.check()?;
        *held = true;
        Ok(ScanGuard {
            gate: Arc::clone(self),
        })
    }
}

struct ScanGuard {
    gate: Arc<ScanGate>,
}

impl Drop for ScanGuard {
    fn drop(&mut self) {
        *self.gate.held.lock().expect("scan gate lock") = false;
        self.gate.changed.notify_one();
    }
}

#[derive(Clone)]
pub struct RequestContext {
    cancellation: CancellationToken,
    deadline: Option<Instant>,
    scan_gate: Arc<ScanGate>,
}

impl RequestContext {
    fn new(
        cancellation: CancellationToken,
        timeout_ms: u64,
        scan_gate: Arc<ScanGate>,
    ) -> Result<Self, ProtocolError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(timeout_ms))
            .ok_or_else(|| ProtocolError::bad_request("timeout_ms overflows deadline"))?;
        Ok(Self {
            cancellation,
            deadline: Some(deadline),
            scan_gate,
        })
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn check(&self) -> Result<(), ProtocolError> {
        if self.cancellation.is_cancelled() {
            return Err(ProtocolError::new(
                ProtocolErrorCode::Cancelled,
                "query cancelled",
                json!({}),
                false,
            ));
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(ProtocolError::new(
                ProtocolErrorCode::DeadlineExceeded,
                "query deadline exceeded",
                json!({}),
                true,
            ));
        }
        Ok(())
    }

    fn acquire_expensive_scan(&self) -> Result<ScanGuard, ProtocolError> {
        self.scan_gate.acquire(self)
    }

    /// Run expensive scan work while holding the generation permit, releasing
    /// it before the caller performs potentially blocked output.
    pub fn with_expensive_scan<T>(
        &self,
        work: impl FnOnce() -> Result<T, ProtocolError>,
    ) -> Result<T, ProtocolError> {
        let guard = self.acquire_expensive_scan()?;
        let result = work();
        drop(guard);
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputLifecycle {
    NotStarted,
    Streaming { next_seq: u32 },
    Terminal,
}

struct BlockedOutputGuard {
    stats: Arc<RuntimeStats>,
    counted: bool,
}

impl BlockedOutputGuard {
    fn new(stats: Arc<RuntimeStats>) -> Self {
        Self {
            stats,
            counted: false,
        }
    }

    fn mark(&mut self) {
        if !self.counted {
            self.counted = true;
            self.stats.output_blocked.fetch_add(1, Ordering::AcqRel);
        }
    }
}

impl Drop for BlockedOutputGuard {
    fn drop(&mut self) {
        if self.counted {
            self.stats.output_blocked.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

struct CappedJsonLineWriter {
    bytes: Vec<u8>,
    payload_cap: usize,
    exceeded: bool,
    attempted: usize,
    stats: Arc<RuntimeStats>,
}

impl CappedJsonLineWriter {
    fn new(max_encoded_frame_bytes: usize, stats: Arc<RuntimeStats>) -> Self {
        let payload_cap = max_encoded_frame_bytes.saturating_sub(1);
        Self {
            bytes: Vec::with_capacity(payload_cap.min(8 * 1024)),
            payload_cap,
            exceeded: false,
            attempted: 0,
            stats,
        }
    }

    fn observe(&self) {
        self.stats
            .max_frame_buffer_bytes
            .fetch_max(self.bytes.len(), Ordering::AcqRel);
    }
}

impl Write for CappedJsonLineWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let attempted = self.bytes.len().saturating_add(buffer.len());
        self.attempted = self.attempted.max(attempted);
        if attempted > self.payload_cap {
            self.exceeded = true;
            return Err(io::Error::other(
                "encoded response frame exceeds configured byte limit",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        self.observe();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct OutputSink {
    sender: SyncSender<Vec<u8>>,
    context: RequestContext,
    connection: Arc<ConnectionState>,
    stats: Arc<RuntimeStats>,
    request_id: String,
    lifecycle: OutputLifecycle,
    max_encoded_frame_bytes: usize,
}

impl OutputSink {
    fn protocol_lifecycle_error(&self, message: &str) -> ProtocolError {
        ProtocolError::new(ProtocolErrorCode::Internal, message, json!({}), false)
    }

    fn validate_frame(&self, frame: &ResponseFrame) -> Result<OutputLifecycle, ProtocolError> {
        use crate::server::protocol::ResponseFrame::*;
        match (self.lifecycle, frame) {
            (OutputLifecycle::NotStarted, Result(result))
                if result.id == self.request_id && result.seq == 0 =>
            {
                Ok(OutputLifecycle::Terminal)
            }
            (OutputLifecycle::NotStarted, Error(error))
                if error.id.as_deref() == Some(self.request_id.as_str()) && error.seq == 0 =>
            {
                Ok(OutputLifecycle::Terminal)
            }
            (OutputLifecycle::NotStarted, Begin(begin))
                if begin.id == self.request_id && begin.seq == 0 =>
            {
                Ok(OutputLifecycle::Streaming { next_seq: 1 })
            }
            (OutputLifecycle::Streaming { next_seq }, Chunk(chunk))
                if chunk.id == self.request_id && chunk.seq == next_seq =>
            {
                Ok(OutputLifecycle::Streaming {
                    next_seq: next_seq.checked_add(1).ok_or_else(|| {
                        self.protocol_lifecycle_error("response sequence overflow")
                    })?,
                })
            }
            (OutputLifecycle::Streaming { next_seq }, End(end))
                if end.id == self.request_id && end.seq == next_seq =>
            {
                Ok(OutputLifecycle::Terminal)
            }
            (OutputLifecycle::Terminal, _) => {
                Err(self.protocol_lifecycle_error("response already terminated"))
            }
            _ => Err(self
                .protocol_lifecycle_error("response frame violates request lifecycle or sequence")),
        }
    }

    fn encode_bounded(&self, frame: &ResponseFrame) -> Result<Vec<u8>, ProtocolError> {
        let mut writer =
            CappedJsonLineWriter::new(self.max_encoded_frame_bytes, Arc::clone(&self.stats));
        let serialization = serde_json::to_writer(&mut writer, frame);
        if writer.exceeded {
            return Err(ProtocolError::new(
                ProtocolErrorCode::LimitExceeded,
                "encoded response frame exceeds configured byte limit",
                json!({
                    "kind":"encoded_frame_bytes",
                    "limit":self.max_encoded_frame_bytes.to_string(),
                    "actual":writer.attempted.saturating_add(1).to_string()
                }),
                false,
            ));
        }
        serialization.map_err(|_| {
            ProtocolError::new(
                ProtocolErrorCode::Internal,
                "response serialization failed",
                json!({}),
                false,
            )
        })?;
        writer.bytes.push(b'\n');
        writer.observe();
        Ok(writer.bytes)
    }

    pub fn send_frame(&mut self, frame: &ResponseFrame) -> Result<(), ProtocolError> {
        let next_lifecycle = self.validate_frame(frame)?;
        let mut bytes = self.encode_bounded(frame)?;
        let mut blocked = BlockedOutputGuard::new(Arc::clone(&self.stats));
        loop {
            self.context.check()?;
            if self.connection.is_disconnected() {
                return Err(ProtocolError::new(
                    ProtocolErrorCode::Cancelled,
                    "connection disconnected",
                    json!({}),
                    false,
                ));
            }
            match self.sender.try_send(bytes) {
                Ok(()) => {
                    self.lifecycle = next_lifecycle;
                    return Ok(());
                }
                Err(TrySendError::Full(returned)) => {
                    blocked.mark();
                    bytes = returned;
                    thread::yield_now();
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.connection.disconnect();
                    return Err(ProtocolError::new(
                        ProtocolErrorCode::Cancelled,
                        "connection disconnected",
                        json!({}),
                        false,
                    ));
                }
            }
        }
    }

    fn required_terminal(&mut self, error: ProtocolError) {
        if self.lifecycle == OutputLifecycle::Terminal {
            return;
        }
        let frame = match self.lifecycle {
            OutputLifecycle::NotStarted => {
                ResponseFrame::error(Some(self.request_id.clone()), 0, error)
            }
            OutputLifecycle::Streaming { next_seq } => {
                ResponseFrame::end_error(self.request_id.clone(), next_seq, error)
            }
            OutputLifecycle::Terminal => return,
        };
        let Ok(mut bytes) = self.encode_bounded(&frame) else {
            self.force_disconnect();
            return;
        };
        let mut blocked = BlockedOutputGuard::new(Arc::clone(&self.stats));
        let give_up = Instant::now() + Duration::from_millis(100);
        loop {
            if self.connection.is_disconnected() {
                return;
            }
            match self.sender.try_send(bytes) {
                Ok(()) => {
                    self.lifecycle = OutputLifecycle::Terminal;
                    return;
                }
                Err(TrySendError::Full(returned)) if Instant::now() < give_up => {
                    blocked.mark();
                    bytes = returned;
                    thread::yield_now();
                }
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                    self.force_disconnect();
                    return;
                }
            }
        }
    }

    fn force_disconnect(&self) {
        if !self.connection.is_disconnected() {
            self.stats.forced_disconnects.fetch_add(1, Ordering::AcqRel);
        }
        self.connection.disconnect();
    }

    fn ensure_terminal(&mut self) {
        if self.lifecycle != OutputLifecycle::Terminal {
            self.required_terminal(ProtocolError::new(
                ProtocolErrorCode::Internal,
                "dispatcher returned without a terminal response",
                json!({}),
                false,
            ));
        }
    }

    pub fn send_result<T: Serialize>(&mut self, id: &str, value: &T) -> Result<(), ProtocolError> {
        let frame = ResponseFrame::result(id.to_string(), value).map_err(|_| {
            ProtocolError::new(
                ProtocolErrorCode::Internal,
                "response serialization failed",
                json!({}),
                false,
            )
        })?;
        self.send_frame(&frame)
    }
}

pub trait Dispatcher: Send + Sync + 'static {
    fn dispatch(
        &self,
        request: Request,
        context: &RequestContext,
        output: &mut OutputSink,
    ) -> Result<(), ProtocolError>;
}

impl<F> Dispatcher for F
where
    F: Fn(Request, &RequestContext, &mut OutputSink) -> Result<(), ProtocolError>
        + Send
        + Sync
        + 'static,
{
    fn dispatch(
        &self,
        request: Request,
        context: &RequestContext,
        output: &mut OutputSink,
    ) -> Result<(), ProtocolError> {
        self(request, context, output)
    }
}

struct Job {
    request: Request,
    context: RequestContext,
    output: SyncSender<Vec<u8>>,
    connection: Arc<ConnectionState>,
    max_encoded_frame_bytes: usize,
}

enum Work {
    Job(Box<Job>),
    Stop,
}

pub struct Scheduler {
    sender: SyncSender<Work>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    stats: Arc<RuntimeStats>,
    dispatcher: Arc<dyn Dispatcher>,
    scan_gate: Arc<ScanGate>,
}

impl Scheduler {
    pub fn new(config: &RuntimeConfig, dispatcher: Arc<dyn Dispatcher>) -> io::Result<Arc<Self>> {
        config.validate()?;
        let (sender, receiver) = mpsc::sync_channel(config.queue_depth);
        let receiver = Arc::new(Mutex::new(receiver));
        let stats = Arc::new(RuntimeStats::default());
        let scan_gate = Arc::new(ScanGate::new());
        let scheduler = Arc::new(Self {
            sender,
            workers: Mutex::new(Vec::new()),
            stats: Arc::clone(&stats),
            dispatcher,
            scan_gate,
        });
        let mut workers = scheduler.workers.lock().expect("workers lock");
        for index in 0..config.workers {
            let receiver = Arc::clone(&receiver);
            let dispatcher = Arc::clone(&scheduler.dispatcher);
            let stats = Arc::clone(&scheduler.stats);
            workers.push(
                thread::Builder::new()
                    .name(format!("vcd-worker-{index}"))
                    .spawn(move || worker_loop(receiver, dispatcher, stats))?,
            );
        }
        drop(workers);
        Ok(scheduler)
    }

    pub fn stats(&self) -> RuntimeStatsSnapshot {
        self.stats.snapshot()
    }

    fn submit(&self, job: Job) -> Result<(), Box<Job>> {
        self.stats.queued.fetch_add(1, Ordering::AcqRel);
        match self.sender.try_send(Work::Job(Box::new(job))) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(Work::Job(job)))
            | Err(TrySendError::Disconnected(Work::Job(job))) => {
                self.stats.queued.fetch_sub(1, Ordering::AcqRel);
                self.stats.rejected.fetch_add(1, Ordering::AcqRel);
                Err(job)
            }
            Err(_) => unreachable!("only jobs are submitted"),
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        let count = self.workers.lock().expect("workers lock").len();
        for _ in 0..count {
            let _ = self.sender.send(Work::Stop);
        }
        for worker in self.workers.lock().expect("workers lock").drain(..) {
            let _ = worker.join();
        }
    }
}

fn worker_loop(
    receiver: Arc<Mutex<Receiver<Work>>>,
    dispatcher: Arc<dyn Dispatcher>,
    stats: Arc<RuntimeStats>,
) {
    loop {
        let work = receiver.lock().expect("receiver lock").recv();
        let Ok(work) = work else { return };
        let Work::Job(job) = work else { return };
        let job = *job;
        stats.queued.fetch_sub(1, Ordering::AcqRel);
        stats.active.fetch_add(1, Ordering::AcqRel);
        let _guard = ActiveGuard {
            connection: Arc::clone(&job.connection),
            id: job.request.id.clone(),
            stats: Arc::clone(&stats),
        };
        let mut output = OutputSink {
            sender: job.output,
            context: job.context.clone(),
            connection: Arc::clone(&job.connection),
            stats: Arc::clone(&stats),
            request_id: job.request.id.clone(),
            lifecycle: OutputLifecycle::NotStarted,
            max_encoded_frame_bytes: job.max_encoded_frame_bytes,
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            job.context.check()?;
            dispatcher.dispatch(job.request, &job.context, &mut output)
        }));
        match result {
            Ok(Ok(())) => output.ensure_terminal(),
            Ok(Err(error)) => output.required_terminal(error),
            Err(_) => {
                stats.panicked.fetch_add(1, Ordering::AcqRel);
                let error = ProtocolError::new(
                    ProtocolErrorCode::Internal,
                    "internal server error",
                    json!({}),
                    false,
                );
                output.required_terminal(error);
            }
        }
    }
}

pub struct ConnectionLimiter {
    max: usize,
    active: AtomicUsize,
}

impl ConnectionLimiter {
    pub fn new(max: usize) -> Self {
        assert!(max > 0);
        Self {
            max,
            active: AtomicUsize::new(0),
        }
    }

    pub fn try_acquire(self: &Arc<Self>) -> Option<ConnectionPermit> {
        let acquired = self
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.max).then_some(active + 1)
            })
            .is_ok();
        acquired.then(|| ConnectionPermit {
            limiter: Arc::clone(self),
        })
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }
}

pub struct ConnectionPermit {
    limiter: Arc<ConnectionLimiter>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.limiter.active.fetch_sub(1, Ordering::AcqRel);
    }
}

struct ConnectionRunGuard {
    state: Arc<ConnectionState>,
    stats: Arc<RuntimeStats>,
}

impl Drop for ConnectionRunGuard {
    fn drop(&mut self) {
        self.state.disconnect();
        self.stats.connections.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn serve_connection(
    stream: UnixStream,
    scheduler: Arc<Scheduler>,
    config: &RuntimeConfig,
    _permit: ConnectionPermit,
) -> io::Result<()> {
    let mut reader = stream.try_clone()?;
    reader.set_read_timeout(Some(Duration::from_millis(10)))?;
    let mut writer = stream;
    writer.set_write_timeout(Some(Duration::from_millis(10)))?;
    let disconnected = Arc::new(AtomicBool::new(false));
    let state = Arc::new(ConnectionState::new(
        config.max_active_requests_per_connection,
        disconnected,
    ));
    scheduler.stats.connections.fetch_add(1, Ordering::AcqRel);
    let _connection_guard = ConnectionRunGuard {
        state: Arc::clone(&state),
        stats: Arc::clone(&scheduler.stats),
    };
    let (output_tx, output_rx) = mpsc::sync_channel::<Vec<u8>>(config.output_chunks);
    let writer_state = Arc::clone(&state);
    let writer_thread = thread::spawn(move || {
        while let Ok(bytes) = output_rx.recv() {
            let mut offset = 0;
            while offset < bytes.len() && !writer_state.is_disconnected() {
                match writer.write(&bytes[offset..]) {
                    Ok(0) => {
                        writer_state.disconnect();
                        break;
                    }
                    Ok(count) => offset += count,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => {
                        writer_state.disconnect();
                        break;
                    }
                }
            }
        }
    });

    let mut accumulator = JsonLineAccumulator::new(config.request_line_bytes);
    let mut buffer = [0_u8; 8192];
    let read_result = 'read: loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break 'read Ok(()),
            Ok(count) => count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if state.is_disconnected() {
                    break 'read Ok(());
                }
                continue 'read;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::UnexpectedEof
                ) =>
            {
                break 'read Ok(());
            }
            Err(error) => break 'read Err(error),
        };
        let frames = match accumulator.push(&buffer[..count]) {
            Ok(frames) => frames,
            Err(error) => {
                let _ = send_uncorrelated(&output_tx, error);
                force_disconnect(&state, &scheduler.stats);
                break 'read Ok(());
            }
        };
        for line in frames {
            let request = match decode_request_with_active(&line, |id| state.is_active(id)) {
                Ok(request) => request,
                Err(error) => {
                    let _ = send_uncorrelated(&output_tx, error);
                    force_disconnect(&state, &scheduler.stats);
                    break 'read Ok(());
                }
            };
            if let RequestMethod::Cancel(cancel) = &request.method {
                let found = state.cancel(&cancel.request_id);
                if let Ok(frame) = ResponseFrame::result(&request.id, &CancelResult { found })
                    && !send_encoded_control(&output_tx, frame)
                {
                    force_disconnect(&state, &scheduler.stats);
                    break 'read Ok(());
                }
                continue;
            }
            let timeout_ms = match effective_timeout_ms(&request.method, config.max_timeout_ms) {
                Ok(timeout) => timeout,
                Err(error) => {
                    if !send_encoded_control(
                        &output_tx,
                        ResponseFrame::error(Some(request.id), 0, error),
                    ) {
                        force_disconnect(&state, &scheduler.stats);
                        break 'read Ok(());
                    }
                    continue;
                }
            };
            let cancellation = match state.register(&request.id) {
                Ok(cancellation) => cancellation,
                Err(error) => {
                    let _ = send_uncorrelated(&output_tx, error);
                    force_disconnect(&state, &scheduler.stats);
                    break 'read Ok(());
                }
            };
            let context = match RequestContext::new(
                cancellation,
                timeout_ms,
                Arc::clone(&scheduler.scan_gate),
            ) {
                Ok(context) => context,
                Err(error) => {
                    state.remove(&request.id);
                    if !send_encoded_control(
                        &output_tx,
                        ResponseFrame::error(Some(request.id), 0, error),
                    ) {
                        force_disconnect(&state, &scheduler.stats);
                        break 'read Ok(());
                    }
                    continue;
                }
            };
            let job = Job {
                request,
                context,
                output: output_tx.clone(),
                connection: Arc::clone(&state),
                max_encoded_frame_bytes: config.max_encoded_frame_bytes,
            };
            if let Err(job) = scheduler.submit(job) {
                state.remove(&job.request.id);
                let error = ProtocolError::new(
                    ProtocolErrorCode::QueueFull,
                    "query queue is full",
                    json!({}),
                    true,
                );
                if !send_encoded_control(
                    &output_tx,
                    ResponseFrame::error(Some(job.request.id), 0, error),
                ) {
                    force_disconnect(&state, &scheduler.stats);
                    break 'read Ok(());
                }
            }
        }
    };

    state.disconnect();
    let _ = reader.shutdown(std::net::Shutdown::Both);
    drop(output_tx);
    let _ = writer_thread.join();
    read_result
}

fn force_disconnect(state: &ConnectionState, stats: &RuntimeStats) {
    if !state.is_disconnected() {
        stats.forced_disconnects.fetch_add(1, Ordering::AcqRel);
    }
    state.disconnect();
}

fn effective_timeout_ms(
    method: &RequestMethod,
    configured_maximum: u64,
) -> Result<u64, ProtocolError> {
    let requested = request_timeout_ms(method).unwrap_or(configured_maximum);
    if requested > configured_maximum {
        return Err(ProtocolError::bad_request(
            "timeout_ms may only lower the configured maximum",
        ));
    }
    Ok(requested)
}

fn request_timeout_ms(method: &RequestMethod) -> Option<u64> {
    match method {
        RequestMethod::List(params) => params.timeout_ms,
        RequestMethod::Metadata(params) => params.timeout_ms,
        RequestMethod::Extract(params) => params.timeout_ms,
        RequestMethod::Find(params) => params.timeout_ms,
        RequestMethod::Toggles(params) => params.timeout_ms,
        RequestMethod::Ping | RequestMethod::Describe | RequestMethod::Cancel(_) => None,
    }
}

fn send_uncorrelated(sender: &SyncSender<Vec<u8>>, error: ProtocolError) -> bool {
    send_encoded_control(sender, ResponseFrame::error(None, 0, error))
}

fn send_encoded_control(sender: &SyncSender<Vec<u8>>, frame: ResponseFrame) -> bool {
    let Ok(mut bytes) = encode_json_line(&frame) else {
        return false;
    };
    let give_up = Instant::now() + Duration::from_millis(100);
    loop {
        match sender.try_send(bytes) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) if Instant::now() < give_up => {
                bytes = returned;
                thread::sleep(POLL_INTERVAL);
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}
