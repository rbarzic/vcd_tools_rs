# vcd_tools_rs CLI Reference

A fast streaming VCD (Value Change Dump) analysis tool written in Rust, also available as a Python library via `pip install vcd-tools`.

## Table of Contents

- [Installation](#installation)
- [Global Options](#global-options)
- [Commands](#commands)
  - [list](#list)
  - [meta](#meta)
  - [extract](#extract)
  - [toggle](#toggle)
  - [find](#find)
  - [compare](#compare)
- [Python API](#python-api)
- [vcd2trace](#vcd2trace)
- [Output Formats](#output-formats)
- [Examples](#examples)

---

## Installation

### pip (recommended for Python users)

```sh
pip install vcd-tools
```

Installs the `vcd_tools_rs` CLI and the `vcd_tools` Python module.

### Precompiled binary

```sh
tar xzf vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu.tar.gz
./vcd_tools_rs --help
```

Download from [GitHub Releases](https://github.com/rbarzic/vcd_tools_rs/releases).

### From source

```sh
git clone https://github.com/rbarzic/vcd_tools_rs.git
cd vcd_tools_rs
cargo build --release
```

---

## Global Options

These options apply to all subcommands and must be placed **before** the subcommand name:

```sh
vcd_tools_rs [GLOBAL OPTIONS] <SUBCOMMAND> [SUBCOMMAND OPTIONS]
```

| Option | Default | Description |
|--------|---------|-------------|
| `--log-level <LEVEL>` | `info` | Logging verbosity: `error`, `warn`, `info`, `debug` |
| `--pretty` | false | Render output as formatted tables instead of tab-separated values |

---

## Commands

### list

List all signals declared in a VCD file header.

**Usage:**
```
vcd_tools_rs list [OPTIONS] <VCD>
```

**Options:**

| Option | Description |
|--------|-------------|
| `--filter <SUBSTRING>` | Filter signal names by substring (case-sensitive) |

**Examples:**
```sh
vcd_tools_rs list simulation.vcd
vcd_tools_rs list simulation.vcd --filter counter
vcd_tools_rs --pretty list simulation.vcd --filter clk
```

**Output:**
- Default: one signal name per line
- `--pretty`: formatted table with "Name" header

---

### meta

Display metadata about a VCD file.

**Usage:**
```
vcd_tools_rs meta <VCD>
```

**Output fields:**

| Field | Description |
|-------|-------------|
| `signals` | Total number of signals in the VCD |
| `timescale` | Time resolution (e.g. `1 ns`) |
| `start_time` | First timestamp with a signal change |
| `end_time` | Last timestamp with a signal change |

**Examples:**
```sh
vcd_tools_rs meta simulation.vcd
vcd_tools_rs --pretty meta simulation.vcd
```

---

### extract

Extract time/value pairs for one or more signals, optionally within a time window.

**Usage:**
```
vcd_tools_rs extract [OPTIONS] <VCD>
```

**Options:**

| Option | Description |
|--------|-------------|
| `--signal <NAME>` | Signal to extract (repeatable; also accepts comma-separated list) |
| `--signals-file <FILE>` | Text file with one signal name per line (`#` comments supported) |
| `--start <TIME>` | Start time, inclusive (default: beginning of file) |
| `--end <TIME>` | End time, inclusive (default: end of file) |

At least one `--signal` or `--signals-file` is required.

**Output:** Tab-separated with a header row — one column per signal, one row per timestamp.

```
time    tb.clk  tb.rst_n
0       0       0
10      1       0
20      0       1
```

Multi-bit signals are displayed as hex (e.g. `0x00ff`).

**Examples:**
```sh
vcd_tools_rs extract sim.vcd --signal "tb.clk"
vcd_tools_rs extract sim.vcd --signal "tb.clk" --signal "tb.rst_n"
vcd_tools_rs extract sim.vcd --signal "tb.clk,tb.rst_n"
vcd_tools_rs extract sim.vcd --signals-file signals.txt --start 0 --end 500000
vcd_tools_rs --pretty extract sim.vcd --signal "tb.counter[7:0]"
```

---

### toggle

Count the number of value transitions (toggles) for one or more signals.

**Usage:**
```
vcd_tools_rs toggle [OPTIONS] <VCD>
```

**Options:**

| Option | Description |
|--------|-------------|
| `--signal <NAME>` | Signal to count (repeatable; also accepts comma-separated list) |
| `--signals-file <FILE>` | Text file with one signal name per line (`#` comments supported) |
| `--start <TIME>` | Start time, inclusive |
| `--end <TIME>` | End time, inclusive |

At least one `--signal` or `--signals-file` is required.

**Output:** Tab-separated with a header row (`signal`, `toggles`).

```
signal          toggles
tb.clk          500
tb.counter[7:0] 42
```

**Examples:**
```sh
vcd_tools_rs toggle sim.vcd --signal "tb.clk"
vcd_tools_rs toggle sim.vcd --signals-file signals.txt --start 0 --end 1000000
vcd_tools_rs --pretty toggle sim.vcd --signal "tb.clk,tb.rst_n"
```

---

### find

Find the Nth occurrence of a signal reaching a specific value.

**Usage:**
```
vcd_tools_rs find [OPTIONS] <VCD>
```

**Options:**

| Option | Default | Description |
|--------|---------|-------------|
| `--signal <NAME>` | required | Signal to monitor (exactly one) |
| `--value <VALUE>` | required | Target value: decimal, `0x`-prefixed hex, `x`, or `z` |
| `--occurrence <N>` | `1` | Which occurrence to return (1-indexed). Also accepted: `--occurence` |
| `--start <TIME>` | — | Start time, inclusive |
| `--end <TIME>` | — | End time, inclusive |

**Output:**
```
time    tb.clk
30      1
```

Exits with a non-zero status and an error message if the occurrence is not found.

**Examples:**
```sh
vcd_tools_rs find sim.vcd --signal "tb.clk" --value 1
vcd_tools_rs find sim.vcd --signal "tb.clk" --value 1 --occurrence 5
vcd_tools_rs find sim.vcd --signal "tb.counter[7:0]" --value 0xff --start 1000
vcd_tools_rs find sim.vcd --signal "tb.data" --value z
```

---

### compare

Compare two VCD files signal by signal and report mismatches.

**Usage:**
```
vcd_tools_rs compare [OPTIONS] <REFERENCE> <ACTUAL>
```

**Arguments:**

| Argument | Description |
|----------|-------------|
| `<REFERENCE>` | Path to the reference VCD file |
| `<ACTUAL>` | Path to the actual VCD file to compare against |

**Options:**

| Option | Description |
|--------|-------------|
| `--max-mismatches <N>` | Limit the number of reported mismatches per signal |
| `--signals-only <LIST>` | Comma-separated list of signal names to restrict comparison to |
| `--ignore-unknown` | Treat differences involving `x` or `z` values as matches |
| `--start <TIME>` | Start time, inclusive |
| `--end <TIME>` | End time, inclusive |
| `--output <FORMAT>` | Output format: `default` (detailed), `json`, `compact` |

**Output formats:**

- `default` — detailed report with per-signal mismatch listings and a summary section
- `compact` — single line: `✅ PASS: All N signals match` or `❌ FAIL: X/N signals have mismatches`
- `json` — machine-readable JSON with counts per signal

**Examples:**
```sh
vcd_tools_rs compare reference.vcd actual.vcd
vcd_tools_rs compare reference.vcd actual.vcd --output compact
vcd_tools_rs compare reference.vcd actual.vcd --output json
vcd_tools_rs compare reference.vcd actual.vcd --max-mismatches 5 --ignore-unknown
vcd_tools_rs compare reference.vcd actual.vcd --signals-only "tb.clk,tb.rst_n"
vcd_tools_rs compare reference.vcd actual.vcd --start 0 --end 1000000
```

---

## Python API

Install with `pip install vcd-tools`, then:

```python
import vcd_tools
```

### `vcd_tools.list_signals(path, filter=None)`

Return signal names from the VCD header.

```python
signals = vcd_tools.list_signals("sim.vcd")
clk_signals = vcd_tools.list_signals("sim.vcd", filter="clk")
# → ["tb.clk", "tb.cpu.clk_cpu"]
```

### `vcd_tools.metadata(path)`

Return a dict with VCD metadata.

```python
m = vcd_tools.metadata("sim.vcd")
# → {"signal_count": 42, "timescale": "1 ns", "start_time": 0, "end_time": 1000000}
```

### `vcd_tools.extract(path, signals, start=None, end=None)`

Return time/value events as a list of dicts.

```python
rows = vcd_tools.extract("sim.vcd", signals=["tb.clk", "tb.rst_n"], start=0, end=500)
# → [{"signal": "tb.clk", "time": 0, "value": "0"}, ...]
```

### `vcd_tools.toggles(path, signals, start=None, end=None)`

Return a dict mapping signal name → number of transitions.

```python
counts = vcd_tools.toggles("sim.vcd", signals=["tb.clk"])
# → {"tb.clk": 500}
```

### `vcd_tools.find(path, signal, value, occurrence=1, start=None, end=None)`

Find the Nth occurrence of a signal value.

```python
r = vcd_tools.find("sim.vcd", signal="tb.clk", value="1", occurrence=3)
# → {"found": True, "signal": "tb.clk", "time": 30, "value": "1"}
# → {"found": False, "signal": "tb.clk", "time": None, "value": None}
```

### `vcd_tools.compare(file1, file2, *, max_mismatches=None, signals=None, ignore_unknown=False, start=None, end=None)`

Compare two VCD files. Returns a dict with:

| Key | Type | Description |
|-----|------|-------------|
| `passed` | bool | True if all compared signals match |
| `file1` / `file2` | str | Input file paths |
| `common_signals` | list[str] | Signals present in both files |
| `signals_only_in_file1` | list[str] | Signals only in the first file |
| `signals_only_in_file2` | list[str] | Signals only in the second file |
| `total_mismatches` | int | Total number of mismatching time-points |
| `signals_with_mismatches` | int | Number of signals that have at least one mismatch |
| `mismatches` | list[dict] | Each entry has: `signal`, `time`, `value1`, `value2`, `is_unknown` |

```python
diff = vcd_tools.compare("reference.vcd", "actual.vcd",
                         max_mismatches=10, ignore_unknown=True)
if not diff["passed"]:
    for m in diff["mismatches"]:
        print(f"{m['signal']} at t={m['time']}: {m['value1']} → {m['value2']}")
```

---

## vcd2trace

A separate binary that extracts a RISC-V architectural trace from a VCD file (Aldebaran-style format).

**Usage:**
```
vcd2trace [OPTIONS] <VCD>
```

**Options:**

| Option | Default | Description |
|--------|---------|-------------|
| `--output <FILE>` | `trace.txt` | Output trace file path |
| `--signal-path <JSON>` | — | JSON file with custom signal path mappings |
| `--log-level <LEVEL>` | `info` | Logging verbosity |

**Signal path JSON** (all keys optional):

```json
{
  "clk":         "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.clk_cpu",
  "rst_n":       "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.rst_cpu_n",
  "inst_retired":"tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.inst_retired",
  "instruction": "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.instruction_r[31:0]",
  "pc":          "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.pc_exe_r[21:0]",
  "rf_we_qual":  "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rf_write_enable_qual",
  "rf_we_lsu":   "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rf_write_enable_lsu",
  "rd_addr":     "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rd_addr[4:0]",
  "rd_wdata":    "tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rd_wdata[31:0]",
  "rd_wdata_lsu":"tb.U_CHIP.U_TOP_VCORE.U_TOP_CORE.U_TOP_CPU.U_CPU.U_RF.rd_wdata_lsu[31:0]"
}
```

**Signal definitions:**

| Signal | Description |
|--------|-------------|
| `clk` | CPU clock |
| `rst_n` | CPU reset (active low) |
| `inst_retired` | Instruction completion signal |
| `instruction` | Instruction word being executed |
| `pc` | Program counter |
| `rf_we_qual` | Register-file write enable (qualified path) |
| `rf_we_lsu` | Register-file write enable (load-store unit path) |
| `rd_addr` | Destination register address |
| `rd_wdata` | Register write data (when `rf_we_qual` is high) |
| `rd_wdata_lsu` | Register write data from LSU (when `rf_we_lsu` is high) |

**Output format:**
```
PC : 0x00000100 I : 0x00400113
x01 <= 0x00000004
PC : 0x00000104 I : 0x00800193
x03 <= 0x00000008
```

---

## Output Formats

### Default (tab-separated)

Suitable for piping to `cut`, `awk`, or Python:

```sh
vcd_tools_rs extract sim.vcd --signal "tb.clk" | cut -f2
```

### Pretty (aligned table)

```
time       | tb.counter[7:0]
-----------+-----------------
0          | 0x00
100        | 0x01
200        | 0x02
```

---

## Examples

### Inspect a new VCD file

```sh
vcd_tools_rs meta sim.vcd --pretty
vcd_tools_rs list sim.vcd | wc -l          # total signal count
vcd_tools_rs list sim.vcd --filter clk     # find clock signals
```

### Extract signals in a time window

```sh
vcd_tools_rs extract sim.vcd \
  --signal "tb.clk" --signal "tb.rst_n" \
  --start 0 --end 1000000 --pretty
```

### Use a signals file

```sh
cat > signals.txt << 'EOF'
tb.clk
tb.rst_n
tb.U_CHIP.counter[7:0]   # 8-bit counter
EOF

vcd_tools_rs extract sim.vcd --signals-file signals.txt
```

### Count clock toggles across a window

```sh
vcd_tools_rs toggle sim.vcd --signal "tb.clk" --start 0 --end 10000000
```

### Find the 3rd rising edge of a clock

```sh
vcd_tools_rs find sim.vcd --signal "tb.clk" --value 1 --occurrence 3
```

### Regression: compare two simulation runs

```sh
# Quick pass/fail
vcd_tools_rs compare golden.vcd actual.vcd --output compact

# Full report
vcd_tools_rs compare golden.vcd actual.vcd

# Machine-readable, ignore x/z differences
vcd_tools_rs compare golden.vcd actual.vcd --output json --ignore-unknown
```

### Python: batch comparison script

```python
import vcd_tools, sys

pairs = [("golden_A.vcd", "run_A.vcd"), ("golden_B.vcd", "run_B.vcd")]
for ref, act in pairs:
    r = vcd_tools.compare(ref, act)
    status = "PASS" if r["passed"] else f"FAIL ({r['total_mismatches']} mismatches)"
    print(f"{act}: {status}")
```

---

## Error Handling

The tool exits with a non-zero status and prints an error for:

- File not found or unreadable
- Missing `$enddefinitions` marker in the VCD header
- Signal not found in the VCD
- No matching occurrence found (`find` command)
- No signals specified (`extract` / `toggle`)

---

## Performance Notes

- All commands stream the VCD body; memory usage is independent of file size.
- Release builds (`cargo build --release`) are significantly faster than debug builds.
- Listing filtered signals is ~6× faster than Python VCD parsers in local measurements.
- The pip-installed `vcd_tools_rs` command is backed by the same compiled Rust code as the native binary — no performance difference.
