#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use vcd_tools_rs::server::app::{ServerEvent, run_server_with_dispatcher};
use vcd_tools_rs::server::runtime::{
    Dispatcher, HARD_MAX_ACTIVE_REQUESTS_PER_CONNECTION, HARD_MAX_CONNECTIONS,
    HARD_MAX_OUTPUT_CHUNKS, HARD_MAX_QUEUE_DEPTH, HARD_MAX_TIMEOUT_MS, HARD_MAX_WORKERS,
    RuntimeConfig,
};
use vcd_tools_rs::server::service::{
    HARD_MAX_COMMANDS, HARD_MAX_RESPONSE_BYTES, HARD_MAX_ROWS, HARD_MAX_SIGNALS, ServiceConfig,
    VcdService,
};

struct TestServer {
    _directory: TempDir,
    vcd: PathBuf,
    socket: PathBuf,
    shutdown: Arc<AtomicBool>,
    service: Arc<VcdService>,
    thread: Option<thread::JoinHandle<std::io::Result<()>>>,
}

impl TestServer {
    fn start() -> Self {
        Self::start_with_fixture("tests/fixtures/query_semantics.vcd")
    }

    fn start_with_fixture(fixture: &str) -> Self {
        Self::start_with_service(fixture, |_| {})
    }

    fn start_with_service(fixture: &str, configure: impl FnOnce(&Arc<VcdService>)) -> Self {
        Self::start_configured(fixture, RuntimeConfig::default(), configure)
    }

    fn start_configured(
        fixture: &str,
        runtime: RuntimeConfig,
        configure: impl FnOnce(&Arc<VcdService>),
    ) -> Self {
        Self::start_configured_with_events(fixture, runtime, configure, None)
    }

    fn start_configured_with_events(
        fixture: &str,
        runtime: RuntimeConfig,
        configure: impl FnOnce(&Arc<VcdService>),
        event_hook: Option<Arc<dyn Fn(ServerEvent) + Send + Sync>>,
    ) -> Self {
        let directory = tempfile::tempdir().expect("temp directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("private directory");
        let vcd = directory.path().join("wave.vcd");
        fs::copy(fixture, &vcd).expect("copy VCD");
        let socket = directory.path().join("server.sock");
        let shutdown = Arc::new(AtomicBool::new(false));
        let service = Arc::new(
            VcdService::new(&vcd, runtime.clone(), ServiceConfig::default())
                .expect("create service"),
        );
        configure(&service);
        let dispatcher: Arc<dyn Dispatcher> = service.clone();
        let thread_shutdown = Arc::clone(&shutdown);
        let thread_socket = socket.clone();
        let thread = thread::spawn(move || {
            run_server_with_dispatcher(
                thread_socket,
                runtime,
                dispatcher,
                thread_shutdown,
                None,
                event_hook,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "server socket did not appear");
            thread::sleep(Duration::from_millis(5));
        }
        Self {
            _directory: directory,
            vcd,
            socket,
            shutdown,
            service,
            thread: Some(thread),
        }
    }

    fn connect(&self) -> Client {
        Client::connect(&self.socket)
    }

    fn replace(&self, fixture: &str) {
        let replacement = self.vcd.with_extension("new");
        fs::copy(fixture, &replacement).expect("copy replacement");
        fs::rename(replacement, &self.vcd).expect("replace VCD");
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .expect("server thread")
                .expect("server result");
        }
        assert!(!self.socket.exists(), "server socket was not cleaned");
    }
}

struct Client {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    pending: HashMap<String, Vec<Value>>,
}

impl Client {
    fn connect(path: &Path) -> Self {
        let writer = UnixStream::connect(path).expect("connect");
        writer
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let reader = BufReader::new(writer.try_clone().expect("clone"));
        Self {
            writer,
            reader,
            pending: HashMap::new(),
        }
    }

    fn send_value(&mut self, value: &Value) {
        serde_json::to_writer(&mut self.writer, value).expect("serialize request");
        self.writer.write_all(b"\n").expect("request newline");
    }

    fn send_bytes(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write bytes");
    }

    fn response(&mut self, id: &str) -> Vec<Value> {
        loop {
            if self
                .pending
                .get(id)
                .and_then(|frames| frames.last())
                .is_some_and(is_terminal)
            {
                return self.pending.remove(id).expect("pending response");
            }
            let mut line = String::new();
            assert!(self.reader.read_line(&mut line).expect("read frame") > 0);
            let value: Value = serde_json::from_str(&line).expect("JSON frame");
            let frame_id = value["id"].as_str().expect("correlated frame").to_string();
            self.pending.entry(frame_id).or_default().push(value);
        }
    }
}

fn is_terminal(value: &Value) -> bool {
    matches!(value["type"].as_str(), Some("result" | "error" | "end"))
}

fn request(id: &str, method: &str, params: Value) -> Value {
    json!({"v":1,"id":id,"method":method,"params":params})
}

#[test]
fn every_v1_method_and_generation_replacement_work_over_real_socket() {
    let server = TestServer::start();
    let mut client = server.connect();

    client.send_value(&request("ping", "ping", json!({})));
    assert_eq!(client.response("ping")[0]["result"]["ready"], true);

    client.send_value(&request("describe1", "describe", json!({})));
    let first = client.response("describe1");
    let generation = first[0]["result"]["generation"].clone();
    assert_eq!(first[0]["result"]["methods"].as_array().unwrap().len(), 8);

    client.send_value(&request("list", "list", json!({"filter":"vec"})));
    let list = client.response("list");
    assert_eq!(list.first().unwrap()["type"], "begin");
    assert_eq!(list.last().unwrap()["complete"], true);
    assert!(
        list.iter()
            .any(|frame| frame["rows"] == json!(["top.vec4[3:0]"]))
    );

    client.send_value(&request("meta", "metadata", json!({})));
    assert_eq!(client.response("meta")[0]["result"]["end_time"], "25");

    client.send_value(&request(
        "extract",
        "extract",
        json!({"signals":["top.alias_a","top.a"],"start":"5","end":"5"}),
    ));
    let extract = client.response("extract");
    assert_eq!(extract.first().unwrap()["type"], "begin");
    assert_eq!(extract.last().unwrap()["complete"], true);
    let rows = extract
        .iter()
        .filter_map(|frame| frame["rows"].as_array())
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["signal"], "top.alias_a");
    assert_eq!(rows[1]["signal"], "top.a");

    client.send_value(&request(
        "find",
        "find",
        json!({"signal":"top.a","value":"1","occurrence":"2"}),
    ));
    assert_eq!(client.response("find")[0]["result"]["time"], "15");

    client.send_value(&request(
        "toggles",
        "toggles",
        json!({"signals":["top.a"],"start":"5","end":"20"}),
    ));
    assert_eq!(
        client.response("toggles")[0]["result"]["rows"][0]["toggles"],
        "3"
    );

    client.send_value(&request(
        "cancel",
        "cancel",
        json!({"request_id":"not-active"}),
    ));
    assert_eq!(client.response("cancel")[0]["result"]["found"], false);

    server.replace("tests/fixtures/query_semantics_changed.vcd");
    client.send_value(&request("describe2", "describe", json!({})));
    let second = client.response("describe2");
    assert_ne!(second[0]["result"]["generation"], generation);
}

#[test]
fn active_request_can_be_cancelled_over_the_same_connection() {
    // The scan hook provides deterministic cancellation entry; a compact
    // fixture keeps the uncancelled retry fast in debug builds.
    let server = TestServer::start_with_fixture("tests/fixtures/query_semantics.vcd");
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let hook_reached = Arc::clone(&reached);
    let hook_resume = Arc::clone(&resume);
    server
        .service
        .generation_slot()
        .current()
        .unwrap()
        .set_metadata_scan_hook(Arc::new(move |_| {
            hook_reached.wait();
            hook_resume.wait();
        }));
    let mut client = server.connect();
    client.send_value(&request("scan", "metadata", json!({})));
    reached.wait();
    client.send_value(&request(
        "cancel-scan",
        "cancel",
        json!({"request_id":"scan"}),
    ));
    assert_eq!(client.response("cancel-scan")[0]["result"]["found"], true);
    resume.wait();
    let scan = client.response("scan");
    let terminal = scan.last().expect("scan terminal");
    assert!(matches!(terminal["type"].as_str(), Some("error" | "end")));
    assert_eq!(terminal["error"]["code"], "CANCELLED");

    // Cancellation must release both the worker and the expensive-scan permit;
    // the next metadata attempt completes on the same connection.
    client.send_value(&request("next-scan", "metadata", json!({})));
    let next = client.response("next-scan");
    assert_eq!(next[0]["type"], "result");
    assert!(next[0]["result"]["end_time"].as_str().unwrap() != "0");
}

#[test]
fn partial_multiple_interleaved_errors_limits_and_disconnect_are_operational() {
    let server = TestServer::start();
    let mut client = server.connect();
    let first = serde_json::to_vec(&request("one", "ping", json!({}))).unwrap();
    let second = serde_json::to_vec(&request("two", "describe", json!({}))).unwrap();
    client.send_bytes(&first[..first.len() / 2]);
    client.send_bytes(&first[first.len() / 2..]);
    client.send_bytes(b"\n");
    client.send_bytes(&second);
    client.send_bytes(b"\n");
    assert_eq!(client.response("one")[0]["type"], "result");
    assert_eq!(client.response("two")[0]["type"], "result");

    client.send_value(&request(
        "missing",
        "extract",
        json!({"signals":["top.missing"]}),
    ));
    assert_eq!(
        client.response("missing")[0]["error"]["code"],
        "SIGNAL_NOT_FOUND"
    );

    client.send_value(&request("limit", "list", json!({"max_rows":"1"})));
    let limited = client.response("limit");
    assert_eq!(limited.first().unwrap()["type"], "begin");
    assert_eq!(limited.last().unwrap()["complete"], false);
    assert_eq!(limited.last().unwrap()["error"]["code"], "LIMIT_EXCEEDED");

    let mut disconnected = server.connect();
    disconnected.send_value(&request(
        "drop",
        "extract",
        json!({"signals":["top.a","top.b","top.vec4[3:0]"]}),
    ));
    drop(disconnected);
    thread::sleep(Duration::from_millis(30));

    let mut survivor = server.connect();
    survivor.send_value(&request("survivor", "ping", json!({})));
    assert_eq!(survivor.response("survivor")[0]["type"], "result");
}

#[test]
fn service_hard_maxima_and_cli_max_plus_one_fail_before_bind() {
    let service_fields: Vec<(fn(&mut ServiceConfig, u64), u64)> = vec![
        (|c, v| c.max_rows = v, HARD_MAX_ROWS),
        (|c, v| c.max_response_bytes = v, HARD_MAX_RESPONSE_BYTES),
        (|c, v| c.max_commands = v, HARD_MAX_COMMANDS),
    ];
    for (set, maximum) in service_fields {
        let mut config = ServiceConfig::default();
        set(&mut config, maximum);
        config.validate().expect("service hard maximum accepted");
        set(&mut config, maximum + 1);
        assert!(config.validate().is_err());
    }
    let mut config = ServiceConfig::default();
    config.max_signals = HARD_MAX_SIGNALS;
    config.validate().expect("signals maximum accepted");
    config.max_signals = HARD_MAX_SIGNALS + 1;
    assert!(config.validate().is_err());

    let directory = tempfile::tempdir().expect("temp directory");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let cases = [
        ("--max-connections", (HARD_MAX_CONNECTIONS + 1).to_string()),
        (
            "--max-active-requests",
            (HARD_MAX_ACTIVE_REQUESTS_PER_CONNECTION + 1).to_string(),
        ),
        ("--workers", (HARD_MAX_WORKERS + 1).to_string()),
        ("--queue-depth", (HARD_MAX_QUEUE_DEPTH + 1).to_string()),
        ("--output-chunks", (HARD_MAX_OUTPUT_CHUNKS + 1).to_string()),
        (
            "--max-request-bytes",
            (vcd_tools_rs::server::protocol::MAX_REQUEST_LINE_BYTES + 1).to_string(),
        ),
        (
            "--max-frame-bytes",
            (vcd_tools_rs::server::protocol::CHUNK_BYTES + 1).to_string(),
        ),
        ("--max-timeout-ms", (HARD_MAX_TIMEOUT_MS + 1).to_string()),
        ("--max-signals", (HARD_MAX_SIGNALS + 1).to_string()),
        ("--max-rows", (HARD_MAX_ROWS + 1).to_string()),
        (
            "--max-response-bytes",
            (HARD_MAX_RESPONSE_BYTES + 1).to_string(),
        ),
        ("--max-commands", (HARD_MAX_COMMANDS + 1).to_string()),
    ];
    for (index, (flag, value)) in cases.into_iter().enumerate() {
        let socket = directory.path().join(format!("invalid-{index}.sock"));
        let output = Command::new(env!("CARGO_BIN_EXE_vcd_tools_rs"))
            .args([
                "serve",
                "tests/fixtures/query_semantics.vcd",
                "--socket",
                socket.to_str().unwrap(),
                flag,
                &value,
            ])
            .output()
            .expect("run invalid serve");
        assert!(
            !output.status.success(),
            "{flag} max+1 unexpectedly started"
        );
        assert!(!socket.exists(), "{flag} bound before validation");
    }
}

#[test]
fn fatal_accept_exit_cancels_connections_joins_cleans_and_returns_original_error() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let vcd = directory.path().join("wave.vcd");
    fs::copy("tests/fixtures/query_semantics.vcd", &vcd).unwrap();
    let socket = directory.path().join("fatal.sock");
    let runtime = RuntimeConfig::default();
    let service =
        Arc::new(VcdService::new(&vcd, runtime.clone(), ServiceConfig::default()).unwrap());
    let dispatcher: Arc<dyn Dispatcher> = service;
    let shutdown = Arc::new(AtomicBool::new(false));
    let inject = Arc::new(AtomicBool::new(false));
    let hook_inject = Arc::clone(&inject);
    let hook: Arc<dyn Fn() -> std::io::Result<()> + Send + Sync> = Arc::new(move || {
        if hook_inject.load(Ordering::Acquire) {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "injected fatal accept",
            ))
        } else {
            Ok(())
        }
    });
    let thread_shutdown = Arc::clone(&shutdown);
    let thread_socket = socket.clone();
    let server = thread::spawn(move || {
        run_server_with_dispatcher(
            thread_socket,
            runtime,
            dispatcher,
            thread_shutdown,
            Some(hook),
            None,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(Instant::now() < deadline);
        thread::yield_now();
    }
    let mut client = Client::connect(&socket);
    client.send_value(&request("accepted", "ping", json!({})));
    assert_eq!(client.response("accepted")[0]["type"], "result");
    inject.store(true, Ordering::Release);
    let error = server
        .join()
        .unwrap()
        .expect_err("fatal accept must return error");
    assert_eq!(error.to_string(), "injected fatal accept");
    assert!(shutdown.load(Ordering::Acquire));
    assert!(!socket.exists());
    let mut eof = String::new();
    assert_eq!(client.reader.read_line(&mut eof).unwrap(), 0);
}

#[test]
fn cached_metadata_replacement_before_unary_publish_is_stale() {
    let server = TestServer::start();
    let mut client = server.connect();
    client.send_value(&request("prime", "metadata", json!({})));
    assert_eq!(client.response("prime")[0]["type"], "result");
    assert!(
        server
            .service
            .generation_slot()
            .current()
            .unwrap()
            .has_cached_metadata()
    );

    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let hook_reached = Arc::clone(&reached);
    let hook_resume = Arc::clone(&resume);
    server
        .service
        .set_before_metadata_publish_hook(Arc::new(move || {
            hook_reached.wait();
            hook_resume.wait();
        }));
    client.send_value(&request("cached", "metadata", json!({})));
    reached.wait();
    server.replace("tests/fixtures/query_semantics_changed.vcd");
    resume.wait();
    let response = client.response("cached");
    assert_eq!(response[0]["type"], "error");
    assert_eq!(response[0]["error"]["code"], "STALE_SOURCE");
}

#[test]
fn replacement_after_first_stream_chunk_terminates_incomplete_stale() {
    let server = TestServer::start_with_fixture("tests/waveform.vcd");
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let hook_reached = Arc::clone(&reached);
    let hook_resume = Arc::clone(&resume);
    server
        .service
        .set_after_stream_chunk_hook(Arc::new(move || {
            hook_reached.wait();
            hook_resume.wait();
        }));
    let mut client = server.connect();
    client.send_value(&request("stream-stale", "list", json!({})));
    reached.wait();
    server.replace("tests/fixtures/query_semantics_changed.vcd");
    resume.wait();
    let frames = client.response("stream-stale");
    assert_eq!(frames.first().unwrap()["type"], "begin");
    assert!(frames.iter().any(|frame| frame["type"] == "chunk"));
    let terminal = frames.last().unwrap();
    assert_eq!(terminal["type"], "end");
    assert_eq!(terminal["complete"], false);
    assert_eq!(terminal["error"]["code"], "STALE_SOURCE");
}

#[test]
fn actual_socket_stream_splits_at_frame_cap_and_reserves_terminal_bytes() {
    let mut runtime = RuntimeConfig::default();
    runtime.max_encoded_frame_bytes = 4_096;
    let server =
        TestServer::start_configured("tests/fixtures/server_chunked_list.vcd", runtime, |_| {});
    let mut client = server.connect();
    client.send_value(&request("split", "list", json!({})));
    let frames = client.response("split");
    let chunks = frames
        .iter()
        .filter(|frame| frame["type"] == "chunk")
        .collect::<Vec<_>>();
    assert!(chunks.len() > 1, "lower frame cap must split list chunks");
    for frame in &frames {
        assert!(serde_json::to_vec(frame).unwrap().len() + 1 <= 4_096);
    }
    let total = frames
        .iter()
        .map(|frame| serde_json::to_vec(frame).unwrap().len() as u64 + 1)
        .sum::<u64>();
    assert_eq!(
        frames.last().unwrap()["stats"]["encoded_bytes"],
        total.to_string()
    );

    client.send_value(&request(
        "reserve",
        "list",
        json!({"max_response_bytes":"5000"}),
    ));
    let limited = client.response("reserve");
    assert_eq!(limited.first().unwrap()["type"], "begin");
    assert_eq!(limited.last().unwrap()["type"], "end");
    assert_eq!(limited.last().unwrap()["complete"], false);
    assert_eq!(limited.last().unwrap()["error"]["code"], "LIMIT_EXCEEDED");
    let limited_bytes = limited
        .iter()
        .map(|frame| serde_json::to_vec(frame).unwrap().len() as u64 + 1)
        .sum::<u64>();
    assert!(limited_bytes <= 5_000);
}

#[test]
fn actual_socket_connection_saturation_and_disconnect_cleanup_are_race_controlled() {
    let events = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let hook_events = Arc::clone(&events);
    let hook: Arc<dyn Fn(ServerEvent) + Send + Sync> = Arc::new(move |event| {
        let (lock, changed) = &*hook_events;
        lock.lock().unwrap().push(event);
        changed.notify_all();
    });
    let mut runtime = RuntimeConfig::default();
    runtime.max_connections = 1;
    let server = TestServer::start_configured_with_events(
        "tests/fixtures/query_semantics.vcd",
        runtime,
        |_| {},
        Some(hook),
    );
    let first = server.connect();
    wait_server_events(&events, |events| {
        events
            .iter()
            .filter(|event| **event == ServerEvent::ConnectionAccepted)
            .count()
            >= 1
    });

    let mut rejected = UnixStream::connect(&server.socket).expect("second connect");
    rejected
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    wait_server_events(&events, |events| {
        events.contains(&ServerEvent::ConnectionRejected)
    });
    let mut byte = [0_u8; 1];
    match std::io::Read::read(&mut rejected, &mut byte) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            ) => {}
        result => panic!("rejected connection remained open: {result:?}"),
    }

    drop(first);
    wait_server_events(&events, |events| {
        events.contains(&ServerEvent::ConnectionFinished)
    });
    let mut successor = server.connect();
    wait_server_events(&events, |events| {
        events
            .iter()
            .filter(|event| **event == ServerEvent::ConnectionAccepted)
            .count()
            >= 2
    });
    successor.send_value(&request("after-disconnect", "ping", json!({})));
    assert_eq!(successor.response("after-disconnect")[0]["type"], "result");
}

fn wait_server_events(
    events: &Arc<(Mutex<Vec<ServerEvent>>, Condvar)>,
    predicate: impl Fn(&[ServerEvent]) -> bool,
) {
    let (lock, changed) = &**events;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut values = lock.lock().unwrap();
    while !predicate(&values) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "server event timed out: {values:?}");
        values = changed.wait_timeout(values, remaining).unwrap().0;
    }
}

#[test]
fn spawned_native_serve_ping_sigterm_and_socket_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let socket = directory.path().join("binary.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_vcd_tools_rs"))
        .args([
            "serve",
            "tests/fixtures/query_semantics.vcd",
            "--socket",
            socket.to_str().unwrap(),
        ])
        .spawn()
        .expect("spawn native server");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "server exited during startup"
        );
        assert!(Instant::now() < deadline, "binary socket startup timed out");
        thread::yield_now();
    }
    let mut client = Client::connect(&socket);
    client.send_value(&request("binary-ping", "ping", json!({})));
    assert_eq!(client.response("binary-ping")[0]["result"]["ready"], true);
    drop(client);

    let result = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(result, 0, "send SIGTERM");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "server did not exit on SIGTERM");
        thread::yield_now();
    }
    assert!(!socket.exists(), "binary server left socket after SIGTERM");
}

#[test]
fn fst_describe_extract_and_find_work_over_real_socket() {
    let server = TestServer::start_with_fixture("tests/fixtures/fst/tiny.fst");
    let mut client = server.connect();

    client.send_value(&request("describe", "describe", json!({})));
    let describe = client.response("describe");
    assert_eq!(describe[0]["result"]["source_format"], "fst");
    assert_eq!(
        describe[0]["result"]["timescale"],
        json!({"magnitude":"1","unit":"ns"})
    );

    client.send_value(&request(
        "extract",
        "extract",
        json!({"signals":["top.a","top.a_alias"]}),
    ));
    let extract = client.response("extract");
    let rows = extract
        .iter()
        .filter_map(|frame| frame["rows"].as_array())
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 4);
    assert_eq!(extract.last().unwrap()["complete"], true);

    client.send_value(&request(
        "find",
        "find",
        json!({"signal":"top.a","value":"1","occurrence":"1"}),
    ));
    let find = client.response("find");
    assert_eq!(find[0]["result"]["time"], "5");
}

#[test]
fn corrupt_fst_signal_data_reports_fst_error() {
    for (offset, id) in [(435usize, "panic-path"), (441usize, "read-error-path")] {
        let mut bytes = fs::read("tests/fixtures/fst/tiny.fst").unwrap();
        // These bytes are inside the compressed dynamic-alias signal-data payload;
        // hierarchy/header parsing remains valid while signal reading fails.
        bytes[offset] ^= 0xff;
        let mut fixture = tempfile::NamedTempFile::new().unwrap();
        fixture.write_all(&bytes).unwrap();
        let server = TestServer::start_with_fixture(fixture.path().to_str().unwrap());
        let mut client = server.connect();
        client.send_value(&request(id, "extract", json!({"signals":["top.a"]})));
        let response = client.response(id);
        assert_eq!(response.last().unwrap()["error"]["code"], "FST_ERROR");
        assert_ne!(response.last().unwrap()["error"]["code"], "VCD_ERROR");
    }
}
