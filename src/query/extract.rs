use std::collections::VecDeque;
use std::sync::Arc;

use crate::opened::OpenedVcd;
use crate::query::stream::{SelectedChange, SelectedChangeStream, TargetBinding};
use crate::query::{QueryContext, QueryError, QueryLimitKind, QueryResult};
use crate::{ChangeValue, TimeValue, TimeWindow};

/// Logical bytes charged for one extracted event.
///
/// This transport-neutral accounting is the UTF-8 signal-name length, eight
/// bytes for the timestamp, and the in-memory logical value payload (16 bytes
/// for an integer, 8 for a float, or UTF-8 text length). Protocol encoders must
/// separately enforce their actual encoded-byte limits.
pub fn logical_event_bytes(signal: &str, value: &ChangeValue) -> u64 {
    let value_bytes = match value {
        ChangeValue::Integer(_) => 16,
        ChangeValue::Float(_) => 8,
        ChangeValue::Text(text) => text.len() as u64,
    };
    (signal.len() as u64)
        .saturating_add(8)
        .saturating_add(value_bytes)
}

#[derive(Debug, Clone)]
struct PendingEvent {
    name: Arc<str>,
    value: ChangeValue,
    time: u64,
}

/// Reusable extraction iterator over one immutable [`OpenedVcd`] generation.
///
/// Dropping the iterator early intentionally performs no completion check.
/// Consumers that publish an early successful result (for example `find`) must
/// call [`OpenedTimeValueIter::validate_current_generation`]. Exhausting the
/// iterator validates the generation through the underlying stream.
pub struct OpenedTimeValueIter {
    stream: SelectedChangeStream,
    context: QueryContext,
    pending: VecDeque<PendingEvent>,
    rows: u64,
    logical_bytes: u64,
    terminated: bool,
}

impl std::fmt::Debug for OpenedTimeValueIter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpenedTimeValueIter")
            .field("rows", &self.rows)
            .field("logical_bytes", &self.logical_bytes)
            .field("pending", &self.pending.len())
            .field("terminated", &self.terminated)
            .finish_non_exhaustive()
    }
}

impl OpenedVcd {
    pub fn extract(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
    ) -> QueryResult<OpenedTimeValueIter> {
        let stream = self.selected_change_stream(targets, window, context)?;
        Ok(OpenedTimeValueIter {
            stream,
            context: context.clone(),
            pending: VecDeque::new(),
            rows: 0,
            logical_bytes: 0,
            terminated: false,
        })
    }
}

impl OpenedTimeValueIter {
    pub fn rows_emitted(&self) -> u64 {
        self.rows
    }

    pub fn logical_bytes_emitted(&self) -> u64 {
        self.logical_bytes
    }

    pub fn validate_current_generation(&mut self) -> QueryResult<()> {
        self.context.check()?;
        self.stream.validate_current_generation()?;
        self.context.check()
    }

    fn queue_change(&mut self, change: SelectedChange) {
        let bindings = self.stream.bindings(change.id_code);
        self.pending.extend(
            bindings
                .iter()
                .map(|TargetBinding { name, .. }| PendingEvent {
                    name: Arc::clone(name),
                    value: change.value.clone(),
                    time: change.time,
                }),
        );
    }

    fn limit_error(kind: QueryLimitKind, limit: u64, actual: u64) -> QueryError {
        QueryError::LimitExceeded {
            kind,
            limit,
            actual,
        }
    }

    fn emit_pending(&mut self) -> Option<QueryResult<TimeValue>> {
        let event = self.pending.pop_front()?;
        if let Err(error) = self.context.check() {
            self.terminated = true;
            self.pending.clear();
            return Some(Err(error));
        }

        let next_rows = self.rows.saturating_add(1);
        if let Some(limit) = self.context.limits().max_rows() {
            if next_rows > limit {
                self.terminated = true;
                self.pending.clear();
                return Some(Err(Self::limit_error(
                    QueryLimitKind::Rows,
                    limit,
                    next_rows,
                )));
            }
        }

        let event_bytes = logical_event_bytes(&event.name, &event.value);
        let next_bytes = self.logical_bytes.saturating_add(event_bytes);
        if let Some(limit) = self.context.limits().max_result_bytes() {
            if next_bytes > limit {
                self.terminated = true;
                self.pending.clear();
                return Some(Err(Self::limit_error(
                    QueryLimitKind::ResultBytes,
                    limit,
                    next_bytes,
                )));
            }
        }

        self.rows = next_rows;
        self.logical_bytes = next_bytes;
        Some(Ok(TimeValue {
            signal: event.name.to_string(),
            time: event.time,
            value: event.value,
        }))
    }
}

impl Iterator for OpenedTimeValueIter {
    type Item = QueryResult<TimeValue>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.terminated {
            return None;
        }
        if let Some(event) = self.emit_pending() {
            return Some(event);
        }

        match self.stream.next()? {
            Ok(change) => {
                self.queue_change(change);
                self.emit_pending()
            }
            Err(error) => {
                self.terminated = true;
                Some(Err(error))
            }
        }
    }
}
