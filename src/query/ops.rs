use std::collections::HashMap;

use crate::opened::OpenedVcd;
use crate::query::{QueryContext, QueryError, QueryResult};
use crate::{TargetValue, TimeValue, TimeWindow, VcdError, format_value_for_signal};

impl OpenedVcd {
    pub fn find_nth_occurrence(
        &self,
        signal: &str,
        target_value: TargetValue,
        occurrence: usize,
        window: TimeWindow,
        context: &QueryContext,
    ) -> QueryResult<(Option<TimeValue>, u32)> {
        if occurrence < 1 {
            return Err(QueryError::Vcd(VcdError::InvalidOccurrence));
        }
        // Invalid occurrence retains legacy precedence; all remaining
        // preparation is cancellation/deadline aware before lookup or error
        // allocation.
        context.check()?;
        let size = self
            .signal(signal)
            .ok_or_else(|| QueryError::Vcd(VcdError::MissingSignal(signal.to_string())))?
            .size();
        let targets = [signal.to_string()];
        let mut events = self.extract(&targets, window, context)?;
        let normalized_target = target_value.normalize();
        let mut count = 0_usize;
        while let Some(event) = events.next() {
            let event = event?;
            if event.value.normalize() == normalized_target {
                count += 1;
                if count == occurrence {
                    events.validate_current_generation()?;
                    return Ok((Some(event), size));
                }
            }
        }
        Ok((None, size))
    }

    pub fn count_toggles(
        &self,
        targets: &[String],
        window: TimeWindow,
        context: &QueryContext,
    ) -> QueryResult<HashMap<String, usize>> {
        // Check cancellation/deadline and the signal budget before any
        // target-sized map, lookup, or allocation.
        context.check_signal_count(targets.len())?;
        let mut toggle_counts: HashMap<String, usize> =
            targets.iter().map(|name| (name.clone(), 0)).collect();
        let mut last_values: HashMap<String, String> = targets
            .iter()
            .map(|name| (name.clone(), String::new()))
            .collect();
        let sizes: HashMap<String, u32> = targets
            .iter()
            .filter_map(|name| {
                self.signal(name)
                    .map(|signal| (name.clone(), signal.size()))
            })
            .collect();

        let events = self.extract(targets, window, context)?;
        for event in events {
            let event = event?;
            let formatted = format_value_for_signal(
                &event.value,
                sizes.get(&event.signal).copied().unwrap_or(1),
            );
            if let Some(last) = last_values.get(&event.signal) {
                if !last.is_empty() && *last != formatted {
                    if let Some(count) = toggle_counts.get_mut(&event.signal) {
                        *count += 1;
                    }
                }
            }
            last_values.insert(event.signal, formatted);
        }
        Ok(toggle_counts)
    }
}
