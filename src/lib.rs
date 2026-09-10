mod catalog;
mod header;
pub mod opened;
pub mod query;
pub mod server;

pub use opened::{
    CatalogMemoryUsage, ContentFingerprint, FileIdentity, FingerprintPolicy, GenerationId,
    OpenOptions, OpenedBodyReader, OpenedVcd, SignalRef,
};

use std::collections::{HashMap, VecDeque};
use std::fmt::{self, Display};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use thiserror::Error;
use vcd::{Command, IdCode, Parser, TimescaleUnit, Value, VarType, Vector};

#[derive(Debug, Error)]
pub enum VcdError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("missing $enddefinitions marker in VCD file")]
    MissingEndDefinitions,
    #[error("duplicate signal name detected: {0}")]
    DuplicateSignal(String),
    #[error("signals not found in VCD: {0}")]
    MissingSignals(String),
    #[error("signal not found in VCD: {0}")]
    MissingSignal(String),
    #[error("occurrence must be >= 1")]
    InvalidOccurrence,
    #[error("unexpected parse error: {0}")]
    Parse(String),
}

pub type Result<T> = std::result::Result<T, VcdError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timescale {
    pub magnitude: u32,
    pub unit: TimescaleUnit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub name: String,
    pub id_code: IdCode,
    pub size: u32,
    pub type_: VarType,
    pub scope: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalIndex {
    pub by_id: HashMap<IdCode, Vec<Signal>>, // a code can map to several names
    pub by_name: HashMap<String, Signal>,
}

impl SignalIndex {
    pub fn build(signals: &[Signal]) -> Result<Self> {
        let mut by_id: HashMap<IdCode, Vec<Signal>> = HashMap::new();
        let mut by_name: HashMap<String, Signal> = HashMap::new();
        for sig in signals.iter().cloned() {
            by_id.entry(sig.id_code).or_default().push(sig.clone());
            if by_name.contains_key(&sig.name) {
                return Err(VcdError::DuplicateSignal(sig.name));
            }
            by_name.insert(sig.name.clone(), sig);
        }
        Ok(Self { by_id, by_name })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimeWindow {
    pub start: Option<u64>,
    pub end: Option<u64>,
}

impl TimeWindow {
    pub fn contains(&self, time: u64) -> bool {
        if let Some(start) = self.start {
            if time < start {
                return false;
            }
        }
        if let Some(end) = self.end {
            if time > end {
                return false;
            }
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeValue {
    Integer(u128),
    Float(f64),
    Text(String),
}

impl ChangeValue {
    pub fn normalize(&self) -> String {
        match self {
            ChangeValue::Integer(i) => i.to_string(),
            ChangeValue::Float(f) => f.to_string(),
            ChangeValue::Text(t) => t.to_lowercase(),
        }
    }

    pub fn as_integer(&self) -> Option<u128> {
        match self {
            ChangeValue::Integer(i) => Some(*i),
            _ => None,
        }
    }
}

impl Display for ChangeValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChangeValue::Integer(i) => write!(f, "{}", i),
            ChangeValue::Float(fl) => write!(f, "{}", fl),
            ChangeValue::Text(t) => write!(f, "{}", t),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimeValue {
    pub signal: String,
    pub time: u64,
    pub value: ChangeValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VcdMeta {
    pub timescale: Option<Timescale>,
    pub signal_count: usize,
    pub start_time: u64,
    pub end_time: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TargetValue {
    Integer(u128),
    Text(String),
}

impl TargetValue {
    pub fn normalize(&self) -> String {
        match self {
            TargetValue::Integer(i) => i.to_string(),
            TargetValue::Text(t) => t.to_lowercase(),
        }
    }
}

pub fn normalize_change_value(value: &ChangeValue) -> String {
    value.normalize()
}

pub fn parse_target_value(raw: &str) -> TargetValue {
    let text = raw.trim().to_lowercase();
    if let Some(hex) = text.strip_prefix("0x") {
        if let Ok(v) = u128::from_str_radix(hex, 16) {
            return TargetValue::Integer(v);
        }
    }
    if let Ok(v) = text.parse::<u128>() {
        return TargetValue::Integer(v);
    }
    TargetValue::Text(text)
}

pub fn read_signals_with_offset(
    path: impl AsRef<Path>,
) -> Result<(Vec<Signal>, SignalIndex, Option<Timescale>, u64)> {
    let parsed = header::read_compact_header(path)?;
    let (signals, index) = parsed.catalog.into_compatibility_parts()?;
    Ok((signals, index, parsed.timescale, parsed.body_offset))
}

pub fn read_signals(
    path: impl AsRef<Path>,
) -> Result<(Vec<Signal>, SignalIndex, Option<Timescale>)> {
    let (signals, index, timescale, _) = read_signals_with_offset(path)?;
    Ok((signals, index, timescale))
}

pub fn list_signals(signals: &[Signal], name_filter: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = signals.iter().map(|s| s.name.clone()).collect();
    if let Some(filter) = name_filter {
        names.retain(|n| n.contains(filter));
    }
    names
}

pub fn load_signal_list(path: impl AsRef<Path>) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)?;
    let names = content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|s| s.split('#').next().unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Ok(names)
}

fn vector_to_change_value(vec: &Vector) -> ChangeValue {
    if vec.iter().all(|v| matches!(v, Value::V0 | Value::V1)) && vec.len() <= 128 {
        let mut acc: u128 = 0;
        for v in vec.iter() {
            acc = (acc << 1)
                | match v {
                    Value::V0 => 0,
                    Value::V1 => 1,
                    _ => 0,
                } as u128;
        }
        ChangeValue::Integer(acc)
    } else {
        ChangeValue::Text(vec.to_string())
    }
}

fn command_to_change_value(cmd: Command) -> Option<(IdCode, ChangeValue)> {
    match cmd {
        Command::ChangeScalar(id, value) => Some((id, ChangeValue::Text(value.to_string()))),
        Command::ChangeVector(id, vec) => Some((id, vector_to_change_value(&vec))),
        Command::ChangeReal(id, value) => Some((id, ChangeValue::Float(value))),
        Command::ChangeString(id, value) => Some((id, ChangeValue::Text(value))),
        _ => None,
    }
}

pub struct TimeValueIter<R: BufRead> {
    parser: Parser<R>,
    target_map: HashMap<IdCode, Vec<String>>,
    window: TimeWindow,
    current_time: u64,
    finished: bool,
    pending: VecDeque<TimeValue>,
}

impl<R: BufRead> TimeValueIter<R> {
    pub fn new(
        parser: Parser<R>,
        target_map: HashMap<IdCode, Vec<String>>,
        window: TimeWindow,
    ) -> Self {
        Self {
            parser,
            target_map,
            window,
            current_time: 0,
            finished: false,
            pending: VecDeque::new(),
        }
    }
}

impl<R: BufRead> Iterator for TimeValueIter<R> {
    type Item = Result<TimeValue>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(tv) = self.pending.pop_front() {
            return Some(Ok(tv));
        }

        if self.finished {
            return None;
        }

        while let Some(result) = self.parser.next() {
            match result {
                Ok(Command::Timestamp(t)) => {
                    self.current_time = t;
                    if let Some(end) = self.window.end {
                        if t > end {
                            self.finished = true;
                            return None;
                        }
                    }
                }
                Ok(cmd) => {
                    if let Some((id, value)) = command_to_change_value(cmd) {
                        if let Some(names) = self.target_map.get(&id) {
                            if !self.window.contains(self.current_time) {
                                if let Some(end) = self.window.end {
                                    if self.current_time > end {
                                        self.finished = true;
                                        return None;
                                    }
                                }
                                continue;
                            }
                            for name in names {
                                self.pending.push_back(TimeValue {
                                    signal: name.clone(),
                                    time: self.current_time,
                                    value: value.clone(),
                                });
                            }
                            if let Some(tv) = self.pending.pop_front() {
                                return Some(Ok(tv));
                            }
                        }
                    }
                }
                Err(e) => return Some(Err(e.into())),
            }
        }

        self.finished = true;
        None
    }
}

fn compute_time_bounds_in_place<R: BufRead>(parser: &mut Parser<R>) -> Result<(u64, u64)> {
    let mut current_time: u64 = 0;
    let mut start_time: Option<u64> = None;
    let mut end_time: Option<u64> = None;

    for cmd in parser {
        match cmd.map_err(VcdError::from)? {
            Command::Timestamp(t) => {
                current_time = t;
                start_time.get_or_insert(t);
                end_time = Some(t);
            }
            Command::ChangeScalar(_, _)
            | Command::ChangeVector(_, _)
            | Command::ChangeReal(_, _)
            | Command::ChangeString(_, _) => {
                start_time.get_or_insert(current_time);
                end_time = Some(current_time);
            }
            _ => {}
        }
    }

    let start = start_time.unwrap_or(0);
    let end = end_time.unwrap_or(start);
    Ok((start, end))
}

pub fn compute_time_bounds<R: BufRead>(mut parser: Parser<R>) -> Result<(u64, u64)> {
    compute_time_bounds_in_place(&mut parser)
}

pub fn read_vcd_metadata(path: impl AsRef<Path>) -> Result<VcdMeta> {
    let opened = opened::OpenedVcd::open(path)?;
    opened
        .metadata()
        .map(|metadata| metadata.as_ref().clone())
}

pub fn build_target_map(
    index: &SignalIndex,
    targets: &[String],
) -> (HashMap<IdCode, Vec<String>>, Vec<String>) {
    let mut missing = Vec::new();
    let mut mapping: HashMap<IdCode, Vec<String>> = HashMap::new();
    for name in targets {
        if let Some(sig) = index.by_name.get(name) {
            mapping
                .entry(sig.id_code)
                .or_default()
                .push(sig.name.clone());
        } else {
            missing.push(name.clone());
        }
    }
    (mapping, missing)
}

pub fn extract_time_values_from_file(
    path: impl AsRef<Path>,
    targets: &[String],
    window: TimeWindow,
) -> Result<Vec<TimeValue>> {
    let opened = opened::OpenedVcd::open(path)?;
    opened
        .extract(targets, window, &query::QueryContext::legacy_unlimited())
        .map_err(query::into_vcd_error)?
        .collect::<query::QueryResult<Vec<_>>>()
        .map_err(query::into_vcd_error)
}

pub fn find_nth_occurrence(
    path: impl AsRef<Path>,
    signal: &str,
    target_value: TargetValue,
    occurrence: usize,
    window: TimeWindow,
) -> Result<(Option<TimeValue>, u32)> {
    let opened = opened::OpenedVcd::open(path)?;
    opened
        .find_nth_occurrence(
            signal,
            target_value,
            occurrence,
            window,
            &query::QueryContext::legacy_unlimited(),
        )
        .map_err(query::into_vcd_error)
}

pub fn tokenize_file(path: impl AsRef<Path>) -> Result<Parser<BufReader<File>>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    Ok(Parser::new(reader))
}

pub fn list_signals_from_file(path: impl AsRef<Path>, filter: Option<&str>) -> Result<Vec<String>> {
    Ok(opened::OpenedVcd::open(path)?.list_signals(filter))
}

pub fn time_value_iter_from_body(
    path: impl AsRef<Path>,
    target_map: HashMap<IdCode, Vec<String>>,
    window: TimeWindow,
    offset: u64,
) -> Result<TimeValueIter<BufReader<File>>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let reader = BufReader::new(file);
    let parser = Parser::new(reader);
    Ok(TimeValueIter::new(parser, target_map, window))
}

pub fn format_value_for_signal(value: &ChangeValue, signal_size: u32) -> String {
    if signal_size > 1 {
        if let Some(int_val) = value.as_integer() {
            let width = ((signal_size + 3) / 4) as usize;
            return format!("0x{int_val:0width$x}");
        }
    }
    value.to_string()
}

/// Format a ChangeValue for display (simple conversion)
pub fn format_change_value(value: &ChangeValue) -> String {
    value.to_string()
}

pub fn build_sizes(signals: &[Signal]) -> HashMap<String, u32> {
    signals.iter().map(|s| (s.name.clone(), s.size)).collect()
}

pub fn count_toggles(
    path: impl AsRef<Path>,
    targets: &[String],
    window: TimeWindow,
) -> Result<HashMap<String, usize>> {
    let opened = opened::OpenedVcd::open(path)?;
    opened
        .count_toggles(
            targets,
            window,
            &query::QueryContext::legacy_unlimited(),
        )
        .map_err(query::into_vcd_error)
}

// ============================================================================
// VCD Comparison
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct SignalMismatch {
    pub signal_name: String,
    pub time: u64,
    pub value1: ChangeValue,
    pub value2: ChangeValue,
    pub is_unknown: bool,  // true if one value is 'x' or 'z'
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonOptions {
    pub max_mismatches: Option<usize>,  // Limit number of mismatches per signal
    pub signals_only: Vec<String>,      // Only compare these signals
    pub ignore_unknown: bool,            // Treat x/z differences as matches
    pub time_window: TimeWindow,          // Only compare within this time range
}

impl Default for ComparisonOptions {
    fn default() -> Self {
        Self {
            max_mismatches: None,
            signals_only: Vec::new(),
            ignore_unknown: false,
            time_window: TimeWindow { start: None, end: None },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonResult {
    pub file1: String,
    pub file2: String,
    pub common_signals: Vec<String>,
    pub signals_only_in_file1: Vec<String>,
    pub signals_only_in_file2: Vec<String>,
    pub mismatches: Vec<SignalMismatch>,
    pub total_mismatches: usize,
    pub signals_with_mismatches: usize,
    pub passed: bool,
}

impl ComparisonResult {
    pub fn get_summary(&self) -> String {
        if self.passed {
            format!("✅ PASS - All {} common signals match", self.common_signals.len())
        } else {
            format!(
                "❌ FAIL - {}/{} signals have mismatches ({} total mismatches)",
                self.signals_with_mismatches,
                self.common_signals.len(),
                self.total_mismatches
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct JsonComparisonResult {
    pub file1: String,
    pub file2: String,
    pub common_signal_count: usize,
    pub signals_only_in_file1: Vec<String>,
    pub signals_only_in_file2: Vec<String>,
    pub mismatches_by_signal: std::collections::HashMap<String, usize>,
    pub total_mismatches: usize,
    pub signals_with_mismatches: usize,
    pub passed: bool,
}

impl From<&ComparisonResult> for JsonComparisonResult {
    fn from(result: &ComparisonResult) -> Self {
        let mut mismatches_by_signal = std::collections::HashMap::new();
        for mismatch in &result.mismatches {
            *mismatches_by_signal.entry(mismatch.signal_name.clone()).or_insert(0) += 1;
        }

        Self {
            file1: result.file1.clone(),
            file2: result.file2.clone(),
            common_signal_count: result.common_signals.len(),
            signals_only_in_file1: result.signals_only_in_file1.clone(),
            signals_only_in_file2: result.signals_only_in_file2.clone(),
            mismatches_by_signal,
            total_mismatches: result.total_mismatches,
            signals_with_mismatches: result.signals_with_mismatches,
            passed: result.passed,
        }
    }
}

#[cfg(feature = "python")]
pub mod python;

pub fn compare_vcd_files(
    file1: &str,
    file2: &str,
    options: &ComparisonOptions,
) -> Result<ComparisonResult> {
    let first = opened::OpenedVcd::open(file1)?;
    let second = opened::OpenedVcd::open(file2)?;
    first
        .compare(
            &second,
            options,
            &query::QueryContext::legacy_unlimited(),
        )
        .map_err(query::into_vcd_error)
}
