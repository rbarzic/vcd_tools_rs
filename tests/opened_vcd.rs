use std::io::{Read, Seek};

use vcd_tools_rs::{FingerprintPolicy, OpenOptions, OpenedVcd};

const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";

#[test]
fn public_opened_api_exposes_borrowed_catalog_without_legacy_materialization() {
    let opened = OpenedVcd::open(SEMANTICS).expect("open VCD");
    assert!(opened.configured_path().is_absolute());
    assert!(opened.configured_path().ends_with(SEMANTICS));
    assert_eq!(opened.signal_count(), 7);
    assert_eq!(opened.signal_names().next(), Some("top.a"));

    let signal = opened.signal("top.a").expect("signal");
    assert_eq!(signal.name(), "top.a");
    assert_eq!(signal.scope_components().collect::<Vec<_>>(), ["top"]);
    assert_eq!(
        opened
            .aliases(signal.id_code())
            .map(|alias| alias.name())
            .collect::<Vec<_>>(),
        ["top.a", "top.alias_a"]
    );
}

#[test]
fn public_body_readers_are_independent_and_completion_validated() {
    let opened = OpenedVcd::open(SEMANTICS).expect("open VCD");
    let mut first = opened.body_reader().expect("first reader");
    let mut second = opened.body_reader().expect("second reader");
    let body_offset = opened.body_offset();

    let mut byte = [0_u8; 1];
    first.read_exact(&mut byte).expect("read first");
    assert_eq!(
        second.stream_position().expect("second position"),
        body_offset
    );
    first.validate_complete().expect("first completion");
    second.validate_complete().expect("second completion");
}

#[test]
fn public_strict_option_is_explicit() {
    let options = OpenOptions::new().with_fingerprint_policy(FingerprintPolicy::StrictFullContent);
    let opened = OpenedVcd::open_with_options(SEMANTICS, options).expect("strict open");
    assert!(opened.identity().full_content_fingerprint().is_some());
}
