use std::collections::HashMap;
#[cfg(unix)]
use std::path::Path;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::opened::OpenedWaveform;
use crate::query::QueryContext;
#[cfg(unix)]
use crate::server::app::run_server_until_signal;
#[cfg(unix)]
use crate::server::runtime::RuntimeConfig;
#[cfg(unix)]
use crate::server::service::ServiceConfig;
use crate::{
    ComparisonOptions, TimeWindow, WaveformFormatHint, detect_waveform_format, parse_target_value,
};

fn open_waveform(path: &str) -> PyResult<OpenedWaveform> {
    OpenedWaveform::open(path).map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

fn query_py(error: crate::query::QueryError) -> PyErr {
    PyRuntimeError::new_err(error.to_string())
}

#[pyfunction]
fn assert_format(path: &str, format: &str) -> PyResult<String> {
    let hint = match format {
        "auto" => WaveformFormatHint::Auto,
        "vcd" => WaveformFormatHint::Vcd,
        "fst" => WaveformFormatHint::Fst,
        _ => return Err(PyRuntimeError::new_err("format must be auto, vcd, or fst")),
    };
    detect_waveform_format(path, hint)
        .map(|value| value.to_string())
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

/// List signal names declared in a VCD or FST waveform.
///
/// Args:
///     path: Path to the VCD or FST waveform.
///     filter: Optional substring; only signals containing it are returned.
///
/// Returns:
///     List of signal name strings.
#[pyfunction]
#[pyo3(signature = (path, filter=None))]
fn list_signals(path: &str, filter: Option<&str>) -> PyResult<Vec<String>> {
    Ok(open_waveform(path)?.list_signals(filter))
}

/// Return metadata for a VCD or FST waveform.
///
/// Returns a dict with keys:
///     signal_count (int), start_time (int), end_time (int),
///     timescale (str | None)  e.g. "1 ns"
#[pyfunction]
fn metadata(py: Python<'_>, path: &str) -> PyResult<PyObject> {
    let (signal_count, timescale, start_time, end_time) = open_waveform(path)?
        .metadata(&QueryContext::legacy_unlimited())
        .map_err(query_py)?;
    let d = PyDict::new(py);
    d.set_item("signal_count", signal_count)?;
    d.set_item("start_time", start_time)?;
    d.set_item("end_time", end_time)?;
    d.set_item(
        "timescale",
        timescale.map(|t| format!("{} {}", t.magnitude, t.unit)),
    )?;
    Ok(d.unbind().into_any())
}

/// Extract time/value pairs for a list of signals.
///
/// Args:
///     path:    Path to the VCD or FST waveform.
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
    let values = open_waveform(path)?
        .extract(&signals, window, &QueryContext::legacy_unlimited())
        .map_err(query_py)?;
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
///     path:    Path to the VCD or FST waveform.
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
    open_waveform(path)?
        .count_toggles(&signals, window, &QueryContext::legacy_unlimited())
        .map_err(query_py)
}

/// Compare two VCD or FST waveforms and report signal mismatches.
///
/// Args:
///     file1:           Path to the first VCD or FST waveform.
///     file2:           Path to the second VCD or FST waveform.
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
    let result = open_waveform(file1)?
        .compare(
            &open_waveform(file2)?,
            &options,
            &QueryContext::legacy_unlimited(),
        )
        .map_err(query_py)?;

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
///     path:       Path to the VCD or FST waveform.
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
    let (result, _size) = open_waveform(path)?
        .find_nth_occurrence(
            signal,
            target,
            occurrence,
            window,
            &QueryContext::legacy_unlimited(),
        )
        .map_err(query_py)?;

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

#[cfg(unix)]
#[pyfunction]
#[pyo3(signature = (
    path,
    socket,
    workers=4,
    queue_depth=64,
    max_connections=32,
    max_active_requests=8,
    output_chunks=8,
    max_request_bytes=1_048_576,
    max_frame_bytes=262_144,
    max_timeout_ms=120_000,
    max_signals=4_096,
    max_rows=1_000_000,
    max_response_bytes=268_435_456,
    max_commands=1_000_000_000
))]
#[allow(clippy::too_many_arguments)]
fn serve(
    py: Python<'_>,
    path: &str,
    socket: &str,
    workers: usize,
    queue_depth: usize,
    max_connections: usize,
    max_active_requests: usize,
    output_chunks: usize,
    max_request_bytes: usize,
    max_frame_bytes: usize,
    max_timeout_ms: u64,
    max_signals: usize,
    max_rows: u64,
    max_response_bytes: u64,
    max_commands: u64,
) -> PyResult<()> {
    if !Path::new(socket).is_absolute() {
        return Err(PyRuntimeError::new_err("socket path must be absolute"));
    }
    let path = path.to_owned();
    let socket = socket.to_owned();
    let runtime = RuntimeConfig {
        max_connections,
        max_active_requests_per_connection: max_active_requests,
        workers,
        queue_depth,
        output_chunks,
        request_line_bytes: max_request_bytes,
        max_encoded_frame_bytes: max_frame_bytes,
        max_timeout_ms,
    };
    let service = ServiceConfig {
        max_signals,
        max_rows,
        max_response_bytes,
        max_commands,
    };
    py.allow_threads(move || run_server_until_signal(path, socket, runtime, service))
        .map_err(|error| PyRuntimeError::new_err(error.to_string()))
}

#[pymodule]
fn vcd_tools(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(assert_format, m)?)?;
    m.add_function(wrap_pyfunction!(list_signals, m)?)?;
    m.add_function(wrap_pyfunction!(metadata, m)?)?;
    m.add_function(wrap_pyfunction!(extract, m)?)?;
    m.add_function(wrap_pyfunction!(toggles, m)?)?;
    m.add_function(wrap_pyfunction!(find, m)?)?;
    m.add_function(wrap_pyfunction!(compare, m)?)?;
    #[cfg(unix)]
    m.add_function(wrap_pyfunction!(serve, m)?)?;
    Ok(())
}
