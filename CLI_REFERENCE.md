# vcd_tools_rs CLI Reference

A fast streaming VCD (Value Change Dump) analysis tool written in Rust. This document provides a complete reference for all commands and options.

## Table of Contents

- [Installation](#installation)
- [Global Options](#global-options)
- [Commands](#commands)
  - [list](#list)
  - [meta](#meta)
  - [extract](#extract)
  - [find](#find)
- [vcd2trace](#vcd2trace)
- [Output Formats](#output-formats)
- [Examples](#examples)

## Installation

### Precompiled Binaries

Download from [GitHub Releases](https://github.com/rbarzic/vcd_tools_rs/releases):

```sh
tar xzf vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu.tar.gz
./vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu/vcd_tools_rs --help
```

### From Source

```sh
git clone https://github.com/rbarzic/vcd_tools_rs.git
cd vcd_tools_rs
cargo build --release
```

---

## Global Options

These options apply to all commands:

| Option | Default | Description |
|--------|---------|-------------|
| `--log-level <LEVEL>` | `info` | Set logging verbosity. Values: `error`, `warn`, `info`, `debug`, `trace` |
| `--pretty` | false | Render output as formatted tables instead of tab-separated values |

---

## Commands

### list

List all signals declared in a VCD file header.

**Usage:**
```
vcd_tools_rs list [OPTIONS] <VCD>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<VCD>` | Path to the VCD file |

**Options:**

| Option | Description |
|--------|-------------|
| `--filter <SUBSTRING>` | Filter signal names by substring (case-sensitive) |

**Examples:**
```sh
vcd_tools_rs list simulation.vcd
vcd_tools_rs list simulation.vcd --filter counter
vcd_tools_rs list simulation.vcd --filter clk --pretty
```

**Output:**
- Default: One signal name per line
- With `--pretty`: Formatted table with header "Name"

---

### meta

Display metadata about a VCD file.

**Usage:**
```
vcd_tools_rs meta [OPTIONS] <VCD>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<VCD>` | Path to the VCD file |

**Examples:**
```sh
vcd_tools_rs meta simulation.vcd
vcd_tools_rs meta simulation.vcd --pretty
```

**Output Fields:**

| Field | Description |
|-------|-------------|
| `signals` | Total number of signals in the VCD |
| `timescale` | Time resolution (e.g., "1 ns") |
| `start_time` | First timestamp with signal changes |
| `end_time` | Last timestamp with signal changes |

---

### extract

Extract time/value pairs for one or more signals within an optional time window.

**Usage:**
```
vcd_tools_rs extract [OPTIONS] <VCD>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<VCD>` | Path to the VCD file |

**Options:**

| Option | Description |
|--------|-------------|
| `--signal <NAME>` | Signal name to extract. Can be repeated for multiple signals. Also accepts comma-separated list. |
| `--signals-file <FILE>` | Path to a text file containing signal names (one per line, empty lines and whitespace ignored) |
| `--start <TIME>` | Start time (inclusive). If omitted, starts from beginning. |
| `--end <TIME>` | End time (inclusive). If omitted, processes until end of file. |

**Note:** At least one `--signal` or `--signals-file` must be provided.

**Examples:**
```sh
vcd_tools_rs extract sim.vcd --signal "tb.clk"
vcd_tools_rs extract sim.vcd --signal "tb.clk" --signal "tb.rst_n"
vcd_tools_rs extract sim.vcd --signal "tb.clk,tb.rst_n,tb.data"
vcd_tools_rs extract sim.vcd --signals-file signals.txt
vcd_tools_rs extract sim.vcd --signal "tb.counter[7:0]" --start 0 --end 500000 --pretty
```

**Output:**
- Default: Tab-separated values with header row (`time` followed by signal names)
- With `--pretty`: Formatted table

**Output Format:**
```
time	tb.clk	tb.rst_n
0	0	0
10	1	0
20	0	1
...
```

Multi-bit signals are displayed in hexadecimal format (e.g., `0x00ff`).

---

### find

Find the Nth occurrence of a signal reaching a specific target value.

**Usage:**
```
vcd_tools_rs find [OPTIONS] <VCD>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<VCD>` | Path to the VCD file |

**Options:**

| Option | Default | Description |
|--------|---------|-------------|
| `--signal <NAME>` | (required) | Signal name to monitor (exactly one signal) |
| `--value <VALUE>` | (required) | Target value to match. Formats: decimal, `0x`-prefixed hex, or special values (`z`, `x`) |
| `--occurrence <N>` | `1` | Which occurrence to return (1-indexed). Also accepts `--occurence` (alternate spelling). |
| `--start <TIME>` | - | Start time (inclusive) |
| `--end <TIME>` | - | End time (inclusive) |

**Value Formats:**
- Decimal: `0`, `1`, `42`, `255`
- Hexadecimal: `0x0`, `0x1`, `0xff`
- Special: `z` (high-impedance), `x` (unknown)

**Examples:**
```sh
vcd_tools_rs find sim.vcd --signal "tb.clk" --value 1
vcd_tools_rs find sim.vcd --signal "tb.clk" --value 1 --occurrence 5
vcd_tools_rs find sim.vcd --signal "tb.counter[7:0]" --value 0xff --start 1000
vcd_tools_rs find sim.vcd --signal "tb.data" --value z --pretty
```

**Output:**
```
time	tb.clk
10	1
```

---

## vcd2trace

A separate binary that extracts a RISC-V architectural trace from a VCD file (Aldebaran-style format).

**Usage:**
```
vcd2trace [OPTIONS] <VCD>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<VCD>` | Path to the VCD input file |

**Options:**

| Option | Default | Description |
|--------|---------|-------------|
| `--output <FILE>` | `trace.txt` | Output trace file path |
| `--signal-path <JSON>` | - | JSON file with custom signal path mappings |
| `--log-level <LEVEL>` | `info` | Logging verbosity (`error`, `warn`, `info`, `debug`, `trace`) |

**Examples:**
```sh
vcd2trace simulation.vcd
vcd2trace simulation.vcd --output my_trace.txt
vcd2trace simulation.vcd --signal-path custom_paths.json --output trace.txt
```

**Signal Path JSON:**

Create a JSON file to override default signal paths. All fields are optional; only specify what you need to change.

```json
{
  "clk": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.clk_cpu",
  "rst_n": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.rst_cpu_n",
  "inst_retired": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.inst_retired",
  "instruction": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.instruction_r[31:0]",
  "pc": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.pc_exe_r[21:0]",
  "rf_we_qual": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rf_write_enable_qual",
  "rf_we_lsu": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rf_write_enable_lsu",
  "rd_addr": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rd_addr[4:0]",
  "rd_wdata": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rd_wdata[31:0]",
  "rd_wdata_lsu": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rd_wdata_lsu[31:0]"
}
```

**Signal Definitions:**

| Signal | Description |
|--------|-------------|
| `clk` | CPU clock |
| `rst_n` | CPU reset (active low) |
| `inst_retired` | Instruction completion signal |
| `instruction` | The instruction being executed |
| `pc` | Program counter |
| `rf_we_qual` | Write enable signal to register file (qualified) |
| `rf_we_lsu` | Alternate write enable signal to register file (Load-Store Unit) |
| `rd_addr` | Destination register address |
| `rd_wdata` | Register write data (used when `rf_we_qual` is high) |
| `rd_wdata_lsu` | Register write data from LSU (used when `rf_we_lsu` is high) |

**Output Format:**
```
PC : 0x00000100 I : 0x00400113 
x01 <= 0x00000004
PC : 0x00000104 I : 0x00800193 
x03 <= 0x00000008
```

---

## Output Formats

### Default (Tab-Separated)

Output is tab-separated values (TSV), suitable for piping to other tools:

```sh
vcd_tools_rs extract sim.vcd --signal "tb.clk" | cut -f2
```

### Pretty (Table)

With `--pretty`, output is formatted as aligned tables:

```
time       | tb.counter[7:0]
-----------+----------------
0          | 0x00          
100        | 0x01          
200        | 0x02          
```

---

## Examples

### List all clock-related signals
```sh
vcd_tools_rs list sim.vcd --filter clk
```

### Get VCD metadata
```sh
vcd_tools_rs meta sim.vcd --pretty
```

### Extract multiple signals in a time window
```sh
vcd_tools_rs extract sim.vcd \
  --signal "tb.U_CHIP.counter[7:0]" \
  --signal "tb.U_CHIP.enable" \
  --start 0 --end 1000000 \
  --pretty
```

### Use a signals file
```sh
cat > signals.txt << EOF
tb.clk
tb.rst_n
tb.U_CHIP.counter[7:0]
EOF

vcd_tools_rs extract sim.vcd --signals-file signals.txt
```

### Find the 3rd rising edge of a clock
```sh
vcd_tools_rs find sim.vcd \
  --signal "tb.clk" \
  --value 1 \
  --occurrence 3
```

### Extract RISC-V trace with custom signal paths
```sh
vcd2trace sim.vcd \
  --signal-path my_cpu_paths.json \
  --output cpu_trace.txt
```

---

## Error Handling

The tool will exit with a non-zero status and display an error message for:

- File not found
- Invalid VCD format
- Missing `$enddefinitions` marker
- Signal not found in VCD
- No matching occurrence found (for `find` command)
- No signals specified (for `extract` command)

---

## Performance Notes

- Large VCD files are streamed from disk (memory-efficient)
- Release builds provide significantly better performance
- Listing filtered signals can be ~6x faster than Python equivalents
