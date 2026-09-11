use std::fs::File;
use std::io::Write;

use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalValue};
use tempfile::tempdir;
use vcd_tools_rs::{
    WaveformDetectionError, WaveformFormat, WaveformFormatHint, detect_waveform_format,
};

#[path = "fixtures/fst/generator.rs"]
mod fst_fixture;

const VCD_FIXTURE: &str = "tests/fixtures/query_semantics.vcd";
const FST_FIXTURE: &str = "tests/fixtures/fst/tiny.fst";
const FST_ORACLE: &str = "tests/fixtures/fst/tiny.oracle.json";

#[test]
fn committed_fixture_is_reproducible_and_matches_semantic_oracle() {
    let directory = tempdir().expect("temp directory");
    let generated = directory.path().join("tiny.fst");
    fst_fixture::write_tiny_fst(&generated);
    assert_eq!(
        std::fs::read(&generated).expect("generated fixture"),
        std::fs::read(FST_FIXTURE).expect("committed fixture"),
        "fixture recipe or fst-writer output drifted; regenerate deliberately"
    );

    let oracle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(FST_ORACLE).expect("read semantic oracle"))
            .expect("parse semantic oracle");
    let mut reader = FstReader::open(std::io::BufReader::new(File::open(&generated).unwrap()))
        .expect("read generated FST");
    let header = reader.get_header();
    assert_eq!(header.start_time, oracle["start_time"].as_u64().unwrap());
    assert_eq!(header.end_time, oracle["end_time"].as_u64().unwrap());
    assert_eq!(
        header.var_count,
        oracle["declarations"].as_array().unwrap().len() as u64
    );
    assert_eq!(header.max_handle, 1);
    assert_eq!(
        i64::from(header.timescale_exponent),
        oracle["timescale_exponent"].as_i64().unwrap()
    );

    let mut hierarchy = Vec::new();
    reader
        .read_hierarchy(|entry| hierarchy.push(entry))
        .expect("read hierarchy");
    assert!(matches!(hierarchy[0], FstHierarchyEntry::Scope { .. }));
    assert!(matches!(
        hierarchy[1],
        FstHierarchyEntry::Var {
            is_alias: false,
            ..
        }
    ));
    assert!(matches!(
        hierarchy[2],
        FstHierarchyEntry::Var { is_alias: true, .. }
    ));

    let mut events = Vec::new();
    reader
        .read_signals(&FstFilter::all(), |time, handle, value| {
            let value = match value {
                FstSignalValue::String(bytes) => std::str::from_utf8(bytes)
                    .expect("ASCII fixture")
                    .to_owned(),
                FstSignalValue::Real(value) => value.to_string(),
            };
            events.push((time, handle.get_index(), value));
            Ok::<(), ()>(())
        })
        .expect("read changes");
    let expected = oracle["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| {
            (
                event["time"].as_u64().unwrap(),
                event["handle_index"].as_u64().unwrap() as usize,
                event["value"].as_str().unwrap().to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(events, expected);
}

#[test]
fn detects_valid_mislabeled_and_extensionless_fst() {
    let directory = tempdir().expect("temp directory");
    let source = directory.path().join("fixture.fst");
    std::fs::copy(FST_FIXTURE, &source).expect("copy fixture");

    let mislabeled = directory.path().join("fixture.vcd");
    std::fs::copy(&source, &mislabeled).expect("copy mislabeled fixture");
    let extensionless = directory.path().join("fixture");
    std::fs::copy(&source, &extensionless).expect("copy extensionless fixture");

    for path in [&source, &mislabeled, &extensionless] {
        assert_eq!(
            detect_waveform_format(path, WaveformFormatHint::Auto).unwrap(),
            WaveformFormat::Fst
        );
    }
}

#[test]
fn explicit_format_mismatch_is_deterministic() {
    let error = detect_waveform_format(FST_FIXTURE, WaveformFormatHint::Vcd).unwrap_err();
    assert!(matches!(
        error,
        WaveformDetectionError::ExplicitMismatch {
            requested: WaveformFormat::Vcd,
            detected: WaveformFormat::Fst
        }
    ));

    let error = detect_waveform_format(VCD_FIXTURE, WaveformFormatHint::Fst).unwrap_err();
    assert!(matches!(
        error,
        WaveformDetectionError::ExplicitMismatch {
            requested: WaveformFormat::Fst,
            detected: WaveformFormat::Vcd
        }
    ));
}

#[test]
fn detects_valid_vcd_even_when_extensionless() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("input");
    std::fs::copy(VCD_FIXTURE, &path).expect("copy VCD");
    assert_eq!(
        detect_waveform_format(&path, WaveformFormatHint::Auto).unwrap(),
        WaveformFormat::Vcd
    );
}

#[test]
fn empty_and_truncated_inputs_are_rejected() {
    let directory = tempdir().expect("temp directory");
    let empty = directory.path().join("empty");
    File::create(&empty).expect("empty input");
    let error = detect_waveform_format(&empty, WaveformFormatHint::Auto).unwrap_err();
    assert!(matches!(error, WaveformDetectionError::Undetected { .. }));

    let truncated = directory.path().join("truncated.fst");
    let mut bytes = std::fs::read(FST_FIXTURE).expect("read fixture");
    bytes.truncate(bytes.len() / 2);
    File::create(&truncated)
        .expect("create truncated input")
        .write_all(&bytes)
        .expect("write truncated input");
    let error = detect_waveform_format(&truncated, WaveformFormatHint::Auto).unwrap_err();
    assert!(matches!(
        error,
        WaveformDetectionError::Invalid {
            format: WaveformFormat::Fst,
            ..
        }
    ));
}

#[test]
fn whole_file_gzip_wrapper_is_rejected_before_reader_open() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("wrapped.fst");
    File::create(&path)
        .expect("create wrapper")
        .write_all(&[254, 0, 0, 0, 0, 0, 0, 0, 8])
        .expect("write wrapper marker");
    let error = detect_waveform_format(&path, WaveformFormatHint::Auto).unwrap_err();
    assert!(matches!(
        error,
        WaveformDetectionError::UnsupportedFst { .. }
    ));
}

#[test]
fn oversized_sparse_input_is_rejected_before_probe() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("oversized");
    let file = File::create(&path).expect("create sparse input");
    file.set_len(8 * 1024 * 1024 * 1024 + 1)
        .expect("set sparse length");
    let error = detect_waveform_format(&path, WaveformFormatHint::Auto).unwrap_err();
    assert!(matches!(
        error,
        WaveformDetectionError::ResourceLimit { .. }
    ));
}
