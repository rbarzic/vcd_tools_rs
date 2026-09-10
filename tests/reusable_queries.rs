use std::io::Write as _;

use vcd_tools_rs::opened::OpenedVcd;
use vcd_tools_rs::query::{
    CancellationToken, QueryContext, QueryErrorCode, QueryLimitKind, QueryLimits,
    logical_event_bytes,
};
use vcd_tools_rs::{
    ChangeValue, ComparisonOptions, TargetValue, TimeWindow, compare_vcd_files, count_toggles,
    extract_time_values_from_file, find_nth_occurrence,
};

const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";
const CHANGED: &str = "tests/fixtures/query_semantics_changed.vcd";

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[test]
fn reusable_extract_preserves_alias_duplicate_and_body_order() {
    let opened = OpenedVcd::open(SEMANTICS).expect("open");
    let targets = names(&["top.alias_a", "top.a", "top.a", "top.b"]);
    let window = TimeWindow {
        start: Some(5),
        end: Some(10),
    };
    let events = opened
        .extract(&targets, window, &QueryContext::legacy_unlimited())
        .expect("extract")
        .collect::<Result<Vec<_>, _>>()
        .expect("events");
    let observed = events
        .iter()
        .map(|event| (event.signal.as_str(), event.time, event.value.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(
        observed,
        vec![
            ("top.alias_a", 5, "1".to_string()),
            ("top.a", 5, "1".to_string()),
            ("top.a", 5, "1".to_string()),
            ("top.b", 5, "1".to_string()),
            ("top.alias_a", 10, "0".to_string()),
            ("top.a", 10, "0".to_string()),
            ("top.a", 10, "0".to_string()),
            ("top.b", 10, "x".to_string()),
            ("top.b", 10, "1".to_string()),
        ]
    );
}

#[test]
fn extraction_limits_use_documented_logical_bytes_before_delivery() {
    let opened = OpenedVcd::open(SEMANTICS).expect("open");
    let targets = names(&["top.a"]);
    let event_bytes = logical_event_bytes("top.a", &ChangeValue::Text("0".to_string()));
    assert_eq!(event_bytes, 14);

    let rows = QueryContext::new().with_limits(QueryLimits::unlimited().with_max_rows(1));
    let mut iter = opened
        .extract(&targets, TimeWindow::default(), &rows)
        .expect("iter");
    assert!(iter.next().expect("first").is_ok());
    let error = iter.next().expect("limit").expect_err("row limit");
    assert_eq!(error.limit_kind(), Some(QueryLimitKind::Rows));
    assert_eq!(iter.rows_emitted(), 1);

    let bytes = QueryContext::new()
        .with_limits(QueryLimits::unlimited().with_max_result_bytes(event_bytes));
    let mut iter = opened
        .extract(&targets, TimeWindow::default(), &bytes)
        .expect("iter");
    assert!(iter.next().expect("first").is_ok());
    let error = iter.next().expect("limit").expect_err("byte limit");
    assert_eq!(error.limit_kind(), Some(QueryLimitKind::ResultBytes));
    assert_eq!(iter.logical_bytes_emitted(), event_bytes);
}

#[test]
fn cancellation_before_delivery_wins_without_emitting_a_row() {
    let opened = OpenedVcd::open(SEMANTICS).expect("open");
    let token = CancellationToken::new();
    let context = QueryContext::new().with_cancellation(token.clone());
    let mut iter = opened
        .extract(&names(&["top.a"]), TimeWindow::default(), &context)
        .expect("iter");
    token.cancel();
    assert_eq!(
        iter.next().expect("error").expect_err("cancelled").code(),
        QueryErrorCode::Cancelled
    );
    assert_eq!(iter.rows_emitted(), 0);
}

#[test]
fn successful_exhaustion_rejects_mutation_but_early_drop_does_not_publish() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("wave.vcd");
    std::fs::copy(SEMANTICS, &path).expect("copy");
    let opened = OpenedVcd::open(&path).expect("open");
    let targets = names(&["top.a"]);
    let mut iter = opened
        .extract(
            &targets,
            TimeWindow::default(),
            &QueryContext::legacy_unlimited(),
        )
        .expect("iter");
    assert!(iter.next().expect("first").is_ok());
    drop(iter); // Early drop intentionally has no successful-completion claim.

    let mut iter = opened
        .extract(
            &targets,
            TimeWindow::default(),
            &QueryContext::legacy_unlimited(),
        )
        .expect("second iter");
    assert!(iter.next().expect("first").is_ok());
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append")
        .write_all(b"\n#999\n")
        .expect("write");
    let error = iter.find_map(Result::err).expect("stale terminal error");
    assert_eq!(error.code(), QueryErrorCode::StaleSource);
}

#[test]
fn reusable_find_toggle_and_legacy_wrappers_match_contract() {
    let opened = OpenedVcd::open(SEMANTICS).expect("open");
    let context = QueryContext::legacy_unlimited();
    let reusable = opened
        .find_nth_occurrence(
            "top.a",
            TargetValue::Integer(1),
            2,
            TimeWindow::default(),
            &context,
        )
        .expect("find");
    let wrapper = find_nth_occurrence(
        SEMANTICS,
        "top.a",
        TargetValue::Integer(1),
        2,
        TimeWindow::default(),
    )
    .expect("wrapper");
    assert_eq!(reusable, wrapper);
    assert_eq!(reusable.0.expect("event").time, 15);

    let targets = names(&["top.alias_a", "top.a"]);
    let window = TimeWindow {
        start: Some(5),
        end: Some(20),
    };
    let reusable = opened
        .count_toggles(&targets, window, &context)
        .expect("toggles");
    let wrapper = count_toggles(SEMANTICS, &targets, window).expect("wrapper toggles");
    assert_eq!(reusable, wrapper);
    assert_eq!(reusable["top.a"], 3);

    let extracted =
        extract_time_values_from_file(SEMANTICS, &targets, window).expect("wrapper extract");
    assert!(!extracted.is_empty());
}

#[test]
fn find_and_toggle_preparation_preserve_error_precedence() {
    let opened = OpenedVcd::open(SEMANTICS).expect("open");
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let cancelled_context = QueryContext::new().with_cancellation(cancelled);

    let occurrence = opened
        .find_nth_occurrence(
            "top.missing",
            TargetValue::Integer(1),
            0,
            TimeWindow::default(),
            &cancelled_context,
        )
        .expect_err("occurrence zero wins");
    assert_eq!(occurrence.code(), QueryErrorCode::Vcd);
    assert_eq!(occurrence.to_string(), "occurrence must be >= 1");

    let cancelled_find = opened
        .find_nth_occurrence(
            "top.missing",
            TargetValue::Integer(1),
            1,
            TimeWindow::default(),
            &cancelled_context,
        )
        .expect_err("cancellation precedes lookup");
    assert_eq!(cancelled_find.code(), QueryErrorCode::Cancelled);

    let cancelled_toggle = opened
        .count_toggles(
            &names(&["top.missing"]),
            TimeWindow::default(),
            &cancelled_context,
        )
        .expect_err("cancellation precedes signal limit and preparation");
    assert_eq!(cancelled_toggle.code(), QueryErrorCode::Cancelled);

    let limited = QueryContext::new().with_limits(QueryLimits::unlimited().with_max_signals(0));
    let limited_toggle = opened
        .count_toggles(&names(&["top.missing"]), TimeWindow::default(), &limited)
        .expect_err("signal limit precedes lookup");
    assert_eq!(limited_toggle.limit_kind(), Some(QueryLimitKind::Signals));
}

#[test]
fn opened_compare_and_wrapper_preserve_results_and_limits() {
    let first = OpenedVcd::open(SEMANTICS).expect("first");
    let second = OpenedVcd::open(CHANGED).expect("second");
    let options = ComparisonOptions {
        signals_only: vec!["top.a".to_string()],
        ..ComparisonOptions::default()
    };
    let reusable = first
        .compare(&second, &options, &QueryContext::legacy_unlimited())
        .expect("compare");
    let wrapper = compare_vcd_files(SEMANTICS, CHANGED, &options).expect("wrapper");
    assert_eq!(reusable, wrapper);
    assert_eq!(reusable.total_mismatches, 1);

    let limited = QueryContext::new().with_limits(QueryLimits::unlimited().with_max_signals(0));
    let error = first
        .compare(&second, &options, &limited)
        .expect_err("signal limit");
    assert_eq!(error.limit_kind(), Some(QueryLimitKind::Signals));
}
