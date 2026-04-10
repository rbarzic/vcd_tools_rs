use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{ArgAction, Parser, Subcommand};
use log::LevelFilter;

use vcd_tools_rs::{
    TimeValue, TimeWindow, VcdError, build_sizes, build_target_map, find_nth_occurrence,
    format_value_for_signal, list_signals, load_signal_list, parse_target_value, read_signals,
    read_signals_with_offset, read_vcd_metadata,
    compare_vcd_files, ComparisonOptions,
};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Stream VCD files and query signals (Rust edition)"
)]
struct Cli {
    #[arg(
        long,
        default_value = "info",
        help = "Logging level (info, debug, warn, error)"
    )]
    log_level: String,
    #[arg(long, action = ArgAction::SetTrue, help = "Render output using tables")]
    pretty: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List signals declared in a VCD header
    List {
        vcd: PathBuf,
        #[arg(long, help = "Substring filter applied to signal names")]
        filter: Option<String>,
    },
    /// Show metadata for a VCD file
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
    /// Compare two VCD files and report differences
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
        #[arg(
            long,
            help = "Output format (default, json, compact)"
        )]
        output: Option<String>,
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

fn print_metadata(meta: &vcd_tools_rs::VcdMeta, pretty: bool) {
    let timescale = meta
        .timescale
        .as_ref()
        .map(|t| format!("{} {}", t.magnitude, t.unit))
        .unwrap_or_else(|| "n/a".to_string());
    if pretty {
        let rows = vec![
            vec!["signals".to_string(), meta.signal_count.to_string()],
            vec!["timescale".to_string(), timescale.clone()],
            vec!["start_time".to_string(), meta.start_time.to_string()],
            vec!["end_time".to_string(), meta.end_time.to_string()],
        ];
        print_table(&["Field", "Value"], &rows);
    } else {
        println!("signals\t{}", meta.signal_count);
        println!("timescale\t{}", timescale);
        println!("start_time\t{}", meta.start_time);
        println!("end_time\t{}", meta.end_time);
    }
}

fn emit_time_aligned<I>(
    events: I,
    signal_order: &[String],
    sizes: &HashMap<String, u32>,
    pretty: bool,
) -> Result<()>
where
    I: IntoIterator<Item = Result<TimeValue, VcdError>>,
{
    let headers: Vec<String> = std::iter::once("time".to_string())
        .chain(signal_order.iter().cloned())
        .collect();
    let mut last_values: HashMap<String, String> = signal_order
        .iter()
        .map(|s| (s.clone(), String::new()))
        .collect();
    let mut current_time: Option<u64> = None;
    let mut rows: Vec<Vec<String>> = Vec::new();

    if !pretty {
        println!("{}", headers.join("\t"));
    }

    let emit_row =
        |time_value: u64, values: &HashMap<String, String>, pretty_store: &mut Vec<Vec<String>>| {
            let mut row: Vec<String> = Vec::with_capacity(signal_order.len() + 1);
            row.push(time_value.to_string());
            for name in signal_order {
                row.push(values.get(name).cloned().unwrap_or_default());
            }
            if pretty {
                pretty_store.push(row);
            } else {
                println!("{}", row.join("\t"));
            }
        };

    for evt in events {
        let evt = evt?;
        if current_time.is_none() {
            current_time = Some(evt.time);
        }
        if let Some(ct) = current_time {
            if evt.time != ct {
                emit_row(ct, &last_values, &mut rows);
                current_time = Some(evt.time);
            }
        }
        let size = *sizes.get(&evt.signal).unwrap_or(&1);
        last_values.insert(
            evt.signal.clone(),
            format_value_for_signal(&evt.value, size),
        );
    }

    if let Some(ct) = current_time {
        emit_row(ct, &last_values, &mut rows);
    }

    if pretty {
        let header_refs: Vec<&str> = headers.iter().map(|h| h.as_str()).collect();
        print_table(&header_refs, &rows);
    }

    Ok(())
}

fn handle_list(vcd: &Path, filter: Option<String>, pretty: bool) -> Result<()> {
    let (signals, _index, _timescale) = read_signals(vcd)?;
    let names = list_signals(&signals, filter.as_deref());
    print_signal_list(&names, pretty);
    Ok(())
}

fn handle_meta(vcd: &Path, pretty: bool) -> Result<()> {
    let meta = read_vcd_metadata(vcd)?;
    print_metadata(&meta, pretty);
    Ok(())
}

fn handle_extract(
    vcd: &Path,
    signals: Vec<String>,
    signals_file: Option<PathBuf>,
    window: TimeWindow,
    pretty: bool,
) -> Result<()> {
    let names = collect_signal_names(&signals, signals_file.as_deref())?;
    let (all_signals, index, _timescale, offset) = read_signals_with_offset(vcd)?;
    let (target_map, missing) = build_target_map(&index, &names);
    if !missing.is_empty() {
        bail!("Signals not found in VCD: {}", missing.join(", "));
    }

    let sizes = build_sizes(&all_signals);
    let iter = vcd_tools_rs::time_value_iter_from_body(vcd, target_map, window, offset)?;
    emit_time_aligned(iter, &names, &sizes, pretty)?;
    Ok(())
}

fn handle_find(
    vcd: &Path,
    signal: String,
    value: String,
    occurrence: usize,
    window: TimeWindow,
    pretty: bool,
) -> Result<()> {
    if signal.split(',').filter(|s| !s.trim().is_empty()).count() != 1 {
        bail!("Provide exactly one signal for find (no comma-separated lists).");
    }

    let parsed_value = parse_target_value(&value);
    let (event, size_bits) = find_nth_occurrence(vcd, &signal, parsed_value, occurrence, window)?;
    let event = match event {
        Some(e) => e,
        None => bail!("No matching occurrence found in the specified window."),
    };

    let formatted_value = format_value_for_signal(&event.value, size_bits);
    let headers = vec!["time".to_string(), signal.clone()];
    let row = vec![event.time.to_string(), formatted_value];
    if pretty {
        let header_refs: Vec<&str> = headers.iter().map(|h| h.as_str()).collect();
        print_table(&header_refs, &[row]);
    } else {
        println!("{}", headers.join("\t"));
        println!("{}", row.join("\t"));
    }

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    configure_logging(&cli.log_level);

    match cli.command {
        Commands::List { vcd, filter } => handle_list(&vcd, filter, cli.pretty),
        Commands::Meta { vcd } => handle_meta(&vcd, cli.pretty),
        Commands::Extract {
            vcd,
            signals,
            signals_file,
            start,
            end,
        } => {
            let window = TimeWindow { start, end };
            handle_extract(&vcd, signals, signals_file, window, cli.pretty)
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
            handle_find(&vcd, signal, value, occurrence, window, cli.pretty)
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

            let result = compare_vcd_files(
                reference.to_str().unwrap_or("reference.vcd"),
                actual.to_str().unwrap_or("actual.vcd"),
                &options,
            )?;

            handle_compare(&result, output.as_deref())
        }
    }
}

fn handle_compare(result: &vcd_tools_rs::ComparisonResult, output_format: Option<&str>) -> Result<()> {
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
                println!("❌ FAIL: {}/{} signals have mismatches ({} total)",
                    result.signals_with_mismatches,
                    result.common_signals.len(),
                    result.total_mismatches);
            }
        }
        _ => {
            // Default detailed format
            println!("==========================================");
            println!("VCD Comparison Results");
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
            let mut mismatches_by_signal: std::collections::HashMap<&String, Vec<&vcd_tools_rs::SignalMismatch>> =
                std::collections::HashMap::new();
            for mm in &result.mismatches {
                mismatches_by_signal
                    .entry(&mm.signal_name)
                    .or_insert_with(Vec::new)
                    .push(mm);
            }

            if result.passed {
                println!("All common signals match ({} total).", result.common_signals.len());
                println!();
            } else {
                let mut mismatch_signals: Vec<&String> = mismatches_by_signal.keys().copied().collect();
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
                        println!("  Time #{}: Ref='{}' | Actual='{}' {}",
                            mm.time, val1_str, val2_str, status);
                    }
                    if mismatches.len() > 10 {
                        println!("  ... and {} more", mismatches.len() - 10);
                    }
                    println!("  ❌ {} mismatches", mismatches.len());
                    println!();
                }
                println!(
                    "Matched signals: {} / {}",
                    result.common_signals.len().saturating_sub(result.signals_with_mismatches),
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
                println!("The two VCD files are equivalent.");
            } else {
                println!("❌ FAILURES FOUND");
                println!();
                println!("Total mismatches: {}", result.total_mismatches);
                println!("Signals with mismatches: {} / {}",
                    result.signals_with_mismatches,
                    result.common_signals.len());
            }
            println!();
        }
    }

    Ok(())
}
