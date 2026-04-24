use std::path::PathBuf;

const VCD_PATH: &str = "tests/waveform.vcd";

fn vcd_path() -> PathBuf {
    PathBuf::from(VCD_PATH)
}

#[test]
fn meta_returns_signal_count() {
    let meta = vcd_tools_rs::read_vcd_metadata(vcd_path()).expect("read metadata");
    assert!(meta.signal_count > 100_000);
    assert_eq!(meta.start_time, 0);
    assert!(meta.end_time > 0);
}

#[test]
fn meta_returns_timescale() {
    let meta = vcd_tools_rs::read_vcd_metadata(vcd_path()).expect("read metadata");
    assert!(meta.timescale.is_some());
    let ts = meta.timescale.unwrap();
    assert_eq!(ts.magnitude, 1);
}

#[test]
fn list_signals_returns_all_signals() {
    let (signals, _index, _ts) = vcd_tools_rs::read_signals(vcd_path()).expect("read signals");
    assert!(signals.len() > 100_000);
}

#[test]
fn list_signals_filter_returns_subset() {
    let (signals, _index, _ts) = vcd_tools_rs::read_signals(vcd_path()).expect("read signals");
    let filtered = vcd_tools_rs::list_signals(&signals, Some("TRSTN"));
    assert!(!filtered.is_empty());
    for name in &filtered {
        assert!(name.contains("TRSTN"));
    }
}

#[test]
fn list_signals_filter_no_match() {
    let (signals, _index, _ts) = vcd_tools_rs::read_signals(vcd_path()).expect("read signals");
    let filtered = vcd_tools_rs::list_signals(&signals, Some("zzzz_not_a_signal"));
    assert!(filtered.is_empty());
}

#[test]
fn signal_index_lookup_by_name() {
    let (_signals, index, _ts) = vcd_tools_rs::read_signals(vcd_path()).expect("read signals");
    let trstn = "chip_wrapper.TRSTN";
    let found = index.by_name.get(trstn);
    assert!(found.is_some(), "expected to find signal: {}", trstn);
    assert_eq!(found.unwrap().name, trstn);
}

#[test]
fn signal_index_lookup_missing() {
    let (_signals, index, _ts) = vcd_tools_rs::read_signals(vcd_path()).expect("read signals");
    let missing = index.by_name.get("nonexistent_signal");
    assert!(missing.is_none());
}

#[test]
fn build_target_map_finds_signals() {
    let (_signals, index, _ts) = vcd_tools_rs::read_signals(vcd_path()).expect("read signals");
    let targets = vec!["chip_wrapper.TRSTN".to_string()];
    let (map, missing) = vcd_tools_rs::build_target_map(&index, &targets);
    assert!(missing.is_empty());
    assert!(!map.is_empty());
}

#[test]
fn build_target_map_reports_missing() {
    let (_signals, index, _ts) = vcd_tools_rs::read_signals(vcd_path()).expect("read signals");
    let targets = vec!["nonexistent".to_string()];
    let (_map, missing) = vcd_tools_rs::build_target_map(&index, &targets);
    assert_eq!(missing, vec!["nonexistent"]);
}

#[test]
fn extract_time_values_returns_results() {
    let targets = vec!["chip_wrapper.TRSTN".to_string()];
    let window = vcd_tools_rs::TimeWindow {
        start: Some(0),
        end: Some(100),
    };
    let values = vcd_tools_rs::extract_time_values_from_file(vcd_path(), &targets, window)
        .expect("extract values");
    assert!(!values.is_empty());
    let first = &values[0];
    assert_eq!(first.time, 0);
    assert_eq!(first.signal, "chip_wrapper.TRSTN");
}

#[test]
fn extract_time_values_empty_window() {
    let targets = vec!["chip_wrapper.TRSTN".to_string()];
    let window = vcd_tools_rs::TimeWindow {
        start: Some(999999999999),
        end: Some(1000000000000),
    };
    let values = vcd_tools_rs::extract_time_values_from_file(vcd_path(), &targets, window)
        .expect("extract values");
    assert!(values.is_empty());
}

#[test]
fn extract_time_values_multiple_signals() {
    let targets = vec![
        "chip_wrapper.TRSTN".to_string(),
        "chip_wrapper.U_CHIP.TRSTN".to_string(),
    ];
    let window = vcd_tools_rs::TimeWindow {
        start: Some(0),
        end: Some(100),
    };
    let values = vcd_tools_rs::extract_time_values_from_file(vcd_path(), &targets, window)
        .expect("extract values");
    assert!(!values.is_empty());
}

#[test]
fn extract_time_values_missing_signal() {
    let targets = vec!["nonexistent_signal".to_string()];
    let window = vcd_tools_rs::TimeWindow {
        start: Some(0),
        end: Some(100),
    };
    let result = vcd_tools_rs::extract_time_values_from_file(vcd_path(), &targets, window);
    assert!(result.is_err());
}

#[test]
fn find_nth_occurrence_finds_value() {
    let (event, _size) = vcd_tools_rs::find_nth_occurrence(
        vcd_path(),
        "chip_wrapper.TRSTN",
        vcd_tools_rs::TargetValue::Integer(1),
        1,
        vcd_tools_rs::TimeWindow { start: None, end: None },
    )
    .expect("find occurrence");
    assert!(event.is_some());
    let evt = event.unwrap();
    assert_eq!(evt.value.normalize(), "1");
}

#[test]
fn find_nth_occurrence_not_found() {
    let (event, _size) = vcd_tools_rs::find_nth_occurrence(
        vcd_path(),
        "chip_wrapper.TRSTN",
        vcd_tools_rs::TargetValue::Integer(999),
        1,
        vcd_tools_rs::TimeWindow { start: None, end: None },
    )
    .expect("find occurrence");
    assert!(event.is_none());
}

#[test]
fn find_nth_occurrence_window() {
    let (event, _size) = vcd_tools_rs::find_nth_occurrence(
        vcd_path(),
        "chip_wrapper.TRSTN",
        vcd_tools_rs::TargetValue::Integer(1),
        1,
        vcd_tools_rs::TimeWindow {
            start: Some(0),
            end: Some(50),
        },
    )
    .expect("find occurrence");
    // In a small window there may be no transition to 1
    let _ = event;
}

#[test]
fn find_nth_occurrence_invalid() {
    let result = vcd_tools_rs::find_nth_occurrence(
        vcd_path(),
        "chip_wrapper.TRSTN",
        vcd_tools_rs::TargetValue::Integer(1),
        0,
        vcd_tools_rs::TimeWindow { start: None, end: None },
    );
    assert!(result.is_err());
}

#[test]
fn parse_target_value_integer() {
    let val = vcd_tools_rs::parse_target_value("42");
    assert_eq!(val, vcd_tools_rs::TargetValue::Integer(42));
}

#[test]
fn parse_target_value_hex() {
    let val = vcd_tools_rs::parse_target_value("0xff");
    assert_eq!(val, vcd_tools_rs::TargetValue::Integer(255));
}

#[test]
fn parse_target_value_text() {
    let val = vcd_tools_rs::parse_target_value("z");
    assert_eq!(val, vcd_tools_rs::TargetValue::Text("z".to_string()));
}

#[test]
fn parse_target_value_hex_uppercase() {
    let val = vcd_tools_rs::parse_target_value("0xFF");
    assert_eq!(val, vcd_tools_rs::TargetValue::Integer(255));
}

#[test]
fn load_signal_list_reads_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        tmp.path(),
        "signal_one\nsignal_two\n\nsignal_three\n",
    )
    .unwrap();
    let names = vcd_tools_rs::load_signal_list(tmp.path()).unwrap();
    assert_eq!(names, vec!["signal_one", "signal_two", "signal_three"]);
}

#[test]
fn load_signal_list_empty_file() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "").unwrap();
    let names = vcd_tools_rs::load_signal_list(tmp.path()).unwrap();
    assert!(names.is_empty());
}

#[test]
fn load_signal_list_whitespace_only() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "  \n  \n").unwrap();
    let names = vcd_tools_rs::load_signal_list(tmp.path()).unwrap();
    assert!(names.is_empty());
}

#[test]
fn load_signal_list_comments() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        tmp.path(),
        "# full line comment\nsignal_one\n\n   \nsignal_two # inline comment\n# another comment\nsignal_three\n",
    )
    .unwrap();
    let names = vcd_tools_rs::load_signal_list(tmp.path()).unwrap();
    assert_eq!(names, vec!["signal_one", "signal_two", "signal_three"]);
}

#[test]
fn load_signal_list_only_comments() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        tmp.path(),
        "# comment 1\n# comment 2\n",
    )
    .unwrap();
    let names = vcd_tools_rs::load_signal_list(tmp.path()).unwrap();
    assert!(names.is_empty());
}

#[test]
fn load_signal_list_inline_comment_only_line() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(tmp.path(), "signal # comment\n").unwrap();
    let names = vcd_tools_rs::load_signal_list(tmp.path()).unwrap();
    assert_eq!(names, vec!["signal"]);
}

#[test]
fn time_window_contains() {
    let w = vcd_tools_rs::TimeWindow {
        start: Some(10),
        end: Some(100),
    };
    assert!(w.contains(10));
    assert!(w.contains(50));
    assert!(w.contains(100));
    assert!(!w.contains(9));
    assert!(!w.contains(101));
}

#[test]
fn time_window_no_bounds() {
    let w = vcd_tools_rs::TimeWindow {
        start: None,
        end: None,
    };
    assert!(w.contains(0));
    assert!(w.contains(1000000));
}

#[test]
fn time_window_only_start() {
    let w = vcd_tools_rs::TimeWindow {
        start: Some(50),
        end: None,
    };
    assert!(w.contains(50));
    assert!(w.contains(1000));
    assert!(!w.contains(49));
}

#[test]
fn time_window_only_end() {
    let w = vcd_tools_rs::TimeWindow {
        start: None,
        end: Some(50),
    };
    assert!(w.contains(0));
    assert!(w.contains(50));
    assert!(!w.contains(51));
}

#[test]
fn format_value_for_signal_scalar() {
    let val = vcd_tools_rs::ChangeValue::Integer(1);
    let formatted = vcd_tools_rs::format_value_for_signal(&val, 1);
    assert_eq!(formatted, "1");
}

#[test]
fn format_value_for_signal_wide() {
    let val = vcd_tools_rs::ChangeValue::Integer(0xff);
    let formatted = vcd_tools_rs::format_value_for_signal(&val, 32);
    assert!(formatted.starts_with("0x"));
    assert_eq!(formatted.len(), 10); // "0x" + 8 hex digits for 32-bit
}

#[test]
fn format_value_for_signal_text() {
    let val = vcd_tools_rs::ChangeValue::Text("z".to_string());
    let formatted = vcd_tools_rs::format_value_for_signal(&val, 1);
    assert_eq!(formatted, "z");
}

#[test]
fn format_change_value_integer() {
    let val = vcd_tools_rs::ChangeValue::Integer(42);
    assert_eq!(vcd_tools_rs::format_change_value(&val), "42");
}

#[test]
fn format_change_value_text() {
    let val = vcd_tools_rs::ChangeValue::Text("x".to_string());
    assert_eq!(vcd_tools_rs::format_change_value(&val), "x");
}

#[test]
fn compare_vcd_files_self_matches() {
    let result = vcd_tools_rs::compare_vcd_files(
        VCD_PATH,
        VCD_PATH,
        &vcd_tools_rs::ComparisonOptions {
            max_mismatches: None,
            signals_only: vec!["chip_wrapper.TRSTN".to_string()],
            ignore_unknown: false,
            time_window: vcd_tools_rs::TimeWindow {
                start: Some(0),
                end: Some(1000),
            },
        },
    )
    .expect("compare files");
    assert!(result.passed);
    assert_eq!(result.total_mismatches, 0);
}

#[test]
fn compare_vcd_files_signals_only() {
    let result = vcd_tools_rs::compare_vcd_files(
        VCD_PATH,
        VCD_PATH,
        &vcd_tools_rs::ComparisonOptions {
            max_mismatches: None,
            signals_only: vec!["chip_wrapper.TRSTN".to_string()],
            ignore_unknown: false,
            time_window: vcd_tools_rs::TimeWindow {
                start: Some(0),
                end: Some(1000),
            },
        },
    )
    .expect("compare files");
    assert!(result.common_signals.contains(&"chip_wrapper.TRSTN".to_string()));
}

#[test]
fn compare_vcd_files_get_summary() {
    let result = vcd_tools_rs::compare_vcd_files(
        VCD_PATH,
        VCD_PATH,
        &vcd_tools_rs::ComparisonOptions::default(),
    )
    .expect("compare files");
    let summary = result.get_summary();
    assert!(summary.contains("PASS"));
}

#[test]
fn list_signals_from_file() {
    let names = vcd_tools_rs::list_signals_from_file(vcd_path(), Some("TRSTN"))
        .expect("list signals");
    assert!(!names.is_empty());
    for name in &names {
        assert!(name.contains("TRSTN"));
    }
}

#[test]
fn list_signals_from_file_no_filter() {
    let names = vcd_tools_rs::list_signals_from_file(vcd_path(), None)
        .expect("list signals");
    assert!(names.len() > 100_000);
}

#[test]
fn build_sizes_returns_size_map() {
    let (signals, _index, _ts) = vcd_tools_rs::read_signals(vcd_path()).expect("read signals");
    let sizes = vcd_tools_rs::build_sizes(&signals);
    assert!(!sizes.is_empty());
    let trstn_size = sizes.get("chip_wrapper.TRSTN");
    assert!(trstn_size.is_some());
    assert_eq!(*trstn_size.unwrap(), 1);
}

#[test]
fn change_value_normalize_integer() {
    let val = vcd_tools_rs::ChangeValue::Integer(42);
    assert_eq!(val.normalize(), "42");
}

#[test]
fn change_value_normalize_text() {
    let val = vcd_tools_rs::ChangeValue::Text("Z".to_string());
    assert_eq!(val.normalize(), "z");
}

#[test]
fn change_value_as_integer_some() {
    let val = vcd_tools_rs::ChangeValue::Integer(42);
    assert_eq!(val.as_integer(), Some(42));
}

#[test]
fn change_value_as_integer_none() {
    let val = vcd_tools_rs::ChangeValue::Text("x".to_string());
    assert_eq!(val.as_integer(), None);
}

#[test]
fn signal_index_duplicate_error() {
    let signals = vec![
        vcd_tools_rs::Signal {
            name: "dup".to_string(),
            id_code: vcd::IdCode::from(1u32),
            size: 1,
            type_: vcd::VarType::Wire,
            scope: vec![],
        },
        vcd_tools_rs::Signal {
            name: "dup".to_string(),
            id_code: vcd::IdCode::from(2u32),
            size: 1,
            type_: vcd::VarType::Wire,
            scope: vec![],
        },
    ];
    let result = vcd_tools_rs::SignalIndex::build(&signals);
    assert!(result.is_err());
}

#[test]
fn normalize_change_value() {
    let val = vcd_tools_rs::ChangeValue::Text("ZzZ".to_string());
    assert_eq!(vcd_tools_rs::normalize_change_value(&val), "zzz");
}

#[test]
fn compare_result_passed_summary() {
    let result = vcd_tools_rs::ComparisonResult {
        file1: "a.vcd".to_string(),
        file2: "b.vcd".to_string(),
        common_signals: vec!["sig1".to_string()],
        signals_only_in_file1: vec![],
        signals_only_in_file2: vec![],
        mismatches: vec![],
        total_mismatches: 0,
        signals_with_mismatches: 0,
        passed: true,
    };
    assert!(result.get_summary().contains("PASS"));
}

#[test]
fn compare_result_failed_summary() {
    let result = vcd_tools_rs::ComparisonResult {
        file1: "a.vcd".to_string(),
        file2: "b.vcd".to_string(),
        common_signals: vec!["sig1".to_string(), "sig2".to_string()],
        signals_only_in_file1: vec![],
        signals_only_in_file2: vec![],
        mismatches: vec![
            vcd_tools_rs::SignalMismatch {
                signal_name: "sig1".to_string(),
                time: 10,
                value1: vcd_tools_rs::ChangeValue::Integer(1),
                value2: vcd_tools_rs::ChangeValue::Integer(0),
                is_unknown: false,
            },
        ],
        total_mismatches: 1,
        signals_with_mismatches: 1,
        passed: false,
    };
    assert!(result.get_summary().contains("FAIL"));
    assert_eq!(result.signals_with_mismatches, 1);
    assert_eq!(result.total_mismatches, 1);
}

#[test]
fn json_comparison_result_from() {
    let result = vcd_tools_rs::ComparisonResult {
        file1: "a.vcd".to_string(),
        file2: "b.vcd".to_string(),
        common_signals: vec!["sig1".to_string()],
        signals_only_in_file1: vec![],
        signals_only_in_file2: vec![],
        mismatches: vec![],
        total_mismatches: 0,
        signals_with_mismatches: 0,
        passed: true,
    };
    let json_result = vcd_tools_rs::JsonComparisonResult::from(&result);
    assert_eq!(json_result.common_signal_count, 1);
    assert!(json_result.passed);
}

#[test]
fn comparison_options_default() {
    let opts = vcd_tools_rs::ComparisonOptions::default();
    assert!(opts.signals_only.is_empty());
    assert!(!opts.ignore_unknown);
    assert!(opts.time_window.start.is_none());
    assert!(opts.time_window.end.is_none());
}
