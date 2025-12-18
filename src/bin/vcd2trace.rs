use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use env_logger::Env;
use log::info;
use serde::Deserialize;

use vcd_tools_rs::{
    ChangeValue, TimeValue, TimeWindow, build_target_map, read_signals_with_offset,
    time_value_iter_from_body,
};

#[derive(Parser, Debug)]
#[command(about = "Extract architectural trace from a VCD (Aldebaran-style)")]
struct Args {
    /// VCD input file
    vcd: PathBuf,
    /// Trace output file (default: trace.txt)
    #[arg(long, default_value = "trace.txt")]
    output: PathBuf,
    /// Optional JSON file providing explicit signal paths
    #[arg(long, value_name = "JSON")]
    signal_path: Option<PathBuf>,
    /// Logging level (error, warn, info, debug, trace)
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[derive(Debug, Clone)]
struct SignalPaths {
    clk: String,
    rst_n: String,
    inst_retired: String,
    instruction: String,
    pc: String,
    rf_we_qual: String,
    rf_we_lsu: String,
    rd_addr: String,
    rd_wdata: String,
    rd_wdata_lsu: String,
}

#[derive(Debug, Deserialize)]
struct PathOverrides {
    clk: Option<String>,
    rst_n: Option<String>,
    inst_retired: Option<String>,
    instruction: Option<String>,
    pc: Option<String>,
    rf_we_qual: Option<String>,
    rf_we_lsu: Option<String>,
    rd_addr: Option<String>,
    rd_wdata: Option<String>,
    rd_wdata_lsu: Option<String>,
}

fn default_paths() -> SignalPaths {
    let cpu = "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU";
    let clk_parent = "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU";
    let rf = "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF";
    SignalPaths {
        clk: format!("{clk_parent}.clk_cpu"),
        rst_n: format!("{cpu}.rst_cpu_n"),
        inst_retired: format!("{cpu}.inst_retired"),
        instruction: format!("{cpu}.instruction_r[31:0]"),
        pc: format!("{cpu}.pc_exe_r[21:0]"),
        rf_we_qual: format!("{rf}.rf_write_enable_qual"),
        rf_we_lsu: format!("{rf}.rf_write_enable_lsu"),
        rd_addr: format!("{rf}.rd_addr[4:0]"),
        rd_wdata: format!("{rf}.rd_wdata[31:0]"),
        rd_wdata_lsu: format!("{rf}.rd_wdata_lsu[31:0]"),
    }
}

fn load_paths(signal_path: &Option<PathBuf>) -> Result<SignalPaths> {
    let mut base = default_paths();
    if let Some(path) = signal_path {
        let file = File::open(path)
            .with_context(|| format!("opening signal path json {}", path.display()))?;
        let overrides: PathOverrides =
            serde_json::from_reader(file).with_context(|| format!("parsing {}", path.display()))?;
        if let Some(v) = overrides.clk {
            base.clk = v;
        }
        if let Some(v) = overrides.rst_n {
            base.rst_n = v;
        }
        if let Some(v) = overrides.inst_retired {
            base.inst_retired = v;
        }
        if let Some(v) = overrides.instruction {
            base.instruction = v;
        }
        if let Some(v) = overrides.pc {
            base.pc = v;
        }
        if let Some(v) = overrides.rf_we_qual {
            base.rf_we_qual = v;
        }
        if let Some(v) = overrides.rf_we_lsu {
            base.rf_we_lsu = v;
        }
        if let Some(v) = overrides.rd_addr {
            base.rd_addr = v;
        }
        if let Some(v) = overrides.rd_wdata {
            base.rd_wdata = v;
        }
        if let Some(v) = overrides.rd_wdata_lsu {
            base.rd_wdata_lsu = v;
        }
    }
    Ok(base)
}

fn value_to_bool(val: &ChangeValue) -> Option<bool> {
    match val {
        ChangeValue::Integer(i) => Some(*i != 0),
        ChangeValue::Text(t) => match t.as_str() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        },
        ChangeValue::Float(_) => None,
    }
}

fn value_to_u64(val: &ChangeValue) -> Option<u64> {
    match val {
        ChangeValue::Integer(i) => Some(*i as u64),
        ChangeValue::Text(t) => t.trim().parse::<u64>().ok(),
        ChangeValue::Float(_) => None,
    }
}

fn process_cycle(
    paths: &SignalPaths,
    state: &HashMap<String, ChangeValue>,
    out: &mut dyn Write,
) -> Result<()> {
    let rst = state
        .get(&paths.rst_n)
        .and_then(value_to_bool)
        .unwrap_or(false);
    if !rst {
        return Ok(());
    }

    let inst_retired = state
        .get(&paths.inst_retired)
        .and_then(value_to_bool)
        .unwrap_or(false);
    if inst_retired {
        if let (Some(pc), Some(instr)) = (
            state.get(&paths.pc).and_then(value_to_u64),
            state.get(&paths.instruction).and_then(value_to_u64),
        ) {
            writeln!(out, "PC : 0x{pc:08x} I : 0x{instr:08x} ")?;
        }
    }

    let rf_we_qual = state
        .get(&paths.rf_we_qual)
        .and_then(value_to_bool)
        .unwrap_or(false);
    let rf_we_lsu = state
        .get(&paths.rf_we_lsu)
        .and_then(value_to_bool)
        .unwrap_or(false);

    if rf_we_qual {
        if let (Some(addr), Some(data)) = (
            state.get(&paths.rd_addr).and_then(value_to_u64),
            state.get(&paths.rd_wdata).and_then(value_to_u64),
        ) {
            writeln!(out, "x{addr:02} <= 0x{data:08x}")?;
        }
    } else if rf_we_lsu {
        if let (Some(addr), Some(data)) = (
            state.get(&paths.rd_addr).and_then(value_to_u64),
            state.get(&paths.rd_wdata_lsu).and_then(value_to_u64),
        ) {
            writeln!(out, "x{addr:02} <= 0x{data:08x}")?;
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    env_logger::Builder::from_env(Env::default().default_filter_or(&args.log_level))
        .format_timestamp(None)
        .format_target(false)
        .format_module_path(false)
        .init();

    let paths = load_paths(&args.signal_path)?;
    if let Some(p) = &args.signal_path {
        info!("using signal paths from {}", p.display());
    } else {
        info!("using built-in signal paths");
    }

    let (_signals, index, _timescale, offset) = read_signals_with_offset(&args.vcd)?;
    let target_names = vec![
        paths.clk.clone(),
        paths.rst_n.clone(),
        paths.inst_retired.clone(),
        paths.instruction.clone(),
        paths.pc.clone(),
        paths.rf_we_qual.clone(),
        paths.rf_we_lsu.clone(),
        paths.rd_addr.clone(),
        paths.rd_wdata.clone(),
        paths.rd_wdata_lsu.clone(),
    ];
    let (target_map, missing) = build_target_map(&index, &target_names);
    if !missing.is_empty() {
        bail!("signals not found in VCD: {}", missing.join(", "));
    }

    let iter = time_value_iter_from_body(&args.vcd, target_map, TimeWindow::default(), offset)?;
    let mut state: HashMap<String, ChangeValue> = HashMap::new();
    let mut prev_clk: Option<bool> = None;
    let mut started = false;
    let mut current_time: Option<u64> = None;
    let mut buffer: Vec<TimeValue> = Vec::new();

    let mut out = File::create(&args.output)
        .with_context(|| format!("creating {}", args.output.display()))?;

    let process_buffer = |buffer: &mut Vec<TimeValue>,
                          state: &mut HashMap<String, ChangeValue>,
                          prev_clk: &mut Option<bool>,
                          started: &mut bool,
                          out: &mut File|
     -> Result<()> {
        // Detect a rising edge in this timestamp before applying updates.
        let mut rising = false;
        for evt in buffer.iter() {
            if evt.signal == paths.clk {
                let new_val = value_to_bool(&evt.value);
                if let (Some(false), Some(true)) = (*prev_clk, new_val) {
                    rising = true;
                    break;
                }
            }
        }

        if rising {
            if *started {
                process_cycle(&paths, state, out)?;
            } else {
                *started = true;
            }
        }

        for evt in buffer.drain(..) {
            if evt.signal == paths.clk {
                *prev_clk = value_to_bool(&evt.value);
            }
            state.insert(evt.signal.clone(), evt.value.clone());
        }

        Ok(())
    };

    for evt_res in iter {
        let evt = evt_res?;
        if current_time.is_none() {
            current_time = Some(evt.time);
        }
        if Some(evt.time) != current_time {
            process_buffer(
                &mut buffer,
                &mut state,
                &mut prev_clk,
                &mut started,
                &mut out,
            )?;
            current_time = Some(evt.time);
        }
        buffer.push(evt);
    }

    if !buffer.is_empty() {
        process_buffer(
            &mut buffer,
            &mut state,
            &mut prev_clk,
            &mut started,
            &mut out,
        )?;
    }

    if started {
        process_cycle(&paths, &state, &mut out)?;
    }

    info!(
        "trace extracted to {} (from {})",
        args.output.display(),
        args.vcd.display()
    );
    Ok(())
}
