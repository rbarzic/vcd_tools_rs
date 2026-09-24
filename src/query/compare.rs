use std::collections::{HashMap, HashSet};

use crate::opened::{OpenedFst, OpenedVcd, OpenedWaveform};
use crate::query::{QueryContext, QueryResult};
use crate::{ChangeValue, ComparisonOptions, ComparisonResult, SignalMismatch};

const TIMELINE_CONTEXT_CHECK_INTERVAL: u64 = 4096;

pub(crate) type SignalTimelines = HashMap<String, Vec<(u64, ChangeValue)>>;

/// Minimal backend seam shared by streaming and future cached timelines.
pub(crate) trait TimelineProvider {
    fn ordered_signal_names(&self) -> Vec<String>;
    fn display_name(&self) -> String;
    fn collect_timelines(
        &self,
        signals: &[String],
        options: &ComparisonOptions,
        context: &QueryContext,
    ) -> QueryResult<SignalTimelines>;
    fn validate_complete(&self, context: &QueryContext) -> QueryResult<()>;
}

impl TimelineProvider for OpenedVcd {
    fn ordered_signal_names(&self) -> Vec<String> {
        self.signal_names().map(str::to_string).collect()
    }

    fn display_name(&self) -> String {
        self.path().to_string_lossy().into_owned()
    }

    fn collect_timelines(
        &self,
        signals: &[String],
        options: &ComparisonOptions,
        context: &QueryContext,
    ) -> QueryResult<SignalTimelines> {
        let mut timelines = signals
            .iter()
            .map(|signal| (signal.clone(), Vec::new()))
            .collect::<SignalTimelines>();
        let events = self.extract(signals, options.time_window, context)?;
        for event in events {
            let event = event?;
            if let Some(timeline) = timelines.get_mut(&event.signal) {
                timeline.push((event.time, event.value));
            }
        }
        Ok(timelines)
    }

    fn validate_complete(&self, context: &QueryContext) -> QueryResult<()> {
        context.check()?;
        self.validate_source()
            .map_err(crate::query::QueryError::from)?;
        context.check()
    }
}

impl TimelineProvider for OpenedFst {
    fn ordered_signal_names(&self) -> Vec<String> {
        self.signal_names().map(str::to_string).collect()
    }

    fn display_name(&self) -> String {
        self.path().to_string_lossy().into_owned()
    }

    fn collect_timelines(
        &self,
        signals: &[String],
        options: &ComparisonOptions,
        context: &QueryContext,
    ) -> QueryResult<SignalTimelines> {
        let mut timelines = signals
            .iter()
            .map(|signal| (signal.clone(), Vec::new()))
            .collect::<SignalTimelines>();
        for event in self.extract_with_context(signals, options.time_window, context)? {
            if let Some(timeline) = timelines.get_mut(&event.signal) {
                timeline.push((event.time, event.value));
            }
        }
        Ok(timelines)
    }

    fn validate_complete(&self, context: &QueryContext) -> QueryResult<()> {
        context.check()?;
        self.validate_source()
            .map_err(crate::opened::fst::fst_to_query)?;
        context.check()
    }
}

impl OpenedWaveform {
    pub fn compare(
        &self,
        other: &OpenedWaveform,
        options: &ComparisonOptions,
        context: &QueryContext,
    ) -> QueryResult<ComparisonResult> {
        let compatible_timescale = match (self, other) {
            (OpenedWaveform::Vcd(_), OpenedWaveform::Vcd(_)) => true,
            (OpenedWaveform::Fst(first), OpenedWaveform::Fst(second)) => {
                first.metadata().timescale_exponent == second.metadata().timescale_exponent
            }
            (OpenedWaveform::Vcd(vcd), OpenedWaveform::Fst(fst))
            | (OpenedWaveform::Fst(fst), OpenedWaveform::Vcd(vcd)) => {
                fst.metadata().timescale.is_some() && vcd.timescale() == fst.timescale()
            }
        };
        if !compatible_timescale {
            return Err(crate::query::QueryError::UnsupportedWaveform(
                "waveform comparison requires equal physical timescales".into(),
            ));
        }
        match (self, other) {
            (OpenedWaveform::Vcd(first), OpenedWaveform::Vcd(second)) => {
                compare_providers(first, second, options, context)
            }
            (OpenedWaveform::Vcd(first), OpenedWaveform::Fst(second)) => {
                compare_providers(first, second, options, context)
            }
            (OpenedWaveform::Fst(first), OpenedWaveform::Vcd(second)) => {
                compare_providers(first, second, options, context)
            }
            (OpenedWaveform::Fst(first), OpenedWaveform::Fst(second)) => {
                compare_providers(first, second, options, context)
            }
        }
    }
}

impl OpenedVcd {
    pub fn compare(
        &self,
        other: &OpenedVcd,
        options: &ComparisonOptions,
        context: &QueryContext,
    ) -> QueryResult<ComparisonResult> {
        compare_providers(self, other, options, context)
    }
}

pub(crate) fn compare_providers<A: TimelineProvider, B: TimelineProvider>(
    first: &A,
    second: &B,
    options: &ComparisonOptions,
    context: &QueryContext,
) -> QueryResult<ComparisonResult> {
    context.check()?;
    let names1 = first.ordered_signal_names();
    let names2 = second.ordered_signal_names();
    let set1: HashSet<&str> = names1.iter().map(String::as_str).collect();
    let set2: HashSet<&str> = names2.iter().map(String::as_str).collect();

    let mut common_signals = names1
        .iter()
        .filter(|name| set2.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let signals_only_in_file1 = names1
        .iter()
        .filter(|name| !set2.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let signals_only_in_file2 = names2
        .iter()
        .filter(|name| !set1.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    common_signals.sort();

    let signals_to_compare = if options.signals_only.is_empty() {
        common_signals.clone()
    } else {
        let wanted: HashSet<&str> = options.signals_only.iter().map(String::as_str).collect();
        common_signals
            .iter()
            .filter(|signal| wanted.contains(signal.as_str()))
            .cloned()
            .collect()
    };
    context.check_signal_count(signals_to_compare.len())?;

    // V1 intentionally materializes selected timelines. The provider seam
    // allows a future compact/cache backend without changing comparison
    // semantics; server-side response/memory limits remain independently
    // required before exposing comparison remotely.
    let values1 = first.collect_timelines(&signals_to_compare, options, context)?;
    first.validate_complete(context)?;
    let values2 = second.collect_timelines(&signals_to_compare, options, context)?;
    second.validate_complete(context)?;

    let mut mismatches = Vec::new();
    let mut signals_with_mismatches = 0;
    for signal in &signals_to_compare {
        context.check()?;
        let first_values = values1.get(signal).map(Vec::as_slice).unwrap_or(&[]);
        let second_values = values2.get(signal).map(Vec::as_slice).unwrap_or(&[]);
        let count = compare_signal_timeline(
            signal,
            first_values,
            second_values,
            options,
            &mut mismatches,
            context,
        )?;
        if count > 0 {
            signals_with_mismatches += 1;
        }
    }
    // Revalidate both generations immediately before publishing a result. A
    // source may change after its timeline was collected while the other
    // provider remains active.
    context.check()?;
    first.validate_complete(context)?;
    second.validate_complete(context)?;
    context.check()?;
    let total_mismatches = mismatches.len();
    Ok(ComparisonResult {
        file1: first.display_name(),
        file2: second.display_name(),
        common_signals,
        signals_only_in_file1,
        signals_only_in_file2,
        passed: mismatches.is_empty(),
        mismatches,
        total_mismatches,
        signals_with_mismatches,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimelineCheckpoint {
    Interval(u64),
    BeforeMismatch,
}

fn compare_signal_timeline(
    signal: &str,
    first: &[(u64, ChangeValue)],
    second: &[(u64, ChangeValue)],
    options: &ComparisonOptions,
    mismatches: &mut Vec<SignalMismatch>,
    context: &QueryContext,
) -> QueryResult<usize> {
    compare_signal_timeline_with_hook(
        signal,
        first,
        second,
        options,
        mismatches,
        context,
        &mut |_, _| {},
    )
}

fn poll_timeline_context<F>(
    work_units: &mut u64,
    context: &QueryContext,
    hook: &mut F,
) -> QueryResult<()>
where
    F: FnMut(TimelineCheckpoint, &QueryContext),
{
    *work_units = work_units.saturating_add(1);
    if *work_units % TIMELINE_CONTEXT_CHECK_INTERVAL == 0 {
        hook(TimelineCheckpoint::Interval(*work_units), context);
        context.check()?;
    }
    Ok(())
}

fn compare_signal_timeline_with_hook<F>(
    signal: &str,
    first: &[(u64, ChangeValue)],
    second: &[(u64, ChangeValue)],
    options: &ComparisonOptions,
    mismatches: &mut Vec<SignalMismatch>,
    context: &QueryContext,
    hook: &mut F,
) -> QueryResult<usize>
where
    F: FnMut(TimelineCheckpoint, &QueryContext),
{
    let mut i = 0;
    let mut j = 0;
    let mut current1 = ChangeValue::Text("x".to_string());
    let mut current2 = ChangeValue::Text("x".to_string());
    let mut count = 0;
    let mut work_units = 0_u64;
    while i < first.len() || j < second.len() {
        let time1 = first.get(i).map(|entry| entry.0).unwrap_or(u64::MAX);
        let time2 = second.get(j).map(|entry| entry.0).unwrap_or(u64::MAX);
        let time = time1.min(time2);
        if time == u64::MAX {
            break;
        }
        while i < first.len() && first[i].0 == time {
            poll_timeline_context(&mut work_units, context, hook)?;
            current1 = first[i].1.clone();
            i += 1;
        }
        while j < second.len() && second[j].0 == time {
            poll_timeline_context(&mut work_units, context, hook)?;
            current2 = second[j].1.clone();
            j += 1;
        }
        if !values_match(&current1, &current2, options.ignore_unknown) {
            hook(TimelineCheckpoint::BeforeMismatch, context);
            context.check()?;
            mismatches.push(SignalMismatch {
                signal_name: signal.to_string(),
                time,
                value1: current1.clone(),
                value2: current2.clone(),
                is_unknown: is_unknown(&current1) || is_unknown(&current2),
            });
            count += 1;
            if options
                .max_mismatches
                .is_some_and(|maximum| count >= maximum)
            {
                break;
            }
        }
    }
    Ok(count)
}

fn values_match(first: &ChangeValue, second: &ChangeValue, ignore_unknown: bool) -> bool {
    if ignore_unknown && (is_unknown(first) || is_unknown(second)) {
        return true;
    }
    match (first, second) {
        (ChangeValue::Integer(a), ChangeValue::Integer(b)) => a == b,
        (ChangeValue::Float(a), ChangeValue::Float(b)) => a.to_bits() == b.to_bits(),
        (ChangeValue::Text(a), ChangeValue::Text(b)) => a.eq_ignore_ascii_case(b),
        _ => first.normalize() == second.normalize(),
    }
}

fn is_unknown(value: &ChangeValue) -> bool {
    matches!(value, ChangeValue::Text(text) if text.eq_ignore_ascii_case("x") || text.eq_ignore_ascii_case("z"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProvider {
        name: &'static str,
        names: Vec<String>,
        timelines: SignalTimelines,
    }

    impl TimelineProvider for FakeProvider {
        fn ordered_signal_names(&self) -> Vec<String> {
            self.names.clone()
        }

        fn display_name(&self) -> String {
            self.name.to_string()
        }

        fn collect_timelines(
            &self,
            signals: &[String],
            _options: &ComparisonOptions,
            context: &QueryContext,
        ) -> QueryResult<SignalTimelines> {
            context.check_signal_count(signals.len())?;
            Ok(signals
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        self.timelines.get(name).cloned().unwrap_or_default(),
                    )
                })
                .collect())
        }

        fn validate_complete(&self, context: &QueryContext) -> QueryResult<()> {
            context.check()
        }
    }

    #[test]
    fn provider_seam_preserves_order_final_same_time_value_and_file_only_lists() {
        let first = FakeProvider {
            name: "first",
            names: vec!["z_only".into(), "common".into()],
            timelines: HashMap::from([(
                "common".into(),
                vec![
                    (5, ChangeValue::Text("0".into())),
                    (5, ChangeValue::Text("1".into())),
                ],
            )]),
        };
        let second = FakeProvider {
            name: "second",
            names: vec!["common".into(), "a_only".into()],
            timelines: HashMap::from([("common".into(), vec![(5, ChangeValue::Text("1".into()))])]),
        };
        let result = compare_providers(
            &first,
            &second,
            &ComparisonOptions::default(),
            &QueryContext::legacy_unlimited(),
        )
        .expect("compare");
        assert!(result.passed);
        assert_eq!(result.common_signals, ["common"]);
        assert_eq!(result.signals_only_in_file1, ["z_only"]);
        assert_eq!(result.signals_only_in_file2, ["a_only"]);
    }

    #[test]
    fn large_timeline_merge_observes_cancellation_and_deadline_at_bounded_interval() {
        use crate::query::{CancellationToken, QueryErrorCode};

        let timeline = (0..(TIMELINE_CONTEXT_CHECK_INTERVAL + 8))
            .map(|_| (0, ChangeValue::Text("0".into())))
            .collect::<Vec<_>>();
        let token = CancellationToken::new();
        let context = QueryContext::new().with_cancellation(token.clone());
        let mut mismatches = Vec::new();
        let mut cancelled_at = None;
        let error = compare_signal_timeline_with_hook(
            "signal",
            &timeline,
            &timeline,
            &ComparisonOptions::default(),
            &mut mismatches,
            &context,
            &mut |checkpoint, _| {
                if let TimelineCheckpoint::Interval(iteration) = checkpoint {
                    cancelled_at = Some(iteration);
                    token.cancel();
                }
            },
        )
        .expect_err("cancelled merge");
        assert_eq!(cancelled_at, Some(TIMELINE_CONTEXT_CHECK_INTERVAL));
        assert_eq!(error.code(), QueryErrorCode::Cancelled);
        assert!(mismatches.is_empty());

        let context = QueryContext::new();
        let mut expired_at = None;
        let error = compare_signal_timeline_with_hook(
            "signal",
            &timeline,
            &timeline,
            &ComparisonOptions::default(),
            &mut mismatches,
            &context,
            &mut |checkpoint, context| {
                if let TimelineCheckpoint::Interval(iteration) = checkpoint {
                    expired_at = Some(iteration);
                    context.expire_deadline_for_test();
                }
            },
        )
        .expect_err("expired merge");
        assert_eq!(expired_at, Some(TIMELINE_CONTEXT_CHECK_INTERVAL));
        assert_eq!(error.code(), QueryErrorCode::DeadlineExceeded);
        assert!(mismatches.is_empty());
    }

    #[test]
    fn cancellation_immediately_before_mismatch_prevents_delivery() {
        use crate::query::{CancellationToken, QueryErrorCode};

        let token = CancellationToken::new();
        let context = QueryContext::new().with_cancellation(token.clone());
        let mut mismatches = Vec::new();
        let error = compare_signal_timeline_with_hook(
            "signal",
            &[(1, ChangeValue::Text("0".into()))],
            &[(1, ChangeValue::Text("1".into()))],
            &ComparisonOptions::default(),
            &mut mismatches,
            &context,
            &mut |checkpoint, _| {
                if checkpoint == TimelineCheckpoint::BeforeMismatch {
                    token.cancel();
                }
            },
        )
        .expect_err("cancelled before mismatch");
        assert_eq!(error.code(), QueryErrorCode::Cancelled);
        assert!(mismatches.is_empty());
    }

    #[test]
    fn mutation_of_first_source_during_second_collection_rejects_result() {
        use crate::query::QueryErrorCode;
        use std::io::Write as _;

        struct MutatingProvider {
            first_path: std::path::PathBuf,
        }

        impl TimelineProvider for MutatingProvider {
            fn ordered_signal_names(&self) -> Vec<String> {
                vec!["top.a".to_string()]
            }

            fn display_name(&self) -> String {
                "mutating-second".to_string()
            }

            fn collect_timelines(
                &self,
                signals: &[String],
                _options: &ComparisonOptions,
                context: &QueryContext,
            ) -> QueryResult<SignalTimelines> {
                context.check()?;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&self.first_path)
                    .expect("open first source")
                    .write_all(b"\n#999\n")
                    .expect("mutate first source while second provider is active");
                context.check()?;
                Ok(signals
                    .iter()
                    .map(|signal| (signal.clone(), Vec::new()))
                    .collect())
            }

            fn validate_complete(&self, context: &QueryContext) -> QueryResult<()> {
                context.check()
            }
        }

        let directory = tempfile::tempdir().expect("temp directory");
        let first_path = directory.path().join("first.vcd");
        std::fs::copy("tests/fixtures/query_semantics.vcd", &first_path).expect("copy first");
        let first = OpenedVcd::open(&first_path).expect("open first");
        let second = MutatingProvider {
            first_path: first_path.clone(),
        };
        let options = ComparisonOptions {
            signals_only: vec!["top.a".to_string()],
            ..ComparisonOptions::default()
        };
        let error = compare_providers(&first, &second, &options, &QueryContext::legacy_unlimited())
            .expect_err("stale first source must prevent a result");
        assert_eq!(error.code(), QueryErrorCode::StaleSource);
    }

    #[test]
    fn mixed_provider_types_share_semantics() {
        struct Other(FakeProvider);
        impl TimelineProvider for Other {
            fn ordered_signal_names(&self) -> Vec<String> {
                self.0.ordered_signal_names()
            }
            fn display_name(&self) -> String {
                self.0.display_name()
            }
            fn collect_timelines(
                &self,
                signals: &[String],
                options: &ComparisonOptions,
                context: &QueryContext,
            ) -> QueryResult<SignalTimelines> {
                self.0.collect_timelines(signals, options, context)
            }
            fn validate_complete(&self, context: &QueryContext) -> QueryResult<()> {
                self.0.validate_complete(context)
            }
        }
        let first = FakeProvider {
            name: "first",
            names: vec!["common".into()],
            timelines: HashMap::from([("common".into(), vec![(1, ChangeValue::Text("0".into()))])]),
        };
        let second = Other(FakeProvider {
            name: "second",
            names: vec!["common".into()],
            timelines: HashMap::from([("common".into(), vec![(1, ChangeValue::Text("1".into()))])]),
        });
        let result = compare_providers(
            &first,
            &second,
            &ComparisonOptions::default(),
            &QueryContext::legacy_unlimited(),
        )
        .expect("compare");
        assert_eq!(result.total_mismatches, 1);
    }
}
