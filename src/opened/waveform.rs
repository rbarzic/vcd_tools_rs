use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::opened::{OpenedFst, OpenedVcd};
use crate::query::{QueryContext, QueryResult};
use crate::{
    TargetValue, TimeValue, TimeWindow, Timescale, WaveformDetectionError, WaveformFormat,
    WaveformFormatHint, detect_waveform_format, format_value_for_signal,
};

fn map_fst_open(error: crate::opened::FstError) -> WaveformDetectionError {
    match error {
        crate::opened::FstError::ResourceLimit { limit, actual, .. } => {
            WaveformDetectionError::ResourceLimit {
                limit: limit as u64,
                actual: actual as u64,
            }
        }
        crate::opened::FstError::UnsupportedInput(message)
        | crate::opened::FstError::UnsupportedValue(message) => {
            WaveformDetectionError::UnsupportedFst { message }
        }
        crate::opened::FstError::ParserPanicked => WaveformDetectionError::FstParserPanicked,
        error => WaveformDetectionError::Invalid {
            format: WaveformFormat::Fst,
            message: error.to_string(),
        },
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // Public variants intentionally expose the concrete additive backends.
pub enum OpenedWaveform {
    Vcd(OpenedVcd),
    Fst(OpenedFst),
}

impl OpenedWaveform {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WaveformDetectionError> {
        Self::open_with_hint(path, WaveformFormatHint::Auto)
    }

    pub fn open_with_hint(
        path: impl AsRef<Path>,
        hint: WaveformFormatHint,
    ) -> Result<Self, WaveformDetectionError> {
        let path = path.as_ref();
        match detect_waveform_format(path, hint)? {
            WaveformFormat::Vcd => OpenedVcd::open(path).map(Self::Vcd).map_err(|error| {
                WaveformDetectionError::Invalid {
                    format: WaveformFormat::Vcd,
                    message: error.to_string(),
                }
            }),
            WaveformFormat::Fst => OpenedFst::open(path).map(Self::Fst).map_err(map_fst_open),
        }
    }

    pub fn format(&self) -> WaveformFormat {
        match self {
            Self::Vcd(_) => WaveformFormat::Vcd,
            Self::Fst(_) => WaveformFormat::Fst,
        }
    }
    pub fn generation(&self) -> u64 {
        match self {
            Self::Vcd(opened) => opened.generation().get(),
            Self::Fst(opened) => opened.generation().get(),
        }
    }
    pub fn source_size(&self) -> u64 {
        match self {
            Self::Vcd(opened) => opened.identity().len(),
            Self::Fst(opened) => opened.source_size(),
        }
    }
    pub fn signal_count(&self) -> usize {
        match self {
            Self::Vcd(opened) => opened.signal_count(),
            Self::Fst(opened) => opened.signal_count(),
        }
    }
    pub fn visit_signal_names(
        &self,
        context: &QueryContext,
        mut visitor: impl FnMut(&str) -> QueryResult<()>,
    ) -> QueryResult<()> {
        match self {
            Self::Vcd(opened) => {
                for name in opened.signal_names() {
                    context.check()?;
                    visitor(name)?;
                }
            }
            Self::Fst(opened) => {
                for name in opened.signal_names() {
                    context.check()?;
                    visitor(name)?;
                }
            }
        }
        self.validate_source()?;
        context.check()
    }
    pub fn signal_names(&self) -> Vec<String> {
        match self {
            Self::Vcd(opened) => opened.signal_names().map(str::to_string).collect(),
            Self::Fst(opened) => opened.signal_names().map(str::to_string).collect(),
        }
    }
    pub fn list_signals(&self, filter: Option<&str>) -> Vec<String> {
        match self {
            Self::Vcd(opened) => opened.list_signals(filter),
            Self::Fst(opened) => opened.list_signals(filter),
        }
    }
    pub fn timescale(&self) -> Option<&Timescale> {
        match self {
            Self::Vcd(opened) => opened.timescale(),
            Self::Fst(opened) => opened.timescale(),
        }
    }
    pub fn width(&self, signal: &str) -> Option<u32> {
        match self {
            Self::Vcd(opened) => opened.signal(signal).map(|signal| signal.size()),
            Self::Fst(opened) => opened.signal(signal).map(|signal| signal.width),
        }
    }
    pub fn validate_source(&self) -> QueryResult<()> {
        match self {
            Self::Vcd(opened) => opened.validate_source().map_err(Into::into),
            Self::Fst(opened) => opened.validate_source().map_err(super::fst::fst_to_query),
        }
    }
    pub fn has_cached_metadata(&self) -> bool {
        match self {
            Self::Vcd(opened) => opened.has_cached_metadata(),
            Self::Fst(_) => true,
        }
    }
    #[doc(hidden)]
    pub fn set_metadata_scan_hook(&self, hook: Arc<dyn Fn(u64) + Send + Sync>) {
        if let Self::Vcd(opened) = self {
            opened.set_metadata_scan_hook(hook);
        }
    }

    pub fn metadata(
        &self,
        context: &QueryContext,
    ) -> QueryResult<(usize, Option<Timescale>, u64, u64)> {
        context.check()?;
        match self {
            Self::Vcd(opened) => opened.metadata_with_context(context).map(|meta| {
                (
                    meta.signal_count,
                    meta.timescale.clone(),
                    meta.start_time,
                    meta.end_time,
                )
            }),
            Self::Fst(opened) => {
                context.check()?;
                opened.validate_source().map_err(super::fst::fst_to_query)?;
                let meta = opened.metadata();
                let result = (
                    meta.signal_count,
                    meta.timescale.clone(),
                    meta.start_time,
                    meta.end_time,
                );
                context.check()?;
                opened.validate_source().map_err(super::fst::fst_to_query)?;
                context.check()?;
                Ok(result)
            }
        }
    }

    pub fn visit_changes(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
        mut visitor: impl FnMut(TimeValue) -> QueryResult<()>,
    ) -> QueryResult<()> {
        match self {
            Self::Vcd(opened) => {
                for event in opened.extract(targets, window, context)? {
                    visitor(event?)?;
                }
                Ok(())
            }
            Self::Fst(opened) => {
                opened.visit_changes_with_context(targets, window, context, visitor)
            }
        }
    }

    pub fn extract(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
    ) -> QueryResult<Vec<TimeValue>> {
        let mut rows = Vec::new();
        self.visit_changes(targets, window, context, |event| {
            rows.push(event);
            Ok(())
        })?;
        Ok(rows)
    }

    pub fn find_nth_occurrence(
        &self,
        signal: &str,
        target: TargetValue,
        occurrence: usize,
        window: TimeWindow,
        context: &QueryContext,
    ) -> QueryResult<(Option<TimeValue>, u32)> {
        if occurrence == 0 {
            return Err(crate::query::QueryError::Vcd(
                crate::VcdError::InvalidOccurrence,
            ));
        }
        match self {
            Self::Vcd(opened) => {
                opened.find_nth_occurrence(signal, target, occurrence, window, context)
            }
            Self::Fst(opened) => {
                let width = opened
                    .signal(signal)
                    .map(|candidate| candidate.width)
                    .ok_or_else(|| crate::query::QueryError::SignalNotFound(signal.into()))?;
                let normalized = target.normalize();
                let mut count = 0usize;
                let mut found = None;
                opened.visit_changes_until_with_context(
                    &[signal.to_string()],
                    window,
                    context,
                    |event| {
                        if event.value.normalize() == normalized {
                            count += 1;
                            if count == occurrence {
                                found = Some(event);
                                return Ok(false);
                            }
                        }
                        Ok(true)
                    },
                )?;
                Ok((found, width))
            }
        }
    }

    pub fn count_toggles(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
    ) -> QueryResult<HashMap<String, usize>> {
        if let Self::Vcd(opened) = self {
            return opened.count_toggles(targets, window, context);
        }
        let mut counts = targets
            .iter()
            .map(|name| (name.clone(), 0))
            .collect::<HashMap<_, _>>();
        let mut previous: HashMap<String, String> = HashMap::new();
        self.visit_changes(targets, window, context, |event| {
            let value =
                format_value_for_signal(&event.value, self.width(&event.signal).unwrap_or(1));
            if previous.get(&event.signal).is_some_and(|old| old != &value) {
                *counts.entry(event.signal.clone()).or_default() += 1;
            }
            previous.insert(event.signal, value);
            Ok(())
        })?;
        Ok(counts)
    }
}
