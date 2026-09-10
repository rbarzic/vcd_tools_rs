pub(crate) mod stream;

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::VcdError;

/// Cheap-clone cooperative cancellation shared by transports and query work.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Resource whose configured query limit was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum QueryLimitKind {
    Signals,
    Rows,
    ResultBytes,
    Commands,
}

impl QueryLimitKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Signals => "signals",
            Self::Rows => "rows",
            Self::ResultBytes => "result_bytes",
            Self::Commands => "commands",
        }
    }
}

impl fmt::Display for QueryLimitKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// Stable category for mapping query failures into future transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum QueryErrorCode {
    Cancelled,
    DeadlineExceeded,
    LimitExceeded,
    StaleSource,
    QueueFull,
    SourceUnavailable,
    Vcd,
    Internal,
}

impl QueryErrorCode {
    pub fn code(self) -> &'static str {
        match self {
            Self::Cancelled => "CANCELLED",
            Self::DeadlineExceeded => "DEADLINE_EXCEEDED",
            Self::LimitExceeded => "LIMIT_EXCEEDED",
            Self::StaleSource => "STALE_SOURCE",
            Self::QueueFull => "QUEUE_FULL",
            Self::SourceUnavailable => "SOURCE_UNAVAILABLE",
            Self::Vcd => "VCD_ERROR",
            Self::Internal => "INTERNAL",
        }
    }
}

/// Transport-neutral query failure.
///
/// This type is additive and non-exhaustive. Existing APIs continue returning
/// [`VcdError`], avoiding new variants in that exhaustive public enum.
#[derive(Debug)]
#[non_exhaustive]
pub enum QueryError {
    Cancelled,
    DeadlineExceeded,
    LimitExceeded {
        kind: QueryLimitKind,
        limit: u64,
        actual: u64,
    },
    StaleSource(VcdError),
    QueueFull,
    SourceUnavailable(io::Error),
    Vcd(VcdError),
    Internal(String),
}

impl QueryError {
    pub fn code(&self) -> QueryErrorCode {
        match self {
            Self::Cancelled => QueryErrorCode::Cancelled,
            Self::DeadlineExceeded => QueryErrorCode::DeadlineExceeded,
            Self::LimitExceeded { .. } => QueryErrorCode::LimitExceeded,
            Self::StaleSource(_) => QueryErrorCode::StaleSource,
            Self::QueueFull => QueryErrorCode::QueueFull,
            Self::SourceUnavailable(_) => QueryErrorCode::SourceUnavailable,
            Self::Vcd(_) => QueryErrorCode::Vcd,
            Self::Internal(_) => QueryErrorCode::Internal,
        }
    }

    pub fn limit_kind(&self) -> Option<QueryLimitKind> {
        match self {
            Self::LimitExceeded { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    pub fn limit(&self) -> Option<u64> {
        match self {
            Self::LimitExceeded { limit, .. } => Some(*limit),
            _ => None,
        }
    }

    pub fn actual(&self) -> Option<u64> {
        match self {
            Self::LimitExceeded { actual, .. } => Some(*actual),
            _ => None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }

    pub fn source_unavailable(error: io::Error) -> Self {
        Self::SourceUnavailable(error)
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("query cancelled"),
            Self::DeadlineExceeded => formatter.write_str("query deadline exceeded"),
            Self::LimitExceeded {
                kind,
                limit,
                actual,
            } => write!(
                formatter,
                "query {kind} limit exceeded: limit {limit}, actual {actual}"
            ),
            Self::StaleSource(error) => error.fmt(formatter),
            Self::QueueFull => formatter.write_str("query queue is full"),
            Self::SourceUnavailable(error) => write!(formatter, "source unavailable: {error}"),
            Self::Vcd(error) => error.fmt(formatter),
            Self::Internal(message) => write!(formatter, "internal query error: {message}"),
        }
    }
}

impl StdError for QueryError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::StaleSource(error) | Self::Vcd(error) => Some(error),
            Self::SourceUnavailable(error) => Some(error),
            _ => None,
        }
    }
}

impl From<VcdError> for QueryError {
    fn from(error: VcdError) -> Self {
        match error {
            VcdError::Io(error) if crate::opened::is_generation_mismatch(&error) => {
                Self::StaleSource(VcdError::Io(error))
            }
            VcdError::Io(error) => Self::SourceUnavailable(error),
            error => Self::Vcd(error),
        }
    }
}

pub type QueryResult<T> = std::result::Result<T, QueryError>;

/// Optional finite budgets applied by reusable query APIs.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct QueryLimits {
    max_signals: Option<usize>,
    max_rows: Option<u64>,
    max_result_bytes: Option<u64>,
    max_commands: Option<u64>,
    deadline: Option<Instant>,
}

impl QueryLimits {
    pub fn unlimited() -> Self {
        Self::default()
    }

    pub fn max_signals(&self) -> Option<usize> {
        self.max_signals
    }

    pub fn max_rows(&self) -> Option<u64> {
        self.max_rows
    }

    pub fn max_result_bytes(&self) -> Option<u64> {
        self.max_result_bytes
    }

    pub fn max_commands(&self) -> Option<u64> {
        self.max_commands
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub fn with_max_signals(mut self, limit: usize) -> Self {
        self.max_signals = Some(limit);
        self
    }

    pub fn with_max_rows(mut self, limit: u64) -> Self {
        self.max_rows = Some(limit);
        self
    }

    pub fn with_max_result_bytes(mut self, limit: u64) -> Self {
        self.max_result_bytes = Some(limit);
        self
    }

    pub fn with_max_commands(mut self, limit: u64) -> Self {
        self.max_commands = Some(limit);
        self
    }

    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Set a relative deadline when it is representable by [`Instant`].
    ///
    /// An overflowing duration (including [`Duration::MAX`]) is treated as an
    /// effectively unlimited deadline rather than as already expired.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.deadline = Instant::now().checked_add(timeout);
        self
    }
}

/// Cancellation and resource policy shared by library, Python, CLI and server queries.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct QueryContext {
    cancellation: CancellationToken,
    limits: QueryLimits,
    #[cfg(test)]
    forced_deadline_expired: Arc<AtomicBool>,
}

impl QueryContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Unlimited, uncancelled policy used by compatibility wrappers.
    pub fn legacy_unlimited() -> Self {
        Self::default()
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn limits(&self) -> &QueryLimits {
        &self.limits
    }

    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    pub fn with_limits(mut self, limits: QueryLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn check(&self) -> QueryResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(QueryError::Cancelled);
        }
        #[cfg(test)]
        if self.forced_deadline_expired.load(Ordering::Acquire) {
            return Err(QueryError::DeadlineExceeded);
        }
        if self
            .limits
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(QueryError::DeadlineExceeded);
        }
        Ok(())
    }

    pub fn check_signal_count(&self, actual: usize) -> QueryResult<()> {
        self.check()?;
        if let Some(limit) = self.limits.max_signals.filter(|limit| actual > *limit) {
            return Err(QueryError::LimitExceeded {
                kind: QueryLimitKind::Signals,
                limit: u64::try_from(limit).unwrap_or(u64::MAX),
                actual: u64::try_from(actual).unwrap_or(u64::MAX),
            });
        }
        Ok(())
    }

    pub fn check_rows(&self, actual: u64) -> QueryResult<()> {
        self.check_u64_limit(QueryLimitKind::Rows, self.limits.max_rows, actual)
    }

    pub fn check_result_bytes(&self, actual: u64) -> QueryResult<()> {
        self.check_u64_limit(
            QueryLimitKind::ResultBytes,
            self.limits.max_result_bytes,
            actual,
        )
    }

    pub fn check_commands(&self, actual: u64) -> QueryResult<()> {
        self.check_u64_limit(QueryLimitKind::Commands, self.limits.max_commands, actual)
    }

    fn check_u64_limit(
        &self,
        kind: QueryLimitKind,
        limit: Option<u64>,
        actual: u64,
    ) -> QueryResult<()> {
        self.check()?;
        if let Some(limit) = limit.filter(|limit| actual > *limit) {
            return Err(QueryError::LimitExceeded {
                kind,
                limit,
                actual,
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn expire_deadline_for_test(&self) {
        self.forced_deadline_expired.store(true, Ordering::Release);
    }

    pub(crate) fn metadata_wait_timeout(&self) -> Duration {
        #[cfg(test)]
        if self.forced_deadline_expired.load(Ordering::Acquire) {
            return Duration::ZERO;
        }
        const POLL: Duration = Duration::from_millis(10);
        match self.limits.deadline {
            Some(deadline) => deadline.saturating_duration_since(Instant::now()).min(POLL),
            None => POLL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_propagates_across_clones() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn builders_and_accessors_cover_all_limits() {
        let deadline = Instant::now() + Duration::from_secs(1);
        let limits = QueryLimits::unlimited()
            .with_max_signals(2)
            .with_max_rows(3)
            .with_max_result_bytes(4)
            .with_max_commands(5)
            .with_deadline(deadline);
        assert_eq!(limits.max_signals(), Some(2));
        assert_eq!(limits.max_rows(), Some(3));
        assert_eq!(limits.max_result_bytes(), Some(4));
        assert_eq!(limits.max_commands(), Some(5));
        assert_eq!(limits.deadline(), Some(deadline));
    }

    #[test]
    fn context_enforces_each_limit() {
        let context = QueryContext::new().with_limits(
            QueryLimits::unlimited()
                .with_max_signals(1)
                .with_max_rows(2)
                .with_max_result_bytes(3)
                .with_max_commands(4),
        );
        for (error, kind) in [
            (
                context.check_signal_count(2).unwrap_err(),
                QueryLimitKind::Signals,
            ),
            (context.check_rows(3).unwrap_err(), QueryLimitKind::Rows),
            (
                context.check_result_bytes(4).unwrap_err(),
                QueryLimitKind::ResultBytes,
            ),
            (
                context.check_commands(5).unwrap_err(),
                QueryLimitKind::Commands,
            ),
        ] {
            assert_eq!(error.code(), QueryErrorCode::LimitExceeded);
            assert_eq!(error.limit_kind(), Some(kind));
        }
    }

    #[test]
    fn cancellation_precedes_deadline() {
        let token = CancellationToken::new();
        token.cancel();
        let context = QueryContext::new()
            .with_cancellation(token)
            .with_limits(QueryLimits::unlimited().with_deadline(Instant::now()));
        assert_eq!(
            context.check().unwrap_err().code(),
            QueryErrorCode::Cancelled
        );
    }

    #[test]
    fn deadline_is_enforced() {
        let context =
            QueryContext::new().with_limits(QueryLimits::unlimited().with_deadline(Instant::now()));
        assert_eq!(
            context.check().unwrap_err().code(),
            QueryErrorCode::DeadlineExceeded
        );
    }

    #[test]
    fn overflowing_timeout_remains_unlimited() {
        let limits = QueryLimits::unlimited().with_timeout(Duration::MAX);
        assert_eq!(limits.deadline(), None);
        QueryContext::new()
            .with_limits(limits)
            .check()
            .expect("overflowing timeout must not be immediately expired");
    }

    #[test]
    fn errors_have_stable_codes_display_and_sources() {
        let vcd = QueryError::from(VcdError::MissingSignal("top.missing".to_string()));
        assert_eq!(vcd.code().code(), "VCD_ERROR");
        assert!(vcd.to_string().contains("top.missing"));
        assert!(StdError::source(&vcd).is_some());

        let source =
            QueryError::source_unavailable(io::Error::new(io::ErrorKind::NotFound, "gone"));
        assert_eq!(source.code().code(), "SOURCE_UNAVAILABLE");
        assert!(StdError::source(&source).is_some());
    }
}
