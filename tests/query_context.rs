use std::error::Error as _;
use std::io::{self, Write as _};
use std::time::{Duration, Instant};

use vcd_tools_rs::VcdError;
use vcd_tools_rs::opened::OpenedVcd;
use vcd_tools_rs::query::{
    CancellationToken, QueryContext, QueryError, QueryErrorCode, QueryLimitKind, QueryLimits,
};

const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";

#[test]
fn public_context_builders_and_unlimited_policy_are_discoverable() {
    let deadline = Instant::now() + Duration::from_secs(1);
    let limits = QueryLimits::unlimited()
        .with_max_signals(1)
        .with_max_rows(2)
        .with_max_result_bytes(3)
        .with_max_commands(4)
        .with_deadline(deadline);
    assert_eq!(limits.max_signals(), Some(1));
    assert_eq!(limits.max_rows(), Some(2));
    assert_eq!(limits.max_result_bytes(), Some(3));
    assert_eq!(limits.max_commands(), Some(4));
    assert_eq!(limits.deadline(), Some(deadline));

    let context = QueryContext::legacy_unlimited();
    assert_eq!(context.limits().max_signals(), None);
    assert!(!context.cancellation().is_cancelled());
}

#[test]
fn public_cancellation_propagates_and_prevents_metadata_work() {
    let opened = OpenedVcd::open(SEMANTICS).expect("open");
    let cancellation = CancellationToken::new();
    let clone = cancellation.clone();
    cancellation.cancel();
    assert!(clone.is_cancelled());

    let context = QueryContext::new().with_cancellation(clone);
    let error = opened
        .metadata_with_context(&context)
        .expect_err("cancelled metadata");
    assert_eq!(error.code(), QueryErrorCode::Cancelled);
    assert!(!opened.has_cached_metadata());
}

#[test]
fn public_deadline_and_limits_have_stable_categories() {
    let expired =
        QueryContext::new().with_limits(QueryLimits::unlimited().with_deadline(Instant::now()));
    assert_eq!(
        expired.check().expect_err("expired").code(),
        QueryErrorCode::DeadlineExceeded
    );

    let unlimited_timeout = QueryLimits::unlimited().with_timeout(Duration::MAX);
    assert_eq!(unlimited_timeout.deadline(), None);
    QueryContext::new()
        .with_limits(unlimited_timeout)
        .check()
        .expect("overflowing timeout remains effectively unlimited");

    let limited = QueryContext::new().with_limits(
        QueryLimits::unlimited()
            .with_max_signals(1)
            .with_max_rows(2)
            .with_max_result_bytes(3)
            .with_max_commands(4),
    );
    for (error, kind, limit, actual) in [
        (
            limited.check_signal_count(2).expect_err("signals"),
            QueryLimitKind::Signals,
            1,
            2,
        ),
        (
            limited.check_rows(3).expect_err("rows"),
            QueryLimitKind::Rows,
            2,
            3,
        ),
        (
            limited.check_result_bytes(4).expect_err("bytes"),
            QueryLimitKind::ResultBytes,
            3,
            4,
        ),
        (
            limited.check_commands(5).expect_err("commands"),
            QueryLimitKind::Commands,
            4,
            5,
        ),
    ] {
        assert_eq!(error.code(), QueryErrorCode::LimitExceeded);
        assert_eq!(error.limit_kind(), Some(kind));
        assert_eq!(error.limit(), Some(limit));
        assert_eq!(error.actual(), Some(actual));
    }
}

#[test]
fn public_error_mapping_preserves_sources_and_vcd_error_shape() {
    let query = QueryError::from(VcdError::MissingSignal("top.missing".to_string()));
    assert_eq!(query.code(), QueryErrorCode::Vcd);
    assert_eq!(query.to_string(), "signal not found in VCD: top.missing");
    assert!(query.source().is_some());

    let source = QueryError::source_unavailable(io::Error::new(io::ErrorKind::NotFound, "gone"));
    assert_eq!(source.code(), QueryErrorCode::SourceUnavailable);
    assert_eq!(source.to_string(), "source unavailable: gone");
    assert!(source.source().is_some());

    let similarly_worded = QueryError::from(VcdError::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        "VCD source does not match the opened generation",
    )));
    assert_eq!(similarly_worded.code(), QueryErrorCode::SourceUnavailable);

    let internal = QueryError::internal("invariant");
    assert_eq!(internal.code(), QueryErrorCode::Internal);
    assert_eq!(internal.to_string(), "internal query error: invariant");
}

#[test]
fn real_source_mutation_maps_to_stale_source_without_string_matching() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("wave.vcd");
    std::fs::copy(SEMANTICS, &path).expect("copy fixture");
    let opened = OpenedVcd::open(&path).expect("open");

    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append open")
        .write_all(b"\n#999\n")
        .expect("append");

    let error = opened
        .metadata_with_context(&QueryContext::legacy_unlimited())
        .expect_err("mutated source");
    assert_eq!(error.code(), QueryErrorCode::StaleSource);
    assert_eq!(
        error.to_string(),
        "I/O error: VCD source does not match the opened generation"
    );
}
