mod fst;
mod identity;

use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use vcd::{Command, IdCode, Parser, VarType};

use crate::catalog::{CatalogMemoryEstimate, SignalCatalog, SignalKey};
use crate::header::read_identified_compact_header;
use crate::query::{QueryContext, QueryError, QueryLimitKind, QueryResult};
use crate::{Result, Signal, SignalIndex, Timescale, VcdError, VcdMeta};

pub use fst::{FstError, FstMeta, FstSignal, OpenedFst};

pub use identity::{
    ContentFingerprint, FileIdentity, FingerprintPolicy, GenerationId, OpenOptions,
};
pub(crate) use identity::{generation_mismatch_error, is_generation_mismatch};

/// Memory owned by the compact signal catalog.
///
/// Hash-table control bytes and allocator metadata are estimated rather than
/// measured exactly. This structure is intended for diagnostics and regression
/// tracking, not enforcement of a hard allocator limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogMemoryUsage {
    pub total_bytes: usize,
    pub signal_bytes: usize,
    pub name_bytes: usize,
    pub scope_bytes: usize,
    pub alias_bytes: usize,
    pub lookup_bytes: usize,
}

impl From<CatalogMemoryEstimate> for CatalogMemoryUsage {
    fn from(value: CatalogMemoryEstimate) -> Self {
        Self {
            total_bytes: value.total_bytes,
            signal_bytes: value.signal_bytes,
            name_bytes: value.name_bytes,
            scope_bytes: value.scope_bytes,
            alias_bytes: value.alias_bytes,
            lookup_bytes: value.lookup_bytes,
        }
    }
}

/// Borrowed metadata for one signal in an [`OpenedVcd`].
///
/// The reference cannot outlive the opened catalog. Names and scope components
/// borrow the catalog's interned storage; converting to the legacy owned
/// [`Signal`] is explicit.
#[derive(Clone, Copy)]
pub struct SignalRef<'a> {
    catalog: &'a SignalCatalog,
    key: SignalKey,
}

impl fmt::Debug for SignalRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignalRef")
            .field("name", &self.name())
            .field("id_code", &self.id_code())
            .field("size", &self.size())
            .field("type_", &self.var_type())
            .finish()
    }
}

impl<'a> SignalRef<'a> {
    fn metadata(self) -> &'a crate::catalog::SignalMeta {
        self.catalog.signal(self.key)
    }

    pub fn name(self) -> &'a str {
        self.metadata().full_name.as_ref()
    }

    pub(crate) fn shared_name(self) -> Arc<str> {
        Arc::clone(&self.metadata().full_name)
    }

    pub fn id_code(self) -> IdCode {
        self.metadata().id_code
    }

    pub fn size(self) -> u32 {
        self.metadata().size
    }

    pub fn var_type(self) -> VarType {
        self.metadata().type_
    }

    pub fn scope_components(self) -> impl ExactSizeIterator<Item = &'a str> {
        self.catalog.scope_component_refs(self.key).into_iter()
    }

    pub fn to_owned(self) -> Signal {
        Signal {
            name: self.name().to_string(),
            id_code: self.id_code(),
            size: self.size(),
            type_: self.var_type(),
            scope: self.scope_components().map(str::to_string).collect(),
        }
    }
}

#[derive(Debug, Clone)]
enum CachedMetadataError {
    GenerationMismatch,
    Io {
        kind: io::ErrorKind,
        message: Arc<str>,
    },
    MissingEndDefinitions,
    DuplicateSignal(Arc<str>),
    MissingSignals(Arc<str>),
    MissingSignal(Arc<str>),
    InvalidOccurrence,
    Parse(Arc<str>),
    BuilderPanicked,
}

impl CachedMetadataError {
    fn capture(error: &VcdError) -> Self {
        match error {
            VcdError::Io(error) if is_generation_mismatch(error) => Self::GenerationMismatch,
            VcdError::Io(error) => Self::Io {
                kind: error.kind(),
                message: Arc::from(error.to_string()),
            },
            VcdError::MissingEndDefinitions => Self::MissingEndDefinitions,
            VcdError::DuplicateSignal(signal) => Self::DuplicateSignal(Arc::from(signal.as_str())),
            VcdError::MissingSignals(signals) => Self::MissingSignals(Arc::from(signals.as_str())),
            VcdError::MissingSignal(signal) => Self::MissingSignal(Arc::from(signal.as_str())),
            VcdError::InvalidOccurrence => Self::InvalidOccurrence,
            VcdError::Parse(message) => Self::Parse(Arc::from(message.as_str())),
        }
    }

    fn to_error(&self) -> VcdError {
        match self {
            Self::GenerationMismatch => generation_mismatch_error(),
            Self::Io { kind, message } => VcdError::Io(io::Error::new(*kind, message.to_string())),
            Self::MissingEndDefinitions => VcdError::MissingEndDefinitions,
            Self::DuplicateSignal(signal) => VcdError::DuplicateSignal(signal.to_string()),
            Self::MissingSignals(signals) => VcdError::MissingSignals(signals.to_string()),
            Self::MissingSignal(signal) => VcdError::MissingSignal(signal.to_string()),
            Self::InvalidOccurrence => VcdError::InvalidOccurrence,
            Self::Parse(message) => VcdError::Parse(message.to_string()),
            Self::BuilderPanicked => VcdError::Parse("metadata builder panicked".to_string()),
        }
    }
}

/// Attempt-stability invariant:
///
/// A caller that observes `Building` registers against that attempt before
/// waiting. A failed (or panicked) attempt retains its terminal error and the
/// count of registered waiters. Retry cannot replace that outcome until every
/// registered waiter has reacquired the lock and acknowledged it. Thus only
/// one terminal attempt is retained, bounding state while preventing a racing
/// retry from making an old waiter observe a newer attempt.
#[derive(Debug)]
struct MetadataState {
    next_attempt: u64,
    phase: MetadataPhase,
}

#[derive(Debug)]
enum MetadataPhase {
    Absent,
    Building {
        attempt: u64,
        waiters: usize,
    },
    Ready(Arc<VcdMeta>),
    Failed {
        attempt: u64,
        error: CachedMetadataError,
        pending_waiters: usize,
    },
}

#[derive(Debug)]
struct MetadataCache {
    state: Mutex<MetadataState>,
    changed: Condvar,
    #[cfg(test)]
    test_hooks: MetadataTestHooks,
}

impl MetadataCache {
    fn new() -> Self {
        Self {
            state: Mutex::new(MetadataState {
                next_attempt: 1,
                phase: MetadataPhase::Absent,
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            test_hooks: MetadataTestHooks::new(),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, MetadataState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn wait_for_change<'a>(
        &'a self,
        state: MutexGuard<'a, MetadataState>,
    ) -> MutexGuard<'a, MetadataState> {
        let state = self
            .changed
            .wait(state)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[cfg(test)]
        {
            // Drop the state lock while the deterministic wake hook pauses.
            // Attempt waiter accounting guarantees that retry cannot erase the
            // terminal outcome before this caller reacquires the lock.
            drop(state);
            self.test_hooks.after_wait_woken();
            self.lock_state()
        }
        #[cfg(not(test))]
        state
    }

    fn wait_for_change_with_context<'a>(
        &'a self,
        state: MutexGuard<'a, MetadataState>,
        context: &QueryContext,
    ) -> MutexGuard<'a, MetadataState> {
        let state = self
            .changed
            .wait_timeout(state, context.metadata_wait_timeout())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
        #[cfg(test)]
        {
            drop(state);
            self.test_hooks.after_wait_woken();
            self.lock_state()
        }
        #[cfg(not(test))]
        state
    }

    fn get_or_build_with_context<F>(
        &self,
        context: &QueryContext,
        build: F,
    ) -> QueryResult<Arc<VcdMeta>>
    where
        F: FnOnce() -> QueryResult<VcdMeta>,
    {
        context.check()?;
        let mut state = self.lock_state();
        let mut registration: Option<MetadataWaiterRegistration<'_>> = None;
        let attempt = loop {
            match &mut state.phase {
                MetadataPhase::Ready(metadata) => {
                    let metadata = Arc::clone(metadata);
                    drop(state);
                    context.check()?;
                    if let Some(mut registration) = registration.take() {
                        registration.disarm();
                    }
                    return Ok(metadata);
                }
                MetadataPhase::Failed { attempt, error, .. } => {
                    let result = Err(error.to_error().into());
                    let expected_attempt = *attempt;
                    drop(state);
                    if let Some(registration) = registration.take() {
                        debug_assert_eq!(registration.attempt, expected_attempt);
                        drop(registration);
                    }
                    context.check()?;
                    return result;
                }
                MetadataPhase::Building { attempt, waiters } => {
                    if registration
                        .as_ref()
                        .is_some_and(|registration| registration.attempt != *attempt)
                    {
                        let mut old = registration.take().expect("old metadata registration");
                        old.disarm();
                    }
                    if registration.is_none() {
                        *waiters += 1;
                        registration = Some(MetadataWaiterRegistration::new(self, *attempt));
                    }
                    if let Err(error) = context.check() {
                        drop(state);
                        drop(registration);
                        return Err(error);
                    }
                    state = self.wait_for_change_with_context(state, context);
                }
                MetadataPhase::Absent => {
                    // A context-cancelled/deadline-limited builder resets its
                    // transient attempt to Absent. A waiter registered on that
                    // attempt may now elect the replacement build; the old
                    // registration has no terminal acknowledgement to retain.
                    if let Some(mut old) = registration.take() {
                        old.disarm();
                    }
                    context.check()?;
                    let attempt = state.next_attempt;
                    state.next_attempt = state.next_attempt.checked_add(1).ok_or_else(|| {
                        crate::query::QueryError::internal(
                            "metadata attempt identifier space exhausted",
                        )
                    })?;
                    state.phase = MetadataPhase::Building {
                        attempt,
                        waiters: 0,
                    };
                    break attempt;
                }
            }
        };
        drop(state);

        // A context-aware elected builder remains the shared attempt, but its
        // scan cooperatively observes cancellation/deadline/command limits.
        // Transient policy termination resets the attempt to Absent so a
        // registered waiter can elect a fresh builder; source/VCD failures are
        // cached with the existing attempt-stability semantics.
        let mut guard = MetadataBuildGuard {
            cache: self,
            attempt,
            armed: true,
        };
        #[cfg(test)]
        self.test_hooks.before_build();
        let result = build();
        #[cfg(test)]
        self.test_hooks.before_publish();
        let mut state = self.lock_state();
        let waiters = match &state.phase {
            MetadataPhase::Building {
                attempt: active,
                waiters,
            } if *active == attempt => *waiters,
            _ => panic!("metadata attempt changed while its builder was active"),
        };
        match result {
            Ok(metadata) => {
                let metadata = Arc::new(metadata);
                state.phase = MetadataPhase::Ready(Arc::clone(&metadata));
                guard.armed = false;
                drop(state);
                self.changed.notify_all();
                context.check()?;
                Ok(metadata)
            }
            Err(error) => {
                let (cache_error, transient) = match error {
                    QueryError::StaleSource(error) | QueryError::Vcd(error) => (Some(error), None),
                    QueryError::SourceUnavailable(error) => (Some(VcdError::Io(error)), None),
                    error => (None, Some(error)),
                };
                if let Some(error) = cache_error {
                    let cached = CachedMetadataError::capture(&error);
                    state.phase = MetadataPhase::Failed {
                        attempt,
                        error: cached.clone(),
                        pending_waiters: waiters,
                    };
                    guard.armed = false;
                    drop(state);
                    self.changed.notify_all();
                    context.check()?;
                    Err(cached.to_error().into())
                } else {
                    state.phase = MetadataPhase::Absent;
                    guard.armed = false;
                    drop(state);
                    self.changed.notify_all();
                    Err(transient.expect("non-cacheable query failure"))
                }
            }
        }
    }

    fn get_or_build<F>(&self, build: F) -> Result<Arc<VcdMeta>>
    where
        F: FnOnce() -> Result<VcdMeta>,
    {
        let mut state = self.lock_state();
        let mut waiting_for = None;
        let attempt = loop {
            match &mut state.phase {
                MetadataPhase::Ready(metadata) => return Ok(Arc::clone(metadata)),
                MetadataPhase::Failed {
                    attempt,
                    error,
                    pending_waiters,
                } => {
                    let result = Err(error.to_error());
                    if waiting_for == Some(*attempt) {
                        debug_assert!(*pending_waiters > 0);
                        *pending_waiters -= 1;
                        let last_waiter = *pending_waiters == 0;
                        drop(state);
                        if last_waiter {
                            self.changed.notify_all();
                        }
                    }
                    return result;
                }
                MetadataPhase::Building { attempt, waiters } => {
                    match waiting_for {
                        Some(expected) => debug_assert_eq!(expected, *attempt),
                        None => {
                            *waiters += 1;
                            waiting_for = Some(*attempt);
                        }
                    }
                    state = self.wait_for_change(state);
                }
                MetadataPhase::Absent => {
                    debug_assert!(waiting_for.is_none());
                    let attempt = state.next_attempt;
                    state.next_attempt = state.next_attempt.checked_add(1).ok_or_else(|| {
                        VcdError::Parse("metadata attempt identifier space exhausted".to_string())
                    })?;
                    state.phase = MetadataPhase::Building {
                        attempt,
                        waiters: 0,
                    };
                    break attempt;
                }
            }
        };
        drop(state);

        let mut guard = MetadataBuildGuard {
            cache: self,
            attempt,
            armed: true,
        };
        #[cfg(test)]
        self.test_hooks.before_build();
        let result = build();
        let mut state = self.lock_state();
        let waiters = match &state.phase {
            MetadataPhase::Building {
                attempt: active,
                waiters,
            } if *active == attempt => *waiters,
            _ => panic!("metadata attempt changed while its builder was active"),
        };
        match result {
            Ok(metadata) => {
                let metadata = Arc::new(metadata);
                state.phase = MetadataPhase::Ready(Arc::clone(&metadata));
                guard.armed = false;
                drop(state);
                self.changed.notify_all();
                Ok(metadata)
            }
            Err(error) => {
                state.phase = MetadataPhase::Failed {
                    attempt,
                    error: CachedMetadataError::capture(&error),
                    pending_waiters: waiters,
                };
                guard.armed = false;
                drop(state);
                self.changed.notify_all();
                Err(error)
            }
        }
    }

    fn retry_failed<F>(&self, build: F) -> Result<Arc<VcdMeta>>
    where
        F: FnOnce() -> Result<VcdMeta>,
    {
        let mut state = self.lock_state();
        loop {
            match &state.phase {
                MetadataPhase::Failed {
                    pending_waiters: 0, ..
                } => {
                    state.phase = MetadataPhase::Absent;
                    break;
                }
                MetadataPhase::Failed { .. } => {
                    #[cfg(test)]
                    self.test_hooks
                        .retry_waiters
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    #[cfg(test)]
                    self.test_hooks
                        .retry_waiters
                        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
                _ => {
                    drop(state);
                    return self.get_or_build(build);
                }
            }
        }
        drop(state);
        self.get_or_build(build)
    }

    fn is_ready(&self) -> bool {
        matches!(self.lock_state().phase, MetadataPhase::Ready(_))
    }
}

struct MetadataWaiterRegistration<'a> {
    cache: &'a MetadataCache,
    attempt: u64,
    active: bool,
}

impl<'a> MetadataWaiterRegistration<'a> {
    fn new(cache: &'a MetadataCache, attempt: u64) -> Self {
        Self {
            cache,
            attempt,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for MetadataWaiterRegistration<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.cache.lock_state();
        let notify = match &mut state.phase {
            MetadataPhase::Building { attempt, waiters } if *attempt == self.attempt => {
                debug_assert!(*waiters > 0);
                *waiters -= 1;
                false
            }
            MetadataPhase::Failed {
                attempt,
                pending_waiters,
                ..
            } if *attempt == self.attempt => {
                debug_assert!(*pending_waiters > 0);
                *pending_waiters -= 1;
                *pending_waiters == 0
            }
            MetadataPhase::Ready(_) | MetadataPhase::Absent => false,
            _ => false,
        };
        drop(state);
        if notify {
            self.cache.changed.notify_all();
        }
    }
}

struct MetadataBuildGuard<'a> {
    cache: &'a MetadataCache,
    attempt: u64,
    armed: bool,
}

impl Drop for MetadataBuildGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut state = self.cache.lock_state();
        let waiters = match &state.phase {
            MetadataPhase::Building { attempt, waiters } if *attempt == self.attempt => *waiters,
            _ => return,
        };
        state.phase = MetadataPhase::Failed {
            attempt: self.attempt,
            error: CachedMetadataError::BuilderPanicked,
            pending_waiters: waiters,
        };
        drop(state);
        self.cache.changed.notify_all();
    }
}

#[cfg(test)]
#[derive(Debug)]
struct MetadataTestHooks {
    scans: std::sync::atomic::AtomicUsize,
    retry_waiters: std::sync::atomic::AtomicUsize,
    pause: Mutex<Option<MetadataTestPause>>,
    fail_once: std::sync::atomic::AtomicBool,
    panic_once: std::sync::atomic::AtomicBool,
    after_wait: Mutex<Option<MetadataTestPause>>,
    before_validation: Mutex<Option<MetadataTestPause>>,
    before_publish: Mutex<Option<MetadataTestPause>>,
}

#[cfg(test)]
#[derive(Debug, Clone)]
struct MetadataTestPause {
    reached: Arc<std::sync::Barrier>,
    resume: Arc<std::sync::Barrier>,
}

#[cfg(test)]
impl MetadataTestHooks {
    fn new() -> Self {
        Self {
            scans: std::sync::atomic::AtomicUsize::new(0),
            retry_waiters: std::sync::atomic::AtomicUsize::new(0),
            pause: Mutex::new(None),
            fail_once: std::sync::atomic::AtomicBool::new(false),
            panic_once: std::sync::atomic::AtomicBool::new(false),
            after_wait: Mutex::new(None),
            before_validation: Mutex::new(None),
            before_publish: Mutex::new(None),
        }
    }

    fn pause(slot: &Mutex<Option<MetadataTestPause>>) {
        let pause = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(pause) = pause {
            pause.reached.wait();
            pause.resume.wait();
        }
    }

    fn before_build(&self) {
        self.scans.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self::pause(&self.pause);
        if self
            .panic_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            panic!("injected metadata builder panic");
        }
    }

    fn after_wait_woken(&self) {
        Self::pause(&self.after_wait);
    }

    fn before_validation(&self) {
        Self::pause(&self.before_validation);
    }

    fn before_publish(&self) {
        Self::pause(&self.before_publish);
    }
}

#[derive(Default)]
struct MetadataScanHook {
    callback: Mutex<Option<Arc<dyn Fn(u64) + Send + Sync>>>,
}

impl fmt::Debug for MetadataScanHook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MetadataScanHook(..)")
    }
}

#[derive(Debug)]
struct OpenedVcdInner {
    display_path: PathBuf,
    configured_path: PathBuf,
    canonical_target: PathBuf,
    generation: GenerationId,
    identity: FileIdentity,
    body_offset: u64,
    timescale: Option<Timescale>,
    catalog: SignalCatalog,
    options: OpenOptions,
    metadata: MetadataCache,
    metadata_scan_hook: MetadataScanHook,
}

/// One immutable VCD generation with a reusable compact signal catalog.
///
/// Cloning is cheap: clones share immutable state through an [`Arc`]. Body
/// queries open and validate independent file handles; no mutable parser or
/// shared seek cursor is stored here.
#[derive(Debug, Clone)]
pub struct OpenedVcd {
    inner: Arc<OpenedVcdInner>,
}

impl OpenedVcd {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_options(path, OpenOptions::default())
    }

    pub fn open_with_options(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let display_path = path.as_ref().to_path_buf();
        // Stabilize relative paths against later process cwd changes without
        // resolving the final symlink. Reopening this exact configured path is
        // part of generation validation: a retargeted symlink must not silently
        // keep serving the target resolved during open.
        let configured_path = if display_path.is_absolute() {
            display_path.clone()
        } else {
            std::env::current_dir()?.join(&display_path)
        };
        let identified = read_identified_compact_header(&configured_path, &options)?;
        // Canonical target is diagnostic only and is never used to reopen the
        // source. It may cease to exist or change after this generation opens.
        let canonical_target = std::fs::canonicalize(&configured_path)?;
        let header = identified.header;
        debug_assert_eq!(header.body_offset, identified.identity.body_offset());
        Ok(Self {
            inner: Arc::new(OpenedVcdInner {
                display_path,
                configured_path,
                canonical_target,
                generation: GenerationId::fresh(),
                identity: identified.identity,
                body_offset: header.body_offset,
                timescale: header.timescale,
                catalog: header.catalog,
                options,
                metadata: MetadataCache::new(),
                metadata_scan_hook: MetadataScanHook::default(),
            }),
        })
    }

    /// The path spelling supplied by the caller.
    pub fn path(&self) -> &Path {
        &self.inner.display_path
    }

    /// Absolute configured path used for every independent reopen. The final
    /// component is not canonicalized, so symlink retargeting is detectable.
    pub fn configured_path(&self) -> &Path {
        &self.inner.configured_path
    }

    /// Canonical target captured for diagnostics at open time. Readers never
    /// reopen this path; they use [`OpenedVcd::configured_path`].
    pub fn canonical_path(&self) -> &Path {
        &self.inner.canonical_target
    }

    pub fn generation(&self) -> GenerationId {
        self.inner.generation
    }

    pub fn identity(&self) -> &FileIdentity {
        &self.inner.identity
    }

    pub fn options(&self) -> &OpenOptions {
        &self.inner.options
    }

    pub fn body_offset(&self) -> u64 {
        self.inner.body_offset
    }

    pub fn timescale(&self) -> Option<&Timescale> {
        self.inner.timescale.as_ref()
    }

    pub fn signal_count(&self) -> usize {
        self.inner.catalog.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.catalog.is_empty()
    }

    pub fn signals(&self) -> impl ExactSizeIterator<Item = SignalRef<'_>> {
        self.inner.catalog.signal_keys().map(|key| SignalRef {
            catalog: &self.inner.catalog,
            key,
        })
    }

    pub fn signal(&self, name: &str) -> Option<SignalRef<'_>> {
        self.inner.catalog.key_by_name(name).map(|key| SignalRef {
            catalog: &self.inner.catalog,
            key,
        })
    }

    pub fn aliases(&self, id: IdCode) -> impl ExactSizeIterator<Item = SignalRef<'_>> {
        self.inner
            .catalog
            .aliases(id)
            .iter()
            .copied()
            .map(|key| SignalRef {
                catalog: &self.inner.catalog,
                key,
            })
    }

    pub fn signal_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.inner.catalog.names()
    }

    pub fn list_signals(&self, filter: Option<&str>) -> Vec<String> {
        self.signal_names()
            .filter(|name| filter.is_none_or(|filter| name.contains(filter)))
            .map(str::to_string)
            .collect()
    }

    pub fn catalog_memory_usage(&self) -> CatalogMemoryUsage {
        self.inner.catalog.estimated_owned_bytes().into()
    }

    /// Materialize legacy owned signal metadata without changing its shape.
    pub fn to_owned_signals(&self) -> Vec<Signal> {
        self.inner.catalog.to_owned_signals()
    }

    /// Materialize the legacy clone-heavy signal index explicitly.
    pub fn to_compatibility_parts(&self) -> Result<(Vec<Signal>, SignalIndex)> {
        self.inner.catalog.to_compatibility_parts()
    }

    /// Open a new independently positioned reader after validating the source
    /// path against this generation.
    pub fn body_reader(&self) -> Result<OpenedBodyReader> {
        let mut file = File::open(&self.inner.configured_path)?;
        self.inner
            .identity
            .validate_open_file(&mut file, self.inner.options.fingerprint_policy())?;
        Ok(OpenedBodyReader {
            reader: BufReader::new(file),
            generation: Arc::clone(&self.inner),
        })
    }

    /// Create a forward-only VCD command parser backed by an independent,
    /// generation-validated reader.
    pub fn body_parser(&self) -> Result<Parser<OpenedBodyReader>> {
        Ok(Parser::new(self.body_reader()?))
    }

    /// Revalidate the configured source path without creating a parser.
    pub fn validate_source(&self) -> Result<()> {
        let mut file = File::open(&self.inner.configured_path)?;
        self.inner
            .identity
            .validate_open_file(&mut file, self.inner.options.fingerprint_policy())
    }

    /// Return exact body time bounds, scanning the body only on the first call.
    ///
    /// Concurrent callers share one scan and receive the same immutable result.
    /// A failed scan is cached so waiters observe one deterministic failure;
    /// [`OpenedVcd::retry_metadata`] explicitly starts a new attempt. Query
    /// cancellation is intentionally deferred to the M2 `QueryContext` rather
    /// than adding a conflicting metadata-only cancellation API.
    pub fn metadata(&self) -> Result<Arc<VcdMeta>> {
        self.inner.metadata.get_or_build(|| self.compute_metadata())
    }

    /// Return exact metadata under a transport-neutral query policy.
    ///
    /// The elected shared attempt actively observes this context while parsing.
    /// Cancellation/deadline/command-limit termination is not cached: waiters
    /// are woken and may elect a replacement attempt. Source and parser errors
    /// retain normal attempt-stable cache semantics.
    pub fn metadata_with_context(&self, context: &QueryContext) -> QueryResult<Arc<VcdMeta>> {
        self.inner
            .metadata
            .get_or_build_with_context(context, || self.compute_metadata_with_context(context))
    }

    /// Retry metadata after a cached failed attempt.
    ///
    /// Ready metadata is returned without another scan. Concurrent retry
    /// callers coalesce behind one builder. If callers are still waking from
    /// the failed attempt, retry waits for them to acknowledge that exact
    /// failure before replacing it with a new attempt.
    pub fn retry_metadata(&self) -> Result<Arc<VcdMeta>> {
        self.inner.metadata.retry_failed(|| self.compute_metadata())
    }

    /// Whether exact metadata is already available without a body scan.
    pub fn has_cached_metadata(&self) -> bool {
        self.inner.metadata.is_ready()
    }

    #[doc(hidden)]
    pub fn set_metadata_scan_hook(&self, hook: Arc<dyn Fn(u64) + Send + Sync>) {
        *self
            .inner
            .metadata_scan_hook
            .callback
            .lock()
            .expect("metadata scan hook") = Some(hook);
    }

    fn compute_metadata_with_context(&self, context: &QueryContext) -> QueryResult<VcdMeta> {
        context.check()?;
        #[cfg(test)]
        if self
            .inner
            .metadata
            .test_hooks
            .fail_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(QueryError::Vcd(VcdError::Parse(
                "injected metadata build failure".to_string(),
            )));
        }
        let mut parser = self.body_parser().map_err(QueryError::from)?;
        let scan_hook = self
            .inner
            .metadata_scan_hook
            .callback
            .lock()
            .expect("metadata scan hook")
            .take();
        let mut current_time = 0_u64;
        let mut start_time = None;
        let mut end_time = None;
        let mut commands = 0_u64;
        for command in &mut parser {
            commands = commands.saturating_add(1);
            if commands == 1
                && let Some(hook) = &scan_hook
            {
                hook(commands);
            }
            if let Some(limit) = context.limits().max_commands()
                && commands > limit
            {
                return Err(QueryError::LimitExceeded {
                    kind: QueryLimitKind::Commands,
                    limit,
                    actual: commands,
                });
            }
            let command = command.map_err(VcdError::from).map_err(QueryError::from)?;
            if commands % crate::query::stream::COMMAND_CHECK_INTERVAL == 0
                || matches!(command, Command::Timestamp(_))
            {
                context.check()?;
            }
            match command {
                Command::Timestamp(time) => {
                    current_time = time;
                    start_time.get_or_insert(time);
                    end_time = Some(time);
                }
                Command::ChangeScalar(_, _)
                | Command::ChangeVector(_, _)
                | Command::ChangeReal(_, _)
                | Command::ChangeString(_, _) => {
                    start_time.get_or_insert(current_time);
                    end_time = Some(current_time);
                }
                _ => {}
            }
        }
        context.check()?;
        #[cfg(test)]
        self.inner.metadata.test_hooks.before_validation();
        parser
            .reader()
            .validate_complete()
            .map_err(QueryError::from)?;
        context.check()?;
        let start_time = start_time.unwrap_or(0);
        Ok(VcdMeta {
            timescale: self.inner.timescale.clone(),
            signal_count: self.inner.catalog.len(),
            start_time,
            end_time: end_time.unwrap_or(start_time),
        })
    }

    fn compute_metadata(&self) -> Result<VcdMeta> {
        #[cfg(test)]
        if self
            .inner
            .metadata
            .test_hooks
            .fail_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(VcdError::Parse(
                "injected metadata build failure".to_string(),
            ));
        }
        let mut parser = self.body_parser()?;
        let (start_time, end_time) = crate::compute_time_bounds_in_place(&mut parser)?;
        #[cfg(test)]
        self.inner.metadata.test_hooks.before_validation();
        parser.reader().validate_complete()?;
        Ok(VcdMeta {
            timescale: self.inner.timescale.clone(),
            signal_count: self.inner.catalog.len(),
            start_time,
            end_time,
        })
    }
}

/// Independently positioned body reader for one [`OpenedVcd`] generation.
///
/// Call [`OpenedBodyReader::validate_complete`] before publishing a completed
/// result. A dropped reader cannot report a source mutation, so query engines
/// must make completion validation explicit.
pub struct OpenedBodyReader {
    reader: BufReader<File>,
    generation: Arc<OpenedVcdInner>,
}

impl fmt::Debug for OpenedBodyReader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenedBodyReader")
            .field("generation", &self.generation.generation)
            .field("path", &self.generation.display_path)
            .finish_non_exhaustive()
    }
}

impl OpenedBodyReader {
    pub fn generation(&self) -> GenerationId {
        self.generation.generation
    }

    /// Validate source identity after query execution while retaining the
    /// reader's logical cursor position.
    pub fn validate_complete(&mut self) -> Result<()> {
        let position = self.reader.stream_position()?;
        let validation = self.generation.identity.validate_open_file(
            self.reader.get_mut(),
            self.generation.options.fingerprint_policy(),
        );
        let restore = self.reader.seek(SeekFrom::Start(position));
        validation?;
        restore?;

        // The admitted handle can remain valid after a configured symlink is
        // retargeted. Reopen and validate the configured path as well so a
        // completed response is bound to the same path generation.
        let mut current = File::open(&self.generation.configured_path)?;
        self.generation
            .identity
            .validate_open_file(&mut current, self.generation.options.fingerprint_policy())?;
        Ok(())
    }
}

impl Read for OpenedBodyReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

impl BufRead for OpenedBodyReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.reader.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.reader.consume(amount);
    }
}

impl Seek for OpenedBodyReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.reader.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Seek as _, Write as _};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Instant;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;
    use vcd::Command;

    use crate::query::{CancellationToken, QueryErrorCode};

    use super::*;

    const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";

    fn assert_send_sync<T: Send + Sync>() {}

    fn copy_fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("wave.vcd");
        std::fs::copy(SEMANTICS, &path).expect("copy fixture");
        (directory, path)
    }

    fn metadata_pause() -> (MetadataTestPause, Arc<Barrier>, Arc<Barrier>) {
        metadata_pause_for(1)
    }

    fn metadata_pause_for(
        paused_workers: usize,
    ) -> (MetadataTestPause, Arc<Barrier>, Arc<Barrier>) {
        let reached = Arc::new(Barrier::new(paused_workers + 1));
        let resume = Arc::new(Barrier::new(paused_workers + 1));
        (
            MetadataTestPause {
                reached: Arc::clone(&reached),
                resume: Arc::clone(&resume),
            },
            reached,
            resume,
        )
    }

    fn wait_for_metadata_waiters(opened: &OpenedVcd, expected: usize) {
        for _ in 0..100_000 {
            let state = opened.inner.metadata.lock_state();
            let registered = matches!(
                state.phase,
                MetadataPhase::Building { waiters, .. } if waiters == expected
            );
            drop(state);
            if registered {
                return;
            }
            thread::yield_now();
        }
        panic!("metadata waiters did not register");
    }

    fn wait_for_metadata_retry_waiters(opened: &OpenedVcd, expected: usize) {
        for _ in 0..100_000 {
            if opened
                .inner
                .metadata
                .test_hooks
                .retry_waiters
                .load(Ordering::SeqCst)
                == expected
            {
                return;
            }
            thread::yield_now();
        }
        panic!("metadata retry waiters did not block");
    }

    fn replace_file(replacement: &Path, destination: &Path) {
        #[cfg(windows)]
        std::fs::remove_file(destination).expect("remove destination");
        std::fs::rename(replacement, destination).expect("replace");
    }

    #[test]
    fn opened_vcd_is_send_sync_and_clone_shares_inner() {
        assert_send_sync::<OpenedVcd>();
        assert_send_sync::<OpenedBodyReader>();
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let cloned = opened.clone();
        assert!(Arc::ptr_eq(&opened.inner, &cloned.inner));
        assert_eq!(opened.generation(), cloned.generation());
    }

    #[test]
    fn open_exposes_compact_order_lookup_aliases_scope_and_diagnostics() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        assert_eq!(opened.path(), Path::new(SEMANTICS));
        assert!(opened.configured_path().is_absolute());
        assert!(opened.configured_path().ends_with(SEMANTICS));
        assert!(opened.canonical_path().is_absolute());
        assert_eq!(opened.signal_count(), 7);
        assert!(!opened.is_empty());
        assert_eq!(opened.signal_names().next(), Some("top.a"));
        assert_eq!(opened.list_signals(Some("vec")), vec!["top.vec4[3:0]"]);
        let signal = opened.signal("top.a").expect("signal");
        assert_eq!(signal.name(), "top.a");
        assert_eq!(signal.size(), 1);
        assert_eq!(signal.scope_components().collect::<Vec<_>>(), ["top"]);
        assert_eq!(signal.to_owned().scope, ["top"]);
        assert_eq!(
            opened
                .aliases(signal.id_code())
                .map(SignalRef::name)
                .collect::<Vec<_>>(),
            ["top.a", "top.alias_a"]
        );
        assert!(opened.catalog_memory_usage().total_bytes > 0);
    }

    #[test]
    fn legacy_materialization_preserves_order_aliases_and_scopes() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let (signals, index) = opened.to_compatibility_parts().expect("compatibility");
        assert_eq!(signals[0].name, "top.a");
        assert_eq!(signals[0].scope, ["top"]);
        assert_eq!(index.by_id[&signals[0].id_code].len(), 2);
        assert_eq!(opened.to_owned_signals(), signals);
    }

    #[test]
    fn body_readers_have_independent_cursors_and_generation() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut first = opened.body_reader().expect("first reader");
        let mut second = opened.body_reader().expect("second reader");
        assert_eq!(first.generation(), opened.generation());
        assert_eq!(
            first.stream_position().expect("position"),
            opened.body_offset()
        );
        assert_eq!(
            second.stream_position().expect("position"),
            opened.body_offset()
        );

        let mut byte = [0_u8; 1];
        first.read_exact(&mut byte).expect("read first");
        assert_eq!(
            second.stream_position().expect("second unchanged"),
            opened.body_offset()
        );
        first.validate_complete().expect("first complete");
        assert_eq!(
            first.stream_position().expect("restored"),
            opened.body_offset() + 1
        );
    }

    #[test]
    fn concurrent_parsers_are_independent_and_equivalent() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let workers = (0..8)
            .map(|_| {
                let opened = opened.clone();
                thread::spawn(move || {
                    let mut parser = opened.body_parser().expect("parser");
                    let mut timestamps = Vec::new();
                    for command in &mut parser {
                        if let Command::Timestamp(time) = command.expect("command") {
                            timestamps.push(time);
                        }
                    }
                    parser.reader().validate_complete().expect("complete");
                    timestamps
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            assert_eq!(worker.join().expect("worker"), [5, 10, 10, 15, 20, 25]);
        }
    }

    #[test]
    fn open_does_not_parse_the_body() {
        let opened =
            OpenedVcd::open("tests/fixtures/malformed_body.vcd").expect("header-only open");
        assert_eq!(opened.signal_count(), 1);
        let mut parser = opened.body_parser().expect("body parser");
        assert!(parser.any(|command| command.is_err()));
    }

    #[cfg(unix)]
    #[test]
    fn configured_symlink_retarget_rejects_admission_and_completion() {
        let directory = tempdir().expect("temp directory");
        let target_a = directory.path().join("a.vcd");
        let target_b = directory.path().join("b.vcd");
        let configured_link = directory.path().join("configured.vcd");
        std::fs::copy(SEMANTICS, &target_a).expect("copy target A");
        std::fs::copy(SEMANTICS, &target_b).expect("copy target B");
        symlink(&target_a, &configured_link).expect("create configured symlink");

        let opened = OpenedVcd::open(&configured_link).expect("open symlink generation");
        assert_eq!(opened.configured_path(), configured_link);
        assert_eq!(
            opened.canonical_path(),
            target_a.canonicalize().expect("target A")
        );
        let mut admitted = opened.body_reader().expect("admit target A");

        let replacement_link = directory.path().join("replacement-link");
        symlink(&target_b, &replacement_link).expect("create replacement symlink");
        std::fs::rename(&replacement_link, &configured_link).expect("retarget symlink");

        assert!(opened.body_reader().is_err(), "retargeted symlink admitted");
        assert!(
            opened.validate_source().is_err(),
            "retargeted symlink validated"
        );
        assert!(
            admitted.validate_complete().is_err(),
            "completion ignored configured symlink retarget"
        );
    }

    #[test]
    fn replacement_append_and_truncate_are_rejected_before_admission() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");

        let replacement = path.with_extension("replacement");
        std::fs::copy(SEMANTICS, &replacement).expect("copy replacement");
        replace_file(&replacement, &path);
        assert!(opened.body_reader().is_err(), "atomic replacement accepted");

        let opened = OpenedVcd::open(&path).expect("reopen replacement");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append open")
            .write_all(b"\n#999\n")
            .expect("append");
        assert!(opened.body_reader().is_err(), "append accepted");

        let opened = OpenedVcd::open(&path).expect("reopen appended");
        let length = std::fs::metadata(&path).expect("metadata").len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("truncate open")
            .set_len(length - 4)
            .expect("truncate");
        assert!(opened.body_reader().is_err(), "truncate accepted");
    }

    #[test]
    fn sampled_in_place_mutation_is_rejected_before_admission() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("mutate open");
        file.seek(SeekFrom::Start(opened.body_offset()))
            .expect("seek");
        file.write_all(b"!").expect("mutate sample");
        file.flush().expect("flush");
        assert!(opened.body_reader().is_err(), "sampled mutation accepted");
    }

    #[test]
    fn completion_validation_rejects_mutation_after_admission() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");
        let mut reader = opened.body_reader().expect("reader");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append open")
            .write_all(b"\n#999\n")
            .expect("append");
        assert!(reader.validate_complete().is_err());
    }

    #[test]
    fn strict_policy_is_retained_by_opened_generation() {
        let options =
            OpenOptions::new().with_fingerprint_policy(FingerprintPolicy::StrictFullContent);
        let opened = OpenedVcd::open_with_options(SEMANTICS, options).expect("strict open");
        assert_eq!(
            opened.options().fingerprint_policy(),
            FingerprintPolicy::StrictFullContent
        );
        assert!(opened.identity().full_content_fingerprint().is_some());
        opened.validate_source().expect("strict validation");
    }

    #[test]
    #[ignore = "release-only diagnostic; run through scripts/measure-catalog-memory.sh"]
    fn probe_opened_list_peak_rss() {
        let path =
            std::env::var("VCD_CATALOG_PROBE").unwrap_or_else(|_| "tests/waveform.vcd".to_string());
        let opened = OpenedVcd::open(path).expect("opened VCD");
        let signal_count = opened.signal_count();
        let listed_bytes = opened.signal_names().map(str::len).sum::<usize>();
        let first = opened.signals().next().expect("first signal");
        let lookup = opened.signal(first.name()).expect("lookup");
        let list_started = Instant::now();
        let repeated_listed_bytes = (0..100)
            .map(|_| opened.signal_names().map(str::len).sum::<usize>())
            .sum::<usize>();
        let repeated_list_us = list_started.elapsed().as_micros();
        let lookup_started = Instant::now();
        let repeated_lookup_size = (0..100_000)
            .map(|_| opened.signal(first.name()).expect("repeated lookup").size() as usize)
            .sum::<usize>();
        let repeated_lookup_us = lookup_started.elapsed().as_micros();
        let estimate = opened.catalog_memory_usage();
        eprintln!(
            "mode=opened signals={signal_count} listed_bytes={listed_bytes} lookup={} estimated_catalog_bytes={} repeated_list_100_us={repeated_list_us} repeated_lookup_100000_us={repeated_lookup_us}",
            lookup.name(),
            estimate.total_bytes
        );
        std::hint::black_box((
            opened,
            listed_bytes,
            repeated_listed_bytes,
            repeated_lookup_size,
        ));
    }

    #[test]
    fn cancelled_context_does_not_start_metadata_work() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let context = QueryContext::new().with_cancellation(cancellation);
        let error = opened
            .metadata_with_context(&context)
            .expect_err("cancelled before metadata");
        assert_eq!(error.code(), QueryErrorCode::Cancelled);
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            0
        );
        assert!(!opened.has_cached_metadata());
    }

    #[test]
    fn cancelled_metadata_waiter_releases_registration_without_cancelling_builder() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("pause lock") = Some(pause);

        let builder = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata())
        };
        reached.wait();

        let cancellation = CancellationToken::new();
        let waiter = {
            let opened = opened.clone();
            let context = QueryContext::new().with_cancellation(cancellation.clone());
            thread::spawn(move || opened.metadata_with_context(&context))
        };
        wait_for_metadata_waiters(&opened, 1);
        cancellation.cancel();
        let error = waiter
            .join()
            .expect("waiter thread")
            .expect_err("waiter cancelled");
        assert_eq!(error.code(), QueryErrorCode::Cancelled);
        wait_for_metadata_waiters(&opened, 0);

        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("pause lock") = None;
        resume.wait();
        let metadata = builder.join().expect("builder thread").expect("metadata");
        assert_eq!((metadata.start_time, metadata.end_time), (0, 25));
        assert!(opened.has_cached_metadata());
    }

    #[test]
    fn controlled_deadline_metadata_waiter_releases_failed_attempt_acknowledgement() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        opened
            .inner
            .metadata
            .test_hooks
            .fail_once
            .store(true, Ordering::SeqCst);
        let (build_pause, build_reached, build_resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("build pause lock") = Some(build_pause);

        let builder = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata())
        };
        build_reached.wait();

        let context = QueryContext::new();
        let waiter = {
            let opened = opened.clone();
            let waiter_context = context.clone();
            thread::spawn(move || opened.metadata_with_context(&waiter_context))
        };
        wait_for_metadata_waiters(&opened, 1);
        context.expire_deadline_for_test();
        let error = waiter
            .join()
            .expect("waiter thread")
            .expect_err("deadline exceeded");
        assert_eq!(error.code(), QueryErrorCode::DeadlineExceeded);
        wait_for_metadata_waiters(&opened, 0);

        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("build pause lock") = None;
        build_resume.wait();
        assert!(builder.join().expect("builder thread").is_err());
        let metadata = opened.retry_metadata().expect("retry is not stranded");
        assert_eq!((metadata.start_time, metadata.end_time), (0, 25));
    }

    #[test]
    fn metadata_is_lazy_exact_and_ready_result_is_shared() {
        for (path, expected) in [
            (SEMANTICS, (0, 25)),
            ("tests/fixtures/empty_body.vcd", (0, 0)),
            ("tests/fixtures/dumpvars_no_timestamp.vcd", (0, 0)),
            ("tests/fixtures/decreasing_timestamps.vcd", (0, 12)),
        ] {
            let opened = OpenedVcd::open(path).expect("open");
            assert!(!opened.has_cached_metadata());
            assert_eq!(
                opened
                    .inner
                    .metadata
                    .test_hooks
                    .scans
                    .load(Ordering::SeqCst),
                0
            );
            let first = opened.metadata().expect("metadata");
            assert_eq!((first.start_time, first.end_time), expected);
            assert_eq!(first.signal_count, opened.signal_count());
            assert_eq!(first.timescale.as_ref(), opened.timescale());
            assert!(opened.has_cached_metadata());
            let second = opened.metadata().expect("cached metadata");
            assert!(Arc::ptr_eq(&first, &second));
            assert_eq!(
                opened
                    .inner
                    .metadata
                    .test_hooks
                    .scans
                    .load(Ordering::SeqCst),
                1
            );
        }
    }

    #[test]
    fn concurrent_metadata_callers_share_exactly_one_scan() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("pause lock") = Some(pause);
        let start = Arc::new(Barrier::new(17));
        let workers = (0..16)
            .map(|_| {
                let opened = opened.clone();
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    opened.metadata().expect("metadata")
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        reached.wait();
        resume.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker"))
            .collect::<Vec<_>>();
        assert!(
            results
                .iter()
                .all(|metadata| Arc::ptr_eq(metadata, &results[0]))
        );
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            1
        );
    }

    #[test]
    fn failed_metadata_is_shared_and_explicit_retry_rebuilds() {
        let opened = OpenedVcd::open("tests/fixtures/malformed_body.vcd").expect("open header");
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("pause lock") = Some(pause);
        let start = Arc::new(Barrier::new(9));
        let workers = (0..8)
            .map(|_| {
                let opened = opened.clone();
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    opened
                        .metadata()
                        .expect_err("malformed metadata")
                        .to_string()
                })
            })
            .collect::<Vec<_>>();
        start.wait();
        reached.wait();
        resume.wait();
        let errors = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker"))
            .collect::<Vec<_>>();
        assert!(errors.iter().all(|error| error == &errors[0]));
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("pause lock") = None;
        assert!(!opened.has_cached_metadata());
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            opened.metadata().expect_err("cached failure").to_string(),
            errors[0]
        );
        assert!(opened.retry_metadata().is_err());
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            2
        );
    }

    #[test]
    fn retry_cannot_supersede_failure_before_registered_waiters_observe_it() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        opened
            .inner
            .metadata
            .test_hooks
            .fail_once
            .store(true, Ordering::SeqCst);

        let (build_pause, build_reached, build_resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("build pause lock") = Some(build_pause);
        let (wake_pause, wake_reached, wake_resume) = metadata_pause_for(2);
        *opened
            .inner
            .metadata
            .test_hooks
            .after_wait
            .lock()
            .expect("wake pause lock") = Some(wake_pause);

        let builder = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata())
        };
        build_reached.wait();
        let waiters = (0..2)
            .map(|_| {
                let opened = opened.clone();
                thread::spawn(move || {
                    opened
                        .metadata()
                        .expect_err("attempt one must fail")
                        .to_string()
                })
            })
            .collect::<Vec<_>>();
        wait_for_metadata_waiters(&opened, 2);
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("build pause lock") = None;
        build_resume.wait();
        assert!(builder.join().expect("builder thread").is_err());

        // Both registered waiters have been notified but deliberately dropped
        // the state lock before consuming attempt one's failure.
        wake_reached.wait();
        *opened
            .inner
            .metadata
            .test_hooks
            .after_wait
            .lock()
            .expect("wake pause lock") = None;
        let retry_started = Arc::new(Barrier::new(2));
        let retry = {
            let opened = opened.clone();
            let retry_started = Arc::clone(&retry_started);
            thread::spawn(move || {
                retry_started.wait();
                opened.retry_metadata()
            })
        };
        retry_started.wait();
        wait_for_metadata_retry_waiters(&opened, 1);
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            1
        );

        wake_resume.wait();
        let errors = waiters
            .into_iter()
            .map(|waiter| waiter.join().expect("waiter"))
            .collect::<Vec<_>>();
        assert!(
            errors
                .iter()
                .all(|error| error.contains("injected metadata build failure"))
        );
        let metadata = retry.join().expect("retry thread").expect("retry succeeds");
        assert_eq!((metadata.start_time, metadata.end_time), (0, 25));
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            2
        );
    }

    #[test]
    fn panicking_builder_wakes_registered_waiters_before_retry_recovers() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        opened
            .inner
            .metadata
            .test_hooks
            .panic_once
            .store(true, Ordering::SeqCst);

        let (build_pause, build_reached, build_resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("build pause lock") = Some(build_pause);
        let (wake_pause, wake_reached, wake_resume) = metadata_pause_for(2);
        *opened
            .inner
            .metadata
            .test_hooks
            .after_wait
            .lock()
            .expect("wake pause lock") = Some(wake_pause);

        let builder = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata())
        };
        build_reached.wait();
        let waiters = (0..2)
            .map(|_| {
                let opened = opened.clone();
                thread::spawn(move || {
                    opened
                        .metadata()
                        .expect_err("panicked attempt must fail")
                        .to_string()
                })
            })
            .collect::<Vec<_>>();
        wait_for_metadata_waiters(&opened, 2);
        *opened
            .inner
            .metadata
            .test_hooks
            .pause
            .lock()
            .expect("build pause lock") = None;
        build_resume.wait();
        assert!(builder.join().is_err(), "injected builder did not panic");

        wake_reached.wait();
        *opened
            .inner
            .metadata
            .test_hooks
            .after_wait
            .lock()
            .expect("wake pause lock") = None;
        let retry_started = Arc::new(Barrier::new(2));
        let retry = {
            let opened = opened.clone();
            let retry_started = Arc::clone(&retry_started);
            thread::spawn(move || {
                retry_started.wait();
                opened.retry_metadata()
            })
        };
        retry_started.wait();
        wait_for_metadata_retry_waiters(&opened, 1);
        wake_resume.wait();

        let errors = waiters
            .into_iter()
            .map(|waiter| waiter.join().expect("waiter"))
            .collect::<Vec<_>>();
        assert!(
            errors
                .iter()
                .all(|error| error.contains("metadata builder panicked"))
        );
        let metadata = retry.join().expect("retry thread").expect("retry succeeds");
        assert_eq!((metadata.start_time, metadata.end_time), (0, 25));
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            2
        );
    }

    #[test]
    fn elected_builder_cancellation_during_scan_is_not_cached() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let cancellation = CancellationToken::new();
        let context = QueryContext::new().with_cancellation(cancellation.clone());
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = Some(pause);
        let worker = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata_with_context(&context))
        };
        reached.wait();
        cancellation.cancel();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = None;
        resume.wait();
        let error = worker
            .join()
            .expect("worker")
            .expect_err("elected caller must observe cancellation");
        assert_eq!(error.code(), QueryErrorCode::Cancelled);
        assert!(!opened.has_cached_metadata());
        let cached = opened
            .metadata()
            .expect("uncancelled retry builds metadata");
        assert_eq!((cached.start_time, cached.end_time), (0, 25));
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            2
        );
    }

    #[test]
    fn elected_builder_controlled_deadline_during_scan_is_not_cached() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let context = QueryContext::new();
        let worker_context = context.clone();
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = Some(pause);
        let worker = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata_with_context(&worker_context))
        };
        reached.wait();
        context.expire_deadline_for_test();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = None;
        resume.wait();
        let error = worker
            .join()
            .expect("worker")
            .expect_err("elected caller must observe deadline");
        assert_eq!(error.code(), QueryErrorCode::DeadlineExceeded);
        assert!(!opened.has_cached_metadata());
        let cached = opened.metadata().expect("unexpired retry builds metadata");
        assert_eq!((cached.start_time, cached.end_time), (0, 25));
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            2
        );
    }

    #[test]
    fn elected_builder_cancellation_after_failed_build_keeps_error_cached() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        opened
            .inner
            .metadata
            .test_hooks
            .fail_once
            .store(true, Ordering::SeqCst);
        let cancellation = CancellationToken::new();
        let context = QueryContext::new().with_cancellation(cancellation.clone());
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_publish
            .lock()
            .expect("publish pause lock") = Some(pause);
        let worker = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata_with_context(&context))
        };
        reached.wait();
        cancellation.cancel();
        resume.wait();
        let error = worker
            .join()
            .expect("worker")
            .expect_err("elected caller must observe cancellation");
        assert_eq!(error.code(), QueryErrorCode::Cancelled);
        let cached = opened.metadata().expect_err("failed build remains cached");
        assert!(
            cached
                .to_string()
                .contains("injected metadata build failure")
        );
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            1
        );
    }

    #[test]
    fn stale_metadata_attempt_preserves_typed_error_for_builder_waiters_and_cache() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = Some(pause);

        let builder = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata_with_context(&QueryContext::legacy_unlimited()))
        };
        reached.wait();

        let waiters = (0..2)
            .map(|_| {
                let opened = opened.clone();
                thread::spawn(move || {
                    opened.metadata_with_context(&QueryContext::legacy_unlimited())
                })
            })
            .collect::<Vec<_>>();
        wait_for_metadata_waiters(&opened, 2);

        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append open")
            .write_all(b"\n#999\n")
            .expect("append");
        resume.wait();

        let mut errors = vec![
            builder
                .join()
                .expect("builder")
                .expect_err("builder must reject stale source"),
        ];
        errors.extend(waiters.into_iter().map(|waiter| {
            waiter
                .join()
                .expect("waiter")
                .expect_err("waiter must receive stale source")
        }));
        errors.push(
            opened
                .metadata_with_context(&QueryContext::legacy_unlimited())
                .expect_err("cached metadata call must remain stale"),
        );

        let expected = "I/O error: VCD source does not match the opened generation";
        for error in &errors {
            assert_eq!(error.code(), QueryErrorCode::StaleSource);
            assert_eq!(error.to_string(), expected);
        }
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            1
        );
    }

    #[test]
    fn mutation_before_metadata_publication_is_not_cached() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = Some(pause);
        let worker = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata())
        };
        reached.wait();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append open")
            .write_all(b"\n#999\n")
            .expect("append");
        resume.wait();
        assert!(worker.join().expect("worker").is_err());
        assert!(!opened.has_cached_metadata());
    }

    #[test]
    fn replacement_before_metadata_publication_is_not_cached() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = Some(pause);
        let worker = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata())
        };
        reached.wait();
        let replacement = path.with_extension("metadata-replacement");
        std::fs::copy(SEMANTICS, &replacement).expect("copy replacement");
        replace_file(&replacement, &path);
        resume.wait();
        assert!(worker.join().expect("worker").is_err());
        assert!(!opened.has_cached_metadata());
    }

    #[test]
    fn waiter_re_elects_after_context_cancelled_metadata_builder() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let cancellation = CancellationToken::new();
        let builder_context = QueryContext::new().with_cancellation(cancellation.clone());
        let (pause, reached, resume) = metadata_pause();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = Some(pause);
        let builder = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata_with_context(&builder_context))
        };
        reached.wait();
        let waiter = {
            let opened = opened.clone();
            thread::spawn(move || opened.metadata_with_context(&QueryContext::legacy_unlimited()))
        };
        wait_for_metadata_waiters(&opened, 1);
        cancellation.cancel();
        *opened
            .inner
            .metadata
            .test_hooks
            .before_validation
            .lock()
            .expect("validation pause lock") = None;
        resume.wait();
        assert_eq!(
            builder.join().unwrap().unwrap_err().code(),
            QueryErrorCode::Cancelled
        );
        let metadata = waiter.join().unwrap().expect("waiter re-elects build");
        assert_eq!((metadata.start_time, metadata.end_time), (0, 25));
        assert_eq!(
            opened
                .inner
                .metadata
                .test_hooks
                .scans
                .load(Ordering::SeqCst),
            2
        );
    }

    #[test]
    fn body_parser_starts_at_body_without_reparsing_header() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut parser = opened.body_parser().expect("parser");
        assert!(matches!(
            parser.next().expect("command").expect("valid"),
            Command::Begin(_)
        ));
    }
}
