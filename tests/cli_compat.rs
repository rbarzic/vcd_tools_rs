use std::io::Write;
use std::process::{Command, Output};

const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";
const CHANGED: &str = "tests/fixtures/query_semantics_changed.vcd";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vcd_tools_rs"))
        .args(args)
        .output()
        .expect("run native CLI")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("UTF-8 stdout")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("UTF-8 stderr")
}

#[test]
fn version_matches_package_version() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        format!("vcd_tools_rs {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(stderr(&output).is_empty());
}

#[test]
fn list_default_output_is_one_declared_name_per_line() {
    let output = run(&["list", SEMANTICS]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "top.a\n",
            "top.alias_a\n",
            "top.b\n",
            "top.vec4[3:0]\n",
            "top.wide[128:0]\n",
            "top.real_sig\n",
            "top.str_sig\n"
        )
    );
}

#[test]
fn list_filter_is_case_sensitive_and_preserves_matching_order() {
    let output = run(&["list", SEMANTICS, "--filter", "alias"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "top.alias_a\n");

    let no_match = run(&["list", SEMANTICS, "--filter", "ALIAS"]);
    assert!(no_match.status.success(), "{}", stderr(&no_match));
    assert!(stdout(&no_match).is_empty());
}

#[test]
fn metadata_default_output_is_stable_tsv() {
    let output = run(&["meta", SEMANTICS]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "signals\t7\ntimescale\t1 ns\nstart_time\t0\nend_time\t25\n"
    );
}

#[test]
fn global_pretty_flag_is_accepted_after_subcommand_arguments() {
    let output = run(&["meta", SEMANTICS, "--pretty"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        concat!(
            "Field      | Value\n",
            "-----------+------\n",
            "signals    | 7    \n",
            "timescale  | 1 ns \n",
            "start_time | 0    \n",
            "end_time   | 25   \n"
        )
    );
}

#[test]
fn global_pretty_flag_is_accepted_before_subcommand() {
    let output = run(&["--pretty", "meta", SEMANTICS]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).starts_with("Field      | Value\n"));
}

#[test]
fn extract_is_time_aligned_and_carries_values_between_emitted_rows() {
    let output = run(&[
        "extract", SEMANTICS, "--signal", "top.a", "--signal", "top.b", "--start", "5", "--end",
        "15",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "time\ttop.a\ttop.b\n5\t1\t1\n10\t0\t1\n15\t1\tz\n"
    );
}

#[test]
fn toggle_and_find_output_preserve_request_order_and_format() {
    let toggles = run(&[
        "toggle",
        SEMANTICS,
        "--signal",
        "top.alias_a,top.a",
        "--start",
        "5",
        "--end",
        "20",
    ]);
    assert!(toggles.status.success(), "{}", stderr(&toggles));
    assert_eq!(
        stdout(&toggles),
        "signal\ttoggles\ntop.alias_a\t3\ntop.a\t3\n"
    );

    let find = run(&[
        "find",
        SEMANTICS,
        "--signal",
        "top.a",
        "--value",
        "1",
        "--occurrence",
        "2",
    ]);
    assert!(find.status.success(), "{}", stderr(&find));
    assert_eq!(stdout(&find), "time\ttop.a\n15\t1\n");
}

#[test]
fn signals_file_supports_comments_and_preserves_order() {
    let mut file = tempfile::NamedTempFile::new().expect("signals file");
    writeln!(file, "# selected signals").unwrap();
    writeln!(file, "top.b # second declaration").unwrap();
    writeln!(file, "top.a").unwrap();

    let output = run(&[
        "toggle",
        SEMANTICS,
        "--signals-file",
        file.path().to_str().expect("UTF-8 path"),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "signal\ttoggles\ntop.b\t5\ntop.a\t5\n");
}

#[test]
fn find_not_found_is_exit_one_with_no_stdout() {
    let output = run(&[
        "find", SEMANTICS, "--signal", "top.a", "--value", "1", "--start", "6", "--end", "9",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "Error: No matching occurrence found in the specified window.\n"
    );
}

#[test]
fn compare_json_shape_and_mismatch_count_are_stable() {
    let output = run(&[
        "compare",
        SEMANTICS,
        CHANGED,
        "--signals-only",
        "top.a",
        "--output",
        "json",
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON output");
    assert_eq!(json["file1"], SEMANTICS);
    assert_eq!(json["file2"], CHANGED);
    assert_eq!(json["common_signal_count"], 7);
    assert_eq!(json["mismatches_by_signal"]["top.a"], 1);
    assert_eq!(json["total_mismatches"], 1);
    assert_eq!(json["signals_with_mismatches"], 1);
    assert_eq!(json["passed"], false);
}

#[test]
fn compare_difference_is_successful_in_compact_and_default_formats() {
    let compact = run(&[
        "compare",
        SEMANTICS,
        CHANGED,
        "--signals-only",
        "top.a",
        "--output",
        "compact",
    ]);
    assert!(
        compact.status.success(),
        "comparison differences are result data, not process failure: {}",
        stderr(&compact)
    );
    assert_eq!(
        stdout(&compact),
        "❌ FAIL: 1/7 signals have mismatches (1 total)\n"
    );

    let default = run(&["compare", SEMANTICS, CHANGED, "--signals-only", "top.a"]);
    assert!(default.status.success(), "{}", stderr(&default));
    let text = stdout(&default);
    assert!(text.contains("Waveform Comparison Results"));
    assert!(text.contains("Signal: top.a"));
    assert!(text.contains("Time #15: Ref='1' | Actual='0' ❌ MISMATCH"));
    assert!(text.contains("Total mismatches: 1"));
}

#[test]
fn missing_signal_failure_has_empty_stdout_and_stable_error() {
    let output = run(&["extract", SEMANTICS, "--signal", "top.missing"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "Error: Signals not found in VCD: top.missing\n"
    );
}

#[test]
fn extract_without_signals_fails_before_writing_output() {
    let output = run(&["extract", SEMANTICS]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "Error: At least one --signal or --signals-file entry is required\n"
    );
}

#[test]
fn explicit_format_override_accepts_match_and_rejects_mismatch() {
    let matched = run(&["--format", "fst", "list", "tests/fixtures/fst/tiny.fst"]);
    assert!(matched.status.success(), "{}", stderr(&matched));
    let mismatch = run(&["--format", "vcd", "list", "tests/fixtures/fst/tiny.fst"]);
    assert!(!mismatch.status.success());
}
