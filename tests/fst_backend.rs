use vcd_tools_rs::{OpenedFst, TimeWindow};

#[test]
fn opens_lists_and_extracts_fst_fixture() {
    let opened = OpenedFst::open("tests/fixtures/fst/tiny.fst").expect("open FST");
    assert_eq!(opened.metadata().start_time, 0);
    assert_eq!(opened.metadata().end_time, 5);
    assert_eq!(opened.list_signals(None), ["top.a", "top.a_alias"]);

    let rows = opened
        .extract(&["top.a".to_string(), "top.a_alias".to_string()], TimeWindow::default())
        .expect("extract FST");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].signal, "top.a");
    assert_eq!(rows[0].time, 0);
    assert_eq!(rows[0].value.to_string(), "0");
    assert_eq!(rows[3].signal, "top.a_alias");
    assert_eq!(rows[3].time, 5);
    assert_eq!(rows[3].value.to_string(), "1");
}
