use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use fst_reader::{FstFilter, FstHierarchyEntry, FstReader, FstSignalHandle, FstSignalValue, FstVarType};
use thiserror::Error;

use crate::{ChangeValue, TimeValue, TimeWindow};

#[derive(Debug, Error)]
pub enum FstError {
    #[error("failed to open FST {path}: {source}")]
    Io { path: String, #[source] source: std::io::Error },
    #[error("FST parser error: {0}")]
    Parse(String),
    #[error("FST signal not found: {0}")]
    MissingSignal(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FstSignal {
    pub name: String,
    pub handle_index: usize,
    pub width: u32,
    pub var_type: FstVarType,
    pub is_alias: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FstMeta {
    pub start_time: u64,
    pub end_time: u64,
    pub signal_count: usize,
    pub timescale_exponent: i8,
    pub version: String,
    pub date: String,
}

#[derive(Debug, Clone)]
pub struct OpenedFst {
    path: PathBuf,
    meta: FstMeta,
    signals: Vec<FstSignal>,
    by_name: HashMap<String, usize>,
}

impl OpenedFst {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, FstError> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path).map_err(|source| FstError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let mut reader = FstReader::open(BufReader::new(file))
            .map_err(|error| FstError::Parse(error.to_string()))?;
        let header = reader.get_header();
        let mut scopes: Vec<String> = Vec::new();
        let mut signals = Vec::new();
        reader
            .read_hierarchy(|entry| match entry {
                FstHierarchyEntry::Scope { name, component, .. } => {
                    scopes.push(if component.is_empty() { name } else { component });
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
                    let full_name = if scopes.is_empty() {
                        name
                    } else {
                        format!("{}.{}", scopes.join("."), name)
                    };
                    signals.push(FstSignal {
                        name: full_name,
                        handle_index: handle.get_index(),
                        width: length,
                        var_type: tpe,
                        is_alias,
                    });
                }
                _ => {}
            })
            .map_err(|error| FstError::Parse(error.to_string()))?;
        let by_name = signals
            .iter()
            .enumerate()
            .map(|(index, signal)| (signal.name.clone(), index))
            .collect();
        Ok(Self {
            path,
            meta: FstMeta {
                start_time: header.start_time,
                end_time: header.end_time,
                signal_count: signals.len(),
                timescale_exponent: header.timescale_exponent,
                version: header.version,
                date: header.date,
            },
            signals,
            by_name,
        })
    }

    pub fn path(&self) -> &Path { &self.path }
    pub fn metadata(&self) -> &FstMeta { &self.meta }
    pub fn signals(&self) -> &[FstSignal] { &self.signals }

    pub fn list_signals(&self, filter: Option<&str>) -> Vec<String> {
        self.signals
            .iter()
            .filter(|signal| filter.is_none_or(|needle| signal.name.contains(needle)))
            .map(|signal| signal.name.clone())
            .collect()
    }

    pub fn extract(&self, targets: &[String], window: TimeWindow) -> Result<Vec<TimeValue>, FstError> {
        let mut handles = Vec::new();
        let mut names_by_handle: HashMap<usize, Vec<String>> = HashMap::new();
        for target in targets {
            let index = *self.by_name.get(target).ok_or_else(|| FstError::MissingSignal(target.clone()))?;
            let signal = &self.signals[index];
            if !handles.iter().any(|handle: &usize| *handle == signal.handle_index) {
                handles.push(signal.handle_index);
            }
            names_by_handle.entry(signal.handle_index).or_default().push(target.clone());
        }
        let file = File::open(&self.path).map_err(|source| FstError::Io {
            path: self.path.display().to_string(), source,
        })?;
        let mut reader = FstReader::open(BufReader::new(file))
            .map_err(|error| FstError::Parse(error.to_string()))?;
        let start = window.start.unwrap_or(0);
        let end = window.end.unwrap_or(self.meta.end_time);
        let filter = FstFilter {
            start,
            end: Some(end),
            include: Some(handles.into_iter().map(FstSignalHandle::from_index).collect()),
        };
        let mut rows = Vec::new();
        reader
            .read_signals(&filter, |time, handle, value| {
                let Some(names) = names_by_handle.get(&handle.get_index()) else { return Ok::<(), ()>(()); };
                let value = fst_value(value);
                for name in names {
                    rows.push(TimeValue { signal: name.clone(), time, value: value.clone() });
                }
                Ok::<(), ()>(())
            })
            .map_err(|error| FstError::Parse(error.to_string()))?;
        Ok(rows)
    }
}

fn fst_value(value: FstSignalValue<'_>) -> ChangeValue {
    match value {
        FstSignalValue::Real(value) => ChangeValue::Float(value),
        FstSignalValue::String(bytes) => {
            let text = String::from_utf8_lossy(bytes).into_owned();
            if let Some(bits) = text.strip_prefix('b').or_else(|| text.strip_prefix('B')) {
                if bits.len() <= 128 && bits.bytes().all(|byte| matches!(byte, b'0' | b'1')) {
                    if let Ok(value) = u128::from_str_radix(bits, 2) {
                        return ChangeValue::Integer(value);
                    }
                }
            } else if text.len() <= 128 && text.bytes().all(|byte| matches!(byte, b'0' | b'1')) {
                if let Ok(value) = u128::from_str_radix(&text, 2) {
                    return ChangeValue::Integer(value);
                }
            }
            ChangeValue::Text(text)
        }
    }
}
