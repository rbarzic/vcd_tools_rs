use std::collections::HashMap;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::{
    compare_vcd_files, count_toggles, extract_time_values_from_file, find_nth_occurrence,
    list_signals_from_file, parse_target_value, read_vcd_metadata, ComparisonOptions, TimeWindow,
};

fn to_py(e: crate::VcdError) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// List signal names declared in a VCD file.
///
/// Args:
///     path: Path to the VCD file.
///     filter: Optional substring; only signals containing it are returned.
///
/// Returns:
///     List of signal name strings.
#[pyfunction]
#[pyo3(signature = (path, filter=None))]
fn list_signals(path: &str, filter: Option<&str>) -> PyResult<Vec<String>> {
    list_signals_from_file(path, filter).map_err(to_py)
}

/// Return metadata for a VCD file.
///
/// Returns a dict with keys:
///     signal_count (int), start_time (int), end_time (int),
///     timescale (str | None)  e.g. "1 ns"
#[pyfunction]
fn metadata(py: Python<'_>, path: &str) -> PyResult<PyObject> {
    let meta = read_vcd_metadata(path).map_err(to_py)?;
    let d = PyDict::new(py);
    d.set_item("signal_count", meta.signal_count)?;
    d.set_item("start_time", meta.start_time)?;
    d.set_item("end_time", meta.end_time)?;
    d.set_item(
        "timescale",
        meta.timescale.map(|t| format!("{} {}", t.magnitude, t.unit)),
    )?;
    Ok(d.unbind().into_any())
}

/// Extract time/value pairs for a list of signals.
///
/// Args:
///     path:    Path to the VCD file.
///     signals: List of fully-qualified signal names.
///     start:   Optional start time (inclusive).
///     end:     Optional end time (inclusive).
///
/// Returns:
///     List of dicts with keys: signal (str), time (int), value (str).
#[pyfunction]
#[pyo3(signature = (path, signals, start=None, end=None))]
fn extract(
    py: Python<'_>,
    path: &str,
    signals: Vec<String>,
    start: Option<u64>,
    end: Option<u64>,
) -> PyResult<Vec<PyObject>> {
    let window = TimeWindow { start, end };
    let values = extract_time_values_from_file(path, &signals, window).map_err(to_py)?;
    values
        .iter()
        .map(|tv| {
            let d = PyDict::new(py);
            d.set_item("signal", &tv.signal)?;
            d.set_item("time", tv.time)?;
            d.set_item("value", tv.value.to_string())?;
            Ok(d.unbind().into_any())
        })
        .collect()
}

/// Count value transitions (toggles) for a list of signals.
///
/// Args:
///     path:    Path to the VCD file.
///     signals: List of fully-qualified signal names.
///     start:   Optional start time (inclusive).
///     end:     Optional end time (inclusive).
///
/// Returns:
///     Dict mapping signal name → toggle count (int).
#[pyfunction]
#[pyo3(signature = (path, signals, start=None, end=None))]
fn toggles(
    path: &str,
    signals: Vec<String>,
    start: Option<u64>,
    end: Option<u64>,
) -> PyResult<HashMap<String, usize>> {
    let window = TimeWindow { start, end };
    count_toggles(path, &signals, window).map_err(to_py)
}

/// Compare two VCD files and report signal mismatches.
///
/// Args:
///     file1:           Path to the first VCD file.
///     file2:           Path to the second VCD file.
///     max_mismatches:  Optional per-signal mismatch limit.
///     signals:         Optional list of signal names to restrict comparison to.
///     ignore_unknown:  If True, treat x/z differences as matches.
///     start:           Optional start time (inclusive).
///     end:             Optional end time (inclusive).
///
/// Returns:
///     Dict with keys:
///         passed (bool), file1 (str), file2 (str),
///         common_signals (list[str]),
///         signals_only_in_file1 (list[str]),
///         signals_only_in_file2 (list[str]),
///         total_mismatches (int), signals_with_mismatches (int),
///         mismatches (list[dict]) each with:
///             signal (str), time (int), value1 (str), value2 (str),
///             is_unknown (bool)
#[pyfunction]
#[pyo3(signature = (file1, file2, *, max_mismatches=None, signals=None, ignore_unknown=false, start=None, end=None))]
fn compare(
    py: Python<'_>,
    file1: &str,
    file2: &str,
    max_mismatches: Option<usize>,
    signals: Option<Vec<String>>,
    ignore_unknown: bool,
    start: Option<u64>,
    end: Option<u64>,
) -> PyResult<PyObject> {
    let options = ComparisonOptions {
        max_mismatches,
        signals_only: signals.unwrap_or_default(),
        ignore_unknown,
        time_window: TimeWindow { start, end },
    };
    let result = compare_vcd_files(file1, file2, &options).map_err(to_py)?;

    let d = PyDict::new(py);
    d.set_item("passed", result.passed)?;
    d.set_item("file1", &result.file1)?;
    d.set_item("file2", &result.file2)?;
    d.set_item("common_signals", &result.common_signals)?;
    d.set_item("signals_only_in_file1", &result.signals_only_in_file1)?;
    d.set_item("signals_only_in_file2", &result.signals_only_in_file2)?;
    d.set_item("total_mismatches", result.total_mismatches)?;
    d.set_item("signals_with_mismatches", result.signals_with_mismatches)?;

    let mismatches: PyResult<Vec<PyObject>> = result
        .mismatches
        .iter()
        .map(|m| {
            let md = PyDict::new(py);
            md.set_item("signal", &m.signal_name)?;
            md.set_item("time", m.time)?;
            md.set_item("value1", m.value1.to_string())?;
            md.set_item("value2", m.value2.to_string())?;
            md.set_item("is_unknown", m.is_unknown)?;
            Ok(md.unbind().into_any())
        })
        .collect();
    d.set_item("mismatches", mismatches?)?;

    Ok(d.unbind().into_any())
}

/// Find the Nth occurrence of a signal reaching a target value.
///
/// Args:
///     path:       Path to the VCD file.
///     signal:     Fully-qualified signal name.
///     value:      Target value string (decimal, "0x…" hex, or "x"/"z").
///     occurrence: Which occurrence to find (default 1).
///     start:      Optional start time (inclusive).
///     end:        Optional end time (inclusive).
///
/// Returns:
///     Dict with keys: found (bool), signal (str), time (int), value (str).
///     If not found, time and value are None.
#[pyfunction]
#[pyo3(signature = (path, signal, value, occurrence=1, start=None, end=None))]
fn find(
    py: Python<'_>,
    path: &str,
    signal: &str,
    value: &str,
    occurrence: usize,
    start: Option<u64>,
    end: Option<u64>,
) -> PyResult<PyObject> {
    let window = TimeWindow { start, end };
    let target = parse_target_value(value);
    let (result, _size) =
        find_nth_occurrence(path, signal, target, occurrence, window).map_err(to_py)?;

    let d = PyDict::new(py);
    match result {
        Some(tv) => {
            d.set_item("found", true)?;
            d.set_item("signal", &tv.signal)?;
            d.set_item("time", tv.time)?;
            d.set_item("value", tv.value.to_string())?;
        }
        None => {
            d.set_item("found", false)?;
            d.set_item("signal", signal)?;
            d.set_item("time", py.None())?;
            d.set_item("value", py.None())?;
        }
    }
    Ok(d.unbind().into_any())
}

#[pymodule]
fn vcd_tools(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(list_signals, m)?)?;
    m.add_function(wrap_pyfunction!(metadata, m)?)?;
    m.add_function(wrap_pyfunction!(extract, m)?)?;
    m.add_function(wrap_pyfunction!(toggles, m)?)?;
    m.add_function(wrap_pyfunction!(find, m)?)?;
    m.add_function(wrap_pyfunction!(compare, m)?)?;
    Ok(())
}
