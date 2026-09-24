use std::io::{Seek, SeekFrom, Write};

use tempfile::NamedTempFile;
use vcd_tools_rs::query::{QueryContext, QueryLimits};
use vcd_tools_rs::server::runtime::RuntimeConfig;
use vcd_tools_rs::server::service::{ServiceConfig, VcdService};
use vcd_tools_rs::{
    ComparisonOptions, OpenedFst, OpenedWaveform, TimeWindow, WaveformFormat, parse_target_value,
};

const FST: &str = "tests/fixtures/fst/tiny.fst";

fn oversized_hierarchy_fixture() -> NamedTempFile {
    let mut bytes = std::fs::read(FST).unwrap();
    let mut offset = 0usize;
    loop {
        let kind = bytes[offset];
        let section_length =
            u64::from_be_bytes(bytes[offset + 1..offset + 9].try_into().unwrap()) as usize;
        if matches!(kind, 4 | 6 | 7) {
            bytes[offset + 9..offset + 17]
                .copy_from_slice(&(512_u64 * 1024 * 1024 + 1).to_be_bytes());
            break;
        }
        offset += 1 + section_length;
    }
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    file
}

#[test]
fn opens_lists_and_extracts_fst_fixture() {
    let opened = OpenedFst::open(FST).expect("open FST");
    assert_eq!(opened.metadata().start_time, 0);
    assert_eq!(opened.metadata().end_time, 5);
    let timescale = opened.metadata().timescale.as_ref().unwrap();
    assert_eq!(
        (timescale.magnitude, timescale.unit.to_string()),
        (1, "ns".into())
    );
    assert_eq!(opened.metadata().unique_signal_count, 1);
    assert_eq!(opened.list_signals(None), ["top.a", "top.a_alias"]);

    let rows = opened
        .extract(
            &["top.a".to_string(), "top.a_alias".to_string()],
            TimeWindow::default(),
        )
        .expect("extract FST");
    assert_eq!(rows.len(), 4);
    assert_eq!(
        (
            rows[0].signal.as_str(),
            rows[0].time,
            rows[0].value.to_string()
        ),
        ("top.a", 0, "0".into())
    );
    assert_eq!(
        (
            rows[3].signal.as_str(),
            rows[3].time,
            rows[3].value.to_string()
        ),
        ("top.a_alias", 5, "1".into())
    );
}

#[test]
fn fst_generation_detects_in_place_mutation() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(&std::fs::read(FST).unwrap()).unwrap();
    file.flush().unwrap();
    let opened = OpenedFst::open(file.path()).unwrap();
    file.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
    file.as_file_mut().write_all(&[0xff]).unwrap();
    file.flush().unwrap();
    assert!(opened.validate_source().is_err());
    let waveform = OpenedWaveform::Fst(opened);
    assert!(
        waveform
            .metadata(&QueryContext::legacy_unlimited())
            .is_err()
    );
}

#[test]
fn generic_facade_accepts_fst_while_vcd_named_apis_remain_vcd_only() {
    let opened = OpenedWaveform::open(FST).unwrap();
    let context = QueryContext::legacy_unlimited();
    assert_eq!(opened.format(), WaveformFormat::Fst);
    assert_eq!(opened.list_signals(None), ["top.a", "top.a_alias"]);
    let timescale = opened.metadata(&context).unwrap().1.unwrap();
    assert_eq!(
        (timescale.magnitude, timescale.unit.to_string()),
        (1, "ns".into())
    );
    assert_eq!(
        opened
            .extract(&["top.a".into()], TimeWindow::default(), &context)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        opened
            .count_toggles(&["top.a".into()], TimeWindow::default(), &context)
            .unwrap()["top.a"],
        1
    );
    assert_eq!(
        opened
            .find_nth_occurrence(
                "top.a",
                parse_target_value("1"),
                1,
                TimeWindow::default(),
                &context
            )
            .unwrap()
            .0
            .unwrap()
            .time,
        5
    );
    assert!(vcd_tools_rs::list_signals_from_file(FST, None).is_err());
}

#[test]
fn fst_limits_and_inverted_window_are_enforced() {
    let opened = OpenedFst::open(FST).unwrap();
    let context = QueryContext::new().with_limits(QueryLimits::unlimited().with_max_rows(1));
    assert!(
        opened
            .extract_with_context(&["top.a".into()], TimeWindow::default(), &context)
            .is_err()
    );
    assert!(
        opened
            .extract(
                &["top.a".into()],
                TimeWindow {
                    start: Some(6),
                    end: Some(5)
                }
            )
            .unwrap()
            .is_empty()
    );
}

#[test]
fn fst_and_vcd_comparison_pairs_are_supported() {
    let mut vcd = NamedTempFile::new().unwrap();
    write!(vcd, "$timescale 1 ns $end\n$scope module top $end\n$var wire 1 ! a $end\n$var wire 1 ! a_alias $end\n$upscope $end\n$enddefinitions $end\n#0\n0!\n#5\n1!\n").unwrap();
    let path = vcd.path().to_str().unwrap();
    let fst = OpenedWaveform::open(FST).unwrap();
    let vcd_waveform = OpenedWaveform::open(path).unwrap();
    assert!(
        fst.compare(
            &fst,
            &ComparisonOptions::default(),
            &QueryContext::legacy_unlimited()
        )
        .unwrap()
        .passed
    );
    assert!(
        vcd_waveform
            .compare(
                &fst,
                &ComparisonOptions::default(),
                &QueryContext::legacy_unlimited()
            )
            .unwrap()
            .passed
    );
    assert!(
        fst.compare(
            &vcd_waveform,
            &ComparisonOptions::default(),
            &QueryContext::legacy_unlimited()
        )
        .unwrap()
        .passed
    );

    let mut mismatched = NamedTempFile::new().unwrap();
    write!(mismatched, "$timescale 1 ps $end\n$scope module top $end\n$var wire 1 ! a $end\n$upscope $end\n$enddefinitions $end\n#0\n0!\n").unwrap();
    assert!(
        OpenedWaveform::open(mismatched.path())
            .unwrap()
            .compare(
                &fst,
                &ComparisonOptions::default(),
                &QueryContext::legacy_unlimited()
            )
            .is_err()
    );
}

#[test]
fn server_rejects_excessive_compressed_hierarchy_before_admission() {
    let fixture = oversized_hierarchy_fixture();
    assert!(OpenedFst::open(fixture.path()).is_err());
    assert!(
        VcdService::new(
            fixture.path(),
            RuntimeConfig::default(),
            ServiceConfig::default()
        )
        .is_err()
    );
}

#[test]
fn server_generation_accepts_fst() {
    let service = VcdService::new(FST, RuntimeConfig::default(), ServiceConfig::default()).unwrap();
    let opened = service.generation_slot().current().unwrap();
    assert_eq!(opened.format(), WaveformFormat::Fst);
    assert_eq!(opened.signal_count(), 2);
}

#[test]
fn fst_find_stops_successfully_before_later_command_limit() {
    let opened = OpenedWaveform::open(FST).unwrap();
    let context = QueryContext::new().with_limits(QueryLimits::unlimited().with_max_commands(1));
    let found = opened
        .find_nth_occurrence(
            "top.a",
            parse_target_value("0"),
            1,
            TimeWindow::default(),
            &context,
        )
        .expect("first callback must complete before the next command exceeds the limit")
        .0
        .unwrap();
    assert_eq!(found.time, 0);
    assert!(
        opened
            .find_nth_occurrence(
                "top.a",
                parse_target_value("1"),
                1,
                TimeWindow::default(),
                &context,
            )
            .is_err()
    );
}
