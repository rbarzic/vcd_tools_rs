use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use log::LevelFilter;

use vcd_tools_rs::opened::OpenedWaveform;
use vcd_tools_rs::query::QueryContext;
#[cfg(unix)]
use vcd_tools_rs::server::app::run_server_until_signal;
#[cfg(unix)]
use vcd_tools_rs::server::runtime::RuntimeConfig;
#[cfg(unix)]
use vcd_tools_rs::server::service::ServiceConfig;
use vcd_tools_rs::{
    ComparisonOptions, TimeWindow, WaveformFormat, WaveformFormatHint, detect_waveform_format,
    format_value_for_signal, load_signal_list, parse_target_value,
};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Stream VCD/FST waveform files and query signals (Rust edition)"
)]
#[command()]
struct Cli {
    #[command(flatten)]
    global_args: GlobalArgs,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FormatArg {
    Auto,
    Vcd,
    Fst,
}

impl From<FormatArg> for WaveformFormatHint {
    fn from(value: FormatArg) -> Self {
        match value {
            FormatArg::Auto => Self::Auto,
            FormatArg::Vcd => Self::Vcd,
            FormatArg::Fst => Self::Fst,
        }
    }
}

#[derive(clap::Args, Debug)]
struct GlobalArgs {
    #[arg(
        long,
        default_value = "info",
        help = "Logging level (info, debug, warn, error)"
    )]
    log_level: String,
    #[arg(long, value_enum, default_value_t = FormatArg::Auto, global = true, help = "Input format assertion")]
    format: FormatArg,
    #[arg(long, action = ArgAction::SetTrue, help = "Render output using tables", global = true)]
    pretty: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List signals declared in a waveform header
    List {
        vcd: PathBuf,
        #[arg(long, help = "Substring filter applied to signal names")]
        filter: Option<String>,
    },
    /// Show metadata for a VCD or FST waveform
    Meta { vcd: PathBuf },
    /// Extract time/value pairs for specific signals
    Extract {
        vcd: PathBuf,
        #[arg(long = "signal", action = ArgAction::Append, help = "Signal name to extract (repeatable)")]
        signals: Vec<String>,
        #[arg(
            long = "signals-file",
            help = "Text file with one signal name per line"
        )]
        signals_file: Option<PathBuf>,
        #[arg(long, help = "Start time (inclusive)")]
        start: Option<u64>,
        #[arg(long, help = "End time (inclusive)")]
        end: Option<u64>,
    },
    /// Count the number of value transitions (toggles) for specific signals
    Toggle {
        vcd: PathBuf,
        #[arg(long = "signal", action = ArgAction::Append, help = "Signal name to count toggles for (repeatable)")]
        signals: Vec<String>,
        #[arg(
            long = "signals-file",
            help = "Text file with one signal name per line"
        )]
        signals_file: Option<PathBuf>,
        #[arg(long, help = "Start time (inclusive)")]
        start: Option<u64>,
        #[arg(long, help = "End time (inclusive)")]
        end: Option<u64>,
    },
    /// Find the Nth occurrence of a signal reaching a target value
    Find {
        vcd: PathBuf,
        #[arg(long, help = "Signal to watch")]
        signal: String,
        #[arg(
            long,
            help = "Target value to match (digits or 0x prefixed hex or z/x)"
        )]
        value: String,
        #[arg(
            long,
            alias = "occurence",
            default_value_t = 1,
            help = "Which occurrence to return"
        )]
        occurrence: usize,
        #[arg(long, help = "Start time (inclusive)")]
        start: Option<u64>,
        #[arg(long, help = "End time (inclusive)")]
        end: Option<u64>,
    },
    /// Compare two VCD/FST waveforms and report differences
    Compare {
        reference: PathBuf,
        actual: PathBuf,
        #[arg(long, help = "Limit number of mismatches per signal")]
        max_mismatches: Option<usize>,
        #[arg(long, help = "Only compare specific signals (comma-separated)")]
        signals_only: Option<String>,
        #[arg(
            long,
            action = ArgAction::SetTrue,
            help = "Ignore x/z differences when comparing"
        )]
        ignore_unknown: bool,
        #[arg(long, help = "Start time (inclusive)")]
        start: Option<u64>,
        #[arg(long, help = "End time (inclusive)")]
        end: Option<u64>,
        #[arg(long, help = "Output format (default, json, compact)")]
        output: Option<String>,
    },
    /// Serve one immutable VCD or FST generation over an owner-only Unix socket
    #[cfg(unix)]
    Serve {
        vcd: PathBuf,
        #[arg(long)]
        socket: PathBuf,
        #[arg(long, default_value_t = 4)]
        workers: usize,
        #[arg(long, default_value_t = 64)]
        queue_depth: usize,
        #[arg(long, default_value_t = 32)]
        max_connections: usize,
        #[arg(long, default_value_t = 8)]
        max_active_requests: usize,
        #[arg(long, default_value_t = 8)]
        output_chunks: usize,
        #[arg(long, default_value_t = 1_048_576)]
        max_request_bytes: usize,
        #[arg(long, default_value_t = 262_144)]
        max_frame_bytes: usize,
        #[arg(long, default_value_t = 120_000)]
        max_timeout_ms: u64,
        #[arg(long, default_value_t = 4_096)]
        max_signals: usize,
        #[arg(long, default_value_t = 1_000_000)]
        max_rows: u64,
        #[arg(long, default_value_t = 268_435_456)]
        max_response_bytes: u64,
        #[arg(long, default_value_t = 1_000_000_000)]
        max_commands: u64,
    },
}

fn configure_logging(level: &str) {
    let filter = level.parse::<LevelFilter>().unwrap_or(LevelFilter::Info);
    let _ = env_logger::Builder::from_default_env()
        .filter_level(filter)
        .format_timestamp(None)
        .format_module_path(false)
        .format_target(false)
        .try_init();
}

fn collect_signal_names(signals: &[String], signals_file: Option<&Path>) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for entry in signals {
        for part in entry.split(',') {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                names.push(trimmed.to_string());
            }
        }
    }
    if let Some(file) = signals_file {
        names.extend(load_signal_list(file)?);
    }
    if names.is_empty() {
        bail!("At least one --signal or --signals-file entry is required");
    }
    Ok(names)
}

fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (idx, col) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(idx) {
                *w = (*w).max(col.len());
            }
        }
    }

    let separator = widths
        .iter()
        .map(|w| "-".repeat(*w))
        .collect::<Vec<_>>()
        .join("-+-");

    print_row(
        headers.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        &widths,
    );
    println!("{}", separator);
    for row in rows {
        print_row(row.clone(), &widths);
    }
}

fn print_row(row: Vec<String>, widths: &[usize]) {
    let parts: Vec<String> = row
        .into_iter()
        .zip(widths.iter().cloned())
        .map(|(col, width)| format!("{col:<width$}"))
        .collect();
    println!("{}", parts.join(" | "));
}

fn print_signal_list(names: &[String], pretty: bool) {
    if pretty {
        let rows: Vec<Vec<String>> = names.iter().map(|n| vec![n.clone()]).collect();
        print_table(&["Name"], &rows);
    } else {
        for name in names {
            println!("{}", name);
        }
    }
}

fn open_waveform(path: &Path, hint: WaveformFormatHint) -> Result<OpenedWaveform> {
    OpenedWaveform::open_with_hint(path, hint).map_err(Into::into)
}

fn handle_list(
    vcd: &Path,
    filter: Option<String>,
    pretty: bool,
    hint: WaveformFormatHint,
) -> Result<()> {
    let opened = open_waveform(vcd, hint)?;
    if pretty {
        print_signal_list(&opened.list_signals(filter.as_deref()), true);
    } else {
        opened.visit_signal_names(&QueryContext::legacy_unlimited(), |name| {
            if filter.as_ref().is_none_or(|needle| name.contains(needle)) {
                println!("{name}");
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn handle_meta(vcd: &Path, pretty: bool, hint: WaveformFormatHint) -> Result<()> {
    let opened = open_waveform(vcd, hint)?;
    let (signal_count, timescale, start_time, end_time) =
        opened.metadata(&QueryContext::legacy_unlimited())?;
    let timescale = timescale
        .map(|value| format!("{} {}", value.magnitude, value.unit))
        .unwrap_or_else(|| "n/a".into());
    let rows = vec![
        vec!["signals".into(), signal_count.to_string()],
        vec!["timescale".into(), timescale],
        vec!["start_time".into(), start_time.to_string()],
        vec!["end_time".into(), end_time.to_string()],
    ];
    if pretty {
        print_table(&["Field", "Value"], &rows);
    } else {
        for row in rows {
            println!("{}\t{}", row[0], row[1]);
        }
    }
    Ok(())
}

fn handle_extract(
    vcd: &Path,
    signals: Vec<String>,
    signals_file: Option<PathBuf>,
    window: TimeWindow,
    pretty: bool,
    hint: WaveformFormatHint,
) -> Result<()> {
    let names = collect_signal_names(&signals, signals_file.as_deref())?;
    let opened = open_waveform(vcd, hint)?;
    let sizes = names
        .iter()
        .filter_map(|name| opened.width(name).map(|width| (name.clone(), width)))
        .collect::<HashMap<_, _>>();
    if sizes.len() != names.len() {
        let missing = names
            .iter()
            .filter(|name| !sizes.contains_key(*name))
            .cloned()
            .collect::<Vec<_>>();
        let format = if opened.format() == WaveformFormat::Vcd {
            "VCD"
        } else {
            "FST"
        };
        bail!("Signals not found in {format}: {}", missing.join(", "));
    }
    let headers: Vec<String> = std::iter::once("time".to_string())
        .chain(names.iter().cloned())
        .collect();
    let mut last_values = names
        .iter()
        .map(|name| (name.clone(), String::new()))
        .collect::<HashMap<_, _>>();
    let mut current_time = None;
    let mut rows = Vec::new();
    if !pretty {
        println!("{}", headers.join("\t"));
    }
    let emit = |time: u64, values: &HashMap<String, String>, rows: &mut Vec<Vec<String>>| {
        let row = std::iter::once(time.to_string())
            .chain(names.iter().map(|name| values[name].clone()))
            .collect::<Vec<_>>();
        if pretty {
            rows.push(row);
        } else {
            println!("{}", row.join("\t"));
        }
    };
    opened.visit_changes(&names, window, &QueryContext::legacy_unlimited(), |event| {
        if let Some(time) = current_time
            && time != event.time
        {
            emit(time, &last_values, &mut rows);
        }
        current_time = Some(event.time);
        last_values.insert(
            event.signal.clone(),
            format_value_for_signal(&event.value, sizes[&event.signal]),
        );
        Ok(())
    })?;
    if let Some(time) = current_time {
        emit(time, &last_values, &mut rows);
    }
    if pretty {
        print_table(
            &headers.iter().map(String::as_str).collect::<Vec<_>>(),
            &rows,
        );
    }
    Ok(())
}

fn handle_toggle(
    vcd: &Path,
    signals: Vec<String>,
    signals_file: Option<PathBuf>,
    window: TimeWindow,
    pretty: bool,
    hint: WaveformFormatHint,
) -> Result<()> {
    let names = collect_signal_names(&signals, signals_file.as_deref())?;
    let counts = open_waveform(vcd, hint)?.count_toggles(
        &names,
        window,
        &QueryContext::legacy_unlimited(),
    )?;
    let rows = names
        .iter()
        .map(|name| {
            vec![
                name.clone(),
                counts.get(name).copied().unwrap_or(0).to_string(),
            ]
        })
        .collect::<Vec<_>>();
    if pretty {
        print_table(&["signal", "toggles"], &rows);
    } else {
        println!("signal\ttoggles");
        for row in rows {
            println!("{}", row.join("\t"));
        }
    }
    Ok(())
}

fn handle_find(
    vcd: &Path,
    signal: String,
    value: String,
    occurrence: usize,
    window: TimeWindow,
    pretty: bool,
    hint: WaveformFormatHint,
) -> Result<()> {
    if signal
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .count()
        != 1
    {
        bail!("Provide exactly one signal for find (no comma-separated lists).");
    }
    let (event, width) = open_waveform(vcd, hint)?.find_nth_occurrence(
        &signal,
        parse_target_value(&value),
        occurrence,
        window,
        &QueryContext::legacy_unlimited(),
    )?;
    let event = event
        .ok_or_else(|| anyhow::anyhow!("No matching occurrence found in the specified window."))?;
    let headers = ["time", signal.as_str()];
    let row = vec![
        event.time.to_string(),
        format_value_for_signal(&event.value, width),
    ];
    if pretty {
        print_table(&headers, &[row]);
    } else {
        println!("{}", headers.join("\t"));
        println!("{}", row.join("\t"));
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    configure_logging(&cli.global_args.log_level);
    let pretty = cli.global_args.pretty;
    let hint = cli.global_args.format.into();

    match cli.command {
        Commands::List { vcd, filter } => handle_list(&vcd, filter, pretty, hint),
        Commands::Meta { vcd } => handle_meta(&vcd, pretty, hint),
        Commands::Extract {
            vcd,
            signals,
            signals_file,
            start,
            end,
        } => {
            let window = TimeWindow { start, end };
            handle_extract(&vcd, signals, signals_file, window, pretty, hint)
        }
        Commands::Toggle {
            vcd,
            signals,
            signals_file,
            start,
            end,
        } => {
            let window = TimeWindow { start, end };
            handle_toggle(&vcd, signals, signals_file, window, pretty, hint)
        }
        Commands::Find {
            vcd,
            signal,
            value,
            occurrence,
            start,
            end,
        } => {
            let window = TimeWindow { start, end };
            handle_find(&vcd, signal, value, occurrence, window, pretty, hint)
        }
        Commands::Compare {
            reference,
            actual,
            max_mismatches,
            signals_only,
            ignore_unknown,
            start,
            end,
            output,
        } => {
            let options = ComparisonOptions {
                max_mismatches,
                signals_only: signals_only
                    .unwrap_or_default()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                ignore_unknown,
                time_window: TimeWindow { start, end },
            };

            let reference = OpenedWaveform::open_with_hint(&reference, hint)
                .map_err(|error| anyhow::anyhow!(error))?;
            let actual = OpenedWaveform::open_with_hint(&actual, hint)
                .map_err(|error| anyhow::anyhow!(error))?;
            let result = reference.compare(&actual, &options, &QueryContext::legacy_unlimited())?;

            handle_compare(&result, output.as_deref())
        }
        #[cfg(unix)]
        Commands::Serve {
            vcd,
            socket,
            workers,
            queue_depth,
            max_connections,
            max_active_requests,
            output_chunks,
            max_request_bytes,
            max_frame_bytes,
            max_timeout_ms,
            max_signals,
            max_rows,
            max_response_bytes,
            max_commands,
        } => {
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
            runtime.validate()?;
            service.validate()?;
            detect_waveform_format(&vcd, hint)?;
            if !socket.is_absolute() {
                bail!("--socket must be an absolute path");
            }
            run_server_until_signal(vcd, socket, runtime, service)?;
            Ok(())
        }
    }
}

fn handle_compare(
    result: &vcd_tools_rs::ComparisonResult,
    output_format: Option<&str>,
) -> Result<()> {
    let format = output_format.unwrap_or("default");

    match format {
        "json" => {
            // JSON output format
            let json_result = vcd_tools_rs::JsonComparisonResult::from(result);
            let json = serde_json::to_string_pretty(&json_result)
                .map_err(|e| anyhow::anyhow!("JSON serialization error: {}", e))?;
            println!("{}", json);
        }
        "compact" => {
            // Compact single-line output
            if result.passed {
                println!("✅ PASS: All {} signals match", result.common_signals.len());
            } else {
                println!(
                    "❌ FAIL: {}/{} signals have mismatches ({} total)",
                    result.signals_with_mismatches,
                    result.common_signals.len(),
                    result.total_mismatches
                );
            }
        }
        _ => {
            // Default detailed format
            println!("==========================================");
            println!("Waveform Comparison Results");
            println!("==========================================");
            println!();
            println!("Reference: {}", result.file1);
            println!("Actual:    {}", result.file2);
            println!();

            if !result.signals_only_in_file1.is_empty() {
                println!("Signals only in reference:");
                for sig in &result.signals_only_in_file1 {
                    println!("  - {}", sig);
                }
                println!();
            }

            if !result.signals_only_in_file2.is_empty() {
                println!("Signals only in actual:");
                for sig in &result.signals_only_in_file2 {
                    println!("  - {}", sig);
                }
                println!();
            }

            println!("Common signals: {}", result.common_signals.len());
            println!();

            if result.common_signals.is_empty() {
                println!("⚠️  No common signals to compare!");
                return Ok(());
            }

            println!("==========================================");
            println!("Signal Value Comparison");
            println!("==========================================");
            println!();

            // Group mismatches by signal
            let mut mismatches_by_signal: std::collections::HashMap<
                &String,
                Vec<&vcd_tools_rs::SignalMismatch>,
            > = std::collections::HashMap::new();
            for mm in &result.mismatches {
                mismatches_by_signal
                    .entry(&mm.signal_name)
                    .or_insert_with(Vec::new)
                    .push(mm);
            }

            if result.passed {
                println!(
                    "All common signals match ({} total).",
                    result.common_signals.len()
                );
                println!();
            } else {
                let mut mismatch_signals: Vec<&String> =
                    mismatches_by_signal.keys().copied().collect();
                mismatch_signals.sort();
                for signal_name in mismatch_signals {
                    let mismatches = mismatches_by_signal
                        .get(signal_name)
                        .expect("signal key must exist in mismatch map");
                    println!("Signal: {}", signal_name);
                    for mm in mismatches.iter().take(10) {
                        let val1_str = vcd_tools_rs::format_change_value(&mm.value1);
                        let val2_str = vcd_tools_rs::format_change_value(&mm.value2);
                        let status = if mm.is_unknown {
                            "(one or both unknown)"
                        } else {
                            "❌ MISMATCH"
                        };
                        println!(
                            "  Time #{}: Ref='{}' | Actual='{}' {}",
                            mm.time, val1_str, val2_str, status
                        );
                    }
                    if mismatches.len() > 10 {
                        println!("  ... and {} more", mismatches.len() - 10);
                    }
                    println!("  ❌ {} mismatches", mismatches.len());
                    println!();
                }
                println!(
                    "Matched signals: {} / {}",
                    result
                        .common_signals
                        .len()
                        .saturating_sub(result.signals_with_mismatches),
                    result.common_signals.len()
                );
                println!();
            }

            println!("==========================================");
            println!("Summary");
            println!("==========================================");
            println!();

            if result.passed {
                println!("✅ SUCCESS: All signal values match!");
                println!();
                println!("The two waveform files are equivalent.");
            } else {
                println!("❌ FAILURES FOUND");
                println!();
                println!("Total mismatches: {}", result.total_mismatches);
                println!(
                    "Signals with mismatches: {} / {}",
                    result.signals_with_mismatches,
                    result.common_signals.len()
                );
            }
            println!();
        }
    }

    Ok(())
}
