use std::collections::HashMap;
use std::fs;
use std::io;

use serde_json::{Value, json};
use vcd::TimescaleUnit;
use vcd_tools_rs::query::{QueryError, QueryLimitKind};
use vcd_tools_rs::server::protocol::*;
use vcd_tools_rs::{ChangeValue, Timescale, VcdError};

const FIXTURES: &str = "tests/fixtures/protocol/v1";

fn lines(name: &str) -> Vec<String> {
    fs::read_to_string(format!("{FIXTURES}/{name}"))
        .expect("fixture")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn golden_requests_decode_and_encode_exactly() {
    let fixture = lines("requests.jsonl");
    assert_eq!(fixture.len(), 8);
    let expected_methods = [
        "ping", "describe", "list", "metadata", "extract", "find", "toggles", "cancel",
    ];
    for (line, expected_method) in fixture.iter().zip(expected_methods) {
        let request = decode_request(line).expect("golden request decodes");
        assert_eq!(request.method.name(), expected_method);
        let encoded = String::from_utf8(encode_json_line(&request).expect("encode")).unwrap();
        assert_eq!(encoded, format!("{line}\n"));
    }
}

fn golden_responses() -> Vec<ResponseFrame> {
    let ping = PingResult {
        protocol: "1".into(),
        package_version: "0.1.7".into(),
        uptime_ms: "1234".into(),
        ready: true,
    };
    let describe = DescribeResult {
        protocol: "1".into(),
        generation: "7".into(),
        signal_count: "193730".into(),
        timescale: Some(WireTimescale {
            magnitude: "1".into(),
            unit: "fs".into(),
        }),
        source_size: "4589561612".into(),
        immutable_source: true,
        methods: [
            "ping", "describe", "list", "metadata", "extract", "find", "toggles", "cancel",
        ]
        .map(str::to_owned)
        .to_vec(),
        schemas: ["signal_name.v1", "time_value.v1"]
            .map(str::to_owned)
            .to_vec(),
        backends: vec!["stream".into()],
        limits: ServerLimits::default(),
        security: SecurityCapabilities {
            transport: "unix".into(),
            socket_mode: "0600".into(),
            peer_uid_check: false,
        },
        distribution: DistributionCapabilities {
            native_serve: true,
            python_console_serve: false,
        },
    };
    let metadata = MetadataResult {
        signal_count: "7".into(),
        timescale: Some(WireTimescale {
            magnitude: "1".into(),
            unit: "ns".into(),
        }),
        start_time: "0".into(),
        end_time: "25".into(),
    };
    let found = FindResult {
        found: true,
        signal: "tb.done".into(),
        time: Some("1000".into()),
        width: "1".into(),
        value: Some(WireValue::Text { text: "1".into() }),
    };
    let toggles = TogglesResult {
        rows: vec![ToggleRow {
            signal: "tb.clk".into(),
            toggles: "500".into(),
        }],
    };
    let rows = vec![
        serde_json::to_value(TimeValueRow {
            signal: "tb.sig".into(),
            time: "1000".into(),
            width: "4".into(),
            value: WireValue::Integer { text: "10".into() },
        })
        .unwrap(),
        serde_json::to_value(TimeValueRow {
            signal: "tb.sig".into(),
            time: "1000".into(),
            width: "4".into(),
            value: WireValue::Float {
                text: "1.5".into(),
                bits: "3ff8000000000000".into(),
            },
        })
        .unwrap(),
        serde_json::to_value(TimeValueRow {
            signal: "tb.sig".into(),
            time: "1000".into(),
            width: "4".into(),
            value: WireValue::Text {
                text: "10xz".into(),
            },
        })
        .unwrap(),
    ];
    let mut responses = vec![
        ResponseFrame::result("ping-1", &ping).unwrap(),
        ResponseFrame::result("describe-1", &describe).unwrap(),
        ResponseFrame::result("meta-1", &metadata).unwrap(),
        ResponseFrame::result("find-1", &found).unwrap(),
        ResponseFrame::result("toggle-1", &toggles).unwrap(),
        ResponseFrame::result("cancel-1", &CancelResult { found: true }).unwrap(),
    ];

    let mut list_stream = vec![
        ResponseFrame::begin("list-1", "signal_name.v1", "7"),
        ResponseFrame::chunk(
            "list-1",
            1,
            vec![
                Value::String("tb.clk".into()),
                Value::String("tb.cache_clk".into()),
            ],
        ),
    ];
    let list_prior = cumulative_encoded_jsonl_bytes(&list_stream).unwrap();
    list_stream.push(
        ResponseFrame::end_success(
            "list-1",
            2,
            StreamStats {
                rows: "2".into(),
                encoded_bytes: String::new(),
                elapsed_ms: "1".into(),
                commands: "0".into(),
                backend: "stream".into(),
            },
            list_prior,
        )
        .unwrap(),
    );
    responses.extend(list_stream);

    let mut extract_stream = vec![
        ResponseFrame::begin("extract-1", "time_value.v1", "7"),
        ResponseFrame::chunk("extract-1", 1, rows.clone()),
    ];
    let extract_prior = cumulative_encoded_jsonl_bytes(&extract_stream).unwrap();
    extract_stream.push(
        ResponseFrame::end_success(
            "extract-1",
            2,
            StreamStats {
                rows: "3".into(),
                encoded_bytes: String::new(),
                elapsed_ms: "12".into(),
                commands: "5000".into(),
                backend: "stream".into(),
            },
            extract_prior,
        )
        .unwrap(),
    );
    responses.extend(extract_stream);

    responses.extend([
        ResponseFrame::begin("extract-2", "time_value.v1", "7"),
        ResponseFrame::chunk("extract-2", 1, vec![rows[0].clone()]),
        ResponseFrame::end_error(
            "extract-2",
            2,
            ProtocolError::new(
                ProtocolErrorCode::Cancelled,
                "query cancelled",
                json!({}),
                false,
            ),
        ),
        ResponseFrame::error(
            None,
            0,
            ProtocolError::new(
                ProtocolErrorCode::BadRequest,
                "invalid request",
                json!({}),
                false,
            ),
        ),
    ]);
    responses
}

#[test]
fn golden_responses_encode_exactly_and_include_newline_bytes() {
    let fixture = lines("responses.jsonl");
    let responses = golden_responses();
    assert_eq!(fixture.len(), responses.len());
    for (line, response) in fixture.iter().zip(responses) {
        let encoded = encode_json_line(&response).expect("encode");
        assert_eq!(encoded_json_line_bytes(&response).unwrap(), encoded.len());
        assert_eq!(String::from_utf8(encoded).unwrap(), format!("{line}\n"));
    }
}

fn response_fixture_groups() -> HashMap<String, Vec<(String, Value)>> {
    let mut groups: HashMap<String, Vec<(String, Value)>> = HashMap::new();
    for line in lines("responses.jsonl") {
        let value: Value = serde_json::from_str(&line).unwrap();
        if let Some(id) = value["id"].as_str() {
            groups.entry(id.to_owned()).or_default().push((line, value));
        }
    }
    groups
}

fn assert_legal_response_group(id: &str, group: &[(String, Value)]) {
    assert!(!group.is_empty(), "empty response group for {id}");
    for (expected_seq, (_, frame)) in group.iter().enumerate() {
        assert_eq!(
            frame["seq"].as_u64(),
            Some(expected_seq as u64),
            "sequence for {id}"
        );
    }
    let first_type = group[0].1["type"].as_str().unwrap();
    if first_type == "begin" {
        assert_eq!(
            group.last().unwrap().1["type"],
            "end",
            "terminal frame for {id}"
        );
        assert_eq!(
            group
                .iter()
                .filter(|(_, frame)| frame["type"] == "end")
                .count(),
            1,
            "one terminal end for {id}"
        );
        assert!(
            group[1..group.len() - 1]
                .iter()
                .all(|(_, frame)| frame["type"] == "chunk"),
            "only chunks between begin/end for {id}"
        );
    } else {
        assert_eq!(group.len(), 1, "unary response count for {id}");
        assert!(matches!(first_type, "result" | "error"));
    }
}

#[test]
fn every_golden_response_id_has_one_legal_contiguous_lifecycle() {
    let groups = response_fixture_groups();
    for (id, group) in &groups {
        assert_legal_response_group(id, group);
    }
}

#[test]
fn complete_streams_report_authoritative_cumulative_jsonl_bytes() {
    for (id, group) in response_fixture_groups() {
        if group[0].1["type"] != "begin" || group.last().unwrap().1["complete"] != true {
            continue;
        }
        let cumulative = group
            .iter()
            .map(|(line, _)| line.as_bytes().len() as u64 + 1)
            .sum::<u64>();
        let reported = group.last().unwrap().1["stats"]["encoded_bytes"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert_eq!(reported, cumulative, "authoritative encoded bytes for {id}");
    }
}

#[test]
fn incomplete_stream_has_begin_chunk_and_single_terminal_error() {
    let groups = response_fixture_groups();
    let group = &groups["extract-2"];
    assert_legal_response_group("extract-2", group);
    assert_eq!(
        group
            .iter()
            .map(|(_, frame)| frame["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["begin", "chunk", "end"]
    );
    let end = &group.last().unwrap().1;
    assert_eq!(end["complete"], false);
    assert!(end.get("stats").is_none());
    assert_eq!(end["error"]["code"], "CANCELLED");
}

#[test]
fn golden_error_envelopes_cover_every_frozen_code() {
    let definitions = vec![
        (
            ProtocolErrorCode::BadRequest,
            "invalid request",
            json!({}),
            false,
        ),
        (
            ProtocolErrorCode::UnsupportedVersion,
            "unsupported protocol version",
            json!({"supported_versions":["1"]}),
            false,
        ),
        (
            ProtocolErrorCode::UnknownMethod,
            "unknown method",
            json!({"method":"future"}),
            false,
        ),
        (
            ProtocolErrorCode::DuplicateRequestId,
            "request ID is already active",
            json!({"id":"x"}),
            false,
        ),
        (
            ProtocolErrorCode::SignalNotFound,
            "one or more signals were not found",
            json!({"signals":["tb.missing"]}),
            false,
        ),
        (
            ProtocolErrorCode::InvalidWindow,
            "start must be less than or equal to end",
            json!({}),
            false,
        ),
        (
            ProtocolErrorCode::InvalidOccurrence,
            "occurrence must be >= 1",
            json!({}),
            false,
        ),
        (
            ProtocolErrorCode::LimitExceeded,
            "query limit exceeded",
            json!({"actual":"2","kind":"rows","limit":"1"}),
            false,
        ),
        (
            ProtocolErrorCode::QueueFull,
            "query queue is full",
            json!({}),
            true,
        ),
        (
            ProtocolErrorCode::Cancelled,
            "query cancelled",
            json!({}),
            false,
        ),
        (
            ProtocolErrorCode::DeadlineExceeded,
            "query deadline exceeded",
            json!({}),
            true,
        ),
        (
            ProtocolErrorCode::StaleSource,
            "source changed during query",
            json!({}),
            true,
        ),
        (
            ProtocolErrorCode::SourceUnavailable,
            "source unavailable",
            json!({}),
            true,
        ),
        (
            ProtocolErrorCode::ParseError,
            "VCD parse error",
            json!({}),
            false,
        ),
        (
            ProtocolErrorCode::Internal,
            "internal server error",
            json!({}),
            false,
        ),
    ];
    let fixture = lines("errors.jsonl");
    assert_eq!(fixture.len(), definitions.len());
    for (line, (code, message, details, retryable)) in fixture.iter().zip(definitions) {
        let id = format!("e-{}", code.as_str().to_ascii_lowercase().replace('_', "-"));
        let frame = ResponseFrame::error(
            Some(id),
            0,
            ProtocolError::new(code, message, details, retryable),
        );
        assert_eq!(
            String::from_utf8(encode_json_line(&frame).unwrap()).unwrap(),
            format!("{line}\n")
        );
    }
}

#[test]
fn incremental_accumulator_handles_partial_and_multiple_frames() {
    let mut accumulator = JsonLineAccumulator::default();
    assert!(accumulator.push(b"{\"v\":1").unwrap().is_empty());
    let frames = accumulator
        .push(b",\"id\":\"a\",\"method\":\"ping\",\"params\":{}}\n{\"v\":1,\"id\":\"b\",\"method\":\"ping\",\"params\":{}}\n")
        .unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(decode_request(&frames[0]).unwrap().id, "a");
    assert_eq!(decode_request(&frames[1]).unwrap().id, "b");
    assert_eq!(accumulator.buffered_bytes(), 0);
}

#[test]
fn accumulator_rejects_invalid_utf8_and_oversize_before_growth() {
    let mut invalid = JsonLineAccumulator::new(8);
    let error = invalid.push(&[0xff, b'\n']).unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::BadRequest);

    let mut oversized = JsonLineAccumulator::new(8);
    let error = oversized.push(b"12345678").unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::LimitExceeded);
    assert_eq!(oversized.buffered_bytes(), 0);
    assert!(
        oversized.push(b"\n").is_err(),
        "poisoned connection remains failed"
    );
}

#[test]
fn checked_in_invalid_cases_have_stable_categories() {
    let cases: Value = serde_json::from_str(
        &fs::read_to_string(format!("{FIXTURES}/invalid_cases.json")).unwrap(),
    )
    .unwrap();
    let expected = [
        ("malformed", ProtocolErrorCode::BadRequest),
        ("duplicate_envelope_key", ProtocolErrorCode::BadRequest),
        ("duplicate_nested_key", ProtocolErrorCode::BadRequest),
        ("unknown_param", ProtocolErrorCode::BadRequest),
        ("unsupported_version", ProtocolErrorCode::UnsupportedVersion),
        ("leading_zero", ProtocolErrorCode::BadRequest),
        ("u64_overflow", ProtocolErrorCode::BadRequest),
        ("invalid_window", ProtocolErrorCode::InvalidWindow),
        ("zero_occurrence", ProtocolErrorCode::InvalidOccurrence),
        ("too_small_response", ProtocolErrorCode::BadRequest),
        ("oversize_unterminated", ProtocolErrorCode::LimitExceeded),
    ];
    for (case, (expected_name, expected_code)) in cases.as_array().unwrap().iter().zip(expected) {
        assert_eq!(case["name"], expected_name);
        let error = if let Some(line) = case.get("line") {
            decode_request(line.as_str().unwrap()).unwrap_err()
        } else {
            let count = case["count"].as_u64().unwrap() as usize;
            let repeated = case["repeat"].as_str().unwrap().as_bytes()[0];
            JsonLineAccumulator::default()
                .push(&vec![repeated; count])
                .unwrap_err()
        };
        assert_eq!(error.code, expected_code, "case {expected_name}");
    }
}

#[test]
fn unknown_envelope_fields_are_ignored_but_params_are_strict() {
    let request =
        decode_request(r#"{"v":1,"id":"x","method":"ping","params":{},"future":{"nested":true}}"#)
            .unwrap();
    assert_eq!(request.id, "x");
    assert_eq!(request.method, RequestMethod::Ping);
    let error =
        decode_request(r#"{"v":1,"id":"x","method":"ping","params":{"future":true}}"#).unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::BadRequest);
}

#[test]
fn active_id_validation_is_separate_and_bounded() {
    let line = r#"{"v":1,"id":"active","method":"ping","params":{}}"#;
    let error = decode_request_with_active(line, |id| id == "active").unwrap_err();
    assert_eq!(error.code, ProtocolErrorCode::DuplicateRequestId);

    let long = "é".repeat(65); // 130 UTF-8 bytes
    let line = format!(r#"{{"v":1,"id":"{long}","method":"ping","params":{{}}}}"#);
    assert_eq!(
        decode_request(&line).unwrap_err().code,
        ProtocolErrorCode::BadRequest
    );
}

#[test]
fn decimal_parsers_are_canonical_and_checked() {
    assert_eq!(parse_decimal_u64("0", "n").unwrap(), 0);
    assert_eq!(
        parse_decimal_u128(&u128::MAX.to_string(), "n").unwrap(),
        u128::MAX
    );
    assert_eq!(parse_decimal_usize("42", "n").unwrap(), 42);
    for invalid in ["", "00", "+1", "-1", " 1", "1.0"] {
        assert_eq!(
            parse_decimal_u64(invalid, "n").unwrap_err().code,
            ProtocolErrorCode::BadRequest
        );
    }
    assert_eq!(
        parse_decimal_u64("18446744073709551616", "n")
            .unwrap_err()
            .code,
        ProtocolErrorCode::BadRequest
    );
}

#[test]
fn value_and_timescale_wire_forms_preserve_variants_and_bits() {
    assert_eq!(
        serde_json::to_value(WireValue::from(&ChangeValue::Integer(10))).unwrap(),
        json!({"kind":"integer","text":"10"})
    );
    assert_eq!(
        serde_json::to_value(WireValue::from(&ChangeValue::Float(-0.0))).unwrap(),
        json!({"kind":"float","text":"-0","bits":"8000000000000000"})
    );
    assert_eq!(
        serde_json::to_value(WireValue::from(&ChangeValue::Text("Xz".into()))).unwrap(),
        json!({"kind":"text","text":"Xz"})
    );
    assert_eq!(
        serde_json::to_value(FindResult {
            found: false,
            signal: "tb.done".into(),
            time: None,
            width: "1".into(),
            value: None,
        })
        .unwrap(),
        json!({"found":false,"signal":"tb.done","time":null,"value":null,"width":"1"})
    );
    let timescale = Timescale {
        magnitude: 1,
        unit: TimescaleUnit::FS,
    };
    assert_eq!(
        serde_json::to_value(WireTimescale::from(&timescale)).unwrap(),
        json!({"magnitude":"1","unit":"fs"})
    );
}

#[test]
fn error_messages_and_details_fit_the_terminal_reserve() {
    let huge = "é".repeat(100_000);
    let error = ProtocolError::unknown_method(&huge);
    assert!(error.message.len() <= MAX_ERROR_MESSAGE_BYTES);
    assert!(error.details["method"].as_str().unwrap().len() <= MAX_ERROR_DETAIL_STRING_BYTES);
    let frame = ResponseFrame::error(Some("x".into()), 0, error);
    assert!(encoded_json_line_bytes(&frame).unwrap() < TERMINAL_FRAME_RESERVE_BYTES as usize);

    let signals = (0..100)
        .map(|index| format!("{index}-{}", "x".repeat(1_000)))
        .collect::<Vec<_>>()
        .join(", ");
    let error = protocol_error_from_vcd(&VcdError::MissingSignals(signals));
    assert!(error.details["signals"].as_array().unwrap().len() <= MAX_ERROR_DETAIL_ITEMS);
    let frame = ResponseFrame::error(Some("x".into()), 0, error);
    assert!(encoded_json_line_bytes(&frame).unwrap() < TERMINAL_FRAME_RESERVE_BYTES as usize);
}

#[test]
fn query_and_vcd_errors_map_to_frozen_protocol_codes() {
    let cases = vec![
        (QueryError::Cancelled, ProtocolErrorCode::Cancelled, false),
        (
            QueryError::DeadlineExceeded,
            ProtocolErrorCode::DeadlineExceeded,
            true,
        ),
        (
            QueryError::LimitExceeded {
                kind: QueryLimitKind::Rows,
                limit: 1,
                actual: 2,
            },
            ProtocolErrorCode::LimitExceeded,
            false,
        ),
        (
            QueryError::StaleSource(VcdError::Parse("stale".into())),
            ProtocolErrorCode::StaleSource,
            true,
        ),
        (QueryError::QueueFull, ProtocolErrorCode::QueueFull, true),
        (
            QueryError::source_unavailable(io::Error::new(io::ErrorKind::NotFound, "secret/path")),
            ProtocolErrorCode::SourceUnavailable,
            true,
        ),
        (
            QueryError::Vcd(VcdError::MissingSignal("tb.missing".into())),
            ProtocolErrorCode::SignalNotFound,
            false,
        ),
        (
            QueryError::internal("secret"),
            ProtocolErrorCode::Internal,
            false,
        ),
    ];
    for (error, code, retryable) in cases {
        let mapped = protocol_error_from_query(&error);
        assert_eq!(mapped.code, code);
        assert_eq!(mapped.retryable, retryable);
        assert!(!mapped.message.contains("secret"));
    }
    assert_eq!(
        protocol_error_from_vcd(&VcdError::MissingEndDefinitions).code,
        ProtocolErrorCode::ParseError
    );
    assert_eq!(
        protocol_error_from_vcd(&VcdError::InvalidOccurrence).code,
        ProtocolErrorCode::InvalidOccurrence
    );
}
