use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::{self, Display};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use thiserror::Error;
use vcd::{Command, IdCode, Parser, ScopeItem, TimescaleUnit, Value, Var, VarType, Vector};

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

fn resolve_full_name(scope: &[String], var: &Var) -> String {
    let mut name = if scope.is_empty() {
        String::new()
    } else {
        scope.join(".") + "."
    };
    name.push_str(&var.reference);
    if let Some(idx) = &var.index {
        name.push_str(&idx.to_string());
    }
    name
}

fn collect_signals(items: &[ScopeItem], scope: &mut Vec<String>, out: &mut Vec<Signal>) {
    for item in items {
        match item {
            ScopeItem::Scope(scope_item) => {
                scope.push(scope_item.identifier.clone());
                collect_signals(&scope_item.items, scope, out);
                scope.pop();
            }
            ScopeItem::Var(var) => {
                out.push(Signal {
                    name: resolve_full_name(scope, var),
                    id_code: var.code,
                    size: var.size,
                    type_: var.var_type,
                    scope: scope.clone(),
                });
            }
            ScopeItem::Comment(_) => {}
            _ => {}
        }
    }
}

fn sanitize_header(header: &[u8]) -> String {
    fn needs_escape(ident: &str) -> bool {
        if ident.starts_with('\\') {
            return false;
        }
        if ident.is_empty() {
            return false;
        }
        let starts_ok = ident
            .chars()
            .next()
            .map(|ch| ch.is_ascii_alphabetic() || ch == '_')
            .unwrap_or(false);
        if !starts_ok {
            return true;
        }
        let allowed_chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_$().";
        ident.chars().any(|ch| !allowed_chars.contains(ch))
    }

    let text = String::from_utf8_lossy(header);
    let mut sanitized = String::with_capacity(header.len());

    for line in text.split_inclusive(['\n', '\r']) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("$scope ") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 4 && parts[0] == "$scope" {
                let ident = parts[2];
                let fixed = if needs_escape(ident) {
                    if ident.starts_with('\\') {
                        ident.to_string()
                    } else {
                        format!("\\{}", ident)
                    }
                } else {
                    ident.to_string()
                };
                let newline = if line.ends_with('\n') { "\n" } else { "" };
                let prefix_len = line.len() - trimmed.len();
                let prefix = &line[..prefix_len];
                let rebuilt = format!("{prefix}$scope {} {} $end{newline}", parts[1], fixed);
                sanitized.push_str(&rebuilt);
                continue;
            }
        }
        sanitized.push_str(line);
    }

    sanitized
}

fn read_header_bytes(path: &Path) -> Result<(Vec<u8>, u64)> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut header_bytes: Vec<u8> = Vec::new();
    let mut line_buf: Vec<u8> = Vec::new();
    let mut found = false;

    loop {
        line_buf.clear();
        let bytes = reader.read_until(b'\n', &mut line_buf)?;
        if bytes == 0 {
            break;
        }
        header_bytes.extend_from_slice(&line_buf);
        if line_buf
            .windows(b"$enddefinitions".len())
            .any(|w| w == b"$enddefinitions")
        {
            found = true;
            break;
        }
    }

    if !found {
        return Err(VcdError::MissingEndDefinitions);
    }

    let body_offset = reader.stream_position()?;
    Ok((header_bytes, body_offset))
}

fn parse_header_from_bytes(bytes: &[u8]) -> Result<(Vec<Signal>, Option<Timescale>)> {
    let sanitized = sanitize_header(bytes);
    let cursor = io::Cursor::new(sanitized.as_bytes());
    let mut parser = Parser::new(cursor);
    let header = parser.parse_header().map_err(|e| {
        let msg = format!("{}", e);
        VcdError::Parse(msg)
    })?;

    let mut signals = Vec::new();
    let mut scope = Vec::new();
    collect_signals(&header.items, &mut scope, &mut signals);
    let timescale = header.timescale.map(|(mag, unit)| Timescale {
        magnitude: mag,
        unit,
    });
    Ok((signals, timescale))
}

pub fn read_signals_with_offset(
    path: impl AsRef<Path>,
) -> Result<(Vec<Signal>, SignalIndex, Option<Timescale>, u64)> {
    let path = path.as_ref();
    let (header_bytes, offset) = read_header_bytes(path)?;
    let (signals, timescale) = parse_header_from_bytes(&header_bytes)?;
    let index = SignalIndex::build(&signals)?;
    Ok((signals, index, timescale, offset))
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

pub fn compute_time_bounds<R: BufRead>(parser: Parser<R>) -> Result<(u64, u64)> {
    let mut iter = parser;
    let mut current_time: u64 = 0;
    let mut start_time: Option<u64> = None;
    let mut end_time: Option<u64> = None;

    for cmd in &mut iter {
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

pub fn read_vcd_metadata(path: impl AsRef<Path>) -> Result<VcdMeta> {
    let path = path.as_ref();
    let (signals, _index, timescale, offset) = read_signals_with_offset(path)?;
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let reader = BufReader::new(file);
    let parser = Parser::new(reader);
    let (start, end) = compute_time_bounds(parser)?;
    Ok(VcdMeta {
        timescale,
        signal_count: signals.len(),
        start_time: start,
        end_time: end,
    })
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
    let path = path.as_ref();
    let (_signals, index, _timescale, offset) = read_signals_with_offset(path)?;
    let (target_map, missing) = build_target_map(&index, targets);
    if !missing.is_empty() {
        let list = missing.join(", ");
        return Err(VcdError::MissingSignals(list));
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let reader = BufReader::new(file);
    let parser = Parser::new(reader);
    let iter = TimeValueIter::new(parser, target_map, window);
    iter.collect()
}

pub fn find_nth_occurrence(
    path: impl AsRef<Path>,
    signal: &str,
    target_value: TargetValue,
    occurrence: usize,
    window: TimeWindow,
) -> Result<(Option<TimeValue>, u32)> {
    if occurrence < 1 {
        return Err(VcdError::InvalidOccurrence);
    }

    let path_ref = path.as_ref();
    let (signals, index, _timescale, offset) = read_signals_with_offset(path_ref)?;
    let size_map: HashMap<String, u32> = signals.iter().map(|s| (s.name.clone(), s.size)).collect();
    let (target_map, missing) = build_target_map(&index, &[signal.to_string()]);
    if !missing.is_empty() {
        return Err(VcdError::MissingSignal(missing[0].clone()));
    }

    let mut file = File::open(path_ref)?;
    file.seek(SeekFrom::Start(offset))?;
    let reader = BufReader::new(file);
    let parser = Parser::new(reader);
    let iter = TimeValueIter::new(parser, target_map, window);

    let normalized_target = target_value.normalize();
    let mut count = 0usize;
    for evt in iter {
        let evt = evt?;
        if evt.value.normalize() == normalized_target {
            count += 1;
            if count == occurrence {
                let size = size_map.get(signal).copied().unwrap_or(1);
                return Ok((Some(evt), size));
            }
        }
    }

    let size = size_map.get(signal).copied().unwrap_or(1);
    Ok((None, size))
}

pub fn tokenize_file(path: impl AsRef<Path>) -> Result<Parser<BufReader<File>>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    Ok(Parser::new(reader))
}

pub fn list_signals_from_file(path: impl AsRef<Path>, filter: Option<&str>) -> Result<Vec<String>> {
    let (signals, _index, _timescale) = read_signals(path)?;
    Ok(list_signals(&signals, filter))
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
    let path_ref = path.as_ref();
    let (signals, index, _timescale, offset) = read_signals_with_offset(path_ref)?;
    let sizes = build_sizes(&signals);
    let (target_map, missing) = build_target_map(&index, targets);
    if !missing.is_empty() {
        return Err(VcdError::MissingSignals(missing.join(", ")));
    }

    let mut toggle_counts: HashMap<String, usize> = targets.iter().map(|n| (n.clone(), 0)).collect();
    let mut last_values: HashMap<String, String> = targets.iter().map(|n| (n.clone(), String::new())).collect();

    let iter = time_value_iter_from_body(path_ref, target_map, window, offset)?;
    for evt in iter {
        let evt = evt?;
        let size = sizes.get(&evt.signal).copied().unwrap_or(1);
        let formatted = format_value_for_signal(&evt.value, size);
        if let Some(last) = last_values.get(&evt.signal) {
            if !last.is_empty() && *last != formatted {
                if let Some(count) = toggle_counts.get_mut(&evt.signal) {
                    *count += 1;
                }
            }
        }
        last_values.insert(evt.signal.clone(), formatted);
    }

    Ok(toggle_counts)
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

fn normalize_value(value: &ChangeValue) -> String {
    match value {
        ChangeValue::Integer(i) => {
            // For comparison, normalize integers without leading zeros
            i.to_string()
        }
        ChangeValue::Float(f) => f.to_string(),
        ChangeValue::Text(t) => t.to_lowercase(),  // Normalize x, z, etc.
    }
}

fn values_match(val1: &ChangeValue, val2: &ChangeValue, ignore_unknown: bool) -> bool {
    if ignore_unknown && (is_unknown_value(val1) || is_unknown_value(val2)) {
        return true;
    }

    match (val1, val2) {
        (ChangeValue::Integer(a), ChangeValue::Integer(b)) => a == b,
        (ChangeValue::Float(a), ChangeValue::Float(b)) => a.to_bits() == b.to_bits(),
        (ChangeValue::Text(a), ChangeValue::Text(b)) => a.eq_ignore_ascii_case(b),
        _ => normalize_value(val1) == normalize_value(val2),
    }
}

fn is_unknown_value(value: &ChangeValue) -> bool {
    matches!(value, ChangeValue::Text(s) if s.eq_ignore_ascii_case("x") || s.eq_ignore_ascii_case("z"))
}

fn collect_signal_values(
    file: &str,
    index: &SignalIndex,
    offset: u64,
    signals: &[String],
    window: TimeWindow,
) -> Result<HashMap<String, Vec<(u64, ChangeValue)>>> {
    let (target_map, _missing) = build_target_map(index, signals);

    let mut values: HashMap<String, Vec<(u64, ChangeValue)>> =
        HashMap::with_capacity(signals.len());
    for signal_name in signals {
        values.insert(signal_name.clone(), Vec::new());
    }

    let iter = time_value_iter_from_body(file, target_map, window, offset)?;
    for tv in iter {
        let tv = tv?;
        if let Some(entries) = values.get_mut(&tv.signal) {
            entries.push((tv.time, tv.value));
        }
    }
    Ok(values)
}

fn compare_signal_timeline(
    signal_name: &str,
    times1: &[(u64, ChangeValue)],
    times2: &[(u64, ChangeValue)],
    options: &ComparisonOptions,
    all_mismatches: &mut Vec<SignalMismatch>,
) -> usize {
    let mut i = 0usize;
    let mut j = 0usize;
    let mut curr1 = ChangeValue::Text("x".to_string());
    let mut curr2 = ChangeValue::Text("x".to_string());
    let mut signal_mismatches = 0usize;

    while i < times1.len() || j < times2.len() {
        let t1 = times1.get(i).map(|(t, _)| *t).unwrap_or(u64::MAX);
        let t2 = times2.get(j).map(|(t, _)| *t).unwrap_or(u64::MAX);
        let t = t1.min(t2);
        if t == u64::MAX {
            break;
        }

        while i < times1.len() && times1[i].0 == t {
            curr1 = times1[i].1.clone();
            i += 1;
        }
        while j < times2.len() && times2[j].0 == t {
            curr2 = times2[j].1.clone();
            j += 1;
        }

        if !values_match(&curr1, &curr2, options.ignore_unknown) {
            let is_unknown = is_unknown_value(&curr1) || is_unknown_value(&curr2);
            all_mismatches.push(SignalMismatch {
                signal_name: signal_name.to_string(),
                time: t,
                value1: curr1.clone(),
                value2: curr2.clone(),
                is_unknown,
            });
            signal_mismatches += 1;

            if let Some(max) = options.max_mismatches {
                if signal_mismatches >= max {
                    break;
                }
            }
        }
    }

    signal_mismatches
}

#[cfg(feature = "python")]
pub mod python;

pub fn compare_vcd_files(
    file1: &str,
    file2: &str,
    options: &ComparisonOptions,
) -> Result<ComparisonResult> {
    // Read both VCD files
    let (signals1, index1, _timescale1, _offset1) = read_signals_with_offset(file1)?;
    let (signals2, index2, _timescale2, _offset2) = read_signals_with_offset(file2)?;

    // Find common signals by name
    let mut common_signals = Vec::new();
    let mut signals_only_in_1 = Vec::new();
    let mut signals_only_in_2 = Vec::new();

    let signal_names1: HashSet<String> = signals1.iter().map(|s| s.name.clone()).collect();
    let signal_names2: HashSet<String> = signals2.iter().map(|s| s.name.clone()).collect();

    for sig in &signals1 {
        if signal_names2.contains(&sig.name) {
            common_signals.push(sig.name.clone());
        } else {
            signals_only_in_1.push(sig.name.clone());
        }
    }

    for sig in &signals2 {
        if !signal_names1.contains(&sig.name) {
            signals_only_in_2.push(sig.name.clone());
        }
    }

    common_signals.sort();

    // Filter to specific signals if requested
    let signals_to_compare = if options.signals_only.is_empty() {
        common_signals.clone()
    } else {
        let wanted: HashSet<&String> = options.signals_only.iter().collect();
        common_signals
            .iter()
            .filter(|s| wanted.contains(*s))
            .cloned()
            .collect()
    };

    // Build time-value maps in a single scan per file
    let values1 = collect_signal_values(
        file1,
        &index1,
        _offset1,
        &signals_to_compare,
        options.time_window,
    )?;
    let values2 = collect_signal_values(
        file2,
        &index2,
        _offset2,
        &signals_to_compare,
        options.time_window,
    )?;

    // Compare signals
    let mut all_mismatches = Vec::new();
    let mut signals_with_mismatches = 0;

    for signal_name in &signals_to_compare {
        let empty: Vec<(u64, ChangeValue)> = Vec::new();
        let times1 = values1.get(signal_name).unwrap_or(&empty);
        let times2 = values2.get(signal_name).unwrap_or(&empty);
        let signal_mismatches =
            compare_signal_timeline(signal_name, times1, times2, options, &mut all_mismatches);

        if signal_mismatches > 0 {
            signals_with_mismatches += 1;
        }
    }

    let passed = all_mismatches.is_empty();
    let total_mismatches = all_mismatches.len();

    Ok(ComparisonResult {
        file1: file1.to_string(),
        file2: file2.to_string(),
        common_signals,
        signals_only_in_file1: signals_only_in_1,
        signals_only_in_file2: signals_only_in_2,
        mismatches: all_mismatches,
        total_mismatches,
        signals_with_mismatches,
        passed,
    })
}
