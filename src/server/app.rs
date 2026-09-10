//! Native Unix server assembly and accept loop.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use crate::server::listener::BoundUnixListener;
use crate::server::runtime::{
    ConnectionLimiter, Dispatcher, RuntimeConfig, Scheduler, serve_connection_with_shutdown,
};
use crate::server::service::{ServiceConfig, VcdService};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum ServerEvent {
    ConnectionAccepted,
    ConnectionRejected,
    ConnectionFinished,
}

pub fn run_server(
    vcd: impl AsRef<Path>,
    socket: impl AsRef<Path>,
    runtime: RuntimeConfig,
    service: ServiceConfig,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    runtime.validate()?;
    service.validate()?;
    let dispatcher: Arc<dyn Dispatcher> = Arc::new(
        VcdService::new(vcd, runtime.clone(), service)
            .map_err(|error| io::Error::other(error.to_string()))?,
    );
    run_server_with_dispatcher(socket, runtime, dispatcher, shutdown, None, None)
}

#[doc(hidden)]
pub fn run_server_with_dispatcher(
    socket: impl AsRef<Path>,
    runtime: RuntimeConfig,
    dispatcher: Arc<dyn Dispatcher>,
    shutdown: Arc<AtomicBool>,
    accept_hook: Option<Arc<dyn Fn() -> io::Result<()> + Send + Sync>>,
    event_hook: Option<Arc<dyn Fn(ServerEvent) + Send + Sync>>,
) -> io::Result<()> {
    runtime.validate()?;
    let scheduler = Scheduler::new(&runtime, dispatcher)?;
    let limiter = Arc::new(ConnectionLimiter::new(runtime.max_connections));
    let mut listener = BoundUnixListener::bind(socket)?;
    listener.set_nonblocking(true)?;
    let mut connections = Vec::new();

    let accept_result = loop {
        if shutdown.load(Ordering::Acquire) {
            break Ok(());
        }
        reap_finished(&mut connections);
        if let Some(hook) = &accept_hook
            && let Err(error) = hook()
        {
            break Err(error);
        }
        match listener.listener().accept() {
            Ok((stream, _)) => {
                let Some(permit) = limiter.try_acquire() else {
                    if let Some(hook) = &event_hook {
                        hook(ServerEvent::ConnectionRejected);
                    }
                    drop(stream);
                    continue;
                };
                if let Some(hook) = &event_hook {
                    hook(ServerEvent::ConnectionAccepted);
                }
                let scheduler = Arc::clone(&scheduler);
                let config = runtime.clone();
                let shutdown = Arc::clone(&shutdown);
                let connection_hook = event_hook.clone();
                connections.push(thread::spawn(move || {
                    let _ = serve_connection_with_shutdown(
                        stream, scheduler, &config, permit, shutdown,
                    );
                    if let Some(hook) = connection_hook {
                        hook(ServerEvent::ConnectionFinished);
                    }
                }));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => break Err(error),
        }
    };

    // Every post-bind exit follows one teardown path. Signal connection loops,
    // stop admission, join accepted threads, and remove only the owned socket.
    shutdown.store(true, Ordering::Release);
    for connection in connections {
        let _ = connection.join();
    }
    let cleanup_result = listener.cleanup();
    match accept_result {
        Err(error) => {
            let _ = cleanup_result;
            Err(error)
        }
        Ok(()) => cleanup_result,
    }
}

fn reap_finished(connections: &mut Vec<thread::JoinHandle<()>>) {
    let mut active = Vec::with_capacity(connections.len());
    for connection in connections.drain(..) {
        if connection.is_finished() {
            let _ = connection.join();
        } else {
            active.push(connection);
        }
    }
    *connections = active;
}
