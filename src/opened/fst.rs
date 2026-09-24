use std::collections::{HashMap, HashSet};
use std::fs::{File, Metadata};
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use fst_reader::{
    FstFilter, FstHierarchyEntry, FstReader, FstSignalHandle, FstSignalValue, FstVarType,
    ReadSignalsError,
};
use thiserror::Error;

use crate::opened::GenerationId;
use crate::query::{QueryContext, QueryError, QueryLimitKind, QueryResult, logical_event_bytes};
use crate::{ChangeValue, TimeValue, TimeWindow, Timescale};
use vcd::TimescaleUnit;

const MAX_ERROR_BYTES: usize = 512;
const MAX_FST_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_HIERARCHY_DEPTH: usize = 1_024;
const MAX_DECLARATIONS: usize = 1_000_000;
const MAX_NAME_BYTES: usize = 512 * 1024 * 1024;
const MAX_CATALOG_BYTES: usize = 1024 * 1024 * 1024;
const MAX_HIERARCHY_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_HIERARCHY_COMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
const FST_GZIP_WRAPPER_BLOCK: u8 = 254;

#[derive(Debug, Error)]
pub enum FstError {
    #[error("failed to access FST {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("FST parser error: {0}")]
    Parse(String),
    #[error("duplicate FST signal name: {0}")]
    DuplicateSignal(String),
    #[error("FST signal not found: {0}")]
    MissingSignal(String),
    #[error("unsupported FST input: {0}")]
    UnsupportedInput(String),
    #[error("unsupported FST value: {0}")]
    UnsupportedValue(String),
    #[error("FST resource limit exceeded for {kind}: {actual} > {limit}")]
    ResourceLimit {
        kind: &'static str,
        limit: usize,
        actual: usize,
    },
    #[error("FST source changed since it was opened")]
    StaleSource,
    #[error("FST parser panicked")]
    ParserPanicked,
}

fn read_be_u64(file: &mut File, path: &Path) -> Result<u64, FstError> {
    let mut bytes = [0u8; 8];
    file.read_exact(&mut bytes).map_err(|source| FstError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(u64::from_be_bytes(bytes))
}

fn read_variant_u64(file: &mut File, path: &Path) -> Result<u64, FstError> {
    let mut value = 0u64;
    for index in 0..10 {
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte).map_err(|source| FstError::Io {
            path: path.display().to_string(),
            source,
        })?;
        value |= u64::from(byte[0] & 0x7f) << (7 * index);
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(FstError::Parse(
        "invalid FST hierarchy length encoding".into(),
    ))
}

/// Validate top-level section bounds and compressed hierarchy expansion before
/// `fst-reader` allocates its hierarchy buffer.
fn preflight_hierarchy(
    file: &mut File,
    file_len: u64,
    limit: u64,
    path: &Path,
) -> Result<(), FstError> {
    file.seek(SeekFrom::Start(0))
        .map_err(|source| FstError::Io {
            path: path.display().to_string(),
            source,
        })?;
    loop {
        let mut kind = [0u8; 1];
        match file.read(&mut kind) {
            Ok(0) => break,
            Ok(_) => {}
            Err(source) => {
                return Err(FstError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        }
        let section_start = file.stream_position().map_err(|source| FstError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let section_length = read_be_u64(file, path)?;
        if kind[0] == 255 && section_length == 0 {
            break;
        }
        if section_length < 8 {
            return Err(FstError::Parse("invalid FST section length".into()));
        }
        let section_end = section_start
            .checked_add(section_length)
            .ok_or_else(|| FstError::Parse("FST section length overflow".into()))?;
        if section_end > file_len {
            return Err(FstError::Parse("FST section extends beyond input".into()));
        }
        if matches!(kind[0], 4 | 6 | 7) {
            if section_length > MAX_HIERARCHY_COMPRESSED_BYTES {
                return Err(FstError::ResourceLimit {
                    kind: "hierarchy_compressed_bytes",
                    limit: MAX_HIERARCHY_COMPRESSED_BYTES as usize,
                    actual: usize::try_from(section_length).unwrap_or(usize::MAX),
                });
            }
            if section_length < 16 {
                return Err(FstError::Parse(
                    "compressed FST hierarchy header is truncated".into(),
                ));
            }
            let uncompressed = read_be_u64(file, path)?;
            if uncompressed > limit {
                return Err(FstError::ResourceLimit {
                    kind: "hierarchy_uncompressed_bytes",
                    limit: usize::try_from(limit).unwrap_or(usize::MAX),
                    actual: usize::try_from(uncompressed).unwrap_or(usize::MAX),
                });
            }
            if kind[0] == 7 {
                let intermediate = read_variant_u64(file, path)?;
                if intermediate > limit {
                    return Err(FstError::ResourceLimit {
                        kind: "hierarchy_intermediate_bytes",
                        limit: usize::try_from(limit).unwrap_or(usize::MAX),
                        actual: usize::try_from(intermediate).unwrap_or(usize::MAX),
                    });
                }
            }
        }
        file.seek(SeekFrom::Start(section_end))
            .map_err(|source| FstError::Io {
                path: path.display().to_string(),
                source,
            })?;
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|source| FstError::Io {
            path: path.display().to_string(),
            source,
        })?;
    Ok(())
}

fn bounded(error: impl std::fmt::Display) -> String {
    let text = error.to_string();
    if text.len() <= MAX_ERROR_BYTES {
        return text;
    }
    let mut end = MAX_ERROR_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FstSignalType {
    Digital,
    Real,
    GenericString,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FstSignal {
    pub name: String,
    pub handle_index: usize,
    pub width: u32,
    pub signal_type: FstSignalType,
    pub is_alias: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FstMeta {
    pub start_time: u64,
    pub end_time: u64,
    pub signal_count: usize,
    pub unique_signal_count: usize,
    pub timescale_exponent: i8,
    pub timescale: Option<Timescale>,
    pub version: String,
    pub date: String,
}

#[derive(Debug, Clone)]
struct FstIdentity {
    len: u64,
    modified: Option<SystemTime>,
    sample: blake3::Hash,
}

impl FstIdentity {
    fn from_file(file: &mut File, metadata: &Metadata) -> Result<Self, std::io::Error> {
        let len = metadata.len();
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        file.seek(SeekFrom::Start(0))?;
        let count = file.read(&mut buffer)?;
        hasher.update(&buffer[..count]);
        if len > buffer.len() as u64 {
            file.seek(SeekFrom::Start(len - buffer.len() as u64))?;
            let count = file.read(&mut buffer)?;
            hasher.update(&buffer[..count]);
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(Self {
            len,
            modified: metadata.modified().ok(),
            sample: hasher.finalize(),
        })
    }
    fn matches_file(&self, file: &mut File, metadata: &Metadata) -> Result<bool, std::io::Error> {
        let current = Self::from_file(file, metadata)?;
        Ok(self.len == current.len
            && self.modified == current.modified
            && self.sample == current.sample)
    }
}

enum FstCallbackError {
    Query(QueryError),
    Stop,
}

impl From<QueryError> for FstCallbackError {
    fn from(error: QueryError) -> Self {
        Self::Query(error)
    }
}

#[derive(Debug, Clone)]
pub struct OpenedFst {
    display_path: PathBuf,
    configured_path: PathBuf,
    generation: GenerationId,
    identity: FstIdentity,
    meta: FstMeta,
    signals: Vec<FstSignal>,
    by_name: HashMap<String, usize>,
}

impl OpenedFst {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, FstError> {
        Self::open_with_hierarchy_limit(path, MAX_HIERARCHY_UNCOMPRESSED_BYTES)
    }

    fn open_with_hierarchy_limit(
        path: impl AsRef<Path>,
        hierarchy_limit: u64,
    ) -> Result<Self, FstError> {
        let display_path = path.as_ref().to_path_buf();
        let configured_path = if display_path.is_absolute() {
            display_path.clone()
        } else {
            std::env::current_dir()
                .map_err(|source| FstError::Io {
                    path: display_path.display().to_string(),
                    source,
                })?
                .join(&display_path)
        };
        let mut file = File::open(&configured_path).map_err(|source| FstError::Io {
            path: display_path.display().to_string(),
            source,
        })?;
        let metadata = file.metadata().map_err(|source| FstError::Io {
            path: display_path.display().to_string(),
            source,
        })?;
        if metadata.len() > MAX_FST_BYTES {
            return Err(FstError::UnsupportedInput(format!(
                "input exceeds {} byte limit",
                MAX_FST_BYTES
            )));
        }
        let identity =
            FstIdentity::from_file(&mut file, &metadata).map_err(|source| FstError::Io {
                path: display_path.display().to_string(),
                source,
            })?;
        let mut first = [0_u8; 1];
        file.read_exact(&mut first).map_err(|source| FstError::Io {
            path: display_path.display().to_string(),
            source,
        })?;
        file.seek(SeekFrom::Start(0))
            .map_err(|source| FstError::Io {
                path: display_path.display().to_string(),
                source,
            })?;
        if first[0] == FST_GZIP_WRAPPER_BLOCK {
            return Err(FstError::UnsupportedInput(
                "whole-file gzip wrappers are disabled".into(),
            ));
        }
        preflight_hierarchy(&mut file, metadata.len(), hierarchy_limit, &display_path)?;
        let mut reader = catch_unwind(AssertUnwindSafe(|| FstReader::open(BufReader::new(file))))
            .map_err(|_| FstError::ParserPanicked)?
            .map_err(|error| FstError::Parse(bounded(error)))?;
        let header = reader.get_header();
        let mut scopes = Vec::new();
        let mut signals = Vec::new();
        let mut hierarchy_error = None;
        let mut names = HashSet::new();
        let mut name_bytes = 0usize;
        catch_unwind(AssertUnwindSafe(|| {
            reader.read_hierarchy(|entry| {
                if hierarchy_error.is_some() {
                    return;
                }
                match entry {
                    FstHierarchyEntry::Scope {
                        name, component, ..
                    } => {
                        let component = if component.is_empty() {
                            name
                        } else {
                            component
                        };
                        if let Err(error) = ensure_resource(
                            "hierarchy_depth",
                            scopes.len() + 1,
                            MAX_HIERARCHY_DEPTH,
                        ) {
                            hierarchy_error = Some(error);
                            return;
                        }
                        if let Err(error) = charge_resource(
                            "name_bytes",
                            &mut name_bytes,
                            component.len(),
                            MAX_NAME_BYTES,
                        ) {
                            hierarchy_error = Some(error);
                            return;
                        }
                        scopes.push(component);
                    }
                    FstHierarchyEntry::UpScope => {
                        scopes.pop();
                    }
                    FstHierarchyEntry::Var {
                        tpe,
                        name,
                        length,
                        handle,
                        is_alias,
                        ..
                    } => {
                        if let Err(error) =
                            ensure_resource("declarations", signals.len() + 1, MAX_DECLARATIONS)
                        {
                            hierarchy_error = Some(error);
                            return;
                        }
                        let full_len = name.len().saturating_add(
                            scopes.iter().map(|part| part.len() + 1).sum::<usize>(),
                        );
                        if let Err(error) =
                            charge_resource("name_bytes", &mut name_bytes, full_len, MAX_NAME_BYTES)
                        {
                            hierarchy_error = Some(error);
                            return;
                        }
                        let estimated = name_bytes.saturating_add(
                            (signals.len() + 1)
                                .saturating_mul(std::mem::size_of::<FstSignal>() + 64),
                        );
                        if let Err(error) =
                            ensure_resource("catalog_bytes", estimated, MAX_CATALOG_BYTES)
                        {
                            hierarchy_error = Some(error);
                            return;
                        }
                        let full_name = if scopes.is_empty() {
                            name
                        } else {
                            format!("{}.{}", scopes.join("."), name)
                        };
                        if !names.insert(full_name.clone()) {
                            hierarchy_error = Some(FstError::DuplicateSignal(full_name));
                            return;
                        }
                        signals.push(FstSignal {
                            name: full_name,
                            handle_index: handle.get_index(),
                            width: length,
                            signal_type: signal_type(tpe),
                            is_alias,
                        });
                    }
                    _ => {}
                }
            })
        }))
        .map_err(|_| FstError::ParserPanicked)?
        .map_err(|error| FstError::Parse(bounded(error)))?;
        if let Some(error) = hierarchy_error {
            return Err(error);
        }
        let by_name = signals
            .iter()
            .enumerate()
            .map(|(index, signal)| (signal.name.clone(), index))
            .collect();
        let unique_signal_count = signals
            .iter()
            .map(|signal| signal.handle_index)
            .collect::<HashSet<_>>()
            .len();
        Ok(Self {
            display_path,
            configured_path,
            generation: GenerationId::fresh(),
            identity,
            meta: FstMeta {
                start_time: header.start_time,
                end_time: header.end_time,
                signal_count: signals.len(),
                unique_signal_count,
                timescale_exponent: header.timescale_exponent,
                timescale: timescale_from_exponent(header.timescale_exponent),
                version: header.version,
                date: header.date,
            },
            signals,
            by_name,
        })
    }

    pub fn path(&self) -> &Path {
        &self.display_path
    }
    pub fn configured_path(&self) -> &Path {
        &self.configured_path
    }
    pub fn generation(&self) -> GenerationId {
        self.generation
    }
    pub fn source_size(&self) -> u64 {
        self.identity.len
    }
    pub fn metadata(&self) -> &FstMeta {
        &self.meta
    }
    pub fn timescale(&self) -> Option<&Timescale> {
        self.meta.timescale.as_ref()
    }
    pub fn signals(&self) -> &[FstSignal] {
        &self.signals
    }
    pub fn signal(&self, name: &str) -> Option<&FstSignal> {
        self.by_name.get(name).map(|index| &self.signals[*index])
    }
    pub fn signal_count(&self) -> usize {
        self.signals.len()
    }
    pub fn signal_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.signals.iter().map(|signal| signal.name.as_str())
    }

    pub fn validate_source(&self) -> Result<(), FstError> {
        let mut file = File::open(&self.configured_path).map_err(|source| FstError::Io {
            path: self.display_path.display().to_string(),
            source,
        })?;
        let metadata = file.metadata().map_err(|source| FstError::Io {
            path: self.display_path.display().to_string(),
            source,
        })?;
        if self
            .identity
            .matches_file(&mut file, &metadata)
            .map_err(|source| FstError::Io {
                path: self.display_path.display().to_string(),
                source,
            })?
        {
            Ok(())
        } else {
            Err(FstError::StaleSource)
        }
    }

    pub fn list_signals(&self, filter: Option<&str>) -> Vec<String> {
        self.signal_names()
            .filter(|name| filter.is_none_or(|needle| name.contains(needle)))
            .map(str::to_string)
            .collect()
    }

    pub fn extract(
        &self,
        targets: &[String],
        window: TimeWindow,
    ) -> Result<Vec<TimeValue>, FstError> {
        self.extract_with_context(targets, window, &QueryContext::legacy_unlimited())
            .map_err(query_to_fst)
    }

    pub fn extract_with_context(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
    ) -> QueryResult<Vec<TimeValue>> {
        let mut rows = Vec::new();
        self.visit_changes_with_context(targets, window, context, |event| {
            rows.push(event);
            Ok(())
        })?;
        Ok(rows)
    }

    /// Visit selected changes synchronously without materializing the result.
    pub fn visit_changes_with_context(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
        mut visitor: impl FnMut(TimeValue) -> QueryResult<()>,
    ) -> QueryResult<()> {
        self.visit_changes_until_with_context(targets, window, context, |event| {
            visitor(event)?;
            Ok(true)
        })
    }

    pub(crate) fn visit_changes_until_with_context(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
        mut visitor: impl FnMut(TimeValue) -> QueryResult<bool>,
    ) -> QueryResult<()> {
        context.check_signal_count(targets.len())?;
        if window
            .start
            .zip(window.end)
            .is_some_and(|(start, end)| start > end)
        {
            return Ok(());
        }
        let mut handles = Vec::new();
        let mut names_by_handle: HashMap<usize, Vec<String>> = HashMap::new();
        let mut type_by_handle = HashMap::new();
        for target in targets {
            let signal = self
                .signal(target)
                .ok_or_else(|| QueryError::SignalNotFound(target.clone()))?;
            if !handles.contains(&signal.handle_index) {
                handles.push(signal.handle_index);
            }
            type_by_handle.insert(signal.handle_index, signal.signal_type);
            names_by_handle
                .entry(signal.handle_index)
                .or_default()
                .push(target.clone());
        }
        context.check()?;
        self.validate_source().map_err(fst_to_query)?;
        let file = File::open(&self.configured_path).map_err(QueryError::source_unavailable)?;
        let mut reader = catch_unwind(AssertUnwindSafe(|| FstReader::open(BufReader::new(file))))
            .map_err(|_| QueryError::Fst("FST parser panicked while reopening source".into()))?
            .map_err(|error| QueryError::Fst(bounded(error)))?;
        let start = window.start.unwrap_or(0);
        let end = window.end.unwrap_or(self.meta.end_time);
        let filter = FstFilter {
            start,
            end: Some(end),
            include: Some(
                handles
                    .into_iter()
                    .map(FstSignalHandle::from_index)
                    .collect(),
            ),
        };
        let mut row_count = 0_u64;
        let mut work = 0_u64;
        let mut bytes = 0_u64;
        let result = catch_unwind(AssertUnwindSafe(|| {
            reader.read_signals(
                &filter,
                |time, handle, value| -> Result<(), FstCallbackError> {
                    work = work.saturating_add(1);
                    context.check()?;
                    if let Some(limit) = context.limits().max_commands()
                        && work > limit
                    {
                        return Err(FstCallbackError::Query(QueryError::LimitExceeded {
                            kind: QueryLimitKind::Commands,
                            limit,
                            actual: work,
                        }));
                    }
                    if time < start || time > end {
                        return Ok(());
                    }
                    let Some(names) = names_by_handle.get(&handle.get_index()) else {
                        return Ok(());
                    };
                    let signal_type = type_by_handle
                        .get(&handle.get_index())
                        .copied()
                        .unwrap_or(FstSignalType::Digital);
                    let value = fst_value(value, signal_type).map_err(fst_to_query)?;
                    for name in names {
                        let next_rows = row_count.saturating_add(1);
                        if let Some(limit) = context.limits().max_rows()
                            && next_rows > limit
                        {
                            return Err(FstCallbackError::Query(QueryError::LimitExceeded {
                                kind: QueryLimitKind::Rows,
                                limit,
                                actual: next_rows,
                            }));
                        }
                        let event_bytes = logical_event_bytes(name, &value);
                        let next_bytes = bytes.saturating_add(event_bytes);
                        if let Some(limit) = context.limits().max_result_bytes()
                            && next_bytes > limit
                        {
                            return Err(FstCallbackError::Query(QueryError::LimitExceeded {
                                kind: QueryLimitKind::ResultBytes,
                                limit,
                                actual: next_bytes,
                            }));
                        }
                        bytes = next_bytes;
                        row_count = next_rows;
                        if !visitor(TimeValue {
                            signal: name.clone(),
                            time,
                            value: value.clone(),
                        })? {
                            return Err(FstCallbackError::Stop);
                        }
                    }
                    Ok::<(), FstCallbackError>(())
                },
            )
        }))
        .map_err(|_| QueryError::Fst("FST parser panicked while reading signal data".into()))?;
        match result {
            Ok(()) => {}
            Err(ReadSignalsError::CallbackError(FstCallbackError::Query(error))) => {
                return Err(error);
            }
            Err(ReadSignalsError::CallbackError(FstCallbackError::Stop)) => {}
            Err(ReadSignalsError::ReadError(error)) => {
                return Err(QueryError::Fst(bounded(error)));
            }
        }
        context.check()?;
        self.validate_source().map_err(fst_to_query)?;
        Ok(())
    }
}

fn resource_limit(kind: &'static str, limit: usize, actual: usize) -> FstError {
    FstError::ResourceLimit {
        kind,
        limit,
        actual,
    }
}

fn ensure_resource(kind: &'static str, actual: usize, limit: usize) -> Result<(), FstError> {
    if actual > limit {
        Err(resource_limit(kind, limit, actual))
    } else {
        Ok(())
    }
}

fn charge_resource(
    kind: &'static str,
    current: &mut usize,
    amount: usize,
    limit: usize,
) -> Result<(), FstError> {
    let actual = current.checked_add(amount).unwrap_or(usize::MAX);
    if actual > limit {
        return Err(resource_limit(kind, limit, actual));
    }
    *current = actual;
    Ok(())
}

fn signal_type(tpe: FstVarType) -> FstSignalType {
    if tpe == FstVarType::GenericString {
        FstSignalType::GenericString
    } else if tpe.is_real() {
        FstSignalType::Real
    } else {
        FstSignalType::Digital
    }
}

pub(crate) fn fst_to_query(error: FstError) -> QueryError {
    match error {
        FstError::StaleSource => QueryError::StaleSource(super::generation_mismatch_error()),
        FstError::Io { source, .. } => QueryError::SourceUnavailable(source),
        FstError::MissingSignal(signal) => QueryError::SignalNotFound(signal),
        FstError::UnsupportedInput(message) | FstError::UnsupportedValue(message) => {
            QueryError::UnsupportedWaveform(message)
        }
        error => QueryError::Fst(error.to_string()),
    }
}
fn query_to_fst(error: QueryError) -> FstError {
    FstError::Parse(error.to_string())
}

pub fn timescale_from_exponent(exponent: i8) -> Option<Timescale> {
    let units: [(i16, TimescaleUnit); 6] = [
        (0, TimescaleUnit::S),
        (-3, TimescaleUnit::MS),
        (-6, TimescaleUnit::US),
        (-9, TimescaleUnit::NS),
        (-12, TimescaleUnit::PS),
        (-15, TimescaleUnit::FS),
    ];
    for (base, unit) in units {
        let delta = i16::from(exponent) - base;
        let magnitude = match delta {
            0 => 1,
            1 => 10,
            2 => 100,
            _ => continue,
        };
        return Some(Timescale { magnitude, unit });
    }
    None
}

fn fst_value(
    value: FstSignalValue<'_>,
    signal_type: FstSignalType,
) -> Result<ChangeValue, FstError> {
    match value {
        FstSignalValue::Real(value) => Ok(ChangeValue::Float(value)),
        FstSignalValue::String(bytes) => {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| {
                    FstError::UnsupportedValue("generic string is not valid UTF-8".into())
                })?
                .to_string();
            if signal_type == FstSignalType::Digital
                && text.len() > 1
                && text.len() <= 128
                && text.bytes().all(|byte| matches!(byte, b'0' | b'1'))
                && let Ok(value) = u128::from_str_radix(&text, 2)
            {
                return Ok(ChangeValue::Integer(value));
            }
            Ok(ChangeValue::Text(text))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn hierarchy_length_offset(bytes: &[u8]) -> usize {
        let mut offset = 0usize;
        loop {
            let kind = bytes[offset];
            let section_length =
                u64::from_be_bytes(bytes[offset + 1..offset + 9].try_into().unwrap()) as usize;
            if matches!(kind, 4 | 6 | 7) {
                return offset + 9;
            }
            offset += 1 + section_length;
        }
    }

    #[test]
    fn timescale_conversion_is_safe_at_i8_boundaries() {
        assert_eq!(timescale_from_exponent(i8::MIN), None);
        assert_eq!(timescale_from_exponent(i8::MAX), None);
        assert_eq!(
            timescale_from_exponent(-15).unwrap().unit,
            TimescaleUnit::FS
        );
        assert_eq!(timescale_from_exponent(-13).unwrap().magnitude, 100);
        assert_eq!(timescale_from_exponent(-16), None);
    }

    #[test]
    fn hierarchy_resource_limits_accept_n_and_reject_n_plus_one() {
        for kind in ["hierarchy_depth", "declarations", "catalog_bytes"] {
            ensure_resource(kind, 8, 8).unwrap();
            assert!(matches!(
                ensure_resource(kind, 9, 8),
                Err(FstError::ResourceLimit {
                    limit: 8,
                    actual: 9,
                    ..
                })
            ));
        }
        let mut used = 0;
        charge_resource("name_bytes", &mut used, 8, 8).unwrap();
        assert!(matches!(
            charge_resource("name_bytes", &mut used, 1, 8),
            Err(FstError::ResourceLimit {
                kind: "name_bytes",
                limit: 8,
                actual: 9
            })
        ));
    }

    #[test]
    fn compressed_hierarchy_preflight_accepts_n_and_rejects_n_plus_one() {
        let original = std::fs::read("tests/fixtures/fst/tiny.fst").unwrap();
        let length_offset = hierarchy_length_offset(&original);
        let declared = u64::from_be_bytes(
            original[length_offset..length_offset + 8]
                .try_into()
                .unwrap(),
        );

        let mut at_limit = NamedTempFile::new().unwrap();
        at_limit.write_all(&original).unwrap();
        OpenedFst::open_with_hierarchy_limit(at_limit.path(), declared).unwrap();

        let mut over = original;
        over[length_offset..length_offset + 8].copy_from_slice(&(declared + 1).to_be_bytes());
        let mut over_limit = NamedTempFile::new().unwrap();
        over_limit.write_all(&over).unwrap();
        assert!(matches!(
            OpenedFst::open_with_hierarchy_limit(over_limit.path(), declared),
            Err(FstError::ResourceLimit {
                kind: "hierarchy_uncompressed_bytes",
                ..
            })
        ));
    }

    #[test]
    fn generic_binary_looking_string_remains_text() {
        assert_eq!(
            fst_value(FstSignalValue::String(b"001"), FstSignalType::GenericString).unwrap(),
            ChangeValue::Text("001".into())
        );
        assert_eq!(
            fst_value(FstSignalValue::String(b"001"), FstSignalType::Digital).unwrap(),
            ChangeValue::Integer(1)
        );
    }
}
