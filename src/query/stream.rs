use std::collections::HashMap;
use std::sync::Arc;

use vcd::{Command, IdCode, Parser, Value, Vector};

use crate::opened::{OpenedBodyReader, OpenedVcd};
use crate::query::{QueryContext, QueryError, QueryLimitKind, QueryResult};
use crate::{ChangeValue, TimeWindow, VcdError};

/// Cooperative cancellation/deadline polling interval for non-timestamp,
/// unselected command runs. Command limits are still enforced on every command.
pub(crate) const COMMAND_CHECK_INTERVAL: u64 = 4096;

#[derive(Debug, Clone)]
#[allow(dead_code)] // Binding fields are consumed by the T03 extraction iterator.
pub(crate) struct TargetBinding {
    pub(crate) request_index: usize,
    pub(crate) name: Arc<str>,
    pub(crate) size: u32,
}

#[derive(Debug)]
struct TargetSelection {
    by_id: HashMap<IdCode, Box<[TargetBinding]>>,
    request_count: usize,
}

#[allow(dead_code)] // Resolution/bindings are consumed through the T03 iterator seam.
impl TargetSelection {
    fn resolve(
        opened: &OpenedVcd,
        targets: &[String],
        context: &QueryContext,
    ) -> QueryResult<Self> {
        context.check_signal_count(targets.len())?;
        let mut by_id: HashMap<IdCode, Vec<TargetBinding>> = HashMap::new();
        let mut missing = Vec::new();
        for (request_index, name) in targets.iter().enumerate() {
            context.check()?;
            if let Some(signal) = opened.signal(name) {
                by_id
                    .entry(signal.id_code())
                    .or_default()
                    .push(TargetBinding {
                        request_index,
                        name: signal.shared_name(),
                        size: signal.size(),
                    });
            } else {
                missing.push(name.clone());
            }
        }
        if !missing.is_empty() {
            return Err(QueryError::from(VcdError::MissingSignals(
                missing.join(", "),
            )));
        }
        Ok(Self {
            by_id: by_id
                .into_iter()
                .map(|(id, bindings)| (id, bindings.into_boxed_slice()))
                .collect(),
            request_count: targets.len(),
        })
    }

    fn contains(&self, id: IdCode) -> bool {
        self.by_id.contains_key(&id)
    }

    fn bindings(&self, id: IdCode) -> &[TargetBinding] {
        self.by_id.get(&id).map_or(&[], Box::as_ref)
    }
}

/// One selected VCD value-change command before alias/request expansion.
///
/// T03 expands `id_code` through [`SelectedChangeStream::bindings`] while
/// retaining one conversion per body command even when aliases or duplicate
/// request names share the identifier.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SelectedChange {
    pub(crate) id_code: IdCode,
    pub(crate) time: u64,
    pub(crate) value: ChangeValue,
}

/// Transport-neutral, selected-ID-first body decoder.
///
/// This is intentionally crate-private until T03 defines the public extraction
/// row API. It is the common stream seam for extraction/find/toggle and avoids
/// converting values for irrelevant IDs or out-of-window changes.
pub(crate) struct SelectedChangeStream {
    parser: Parser<OpenedBodyReader>,
    selection: TargetSelection,
    window: TimeWindow,
    context: QueryContext,
    current_time: u64,
    command_count: u64,
    finished: bool,
    #[cfg(test)]
    test_hooks: StreamTestHooks,
}

impl std::fmt::Debug for SelectedChangeStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SelectedChangeStream")
            .field("target_count", &self.selection.request_count)
            .field("window", &self.window)
            .field("current_time", &self.current_time)
            .field("command_count", &self.command_count)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl OpenedVcd {
    #[allow(dead_code)] // Becomes production-reachable through T03 extraction.
    pub(crate) fn selected_change_stream(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
    ) -> QueryResult<SelectedChangeStream> {
        context.check()?;
        let selection = TargetSelection::resolve(self, targets, context)?;
        // Reader admission can hash the complete header, so check immediately
        // before that work as well as before target resolution.
        context.check()?;
        let parser = self.body_parser().map_err(QueryError::from)?;
        Ok(SelectedChangeStream {
            parser,
            selection,
            window,
            context: context.clone(),
            current_time: 0,
            command_count: 0,
            finished: false,
            #[cfg(test)]
            test_hooks: StreamTestHooks::default(),
        })
    }
}

impl SelectedChangeStream {
    #[allow(dead_code)] // Consumed by the public T03 extraction iterator.
    pub(crate) fn bindings(&self, id: IdCode) -> &[TargetBinding] {
        self.selection.bindings(id)
    }

    #[allow(dead_code)] // Consumed by the public T03 extraction iterator.
    pub(crate) fn target_count(&self) -> usize {
        self.selection.request_count
    }

    #[cfg(test)]
    pub(crate) fn command_count(&self) -> u64 {
        self.command_count
    }

    fn increment_command_count(&mut self) {
        self.command_count = self.command_count.saturating_add(1);
    }

    fn check_command_limit(&self) -> QueryResult<()> {
        if let Some(limit) = self.context.limits().max_commands() {
            if self.command_count > limit {
                return Err(QueryError::LimitExceeded {
                    kind: QueryLimitKind::Commands,
                    limit,
                    actual: self.command_count,
                });
            }
        }
        Ok(())
    }

    fn check_periodic_context(&mut self) -> QueryResult<()> {
        if self.command_count % COMMAND_CHECK_INTERVAL == 0 {
            #[cfg(test)]
            self.test_hooks.on_periodic_check(&self.context);
            self.context.check()?;
        }
        Ok(())
    }

    fn check_timestamp_context(&mut self) -> QueryResult<()> {
        #[cfg(test)]
        self.test_hooks.on_timestamp(&self.context);
        self.context.check()
    }

    fn finish_success(&mut self) -> QueryResult<()> {
        if self.finished {
            return Ok(());
        }
        #[cfg(test)]
        self.test_hooks.before_completion();
        self.context.check()?;
        self.parser
            .reader()
            .validate_complete()
            .map_err(QueryError::from)?;
        // Do not return a successful terminal result after cancellation or a
        // deadline that raced completion validation.
        self.context.check()?;
        self.finished = true;
        Ok(())
    }

    fn fail(&mut self, error: QueryError) -> Option<QueryResult<SelectedChange>> {
        self.finished = true;
        Some(Err(error))
    }

    fn convert_selected(&mut self, command: Command) -> Option<SelectedChange> {
        let (id_code, value) = match command {
            Command::ChangeScalar(id, value) => (id, ChangeValue::Text(value.to_string())),
            Command::ChangeVector(id, vector) => (id, vector_to_change_value_selected(&vector)),
            Command::ChangeReal(id, value) => (id, ChangeValue::Float(value)),
            Command::ChangeString(id, value) => (id, ChangeValue::Text(value)),
            _ => return None,
        };
        #[cfg(test)]
        {
            self.test_hooks.conversions += 1;
        }
        Some(SelectedChange {
            id_code,
            time: self.current_time,
            value,
        })
    }
}

impl Iterator for SelectedChangeStream {
    type Item = QueryResult<SelectedChange>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        loop {
            let result = match self.parser.next() {
                Some(result) => result,
                None => {
                    return match self.finish_success() {
                        Ok(()) => None,
                        Err(error) => self.fail(error),
                    };
                }
            };
            self.increment_command_count();
            // Cancellation/deadline checkpoints take precedence when they
            // collide with command N+1. Away from a timestamp or the periodic
            // interval, the exact command budget is still enforced first.
            if let Err(error) = self.check_periodic_context() {
                return self.fail(error);
            }
            if matches!(&result, Ok(Command::Timestamp(_))) {
                if let Err(error) = self.check_timestamp_context() {
                    return self.fail(error);
                }
            }
            if let Err(error) = self.check_command_limit() {
                return self.fail(error);
            }

            let command = match result {
                Ok(command) => command,
                Err(error) => {
                    return self.fail(QueryError::Vcd(VcdError::from(error)));
                }
            };

            match command {
                Command::Timestamp(time) => {
                    self.current_time = time;
                    if self.window.end.is_some_and(|end| time > end) {
                        return match self.finish_success() {
                            Ok(()) => None,
                            Err(error) => self.fail(error),
                        };
                    }
                }
                Command::ChangeScalar(id, _)
                | Command::ChangeVector(id, _)
                | Command::ChangeReal(id, _)
                | Command::ChangeString(id, _)
                    if self.selection.contains(id) && self.window.contains(self.current_time) =>
                {
                    #[cfg(test)]
                    self.test_hooks.before_conversion(&self.context);
                    if let Err(error) = self.context.check() {
                        return self.fail(error);
                    }
                    // Membership and time-window checks happen before this
                    // match performs any ChangeValue formatting/allocation.
                    if let Some(change) = self.convert_selected(command) {
                        return Some(Ok(change));
                    }
                }
                _ => {}
            }
        }
    }
}

fn vector_to_change_value_selected(vector: &Vector) -> ChangeValue {
    if vector
        .iter()
        .all(|value| matches!(value, Value::V0 | Value::V1))
        && vector.len() <= 128
    {
        let mut integer = 0_u128;
        for value in vector.iter() {
            integer = (integer << 1)
                | match value {
                    Value::V0 => 0,
                    Value::V1 => 1,
                    _ => 0,
                };
        }
        ChangeValue::Integer(integer)
    } else {
        ChangeValue::Text(vector.to_string())
    }
}

#[cfg(test)]
#[derive(Default)]
struct StreamTestHooks {
    cancel_at_periodic_check: bool,
    expire_at_periodic_check: bool,
    cancel_at_timestamp: bool,
    expire_at_timestamp: bool,
    cancel_before_conversion: bool,
    expire_before_conversion: bool,
    completion_append_path: Option<std::path::PathBuf>,
    conversions: u64,
}

#[cfg(test)]
impl StreamTestHooks {
    fn on_periodic_check(&mut self, context: &QueryContext) {
        if self.cancel_at_periodic_check {
            context.cancellation().cancel();
            self.cancel_at_periodic_check = false;
        }
        if self.expire_at_periodic_check {
            context.expire_deadline_for_test();
            self.expire_at_periodic_check = false;
        }
    }

    fn on_timestamp(&mut self, context: &QueryContext) {
        if self.cancel_at_timestamp {
            context.cancellation().cancel();
            self.cancel_at_timestamp = false;
        }
        if self.expire_at_timestamp {
            context.expire_deadline_for_test();
            self.expire_at_timestamp = false;
        }
    }

    fn before_conversion(&mut self, context: &QueryContext) {
        if self.cancel_before_conversion {
            context.cancellation().cancel();
            self.cancel_before_conversion = false;
        }
        if self.expire_before_conversion {
            context.expire_deadline_for_test();
            self.expire_before_conversion = false;
        }
    }

    fn before_completion(&mut self) {
        if let Some(path) = self.completion_append_path.take() {
            use std::io::Write as _;
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open completion mutation")
                .write_all(b"\n#999\n")
                .expect("append completion mutation");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tempfile::tempdir;

    use super::*;
    use crate::query::{CancellationToken, QueryErrorCode, QueryLimits};

    const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";
    const MALFORMED: &str = "tests/fixtures/malformed_body.vcd";

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn collect_changes(stream: &mut SelectedChangeStream) -> QueryResult<Vec<SelectedChange>> {
        stream.by_ref().collect()
    }

    fn write_generated_body(command_count: usize) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("many.vcd");
        let mut file = std::fs::File::create(&path).expect("create fixture");
        writeln!(file, "$scope module top $end").unwrap();
        writeln!(file, "$var wire 1 ! selected $end").unwrap();
        writeln!(file, "$var wire 1 \" irrelevant $end").unwrap();
        writeln!(file, "$upscope $end").unwrap();
        writeln!(file, "$enddefinitions $end").unwrap();
        for index in 0..command_count {
            writeln!(file, "{}\"", index & 1).unwrap();
        }
        (directory, path)
    }

    fn write_timestamp_first_body() -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("timestamp.vcd");
        let mut file = std::fs::File::create(&path).expect("create fixture");
        writeln!(file, "$scope module top $end").unwrap();
        writeln!(file, "$var wire 1 ! selected $end").unwrap();
        writeln!(file, "$upscope $end").unwrap();
        writeln!(file, "$enddefinitions $end").unwrap();
        writeln!(file, "#5").unwrap();
        writeln!(file, "1!").unwrap();
        (directory, path)
    }

    #[test]
    fn resolves_aliases_and_duplicates_without_duplicate_conversion() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut stream = opened
            .selected_change_stream(
                &strings(&["top.alias_a", "top.a", "top.a"]),
                TimeWindow {
                    start: Some(5),
                    end: Some(5),
                },
                &QueryContext::legacy_unlimited(),
            )
            .expect("stream");
        let changes = collect_changes(&mut stream).expect("changes");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].value, ChangeValue::Text("1".to_string()));
        let bindings = stream.bindings(changes[0].id_code);
        assert_eq!(bindings.len(), 3);
        assert_eq!(bindings[0].name.as_ref(), "top.alias_a");
        assert_eq!(bindings[1].name.as_ref(), "top.a");
        assert_eq!(bindings[2].request_index, 2);
        assert_eq!(bindings[2].size, 1);
        assert!(Arc::ptr_eq(&bindings[1].name, &bindings[2].name));
        let catalog_name = opened.signal("top.a").expect("top.a").shared_name();
        assert!(Arc::ptr_eq(&bindings[1].name, &catalog_name));
        assert_eq!(stream.test_hooks.conversions, 1);
    }

    #[test]
    fn irrelevant_vector_string_real_and_scalar_values_are_not_converted() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut stream = opened
            .selected_change_stream(
                &strings(&["top.a"]),
                TimeWindow {
                    start: Some(10),
                    end: Some(10),
                },
                &QueryContext::legacy_unlimited(),
            )
            .expect("stream");
        let changes = collect_changes(&mut stream).expect("changes");
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].value, ChangeValue::Text("0".to_string()));
        assert_eq!(stream.test_hooks.conversions, 1);
    }

    #[test]
    fn out_of_window_selected_values_are_not_converted() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut stream = opened
            .selected_change_stream(
                &strings(&["top.a"]),
                TimeWindow {
                    start: Some(11),
                    end: Some(14),
                },
                &QueryContext::legacy_unlimited(),
            )
            .expect("stream");
        assert!(collect_changes(&mut stream).expect("changes").is_empty());
        assert_eq!(stream.test_hooks.conversions, 0);
    }

    #[test]
    fn selected_conversion_matches_legacy_value_normalization_and_order() {
        let targets = strings(&[
            "top.a",
            "top.vec4[3:0]",
            "top.wide[128:0]",
            "top.real_sig",
            "top.str_sig",
        ]);
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut stream = opened
            .selected_change_stream(
                &targets,
                TimeWindow::default(),
                &QueryContext::legacy_unlimited(),
            )
            .expect("stream");
        let selected = collect_changes(&mut stream).expect("selected");
        let legacy =
            crate::extract_time_values_from_file(SEMANTICS, &targets, TimeWindow::default())
                .expect("legacy");
        let selected_triplets = selected
            .iter()
            .flat_map(|change| {
                stream.bindings(change.id_code).iter().map(move |binding| {
                    (binding.name.to_string(), change.time, change.value.clone())
                })
            })
            .collect::<Vec<_>>();
        let legacy_triplets = legacy
            .into_iter()
            .map(|event| (event.signal, event.time, event.value))
            .collect::<Vec<_>>();
        assert_eq!(selected_triplets, legacy_triplets);
    }

    #[test]
    fn signal_and_command_limits_are_exact_and_count_irrelevant_commands() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let signals_limited =
            QueryContext::new().with_limits(QueryLimits::unlimited().with_max_signals(0));
        let error = opened
            .selected_change_stream(
                &strings(&["top.a"]),
                TimeWindow::default(),
                &signals_limited,
            )
            .expect_err("signal limit");
        assert_eq!(error.code(), QueryErrorCode::LimitExceeded);
        assert_eq!(error.limit_kind(), Some(QueryLimitKind::Signals));

        let (directory, path) = write_generated_body(8);
        let generated = OpenedVcd::open(&path).expect("generated open");
        let mut unlimited = generated
            .selected_change_stream(
                &strings(&["top.selected"]),
                TimeWindow::default(),
                &QueryContext::legacy_unlimited(),
            )
            .expect("unlimited");
        assert!(
            collect_changes(&mut unlimited)
                .expect("unlimited changes")
                .is_empty()
        );
        let total = unlimited.command_count();
        assert!(total >= 8, "parser did not count irrelevant commands");

        let exact_context =
            QueryContext::new().with_limits(QueryLimits::unlimited().with_max_commands(total));
        let mut exact = generated
            .selected_change_stream(
                &strings(&["top.selected"]),
                TimeWindow::default(),
                &exact_context,
            )
            .expect("exact stream");
        assert!(
            collect_changes(&mut exact)
                .expect("exact changes")
                .is_empty()
        );
        assert_eq!(exact.command_count(), total);

        let limited_context =
            QueryContext::new().with_limits(QueryLimits::unlimited().with_max_commands(total - 1));
        let mut limited = generated
            .selected_change_stream(
                &strings(&["top.selected"]),
                TimeWindow::default(),
                &limited_context,
            )
            .expect("limited stream");
        let error = collect_changes(&mut limited).expect_err("command limit");
        assert_eq!(error.limit_kind(), Some(QueryLimitKind::Commands));
        assert_eq!(error.limit(), Some(total - 1));
        assert_eq!(error.actual(), Some(total));
        drop(directory);
    }

    #[test]
    fn timestamp_context_checks_precede_colliding_command_budget() {
        let (_directory, path) = write_timestamp_first_body();
        let opened = OpenedVcd::open(path).expect("open");
        let limits = QueryLimits::unlimited().with_max_commands(0);

        let cancellation = CancellationToken::new();
        let context = QueryContext::new()
            .with_cancellation(cancellation)
            .with_limits(limits.clone());
        let mut cancelled = opened
            .selected_change_stream(&strings(&["top.selected"]), TimeWindow::default(), &context)
            .expect("cancel stream");
        cancelled.test_hooks.cancel_at_timestamp = true;
        assert_eq!(
            cancelled.next().expect("cancel result").unwrap_err().code(),
            QueryErrorCode::Cancelled
        );
        assert_eq!(cancelled.command_count(), 1);

        let context = QueryContext::new().with_limits(limits);
        let mut expired = opened
            .selected_change_stream(&strings(&["top.selected"]), TimeWindow::default(), &context)
            .expect("deadline stream");
        expired.test_hooks.expire_at_timestamp = true;
        assert_eq!(
            expired.next().expect("deadline result").unwrap_err().code(),
            QueryErrorCode::DeadlineExceeded
        );
        assert_eq!(expired.command_count(), 1);
    }

    #[test]
    fn periodic_context_checks_precede_colliding_command_budget() {
        let (_directory, path) = write_generated_body(COMMAND_CHECK_INTERVAL as usize + 1);
        let opened = OpenedVcd::open(path).expect("open");
        let limits = QueryLimits::unlimited().with_max_commands(COMMAND_CHECK_INTERVAL - 1);

        let cancellation = CancellationToken::new();
        let context = QueryContext::new()
            .with_cancellation(cancellation)
            .with_limits(limits.clone());
        let mut cancelled = opened
            .selected_change_stream(&strings(&["top.selected"]), TimeWindow::default(), &context)
            .expect("cancel stream");
        cancelled.test_hooks.cancel_at_periodic_check = true;
        assert_eq!(
            collect_changes(&mut cancelled)
                .expect_err("cancelled")
                .code(),
            QueryErrorCode::Cancelled
        );
        assert_eq!(cancelled.command_count(), COMMAND_CHECK_INTERVAL);

        let context = QueryContext::new().with_limits(limits);
        let mut expired = opened
            .selected_change_stream(&strings(&["top.selected"]), TimeWindow::default(), &context)
            .expect("deadline stream");
        expired.test_hooks.expire_at_periodic_check = true;
        assert_eq!(
            collect_changes(&mut expired).expect_err("deadline").code(),
            QueryErrorCode::DeadlineExceeded
        );
        assert_eq!(expired.command_count(), COMMAND_CHECK_INTERVAL);
    }

    #[test]
    fn cancellation_is_checked_at_timestamp_periodic_and_preconversion_points() {
        let token = CancellationToken::new();
        let context = QueryContext::new().with_cancellation(token);
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut timestamp = opened
            .selected_change_stream(
                &strings(&["top.a"]),
                TimeWindow {
                    start: Some(5),
                    end: None,
                },
                &context,
            )
            .expect("timestamp stream");
        timestamp.test_hooks.cancel_at_timestamp = true;
        assert_eq!(
            timestamp.next().expect("error").unwrap_err().code(),
            QueryErrorCode::Cancelled
        );
        assert_eq!(timestamp.test_hooks.conversions, 0);

        let token = CancellationToken::new();
        let context = QueryContext::new().with_cancellation(token);
        let mut conversion = opened
            .selected_change_stream(&strings(&["top.a"]), TimeWindow::default(), &context)
            .expect("conversion stream");
        conversion.test_hooks.cancel_before_conversion = true;
        assert_eq!(
            conversion.next().expect("error").unwrap_err().code(),
            QueryErrorCode::Cancelled
        );
        assert_eq!(conversion.test_hooks.conversions, 0);

        let (directory, path) = write_generated_body(COMMAND_CHECK_INTERVAL as usize + 4);
        let generated = OpenedVcd::open(path).expect("generated open");
        let token = CancellationToken::new();
        let context = QueryContext::new().with_cancellation(token);
        let mut periodic = generated
            .selected_change_stream(&strings(&["top.selected"]), TimeWindow::default(), &context)
            .expect("periodic stream");
        periodic.test_hooks.cancel_at_periodic_check = true;
        assert_eq!(
            collect_changes(&mut periodic)
                .expect_err("cancelled")
                .code(),
            QueryErrorCode::Cancelled
        );
        assert_eq!(periodic.command_count(), COMMAND_CHECK_INTERVAL);
        drop(directory);
    }

    #[test]
    fn deadline_is_checked_before_selected_conversion() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let context = QueryContext::new();
        let mut stream = opened
            .selected_change_stream(&strings(&["top.a"]), TimeWindow::default(), &context)
            .expect("stream");
        stream.test_hooks.expire_before_conversion = true;
        assert_eq!(
            stream.next().expect("error").unwrap_err().code(),
            QueryErrorCode::DeadlineExceeded
        );
        assert_eq!(stream.test_hooks.conversions, 0);
    }

    #[test]
    fn eof_and_early_end_validate_generation_before_success() {
        for end in [None, Some(5)] {
            let directory = tempdir().expect("temp directory");
            let path = directory.path().join("wave.vcd");
            std::fs::copy(SEMANTICS, &path).expect("copy fixture");
            let opened = OpenedVcd::open(&path).expect("open");
            let mut stream = opened
                .selected_change_stream(
                    &strings(&["top.a"]),
                    TimeWindow { start: None, end },
                    &QueryContext::legacy_unlimited(),
                )
                .expect("stream");
            stream.test_hooks.completion_append_path = Some(path);
            let error = collect_changes(&mut stream).expect_err("stale completion");
            assert_eq!(error.code(), QueryErrorCode::StaleSource);
        }
    }

    #[test]
    fn parser_error_is_reported_without_successful_completion() {
        let opened = OpenedVcd::open(MALFORMED).expect("open");
        let mut stream = opened
            .selected_change_stream(
                &strings(&["top.good"]),
                TimeWindow::default(),
                &QueryContext::legacy_unlimited(),
            )
            .expect("stream");
        stream.test_hooks.completion_append_path = Some(std::path::PathBuf::from("must-not-run"));
        let mut successful = 0;
        let error = loop {
            match stream.next().expect("terminal error") {
                Ok(_) => successful += 1,
                Err(error) => break error,
            }
        };
        assert_eq!(successful, 2);
        assert_eq!(error.code(), QueryErrorCode::Vcd);
        assert!(std::error::Error::source(&error).is_some());
        assert!(stream.test_hooks.completion_append_path.is_some());
    }

    fn dense_identifier(mut index: usize) -> String {
        const DIGITS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let mut identifier = Vec::new();
        loop {
            identifier.push(DIGITS[index % DIGITS.len()]);
            if index < DIGITS.len() {
                break;
            }
            index = index / DIGITS.len() - 1;
        }
        identifier.reverse();
        String::from_utf8(identifier).expect("ASCII identifier")
    }

    fn write_dense_fixture(
        total_signals: usize,
        cycles: usize,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("dense.vcd");
        let mut file =
            std::io::BufWriter::new(std::fs::File::create(&path).expect("create dense fixture"));
        writeln!(file, "$scope module top $end").unwrap();
        for index in 0..total_signals {
            writeln!(
                file,
                "$var wire 1 {} sig{} $end",
                dense_identifier(index),
                index
            )
            .unwrap();
        }
        writeln!(file, "$upscope $end").unwrap();
        writeln!(file, "$enddefinitions $end").unwrap();
        for cycle in 0..cycles {
            writeln!(file, "#{}", cycle).unwrap();
            for signal in 0..total_signals {
                writeln!(file, "{}{}", (cycle + signal) & 1, dense_identifier(signal)).unwrap();
            }
        }
        file.flush().expect("flush dense fixture");
        (directory, path)
    }

    #[test]
    #[ignore = "release-only targeted-conversion diagnostic; run through scripts/measure-targeted-conversion.sh"]
    fn probe_targeted_conversion() {
        let total_signals = std::env::var("TARGETED_TOTAL_SIGNALS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(128_usize);
        let cycles = std::env::var("TARGETED_CYCLES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1000_usize);
        let target_count = std::env::var("TARGETED_TARGETS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1_usize);
        assert!(target_count > 0 && target_count <= total_signals);
        let (_directory, path) = write_dense_fixture(total_signals, cycles);
        let opened = OpenedVcd::open(path).expect("open dense fixture");
        let targets = (0..target_count)
            .map(|index| format!("top.sig{index}"))
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let mut stream = opened
            .selected_change_stream(
                &targets,
                TimeWindow::default(),
                &QueryContext::legacy_unlimited(),
            )
            .expect("dense stream");
        let selected = stream
            .by_ref()
            .collect::<QueryResult<Vec<_>>>()
            .expect("decode");
        let elapsed_us = started.elapsed().as_micros();
        let expected_conversions = cycles as u64 * target_count as u64;
        assert_eq!(stream.test_hooks.conversions, expected_conversions);
        assert_eq!(selected.len() as u64, expected_conversions);
        assert_eq!(
            stream.command_count(),
            cycles as u64 * (total_signals as u64 + 1)
        );
        eprintln!(
            "targeted_conversion total_signals={total_signals} cycles={cycles} targets={target_count} commands={} conversions={} expected_conversions={expected_conversions} elapsed_us={elapsed_us}",
            stream.command_count(),
            stream.test_hooks.conversions,
        );
    }

    #[test]
    fn decreasing_timestamp_and_end_bound_match_legacy_early_stop() {
        let fixture = "tests/fixtures/decreasing_timestamps.vcd";
        let targets = strings(&["top.value"]);
        let window = TimeWindow {
            start: None,
            end: Some(7),
        };
        let opened = OpenedVcd::open(fixture).expect("open");
        let mut stream = opened
            .selected_change_stream(&targets, window, &QueryContext::legacy_unlimited())
            .expect("stream");
        let selected = collect_changes(&mut stream).expect("selected");
        let legacy =
            crate::extract_time_values_from_file(fixture, &targets, window).expect("legacy");
        assert_eq!(selected.len(), legacy.len());
        assert_eq!(
            selected
                .iter()
                .map(|change| change.time)
                .collect::<Vec<_>>(),
            legacy.iter().map(|event| event.time).collect::<Vec<_>>()
        );
    }
}
