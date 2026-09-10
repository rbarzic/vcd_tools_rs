use std::collections::HashSet;
use std::fmt;
use std::io;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};

use crate::query::{QueryError, QueryLimitKind};
use crate::{ChangeValue, Timescale, VcdError};

pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_REQUEST_LINE_BYTES: usize = 1_048_576;
pub const MAX_REQUEST_ID_BYTES: usize = 128;
pub const TERMINAL_FRAME_RESERVE_BYTES: u64 = 4_096;
pub const CHUNK_ROWS: u64 = 1_024;
pub const CHUNK_BYTES: u64 = 262_144;
pub const MAX_ERROR_MESSAGE_BYTES: usize = 256;
pub const MAX_ERROR_DETAIL_STRING_BYTES: usize = 256;
pub const MAX_ERROR_DETAIL_ITEMS: usize = 8;
pub const MAX_ERROR_DETAIL_FIELDS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolErrorCode {
    BadRequest,
    UnsupportedVersion,
    UnknownMethod,
    DuplicateRequestId,
    SignalNotFound,
    InvalidWindow,
    InvalidOccurrence,
    LimitExceeded,
    QueueFull,
    Cancelled,
    DeadlineExceeded,
    StaleSource,
    SourceUnavailable,
    ParseError,
    Internal,
}

impl ProtocolErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "BAD_REQUEST",
            Self::UnsupportedVersion => "UNSUPPORTED_VERSION",
            Self::UnknownMethod => "UNKNOWN_METHOD",
            Self::DuplicateRequestId => "DUPLICATE_REQUEST_ID",
            Self::SignalNotFound => "SIGNAL_NOT_FOUND",
            Self::InvalidWindow => "INVALID_WINDOW",
            Self::InvalidOccurrence => "INVALID_OCCURRENCE",
            Self::LimitExceeded => "LIMIT_EXCEEDED",
            Self::QueueFull => "QUEUE_FULL",
            Self::Cancelled => "CANCELLED",
            Self::DeadlineExceeded => "DEADLINE_EXCEEDED",
            Self::StaleSource => "STALE_SOURCE",
            Self::SourceUnavailable => "SOURCE_UNAVAILABLE",
            Self::ParseError => "PARSE_ERROR",
            Self::Internal => "INTERNAL",
        }
    }
}

impl Serialize for ProtocolErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
    pub details: Value,
    pub retryable: bool,
}

impl ProtocolError {
    pub fn new(
        code: ProtocolErrorCode,
        message: impl Into<String>,
        details: Value,
        retryable: bool,
    ) -> Self {
        Self {
            code,
            message: truncate_utf8(message.into(), MAX_ERROR_MESSAGE_BYTES),
            details: bound_error_details(details, 0),
            retryable,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ProtocolErrorCode::BadRequest, message, json!({}), false)
    }

    pub fn unsupported_version() -> Self {
        Self::new(
            ProtocolErrorCode::UnsupportedVersion,
            "unsupported protocol version",
            json!({"supported_versions":["1"]}),
            false,
        )
    }

    pub fn unknown_method(method: &str) -> Self {
        Self::new(
            ProtocolErrorCode::UnknownMethod,
            "unknown method",
            json!({"method": method}),
            false,
        )
    }

    pub fn duplicate_request_id(id: &str) -> Self {
        Self::new(
            ProtocolErrorCode::DuplicateRequestId,
            "request ID is already active",
            json!({"id": id}),
            false,
        )
    }

    pub fn invalid_window() -> Self {
        Self::new(
            ProtocolErrorCode::InvalidWindow,
            "start must be less than or equal to end",
            json!({}),
            false,
        )
    }

    pub fn invalid_occurrence() -> Self {
        Self::new(
            ProtocolErrorCode::InvalidOccurrence,
            "occurrence must be >= 1",
            json!({}),
            false,
        )
    }
}

fn truncate_utf8(mut text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut boundary = max_bytes.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    text.push('…');
    text
}

fn bound_error_details(value: Value, depth: usize) -> Value {
    if depth >= 4 {
        return Value::String("…".into());
    }
    match value {
        Value::String(text) => Value::String(truncate_utf8(text, MAX_ERROR_DETAIL_STRING_BYTES)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .take(MAX_ERROR_DETAIL_ITEMS)
                .map(|value| bound_error_details(value, depth + 1))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .take(MAX_ERROR_DETAIL_FIELDS)
                .map(|(key, value)| {
                    (
                        truncate_utf8(key, MAX_ERROR_DETAIL_STRING_BYTES),
                        bound_error_details(value, depth + 1),
                    )
                })
                .collect(),
        ),
        value => value,
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ProtocolError {}

/// Incremental bounded JSON Lines accumulator.
///
/// The configured bound includes the terminating newline. Once an error is
/// returned the accumulator is poisoned because the connection should close.
#[derive(Debug)]
pub struct JsonLineAccumulator {
    buffer: Vec<u8>,
    max_line_bytes: usize,
    poisoned: bool,
}

impl Default for JsonLineAccumulator {
    fn default() -> Self {
        Self::new(MAX_REQUEST_LINE_BYTES)
    }
}

impl JsonLineAccumulator {
    pub fn new(max_line_bytes: usize) -> Self {
        assert!(max_line_bytes > 0, "JSON line bound must include a newline");
        Self {
            buffer: Vec::new(),
            max_line_bytes,
            poisoned: false,
        }
    }

    pub fn buffered_bytes(&self) -> usize {
        self.buffer.len()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, ProtocolError> {
        if self.poisoned {
            return Err(ProtocolError::bad_request(
                "JSON line accumulator is poisoned",
            ));
        }
        let mut frames = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if self.buffer.len() + 1 > self.max_line_bytes {
                    return self.poison("request line exceeds configured byte limit");
                }
                let frame = std::mem::take(&mut self.buffer);
                let text = String::from_utf8(frame)
                    .map_err(|_| ProtocolError::bad_request("request line is not valid UTF-8"));
                match text {
                    Ok(text) => frames.push(text),
                    Err(error) => {
                        self.poisoned = true;
                        return Err(error);
                    }
                }
            } else {
                // Leave room for the mandatory terminating newline.
                if self.buffer.len() >= self.max_line_bytes - 1 {
                    return self.poison("request line exceeds configured byte limit");
                }
                self.buffer.push(byte);
            }
        }
        Ok(frames)
    }

    fn poison<T>(&mut self, message: &str) -> Result<T, ProtocolError> {
        self.buffer.clear();
        self.poisoned = true;
        Err(ProtocolError::new(
            ProtocolErrorCode::LimitExceeded,
            message,
            json!({
                "kind":"request_line_bytes",
                "limit":self.max_line_bytes.to_string(),
                "actual":(self.max_line_bytes + 1).to_string()
            }),
            false,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_response_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataParams {
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_commands: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtractParams {
    pub signals: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub end: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_response_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_commands: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindParams {
    pub signal: String,
    pub value: String,
    #[serde(with = "decimal_usize")]
    pub occurrence: usize,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub end: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_commands: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_response_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TogglesParams {
    pub signals: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub end: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_commands: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub max_response_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", with = "option_decimal")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelParams {
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestMethod {
    Ping,
    Describe,
    List(ListParams),
    Metadata(MetadataParams),
    Extract(ExtractParams),
    Find(FindParams),
    Toggles(TogglesParams),
    Cancel(CancelParams),
}

impl RequestMethod {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Ping => "ping",
            Self::Describe => "describe",
            Self::List(_) => "list",
            Self::Metadata(_) => "metadata",
            Self::Extract(_) => "extract",
            Self::Find(_) => "find",
            Self::Toggles(_) => "toggles",
            Self::Cancel(_) => "cancel",
        }
    }

    fn params_value(&self) -> Value {
        match self {
            Self::Ping | Self::Describe => json!({}),
            Self::List(params) => serde_json::to_value(params).expect("serializable list params"),
            Self::Metadata(params) => {
                serde_json::to_value(params).expect("serializable metadata params")
            }
            Self::Extract(params) => {
                serde_json::to_value(params).expect("serializable extract params")
            }
            Self::Find(params) => serde_json::to_value(params).expect("serializable find params"),
            Self::Toggles(params) => {
                serde_json::to_value(params).expect("serializable toggles params")
            }
            Self::Cancel(params) => {
                serde_json::to_value(params).expect("serializable cancel params")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: String,
    pub method: RequestMethod,
}

impl Request {
    pub fn validate_inactive<F>(&self, is_active: F) -> Result<(), ProtocolError>
    where
        F: FnOnce(&str) -> bool,
    {
        if is_active(&self.id) {
            Err(ProtocolError::duplicate_request_id(&self.id))
        } else {
            Ok(())
        }
    }
}

impl Serialize for Request {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Request", 4)?;
        state.serialize_field("v", &PROTOCOL_VERSION)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("method", self.method.name())?;
        state.serialize_field("params", &self.method.params_value())?;
        state.end()
    }
}

pub fn decode_request(line: &str) -> Result<Request, ProtocolError> {
    let value = parse_strict_json(line)?;
    let mut envelope = expect_object(value, "request envelope")?;
    let version = take_required(&mut envelope, "v")?;
    match version {
        Value::Number(number) if number.as_u64() == Some(PROTOCOL_VERSION.into()) => {}
        Value::Number(_) => return Err(ProtocolError::unsupported_version()),
        _ => return Err(ProtocolError::bad_request("v must be JSON number 1")),
    }
    let id = take_string(&mut envelope, "id")?;
    validate_request_id(&id)?;
    let method_name = take_string(&mut envelope, "method")?;
    let params = expect_object(take_required(&mut envelope, "params")?, "params")?;
    // Unknown top-level envelope fields are intentionally ignored.
    let method = parse_method(&method_name, params)?;
    Ok(Request { id, method })
}

pub fn decode_request_with_active<F>(line: &str, is_active: F) -> Result<Request, ProtocolError>
where
    F: FnOnce(&str) -> bool,
{
    let request = decode_request(line)?;
    request.validate_inactive(is_active)?;
    Ok(request)
}

fn parse_method(
    name: &str,
    mut params: Map<String, Value>,
) -> Result<RequestMethod, ProtocolError> {
    let method = match name {
        "ping" => {
            reject_unknown(&params)?;
            RequestMethod::Ping
        }
        "describe" => {
            reject_unknown(&params)?;
            RequestMethod::Describe
        }
        "list" => {
            let result = ListParams {
                filter: take_optional_string(&mut params, "filter")?,
                max_rows: take_optional_decimal(&mut params, "max_rows")?,
                max_response_bytes: take_optional_decimal(&mut params, "max_response_bytes")?,
                timeout_ms: take_optional_decimal(&mut params, "timeout_ms")?,
            };
            reject_unknown(&params)?;
            validate_response_limit(result.max_response_bytes)?;
            RequestMethod::List(result)
        }
        "metadata" => {
            let result = MetadataParams {
                timeout_ms: take_optional_decimal(&mut params, "timeout_ms")?,
                max_commands: take_optional_decimal(&mut params, "max_commands")?,
            };
            reject_unknown(&params)?;
            RequestMethod::Metadata(result)
        }
        "extract" => {
            let signals = take_required_strings(&mut params, "signals")?;
            validate_signals(&signals)?;
            let result = ExtractParams {
                signals,
                start: take_optional_decimal(&mut params, "start")?,
                end: take_optional_decimal(&mut params, "end")?,
                max_rows: take_optional_decimal(&mut params, "max_rows")?,
                max_response_bytes: take_optional_decimal(&mut params, "max_response_bytes")?,
                max_commands: take_optional_decimal(&mut params, "max_commands")?,
                timeout_ms: take_optional_decimal(&mut params, "timeout_ms")?,
            };
            reject_unknown(&params)?;
            validate_window(result.start, result.end)?;
            validate_response_limit(result.max_response_bytes)?;
            RequestMethod::Extract(result)
        }
        "find" => {
            let signal = take_string(&mut params, "signal")?;
            if signal.is_empty() {
                return Err(ProtocolError::bad_request("signal must not be empty"));
            }
            let occurrence = take_optional_decimal_usize(&mut params, "occurrence")?.unwrap_or(1);
            if occurrence == 0 {
                return Err(ProtocolError::invalid_occurrence());
            }
            let result = FindParams {
                signal,
                value: take_string(&mut params, "value")?,
                occurrence,
                start: take_optional_decimal(&mut params, "start")?,
                end: take_optional_decimal(&mut params, "end")?,
                max_commands: take_optional_decimal(&mut params, "max_commands")?,
                max_response_bytes: take_optional_decimal(&mut params, "max_response_bytes")?,
                timeout_ms: take_optional_decimal(&mut params, "timeout_ms")?,
            };
            reject_unknown(&params)?;
            validate_window(result.start, result.end)?;
            validate_response_limit(result.max_response_bytes)?;
            RequestMethod::Find(result)
        }
        "toggles" => {
            let signals = take_required_strings(&mut params, "signals")?;
            validate_signals(&signals)?;
            let result = TogglesParams {
                signals,
                start: take_optional_decimal(&mut params, "start")?,
                end: take_optional_decimal(&mut params, "end")?,
                max_commands: take_optional_decimal(&mut params, "max_commands")?,
                max_response_bytes: take_optional_decimal(&mut params, "max_response_bytes")?,
                timeout_ms: take_optional_decimal(&mut params, "timeout_ms")?,
            };
            reject_unknown(&params)?;
            validate_window(result.start, result.end)?;
            validate_response_limit(result.max_response_bytes)?;
            RequestMethod::Toggles(result)
        }
        "cancel" => {
            let request_id = take_string(&mut params, "request_id")?;
            validate_request_id(&request_id)?;
            reject_unknown(&params)?;
            RequestMethod::Cancel(CancelParams { request_id })
        }
        _ => return Err(ProtocolError::unknown_method(name)),
    };
    Ok(method)
}

fn validate_request_id(id: &str) -> Result<(), ProtocolError> {
    if id.is_empty() {
        return Err(ProtocolError::bad_request("request ID must not be empty"));
    }
    if id.len() > MAX_REQUEST_ID_BYTES {
        return Err(ProtocolError::bad_request("request ID exceeds 128 bytes"));
    }
    Ok(())
}

fn validate_signals(signals: &[String]) -> Result<(), ProtocolError> {
    if signals.is_empty() {
        return Err(ProtocolError::bad_request("signals must not be empty"));
    }
    if signals.iter().any(String::is_empty) {
        return Err(ProtocolError::bad_request("signal names must not be empty"));
    }
    Ok(())
}

fn validate_window(start: Option<u64>, end: Option<u64>) -> Result<(), ProtocolError> {
    if matches!((start, end), (Some(start), Some(end)) if start > end) {
        Err(ProtocolError::invalid_window())
    } else {
        Ok(())
    }
}

fn validate_response_limit(limit: Option<u64>) -> Result<(), ProtocolError> {
    if matches!(limit, Some(limit) if limit < TERMINAL_FRAME_RESERVE_BYTES) {
        Err(ProtocolError::bad_request(
            "max_response_bytes must reserve at least 4096 terminal bytes",
        ))
    } else {
        Ok(())
    }
}

pub fn parse_decimal_u64(value: &str, field: &str) -> Result<u64, ProtocolError> {
    validate_decimal(value, field)?;
    value
        .parse()
        .map_err(|_| ProtocolError::bad_request(format!("{field} overflows u64")))
}

pub fn parse_decimal_u128(value: &str, field: &str) -> Result<u128, ProtocolError> {
    validate_decimal(value, field)?;
    value
        .parse()
        .map_err(|_| ProtocolError::bad_request(format!("{field} overflows u128")))
}

pub fn parse_decimal_usize(value: &str, field: &str) -> Result<usize, ProtocolError> {
    let number = parse_decimal_u64(value, field)?;
    usize::try_from(number)
        .map_err(|_| ProtocolError::bad_request(format!("{field} overflows usize")))
}

fn validate_decimal(value: &str, field: &str) -> Result<(), ProtocolError> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        Err(ProtocolError::bad_request(format!(
            "{field} must be a canonical unsigned decimal string"
        )))
    } else {
        Ok(())
    }
}

fn take_required(map: &mut Map<String, Value>, field: &str) -> Result<Value, ProtocolError> {
    map.remove(field)
        .ok_or_else(|| ProtocolError::bad_request(format!("missing required field {field}")))
}

fn take_string(map: &mut Map<String, Value>, field: &str) -> Result<String, ProtocolError> {
    match take_required(map, field)? {
        Value::String(value) => Ok(value),
        _ => Err(ProtocolError::bad_request(format!(
            "{field} must be a string"
        ))),
    }
}

fn take_optional_string(
    map: &mut Map<String, Value>,
    field: &str,
) -> Result<Option<String>, ProtocolError> {
    match map.remove(field) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(ProtocolError::bad_request(format!(
            "{field} must be a string"
        ))),
    }
}

fn take_optional_decimal(
    map: &mut Map<String, Value>,
    field: &str,
) -> Result<Option<u64>, ProtocolError> {
    match map.remove(field) {
        None => Ok(None),
        Some(Value::String(value)) => parse_decimal_u64(&value, field).map(Some),
        Some(_) => Err(ProtocolError::bad_request(format!(
            "{field} must be a decimal string"
        ))),
    }
}

fn take_optional_decimal_usize(
    map: &mut Map<String, Value>,
    field: &str,
) -> Result<Option<usize>, ProtocolError> {
    match map.remove(field) {
        None => Ok(None),
        Some(Value::String(value)) => parse_decimal_usize(&value, field).map(Some),
        Some(_) => Err(ProtocolError::bad_request(format!(
            "{field} must be a decimal string"
        ))),
    }
}

fn take_required_strings(
    map: &mut Map<String, Value>,
    field: &str,
) -> Result<Vec<String>, ProtocolError> {
    match take_required(map, field)? {
        Value::Array(values) => values
            .into_iter()
            .map(|value| match value {
                Value::String(value) => Ok(value),
                _ => Err(ProtocolError::bad_request(format!(
                    "{field} entries must be strings"
                ))),
            })
            .collect(),
        _ => Err(ProtocolError::bad_request(format!(
            "{field} must be an array"
        ))),
    }
}

fn reject_unknown(map: &Map<String, Value>) -> Result<(), ProtocolError> {
    if let Some(field) = map.keys().next() {
        Err(ProtocolError::bad_request(format!(
            "unknown method parameter {field}"
        )))
    } else {
        Ok(())
    }
}

fn expect_object(value: Value, name: &str) -> Result<Map<String, Value>, ProtocolError> {
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(ProtocolError::bad_request(format!(
            "{name} must be an object"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireTimescale {
    pub magnitude: String,
    pub unit: String,
}

impl From<&Timescale> for WireTimescale {
    fn from(timescale: &Timescale) -> Self {
        Self {
            magnitude: timescale.magnitude.to_string(),
            unit: timescale.unit.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WireValue {
    Integer { text: String },
    Float { text: String, bits: String },
    Text { text: String },
}

impl From<&ChangeValue> for WireValue {
    fn from(value: &ChangeValue) -> Self {
        match value {
            ChangeValue::Integer(value) => Self::Integer {
                text: value.to_string(),
            },
            ChangeValue::Float(value) => Self::Float {
                text: value.to_string(),
                bits: format!("{:016x}", value.to_bits()),
            },
            ChangeValue::Text(value) => Self::Text {
                text: value.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimeValueRow {
    pub signal: String,
    pub time: String,
    pub width: String,
    pub value: WireValue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindResult {
    pub found: bool,
    pub signal: String,
    pub time: Option<String>,
    pub width: String,
    pub value: Option<WireValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToggleRow {
    pub signal: String,
    pub toggles: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TogglesResult {
    pub rows: Vec<ToggleRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CancelResult {
    pub found: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PingResult {
    pub protocol: String,
    pub package_version: String,
    pub uptime_ms: String,
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetadataResult {
    pub signal_count: String,
    pub timescale: Option<WireTimescale>,
    pub start_time: String,
    pub end_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerLimits {
    pub connections: String,
    pub active_requests_per_connection: String,
    pub workers: String,
    pub queued_jobs: String,
    pub output_chunks_per_connection: String,
    pub request_line_bytes: String,
    pub request_id_bytes: String,
    pub signals_per_query: String,
    pub response_rows: String,
    pub response_bytes: String,
    pub commands: String,
    pub timeout_ms: String,
    pub chunk_rows: String,
    pub chunk_bytes: String,
    pub simultaneous_expensive_scans: String,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            connections: "32".into(),
            active_requests_per_connection: "8".into(),
            workers: "4".into(),
            queued_jobs: "64".into(),
            output_chunks_per_connection: "8".into(),
            request_line_bytes: MAX_REQUEST_LINE_BYTES.to_string(),
            request_id_bytes: MAX_REQUEST_ID_BYTES.to_string(),
            signals_per_query: "4096".into(),
            response_rows: "1000000".into(),
            response_bytes: "268435456".into(),
            commands: "1000000000".into(),
            timeout_ms: "120000".into(),
            chunk_rows: CHUNK_ROWS.to_string(),
            chunk_bytes: CHUNK_BYTES.to_string(),
            simultaneous_expensive_scans: "1".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecurityCapabilities {
    pub transport: String,
    pub socket_mode: String,
    pub peer_uid_check: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DistributionCapabilities {
    pub native_serve: bool,
    pub python_console_serve: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DescribeResult {
    pub protocol: String,
    pub generation: String,
    pub signal_count: String,
    pub timescale: Option<WireTimescale>,
    pub source_size: String,
    pub immutable_source: bool,
    pub methods: Vec<String>,
    pub schemas: Vec<String>,
    pub backends: Vec<String>,
    pub limits: ServerLimits,
    pub security: SecurityCapabilities,
    pub distribution: DistributionCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamStats {
    pub rows: String,
    pub encoded_bytes: String,
    pub elapsed_ms: String,
    pub commands: String,
    pub backend: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResultFrame {
    pub v: u8,
    pub id: String,
    pub seq: u32,
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    pub ok: bool,
    pub result: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorFrame {
    pub v: u8,
    pub id: Option<String>,
    pub seq: u32,
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    pub ok: bool,
    pub error: ProtocolError,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BeginFrame {
    pub v: u8,
    pub id: String,
    pub seq: u32,
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    pub schema: String,
    pub generation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChunkFrame {
    pub v: u8,
    pub id: String,
    pub seq: u32,
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    pub rows: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EndFrame {
    pub v: u8,
    pub id: String,
    pub seq: u32,
    #[serde(rename = "type")]
    pub frame_type: &'static str,
    pub complete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<StreamStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResponseFrame {
    Result(ResultFrame),
    Error(ErrorFrame),
    Begin(BeginFrame),
    Chunk(ChunkFrame),
    End(EndFrame),
}

impl ResponseFrame {
    pub fn result<T: Serialize>(id: impl Into<String>, result: &T) -> serde_json::Result<Self> {
        Ok(Self::Result(ResultFrame {
            v: PROTOCOL_VERSION,
            id: id.into(),
            seq: 0,
            frame_type: "result",
            ok: true,
            result: serde_json::to_value(result)?,
        }))
    }

    pub fn error(id: Option<String>, seq: u32, error: ProtocolError) -> Self {
        Self::Error(ErrorFrame {
            v: PROTOCOL_VERSION,
            id,
            seq,
            frame_type: "error",
            ok: false,
            error,
        })
    }

    pub fn begin(
        id: impl Into<String>,
        schema: impl Into<String>,
        generation: impl Into<String>,
    ) -> Self {
        Self::Begin(BeginFrame {
            v: PROTOCOL_VERSION,
            id: id.into(),
            seq: 0,
            frame_type: "begin",
            schema: schema.into(),
            generation: generation.into(),
        })
    }

    pub fn chunk(id: impl Into<String>, seq: u32, rows: Vec<Value>) -> Self {
        Self::Chunk(ChunkFrame {
            v: PROTOCOL_VERSION,
            id: id.into(),
            seq,
            frame_type: "chunk",
            rows,
        })
    }

    pub fn end_success(
        id: impl Into<String>,
        seq: u32,
        mut stats: StreamStats,
        prior_encoded_bytes: u64,
    ) -> serde_json::Result<Self> {
        stats.encoded_bytes = "0".into();
        let mut frame = Self::End(EndFrame {
            v: PROTOCOL_VERSION,
            id: id.into(),
            seq,
            frame_type: "end",
            complete: true,
            stats: Some(stats),
            error: None,
        });

        let mut candidate = 0_u64;
        for _ in 0..32 {
            if let Self::End(end) = &mut frame {
                end.stats
                    .as_mut()
                    .expect("successful end has stats")
                    .encoded_bytes = candidate.to_string();
            }
            let end_bytes = u64::try_from(encoded_json_line_bytes(&frame)?).map_err(|_| {
                serde_json::Error::io(io::Error::other("encoded end frame length exceeds u64"))
            })?;
            let total = prior_encoded_bytes.checked_add(end_bytes).ok_or_else(|| {
                serde_json::Error::io(io::Error::other("encoded stream byte count overflow"))
            })?;
            if total == candidate {
                return Ok(frame);
            }
            candidate = total;
        }
        Err(serde_json::Error::io(io::Error::other(
            "encoded stream byte count did not stabilize",
        )))
    }

    pub fn end_error(id: impl Into<String>, seq: u32, error: ProtocolError) -> Self {
        Self::End(EndFrame {
            v: PROTOCOL_VERSION,
            id: id.into(),
            seq,
            frame_type: "end",
            complete: false,
            stats: None,
            error: Some(error),
        })
    }
}

pub fn encode_json_line<T: Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    Ok(encoded)
}

pub fn encoded_json_line_bytes<T: Serialize>(value: &T) -> serde_json::Result<usize> {
    encode_json_line(value).map(|line| line.len())
}

pub fn cumulative_encoded_jsonl_bytes(frames: &[ResponseFrame]) -> serde_json::Result<u64> {
    frames.iter().try_fold(0_u64, |total, frame| {
        let bytes = u64::try_from(encoded_json_line_bytes(frame)?).map_err(|_| {
            serde_json::Error::io(io::Error::other("encoded frame length exceeds u64"))
        })?;
        total.checked_add(bytes).ok_or_else(|| {
            serde_json::Error::io(io::Error::other("encoded stream byte count overflow"))
        })
    })
}

pub fn protocol_error_from_query(error: &QueryError) -> ProtocolError {
    match error {
        QueryError::Cancelled => ProtocolError::new(
            ProtocolErrorCode::Cancelled,
            "query cancelled",
            json!({}),
            false,
        ),
        QueryError::DeadlineExceeded => ProtocolError::new(
            ProtocolErrorCode::DeadlineExceeded,
            "query deadline exceeded",
            json!({}),
            true,
        ),
        QueryError::LimitExceeded {
            kind,
            limit,
            actual,
        } => ProtocolError::new(
            ProtocolErrorCode::LimitExceeded,
            "query limit exceeded",
            json!({
                "kind": limit_kind_name(*kind),
                "limit": limit.to_string(),
                "actual": actual.to_string()
            }),
            false,
        ),
        QueryError::StaleSource(_) => ProtocolError::new(
            ProtocolErrorCode::StaleSource,
            "source changed during query",
            json!({}),
            true,
        ),
        QueryError::QueueFull => ProtocolError::new(
            ProtocolErrorCode::QueueFull,
            "query queue is full",
            json!({}),
            true,
        ),
        QueryError::SourceUnavailable(_) => ProtocolError::new(
            ProtocolErrorCode::SourceUnavailable,
            "source unavailable",
            json!({}),
            true,
        ),
        QueryError::Vcd(error) => protocol_error_from_vcd(error),
        QueryError::Internal(_) => ProtocolError::new(
            ProtocolErrorCode::Internal,
            "internal server error",
            json!({}),
            false,
        ),
    }
}

pub fn protocol_error_from_vcd(error: &VcdError) -> ProtocolError {
    match error {
        VcdError::MissingSignal(signal) => ProtocolError::new(
            ProtocolErrorCode::SignalNotFound,
            "one or more signals were not found",
            json!({"signals":[signal]}),
            false,
        ),
        VcdError::MissingSignals(signals) => ProtocolError::new(
            ProtocolErrorCode::SignalNotFound,
            "one or more signals were not found",
            json!({"signals":signals.split(", ").collect::<Vec<_>>()}),
            false,
        ),
        VcdError::InvalidOccurrence => ProtocolError::invalid_occurrence(),
        VcdError::Io(_) => ProtocolError::new(
            ProtocolErrorCode::SourceUnavailable,
            "source unavailable",
            json!({}),
            true,
        ),
        VcdError::MissingEndDefinitions | VcdError::DuplicateSignal(_) | VcdError::Parse(_) => {
            ProtocolError::new(
                ProtocolErrorCode::ParseError,
                "VCD parse error",
                json!({}),
                false,
            )
        }
    }
}

fn limit_kind_name(kind: QueryLimitKind) -> &'static str {
    match kind {
        QueryLimitKind::Signals => "signals",
        QueryLimitKind::Rows => "rows",
        QueryLimitKind::ResultBytes => "result_bytes",
        QueryLimitKind::Commands => "commands",
    }
}

mod decimal_usize {
    use serde::Serializer;

    pub fn serialize<S>(value: &usize, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&value.to_string())
    }
}

mod option_decimal {
    use serde::Serializer;

    pub fn serialize<S>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.serialize_some(&value.to_string()),
            None => serializer.serialize_none(),
        }
    }
}

fn parse_strict_json(input: &str) -> Result<Value, ProtocolError> {
    let mut deserializer = serde_json::Deserializer::from_str(input);
    let value = StrictValueSeed
        .deserialize(&mut deserializer)
        .map_err(|error| ProtocolError::bad_request(format!("invalid JSON: {error}")))?;
    deserializer
        .end()
        .map_err(|error| ProtocolError::bad_request(format!("invalid JSON: {error}")))?;
    Ok(value)
}

struct StrictValueSeed;

impl<'de> DeserializeSeed<'de> for StrictValueSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(StrictValueSeed)? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        let mut seen = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!("duplicate object key {key}")));
            }
            let value = map.next_value_seed(StrictValueSeed)?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
