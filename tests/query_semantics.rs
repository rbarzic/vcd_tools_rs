use std::path::{Path, PathBuf};

use vcd_tools_rs::{
    ChangeValue, ComparisonOptions, TargetValue, TimeWindow, VcdError, compare_vcd_files,
    count_toggles, extract_time_values_from_file, find_nth_occurrence, list_signals_from_file,
    read_signals, read_signals_with_offset, read_vcd_metadata, time_value_iter_from_body,
};

const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";
const CHANGED: &str = "tests/fixtures/query_semantics_changed.vcd";
const EMPTY_BODY: &str = "tests/fixtures/empty_body.vcd";
const NO_TIMESTAMP: &str = "tests/fixtures/dumpvars_no_timestamp.vcd";
const DUPLICATE_NAMES: &str = "tests/fixtures/duplicate_names.vcd";
const MALFORMED_BODY: &str = "tests/fixtures/malformed_body.vcd";
const CRLF_HEADER: &str = "tests/fixtures/crlf_unusual_header.vcd";
const CONTROL_COMMANDS: &str = "tests/fixtures/control_commands.vcd";
const DECREASING_TIMESTAMPS: &str = "tests/fixtures/decreasing_timestamps.vcd";
const COMPARE_REFERENCE: &str = "tests/fixtures/compare_reference.vcd";
const COMPARE_ACTUAL: &str = "tests/fixtures/compare_actual.vcd";

fn names(events: &[vcd_tools_rs::TimeValue]) -> Vec<&str> {
    events.iter().map(|event| event.signal.as_str()).collect()
}

fn times(events: &[vcd_tools_rs::TimeValue]) -> Vec<u64> {
    events.iter().map(|event| event.time).collect()
}

fn window(start: u64, end: u64) -> TimeWindow {
    TimeWindow {
        start: Some(start),
        end: Some(end),
    }
}

#[test]
fn metadata_includes_dumpvars_at_zero_and_last_timestamp() {
    let metadata = read_vcd_metadata(SEMANTICS).expect("metadata");
    assert_eq!(metadata.signal_count, 7);
    assert_eq!(metadata.start_time, 0);
    assert_eq!(metadata.end_time, 25);
    assert_eq!(metadata.timescale.expect("timescale").magnitude, 1);
}

#[test]
fn metadata_for_empty_and_no_timestamp_bodies_is_zero_bounded() {
    for path in [EMPTY_BODY, NO_TIMESTAMP] {
        let metadata = read_vcd_metadata(path).expect("metadata");
        assert_eq!((metadata.start_time, metadata.end_time), (0, 0));
    }

    let events = extract_time_values_from_file(
        NO_TIMESTAMP,
        &["top.only".to_string()],
        TimeWindow::default(),
    )
    .expect("dumpvars event");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].time, 0);
    assert_eq!(events[0].value, ChangeValue::Text("1".to_string()));
}

#[test]
fn list_preserves_declaration_order_and_aliases() {
    assert_eq!(
        list_signals_from_file(SEMANTICS, None).expect("list"),
        vec![
            "top.a",
            "top.alias_a",
            "top.b",
            "top.vec4[3:0]",
            "top.wide[128:0]",
            "top.real_sig",
            "top.str_sig",
        ]
    );
    assert_eq!(
        list_signals_from_file(SEMANTICS, Some("vec")).expect("filtered list"),
        vec!["top.vec4[3:0]"]
    );
}

#[test]
fn alias_expansion_follows_requested_name_order() {
    let events = extract_time_values_from_file(
        SEMANTICS,
        &["top.alias_a".to_string(), "top.a".to_string()],
        window(5, 5),
    )
    .expect("alias extraction");
    assert_eq!(names(&events), vec!["top.alias_a", "top.a"]);
    assert_eq!(times(&events), vec![5, 5]);
    assert_eq!(events[0].value, ChangeValue::Text("1".to_string()));
    assert_eq!(events[1].value, ChangeValue::Text("1".to_string()));
}

#[test]
fn duplicate_requested_target_preserves_event_multiplicity() {
    let events = extract_time_values_from_file(
        SEMANTICS,
        &["top.a".to_string(), "top.a".to_string()],
        window(5, 5),
    )
    .expect("duplicate target extraction");
    assert_eq!(names(&events), vec!["top.a", "top.a"]);
    assert_eq!(times(&events), vec![5, 5]);
}

#[test]
fn extraction_preserves_body_order_and_repeated_timestamp_changes() {
    let targets = [
        "top.a",
        "top.b",
        "top.vec4[3:0]",
        "top.real_sig",
        "top.str_sig",
    ]
    .map(str::to_string);
    let events = extract_time_values_from_file(SEMANTICS, &targets, window(10, 10))
        .expect("same timestamp extraction");

    assert_eq!(
        names(&events),
        vec![
            "top.a",
            "top.b",
            "top.vec4[3:0]",
            "top.real_sig",
            "top.str_sig",
            "top.b",
        ]
    );
    assert!(events.iter().all(|event| event.time == 10));
    assert_eq!(events[0].value, ChangeValue::Text("0".to_string()));
    assert_eq!(events[1].value, ChangeValue::Text("x".to_string()));
    assert_eq!(events[2].value, ChangeValue::Text("10xz".to_string()));
    assert_eq!(events[3].value, ChangeValue::Float(1.5));
    assert_eq!(events[4].value, ChangeValue::Text("run".to_string()));
    assert_eq!(events[5].value, ChangeValue::Text("1".to_string()));
}

#[test]
fn extraction_windows_are_inclusive_and_do_not_seed_prior_values() {
    let events = extract_time_values_from_file(SEMANTICS, &["top.a".to_string()], window(10, 15))
        .expect("bounded extraction");
    assert_eq!(times(&events), vec![10, 15]);
    assert_eq!(
        events
            .iter()
            .map(|event| event.value.to_string())
            .collect::<Vec<_>>(),
        vec!["0", "1"]
    );

    let between_changes =
        extract_time_values_from_file(SEMANTICS, &["top.a".to_string()], window(11, 14))
            .expect("between changes");
    assert!(between_changes.is_empty());

    let inverted = extract_time_values_from_file(SEMANTICS, &["top.a".to_string()], window(15, 10))
        .expect("current inverted-window behavior");
    assert!(inverted.is_empty());
}

#[test]
fn vector_scalar_real_and_string_normalization_is_locked() {
    let known =
        extract_time_values_from_file(SEMANTICS, &["top.vec4[3:0]".to_string()], window(5, 5))
            .expect("known vector");
    assert_eq!(known[0].value, ChangeValue::Integer(10));

    let unknown =
        extract_time_values_from_file(SEMANTICS, &["top.vec4[3:0]".to_string()], window(10, 10))
            .expect("unknown vector");
    assert_eq!(unknown[0].value, ChangeValue::Text("10xz".to_string()));

    let wide =
        extract_time_values_from_file(SEMANTICS, &["top.wide[128:0]".to_string()], window(10, 10))
            .expect("wide vector");
    match &wide[0].value {
        ChangeValue::Text(value) => {
            assert_eq!(value.len(), 129);
            assert!(value.starts_with('1'));
        }
        value => panic!("wide vector unexpectedly normalized as {value:?}"),
    }

    let scalar = extract_time_values_from_file(SEMANTICS, &["top.b".to_string()], window(5, 5))
        .expect("scalar");
    assert_eq!(scalar[0].value, ChangeValue::Text("1".to_string()));

    let real =
        extract_time_values_from_file(SEMANTICS, &["top.real_sig".to_string()], window(10, 10))
            .expect("real");
    assert_eq!(real[0].value, ChangeValue::Float(1.5));

    let string =
        extract_time_values_from_file(SEMANTICS, &["top.str_sig".to_string()], window(10, 10))
            .expect("string");
    assert_eq!(string[0].value, ChangeValue::Text("run".to_string()));
}

#[test]
fn find_counts_value_change_commands_not_transitions() {
    let (event, size) = find_nth_occurrence(
        SEMANTICS,
        "top.a",
        TargetValue::Integer(1),
        2,
        TimeWindow::default(),
    )
    .expect("find second one");
    assert_eq!(size, 1);
    assert_eq!(event.expect("event").time, 15);

    assert!(matches!(
        find_nth_occurrence(
            SEMANTICS,
            "top.a",
            TargetValue::Integer(1),
            0,
            TimeWindow::default(),
        ),
        Err(VcdError::InvalidOccurrence)
    ));
}

#[test]
fn toggle_window_uses_first_in_window_event_as_baseline() {
    let full = count_toggles(SEMANTICS, &["top.a".to_string()], TimeWindow::default())
        .expect("full toggles");
    assert_eq!(full["top.a"], 5);

    let bounded =
        count_toggles(SEMANTICS, &["top.a".to_string()], window(5, 20)).expect("bounded toggles");
    assert_eq!(bounded["top.a"], 3);

    let one_event = count_toggles(SEMANTICS, &["top.a".to_string()], window(15, 15))
        .expect("single-event toggles");
    assert_eq!(one_event["top.a"], 0);
}

#[test]
fn missing_and_empty_target_behavior_is_deterministic() {
    let missing = extract_time_values_from_file(
        SEMANTICS,
        &["top.missing_b".to_string(), "top.missing_a".to_string()],
        TimeWindow::default(),
    )
    .expect_err("missing targets");
    assert_eq!(
        missing.to_string(),
        "signals not found in VCD: top.missing_b, top.missing_a"
    );

    let empty = extract_time_values_from_file(SEMANTICS, &[], TimeWindow::default())
        .expect("empty targets");
    assert!(empty.is_empty());
    let empty_toggles =
        count_toggles(SEMANTICS, &[], TimeWindow::default()).expect("empty toggle targets");
    assert!(empty_toggles.is_empty());
}

#[test]
fn compare_reports_difference_and_ignores_pre_window_state() {
    let options = ComparisonOptions {
        max_mismatches: None,
        signals_only: vec!["top.a".to_string()],
        ignore_unknown: false,
        time_window: TimeWindow::default(),
    };
    let result = compare_vcd_files(SEMANTICS, CHANGED, &options).expect("comparison");
    assert!(!result.passed);
    assert_eq!(result.total_mismatches, 1);
    assert_eq!(result.mismatches[0].signal_name, "top.a");
    assert_eq!(result.mismatches[0].time, 15);

    let window_without_changes = ComparisonOptions {
        time_window: window(16, 19),
        ..options
    };
    let result = compare_vcd_files(SEMANTICS, CHANGED, &window_without_changes)
        .expect("windowed comparison");
    assert!(
        result.passed,
        "pre-window state is not carried into comparison"
    );
}

#[test]
fn comments_and_dump_control_commands_do_not_hide_contained_changes() {
    let events = extract_time_values_from_file(
        CONTROL_COMMANDS,
        &["top.controlled".to_string()],
        TimeWindow::default(),
    )
    .expect("control-command extraction");
    assert_eq!(times(&events), vec![0, 5, 10, 15]);
    assert_eq!(
        events
            .iter()
            .map(|event| event.value.to_string())
            .collect::<Vec<_>>(),
        vec!["0", "x", "1", "z"]
    );
    let metadata = read_vcd_metadata(CONTROL_COMMANDS).expect("control-command metadata");
    assert_eq!((metadata.start_time, metadata.end_time), (0, 15));
}

#[test]
fn decreasing_timestamps_are_accepted_but_end_bound_stops_at_first_later_timestamp() {
    let full = extract_time_values_from_file(
        DECREASING_TIMESTAMPS,
        &["top.value".to_string()],
        TimeWindow::default(),
    )
    .expect("decreasing timestamp extraction");
    assert_eq!(times(&full), vec![0, 10, 5, 12]);

    let bounded = extract_time_values_from_file(
        DECREASING_TIMESTAMPS,
        &["top.value".to_string()],
        window(5, 7),
    )
    .expect("current early-stop behavior");
    assert!(
        bounded.is_empty(),
        "the parser stops at timestamp 10 and never reaches the later body command at timestamp 5"
    );

    let metadata = read_vcd_metadata(DECREASING_TIMESTAMPS).expect("metadata");
    assert_eq!((metadata.start_time, metadata.end_time), (0, 12));
}

#[test]
fn compare_orders_names_and_preserves_file_only_declaration_order() {
    let result = compare_vcd_files(
        COMPARE_REFERENCE,
        COMPARE_ACTUAL,
        &ComparisonOptions::default(),
    )
    .expect("comparison");
    assert_eq!(
        result.common_signals,
        vec![
            "top.a_common",
            "top.same_time",
            "top.unknown",
            "top.z_common"
        ]
    );
    assert_eq!(
        result.signals_only_in_file1,
        vec!["top.only_ref_first", "top.only_ref_second"]
    );
    assert_eq!(
        result.signals_only_in_file2,
        vec!["top.only_actual_second", "top.only_actual_first"]
    );
}

#[test]
fn compare_ignore_unknown_and_same_timestamp_final_value_behavior_is_locked() {
    let unknown = compare_vcd_files(
        COMPARE_REFERENCE,
        COMPARE_ACTUAL,
        &ComparisonOptions {
            signals_only: vec!["top.unknown".to_string()],
            ..ComparisonOptions::default()
        },
    )
    .expect("unknown comparison");
    assert_eq!(unknown.total_mismatches, 1);
    assert!(unknown.mismatches[0].is_unknown);

    let ignored = compare_vcd_files(
        COMPARE_REFERENCE,
        COMPARE_ACTUAL,
        &ComparisonOptions {
            signals_only: vec!["top.unknown".to_string()],
            ignore_unknown: true,
            ..ComparisonOptions::default()
        },
    )
    .expect("ignored unknown comparison");
    assert!(ignored.passed);

    let same_time = compare_vcd_files(
        COMPARE_REFERENCE,
        COMPARE_ACTUAL,
        &ComparisonOptions {
            signals_only: vec!["top.same_time".to_string()],
            ..ComparisonOptions::default()
        },
    )
    .expect("same-time comparison");
    assert!(
        same_time.passed,
        "only each file's final value at a timestamp is compared"
    );
}

#[test]
fn compare_max_mismatches_is_per_signal_and_non_common_filters_are_ignored() {
    let unlimited = compare_vcd_files(
        COMPARE_REFERENCE,
        COMPARE_ACTUAL,
        &ComparisonOptions {
            signals_only: vec!["top.a_common".to_string()],
            ..ComparisonOptions::default()
        },
    )
    .expect("unlimited comparison");
    assert_eq!(unlimited.total_mismatches, 2);
    assert_eq!(
        unlimited
            .mismatches
            .iter()
            .map(|mismatch| mismatch.time)
            .collect::<Vec<_>>(),
        vec![5, 10]
    );

    let limited = compare_vcd_files(
        COMPARE_REFERENCE,
        COMPARE_ACTUAL,
        &ComparisonOptions {
            max_mismatches: Some(1),
            signals_only: vec![
                "top.only_ref_first".to_string(),
                "top.missing".to_string(),
                "top.a_common".to_string(),
            ],
            ..ComparisonOptions::default()
        },
    )
    .expect("limited comparison");
    assert_eq!(limited.total_mismatches, 1);
    assert_eq!(limited.mismatches[0].signal_name, "top.a_common");
    assert_eq!(limited.mismatches[0].time, 5);
}

#[test]
fn duplicate_declared_full_names_are_rejected() {
    let error = read_signals(DUPLICATE_NAMES).expect_err("duplicate names");
    assert_eq!(
        error.to_string(),
        "duplicate signal name detected: top.duplicate"
    );
}

#[test]
fn malformed_body_yields_prior_events_then_parse_error_in_iterator() {
    let (_signals, index, _timescale, offset) =
        read_signals_with_offset(MALFORMED_BODY).expect("valid header");
    let (targets, missing) = vcd_tools_rs::build_target_map(&index, &["top.good".to_string()]);
    assert!(missing.is_empty());
    let mut iter =
        time_value_iter_from_body(MALFORMED_BODY, targets, TimeWindow::default(), offset)
            .expect("iterator");

    assert_eq!(iter.next().expect("first").expect("first valid").time, 0);
    assert_eq!(iter.next().expect("second").expect("second valid").time, 5);
    assert!(iter.next().expect("parse result").is_err());
}

#[test]
fn crlf_and_special_scope_header_behavior_is_locked() {
    let names = list_signals_from_file(CRLF_HEADER, None).expect("CRLF header");
    assert_eq!(names, vec!["\\top-scope.sig"]);
    let metadata = read_vcd_metadata(CRLF_HEADER).expect("CRLF metadata");
    assert_eq!(metadata.signal_count, 1);
    assert_eq!((metadata.start_time, metadata.end_time), (0, 0));
    assert_eq!(metadata.timescale.expect("timescale").magnitude, 10);
}

#[test]
fn fixture_paths_exist_from_repository_root() {
    for path in [
        SEMANTICS,
        CHANGED,
        EMPTY_BODY,
        NO_TIMESTAMP,
        DUPLICATE_NAMES,
        MALFORMED_BODY,
        CRLF_HEADER,
        CONTROL_COMMANDS,
        DECREASING_TIMESTAMPS,
        COMPARE_REFERENCE,
        COMPARE_ACTUAL,
    ] {
        assert!(Path::new(path).exists(), "missing fixture {path}");
    }
    let _type_check: PathBuf = PathBuf::from(SEMANTICS);
}
