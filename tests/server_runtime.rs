#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use vcd_tools_rs::server::listener::BoundUnixListener;
use vcd_tools_rs::server::protocol::{Request, ResponseFrame, encode_json_line};
use vcd_tools_rs::server::runtime::{
    ConnectionLimiter, Dispatcher, OutputSink, RequestContext, RuntimeConfig, Scheduler,
    serve_connection,
};

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        thread::sleep(Duration::from_millis(2));
    }
}

fn line(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read response");
    assert!(!line.is_empty(), "unexpected EOF");
    serde_json::from_str(&line).expect("JSON response")
}

fn start_connection(
    config: RuntimeConfig,
    dispatcher: Arc<dyn Dispatcher>,
) -> (UnixStream, Arc<Scheduler>, thread::JoinHandle<()>) {
    let scheduler = Scheduler::new(&config, dispatcher).expect("scheduler");
    let limiter = Arc::new(ConnectionLimiter::new(config.max_connections));
    let permit = limiter.try_acquire().expect("connection permit");
    let (client, server) = UnixStream::pair().expect("socket pair");
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("timeout");
    let thread_scheduler = Arc::clone(&scheduler);
    let handle = thread::spawn(move || {
        serve_connection(server, thread_scheduler, &config, permit).expect("serve connection")
    });
    (client, scheduler, handle)
}

fn secure_tempdir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("tempdir");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .expect("secure tempdir");
    directory
}

#[test]
fn listener_lifecycle_permissions_stale_live_and_safe_cleanup() {
    let directory = secure_tempdir();
    let path = directory.path().join("server.sock");

    let mut listener = BoundUnixListener::bind(&path).expect("bind");
    let metadata = fs::symlink_metadata(&path).expect("socket metadata");
    assert!(metadata.file_type().is_socket());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(listener.path(), path);

    let live = BoundUnixListener::bind(&path).expect_err("live socket rejected");
    assert_eq!(live.kind(), std::io::ErrorKind::AddrInUse);
    listener.cleanup().expect("cleanup");
    assert!(!path.exists());

    let stale = UnixListener::bind(&path).expect("stale bind");
    drop(stale);
    assert!(path.exists());
    let listener = BoundUnixListener::bind(&path).expect("replace owned stale socket");
    drop(listener);
    assert!(!path.exists());

    fs::write(&path, b"do not remove").expect("regular file");
    BoundUnixListener::bind(&path).expect_err("non-socket rejected");
    assert_eq!(fs::read(&path).expect("preserved"), b"do not remove");

    let missing = directory.path().join("missing").join("socket");
    assert_eq!(
        BoundUnixListener::bind(missing)
            .expect_err("missing parent")
            .kind(),
        std::io::ErrorKind::NotFound
    );

    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o777))
        .expect("make parent unsafe");
    assert_eq!(
        BoundUnixListener::bind(directory.path().join("unsafe.sock"))
            .expect_err("writable parent rejected")
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
}

#[cfg(debug_assertions)]
#[test]
fn post_bind_failure_removes_only_created_socket() {
    let directory = secure_tempdir();
    let path = directory.path().join("failure.sock");
    let error = BoundUnixListener::bind_with_post_bind_hook_for_test(&path, || {
        Err(std::io::Error::other("injected metadata-stage failure"))
    })
    .expect_err("injected failure");
    assert_eq!(error.to_string(), "injected metadata-stage failure");
    assert!(!path.exists(), "armed guard must remove the created socket");
    let leftovers = fs::read_dir(directory.path())
        .expect("read tempdir")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert!(leftovers.is_empty(), "hidden socket leaked: {leftovers:?}");
}

#[cfg(debug_assertions)]
#[test]
fn concurrent_socket_publication_is_atomic_no_replace() {
    let directory = secure_tempdir();
    let path = directory.path().join("server.sock");
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let attempts = (0..2)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                BoundUnixListener::bind_with_post_bind_hook_for_test(path, || {
                    barrier.wait();
                    Ok(())
                })
            })
        })
        .collect::<Vec<_>>();
    let results = attempts
        .into_iter()
        .map(|attempt| attempt.join().expect("bind thread"))
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let loser = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert!(matches!(
        loser.kind(),
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::AddrInUse
    ));
    let winner = results.into_iter().find_map(Result::ok).expect("winner");
    let connection = UnixStream::connect(&path).expect("winner remains live");
    drop(connection);
    let entries = fs::read_dir(directory.path())
        .expect("read tempdir")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec![std::ffi::OsString::from("server.sock")]);
    drop(winner);
    assert!(!path.exists());
}

#[test]
fn cleanup_does_not_remove_replacement_path() {
    let directory = secure_tempdir();
    let path = directory.path().join("server.sock");
    let listener = BoundUnixListener::bind(&path).expect("bind");
    fs::remove_file(&path).expect("unlink socket");
    fs::write(&path, b"replacement").expect("replacement file");
    drop(listener);
    assert_eq!(
        fs::read(path).expect("replacement retained"),
        b"replacement"
    );
}

#[test]
fn connection_limit_is_hard_and_raii_released() {
    let limiter = Arc::new(ConnectionLimiter::new(2));
    let first = limiter.try_acquire().expect("first");
    let second = limiter.try_acquire().expect("second");
    assert!(limiter.try_acquire().is_none());
    assert_eq!(limiter.active(), 2);
    drop(first);
    assert!(limiter.try_acquire().is_some());
    drop(second);
}

#[test]
fn partial_multiple_frames_interleave_but_each_id_is_ordered() {
    let dispatcher: Arc<dyn Dispatcher> = Arc::new(
        |request: Request, _context: &RequestContext, output: &mut OutputSink| {
            if request.id == "slow" {
                thread::sleep(Duration::from_millis(20));
            }
            output.send_result(&request.id, &json!({"echo":request.id}))
        },
    );
    let config = RuntimeConfig {
        workers: 2,
        queue_depth: 4,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    client.write_all(br#"{"v":1,"id":"sl"#).expect("part one");
    client
        .write_all(
            b"ow\",\"method\":\"ping\",\"params\":{}}\n{\"v\":1,\"id\":\"fast\",\"method\":\"ping\",\"params\":{}}\n",
        )
        .expect("part two and second frame");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let first = line(&mut reader);
    let second = line(&mut reader);
    assert_eq!(first["id"], "fast");
    assert_eq!(second["id"], "slow");
    wait_until(|| scheduler.stats().completed == 2);
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    handle.join().expect("connection thread");
}

#[test]
fn queue_saturation_panic_cleanup_and_worker_survival() {
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let started = Arc::new(AtomicBool::new(false));
    let dispatcher: Arc<dyn Dispatcher> = {
        let released = Arc::clone(&released);
        let started = Arc::clone(&started);
        Arc::new(
            move |request: Request, context: &RequestContext, output: &mut OutputSink| {
                if request.id == "hold" {
                    started.store(true, Ordering::Release);
                    let (lock, changed) = &*released;
                    let mut release = lock.lock().expect("release lock");
                    while !*release {
                        context.check()?;
                        let (next, _) = changed
                            .wait_timeout(release, Duration::from_millis(2))
                            .expect("release wait");
                        release = next;
                    }
                }
                if request.id == "panic" {
                    panic!("intentional worker panic");
                }
                output.send_result(&request.id, &json!({"ok":true}))
            },
        )
    };
    let config = RuntimeConfig {
        workers: 1,
        queue_depth: 1,
        max_active_requests_per_connection: 8,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    for id in ["hold", "queued", "overflow"] {
        writeln!(
            client,
            "{{\"v\":1,\"id\":\"{id}\",\"method\":\"ping\",\"params\":{{}}}}"
        )
        .expect("request");
        if id == "hold" {
            wait_until(|| started.load(Ordering::Acquire));
        }
    }
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let rejected = line(&mut reader);
    assert_eq!(rejected["error"]["code"], "QUEUE_FULL");
    *released.0.lock().expect("release lock") = true;
    released.1.notify_all();
    let _ = line(&mut reader);
    let _ = line(&mut reader);
    wait_until(|| scheduler.stats().completed == 2);

    writeln!(
        client,
        "{{\"v\":1,\"id\":\"panic\",\"method\":\"ping\",\"params\":{{}}}}"
    )
    .expect("panic request");
    let panic_frame = line(&mut reader);
    assert_eq!(panic_frame["error"]["code"], "INTERNAL");
    writeln!(
        client,
        "{{\"v\":1,\"id\":\"after\",\"method\":\"ping\",\"params\":{{}}}}"
    )
    .expect("after request");
    let after = line(&mut reader);
    assert_eq!(after["result"]["ok"], true);
    wait_until(|| scheduler.stats().panicked == 1 && scheduler.stats().completed == 4);
    assert!(scheduler.stats().rejected >= 1);
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    handle.join().expect("connection thread");
}

#[test]
fn cancel_reaches_queued_scan_wait_and_work_and_disconnect_releases_everything() {
    let concurrent_scans = Arc::new(AtomicUsize::new(0));
    let max_scans = Arc::new(AtomicUsize::new(0));
    let dispatcher: Arc<dyn Dispatcher> = {
        let concurrent_scans = Arc::clone(&concurrent_scans);
        let max_scans = Arc::clone(&max_scans);
        Arc::new(
            move |request: Request, context: &RequestContext, output: &mut OutputSink| {
                context.with_expensive_scan(|| {
                    let now = concurrent_scans.fetch_add(1, Ordering::AcqRel) + 1;
                    max_scans.fetch_max(now, Ordering::AcqRel);
                    while !context.cancellation().is_cancelled() {
                        thread::sleep(Duration::from_millis(1));
                    }
                    concurrent_scans.fetch_sub(1, Ordering::AcqRel);
                    context.check()
                })?;
                output.send_result(&request.id, &json!({"unexpected":true}))
            },
        )
    };
    let config = RuntimeConfig {
        workers: 2,
        queue_depth: 2,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    writeln!(
        client,
        "{{\"v\":1,\"id\":\"one\",\"method\":\"ping\",\"params\":{{}}}}"
    )
    .expect("one");
    writeln!(
        client,
        "{{\"v\":1,\"id\":\"two\",\"method\":\"ping\",\"params\":{{}}}}"
    )
    .expect("two");
    wait_until(|| scheduler.stats().active == 2);
    let cancel_started = Instant::now();
    writeln!(client, "{{\"v\":1,\"id\":\"cancel-two\",\"method\":\"cancel\",\"params\":{{\"request_id\":\"two\"}}}}")
        .expect("cancel two");
    writeln!(client, "{{\"v\":1,\"id\":\"cancel-one\",\"method\":\"cancel\",\"params\":{{\"request_id\":\"one\"}}}}")
        .expect("cancel one");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let mut codes = Vec::new();
    let mut cancel_results = 0;
    for _ in 0..4 {
        let frame = line(&mut reader);
        if frame["type"] == "error" {
            codes.push(frame["error"]["code"].as_str().unwrap().to_string());
        } else {
            cancel_results += 1;
        }
    }
    assert_eq!(cancel_results, 2);
    assert_eq!(codes, ["CANCELLED", "CANCELLED"]);
    wait_until(|| scheduler.stats().active == 0);
    let cancellation_latency = cancel_started.elapsed();
    eprintln!("work/permit cancellation latency: {cancellation_latency:?}");
    assert!(cancellation_latency < Duration::from_millis(250));
    assert_eq!(max_scans.load(Ordering::Acquire), 1);

    writeln!(
        client,
        "{{\"v\":1,\"id\":\"disconnect\",\"method\":\"ping\",\"params\":{{}}}}"
    )
    .expect("disconnect request");
    wait_until(|| scheduler.stats().active == 1);
    drop(reader);
    drop(client);
    handle.join().expect("connection thread");
    wait_until(|| scheduler.stats().active == 0 && scheduler.stats().connections == 0);
}

#[test]
fn queued_request_cancels_before_dispatch() {
    let release = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let dispatched_queued = Arc::new(AtomicBool::new(false));
    let dispatcher: Arc<dyn Dispatcher> = {
        let release = Arc::clone(&release);
        let started = Arc::clone(&started);
        let dispatched_queued = Arc::clone(&dispatched_queued);
        Arc::new(
            move |request: Request, context: &RequestContext, output: &mut OutputSink| {
                if request.id == "block" {
                    started.store(true, Ordering::Release);
                    while !release.load(Ordering::Acquire) {
                        context.check()?;
                        thread::sleep(Duration::from_millis(1));
                    }
                } else {
                    dispatched_queued.store(true, Ordering::Release);
                }
                output.send_result(&request.id, &json!({"ok":true}))
            },
        )
    };
    let config = RuntimeConfig {
        workers: 1,
        queue_depth: 2,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    client
        .write_all(b"{\"v\":1,\"id\":\"block\",\"method\":\"ping\",\"params\":{}}\n")
        .expect("block");
    wait_until(|| started.load(Ordering::Acquire));
    client
        .write_all(b"{\"v\":1,\"id\":\"queued-cancel\",\"method\":\"ping\",\"params\":{}}\n")
        .expect("queued");
    client
        .write_all(b"{\"v\":1,\"id\":\"cancel\",\"method\":\"cancel\",\"params\":{\"request_id\":\"queued-cancel\"}}\n")
        .expect("cancel");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let cancel = line(&mut reader);
    assert_eq!(cancel["result"]["found"], true);
    release.store(true, Ordering::Release);
    let mut observed_cancel = false;
    for _ in 0..2 {
        let frame = line(&mut reader);
        observed_cancel |= frame["error"]["code"] == "CANCELLED";
    }
    assert!(observed_cancel);
    assert!(!dispatched_queued.load(Ordering::Acquire));
    wait_until(|| scheduler.stats().completed == 2);
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    handle.join().expect("join");
}

#[test]
fn blocked_output_cancellation_and_disconnect_release_worker() {
    let started = Arc::new(AtomicBool::new(false));
    let dispatcher: Arc<dyn Dispatcher> = {
        let started = Arc::clone(&started);
        Arc::new(
            move |request: Request, _context: &RequestContext, output: &mut OutputSink| {
                started.store(true, Ordering::Release);
                output.send_frame(&ResponseFrame::begin(&request.id, "time_value.v1", "1"))?;
                let payload = "x".repeat(64 * 1024);
                for seq in 1..10_000 {
                    output.send_frame(&ResponseFrame::chunk(
                        &request.id,
                        seq,
                        vec![json!({"payload":payload})],
                    ))?;
                }
                Ok(())
            },
        )
    };
    let config = RuntimeConfig {
        workers: 1,
        queue_depth: 1,
        output_chunks: 2,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    client
        .write_all(b"{\"v\":1,\"id\":\"flood\",\"method\":\"ping\",\"params\":{}}\n")
        .expect("flood");
    wait_until(|| started.load(Ordering::Acquire));
    wait_until(|| scheduler.stats().output_blocked > 0);
    let cancel_started = Instant::now();
    client
        .write_all(b"{\"v\":1,\"id\":\"cancel\",\"method\":\"cancel\",\"params\":{\"request_id\":\"flood\"}}\n")
        .expect("cancel");
    wait_until(|| scheduler.stats().active == 0);
    let cancellation_latency = cancel_started.elapsed();
    eprintln!("blocked-output cancellation latency: {cancellation_latency:?}");
    assert!(cancellation_latency < Duration::from_millis(250));

    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let mut legal_terminal = false;
    for _ in 0..32 {
        let mut text = String::new();
        match reader.read_line(&mut text) {
            Ok(0) | Err(_) => break,
            Ok(_) => match serde_json::from_str::<Value>(&text) {
                Ok(frame) => {
                    if frame["id"] == "flood" && frame["type"] == "end" {
                        assert_eq!(frame["complete"], false);
                        assert_eq!(frame["error"]["code"], "CANCELLED");
                        legal_terminal = true;
                        break;
                    }
                }
                Err(_) if scheduler.stats().forced_disconnects >= 1 => break,
                Err(error) => panic!("invalid response without deliberate close: {error}"),
            },
        }
    }
    assert!(legal_terminal || scheduler.stats().forced_disconnects >= 1);
    drop(reader);
    drop(client);
    handle.join().expect("connection joins after disconnect");
    assert_eq!(scheduler.stats().connections, 0);
}

#[test]
fn output_lifecycle_emits_one_legal_terminal_for_errors() {
    let dispatcher: Arc<dyn Dispatcher> = Arc::new(
        |request: Request, _context: &RequestContext, output: &mut OutputSink| {
            if request.id == "stream" {
                output.send_frame(&ResponseFrame::begin(&request.id, "time_value.v1", "1"))?;
                return Err(vcd_tools_rs::server::protocol::ProtocolError::new(
                    vcd_tools_rs::server::protocol::ProtocolErrorCode::Cancelled,
                    "query cancelled",
                    json!({}),
                    false,
                ));
            }
            output.send_result(&request.id, &json!({"ok":true}))?;
            Err(vcd_tools_rs::server::protocol::ProtocolError::new(
                vcd_tools_rs::server::protocol::ProtocolErrorCode::Internal,
                "late error",
                json!({}),
                false,
            ))
        },
    );
    let (mut client, scheduler, handle) = start_connection(RuntimeConfig::default(), dispatcher);
    client
        .write_all(b"{\"v\":1,\"id\":\"stream\",\"method\":\"ping\",\"params\":{}}\n{\"v\":1,\"id\":\"unary\",\"method\":\"ping\",\"params\":{}}\n")
        .expect("requests");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let frames = [line(&mut reader), line(&mut reader), line(&mut reader)];
    let stream = frames
        .iter()
        .filter(|frame| frame["id"] == "stream")
        .collect::<Vec<_>>();
    assert_eq!(stream.len(), 2);
    assert_eq!(stream[0]["type"], "begin");
    assert_eq!(stream[0]["seq"], 0);
    assert_eq!(stream[1]["type"], "end");
    assert_eq!(stream[1]["seq"], 1);
    assert_eq!(stream[1]["complete"], false);
    let unary = frames
        .iter()
        .filter(|frame| frame["id"] == "unary")
        .collect::<Vec<_>>();
    assert_eq!(
        unary.len(),
        1,
        "late errors must not emit a second terminal"
    );
    assert_eq!(unary[0]["type"], "result");
    wait_until(|| scheduler.stats().completed == 2);
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    handle.join().expect("join");
}

#[test]
fn encoded_frame_cap_accepts_n_rejects_n_plus_one_and_bounds_large_writer() {
    let payload = "x".repeat(5_000);
    let exact_frame =
        ResponseFrame::result("boundary", &json!({"payload":payload.clone()})).expect("frame");
    let exact_bytes = encode_json_line(&exact_frame).expect("encoded").len();

    let dispatcher: Arc<dyn Dispatcher> = {
        let payload = payload.clone();
        Arc::new(
            move |request: Request, _context: &RequestContext, output: &mut OutputSink| {
                output.send_result(&request.id, &json!({"payload":payload}))
            },
        )
    };
    let config = RuntimeConfig {
        max_encoded_frame_bytes: exact_bytes,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    client
        .write_all(b"{\"v\":1,\"id\":\"boundary\",\"method\":\"ping\",\"params\":{}}\n")
        .expect("request");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let frame = line(&mut reader);
    assert_eq!(frame["type"], "result");
    assert!(scheduler.stats().max_frame_buffer_bytes <= exact_bytes);
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    handle.join().expect("join");

    let dispatcher: Arc<dyn Dispatcher> = {
        let payload = format!("{payload}x");
        Arc::new(
            move |request: Request, _context: &RequestContext, output: &mut OutputSink| {
                output.send_result(&request.id, &json!({"payload":payload}))
            },
        )
    };
    let config = RuntimeConfig {
        max_encoded_frame_bytes: exact_bytes,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    client
        .write_all(b"{\"v\":1,\"id\":\"boundary\",\"method\":\"ping\",\"params\":{}}\n")
        .expect("request");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let frame = line(&mut reader);
    assert_eq!(frame["error"]["code"], "LIMIT_EXCEEDED");
    assert!(scheduler.stats().max_frame_buffer_bytes <= exact_bytes);
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    handle.join().expect("join");

    let dispatcher: Arc<dyn Dispatcher> = Arc::new(
        |request: Request, _context: &RequestContext, output: &mut OutputSink| {
            output.send_result(&request.id, &json!({"payload":"x".repeat(8 * 1024 * 1024)}))
        },
    );
    let config = RuntimeConfig {
        max_encoded_frame_bytes: 4_096,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    client
        .write_all(b"{\"v\":1,\"id\":\"huge\",\"method\":\"ping\",\"params\":{}}\n")
        .expect("request");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let frame = line(&mut reader);
    assert_eq!(frame["error"]["code"], "LIMIT_EXCEEDED");
    assert!(scheduler.stats().max_frame_buffer_bytes <= 4_096);
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    handle.join().expect("join");
}

#[test]
fn timeout_may_only_lower_maximum_and_overflow_is_rejected() {
    let dispatches = Arc::new(AtomicUsize::new(0));
    let dispatcher: Arc<dyn Dispatcher> = {
        let dispatches = Arc::clone(&dispatches);
        Arc::new(
            move |request: Request, _context: &RequestContext, output: &mut OutputSink| {
                dispatches.fetch_add(1, Ordering::AcqRel);
                output.send_result(&request.id, &json!({"ok":true}))
            },
        )
    };
    let config = RuntimeConfig {
        max_timeout_ms: 10,
        ..RuntimeConfig::default()
    };
    let (mut client, _scheduler, handle) = start_connection(config, dispatcher);
    client
        .write_all(b"{\"v\":1,\"id\":\"high\",\"method\":\"list\",\"params\":{\"timeout_ms\":\"11\"}}\n{\"v\":1,\"id\":\"ok\",\"method\":\"list\",\"params\":{\"timeout_ms\":\"10\"}}\n")
        .expect("requests");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let frames = [line(&mut reader), line(&mut reader)];
    let high = frames
        .iter()
        .find(|frame| frame["id"] == "high")
        .expect("high response");
    let ok = frames
        .iter()
        .find(|frame| frame["id"] == "ok")
        .expect("ok response");
    assert_eq!(high["error"]["code"], "BAD_REQUEST");
    assert_eq!(ok["result"]["ok"], true);
    assert_eq!(dispatches.load(Ordering::Acquire), 1);
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("shutdown");
    handle.join().expect("join");

    let dispatcher: Arc<dyn Dispatcher> = Arc::new(
        |request: Request, _context: &RequestContext, output: &mut OutputSink| {
            output.send_result(&request.id, &json!({"unexpected":true}))
        },
    );
    let config = RuntimeConfig {
        max_timeout_ms: u64::MAX,
        ..RuntimeConfig::default()
    };
    assert!(Scheduler::new(&config, dispatcher).is_err());
}

#[test]
fn malformed_and_oversized_frames_emit_transport_error_and_close() {
    let dispatcher: Arc<dyn Dispatcher> =
        Arc::new(|_request: Request, _context: &RequestContext, _output: &mut OutputSink| Ok(()));
    let config = RuntimeConfig {
        request_line_bytes: 64,
        ..RuntimeConfig::default()
    };
    let (mut client, scheduler, handle) = start_connection(config, dispatcher);
    client.write_all(&vec![b'x'; 65]).expect("oversize");
    let mut reader = BufReader::new(client.try_clone().expect("clone"));
    let mut text = String::new();
    let count = reader.read_line(&mut text).unwrap_or(0);
    if count > 0 {
        let frame: Value = serde_json::from_str(&text).expect("transport error frame");
        assert_eq!(frame["id"], Value::Null);
        assert_eq!(frame["error"]["code"], "LIMIT_EXCEEDED");
    } else {
        assert!(scheduler.stats().forced_disconnects >= 1);
    }
    handle.join().expect("connection thread");
}
