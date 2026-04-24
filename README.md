# vcd_tools_rs

Rust rewrite of the `pyvcd` CLI utilities for streaming VCD analysis. Provides the same subcommands (`list`, `meta`, `extract`, `find`) with a fast streaming parser (`vcd` crate) and plain/pretty outputs.

## Installation

### Option 1: Download Precompiled Binaries (Recommended)

Download precompiled binaries for your platform from the [GitHub Releases](https://github.com/rbarzic/vcd_tools_rs/releases) page:

- **Linux x64**: `vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu.tar.gz`
- **Linux ARM64**: `vcd_tools_rs-vX.X.X-aarch64-unknown-linux-gnu.tar.gz`
- **macOS Intel**: `vcd_tools_rs-vX.X.X-x86_64-apple-darwin.tar.gz`
- **macOS Apple Silicon**: `vcd_tools_rs-vX.X.X-aarch64-apple-darwin.tar.gz`
- **Windows x64**: `vcd_tools_rs-vX.X.X-x86_64-pc-windows-msvc.zip`

Extract the archive and add the binaries to your `$PATH`, or run directly:

```sh
# Linux / macOS
tar xzf vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu.tar.gz
./vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu/vcd_tools_rs --help

# Windows
unzip vcd_tools_rs-vX.X.X-x86_64-pc-windows-msvc.zip
cd vcd_tools_rs-vX.X.X-x86_64-pc-windows-msvc
vcd_tools_rs.exe --help
```

**Verify checksums** (optional but recommended):

```sh
sha256sum -c vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu.tar.gz.sha256
```

### Option 2: Install from Source

```sh
cargo build --release
# optional: install into ~/.cargo/bin
cargo install --path .
```

### Option 3: Install via Cargo (when published to crates.io)

```sh
cargo install vcd_tools_rs
```

## Usage

```sh
# List signals (optionally filter by substring)
vcd_tools_rs list path/to/file.vcd [--filter substring]

# Show metadata (signal count, timescale, time bounds)
vcd_tools_rs meta path/to/file.vcd

# Extract time/value pairs for selected signals
vcd_tools_rs extract path/to/file.vcd \
  --signal "tb.U_CHIP.counter[7:0]" \
  --start 0 --end 500000 \
  [--signals-file signals.txt] [--pretty]

# Find the Nth occurrence of a value on a signal
vcd_tools_rs find path/to/file.vcd \
  --signal "tb.clk" --value 1 --occurrence 3 \
  --start 0 --end 1000000 \
  [--pretty]
```

Flags:
- `--pretty` renders table output instead of tab-separated rows.
- `--log-level` sets logging verbosity (`info`, `debug`, etc.).

Signals can be repeated with `--signal` or supplied via `--signals-file` (one per line).

### Signals file example

Create a text file with one signal name per line. Lines starting with `#` are treated as comments and ignored. Inline comments after `#` are also supported:

```text
# signals.txt
tb.U_CHIP.counter[7:0]
tb.U_CHIP.enable
tb.clk # clock signal
```

Then pass it to `extract`:

```sh
vcd_tools_rs extract path/to/file.vcd \
  --signals-file signals.txt \
  --start 0 --end 100000 \
  --pretty
```

## vcd2trace (Aldebaran trace)

`vcd2trace` recreates a RISC-V trace from a VCD file

Defaults assume the CPU/RF hierarchy as shown in 'examples/signal_paths.json`. If your VCD uses different paths, provide a JSON map to override any of the signal paths.

```sh
# Default paths
cargo run --release --bin vcd2trace -- examples/tb.vcd --output trace.txt

# Override paths via JSON
cargo run --release --bin vcd2trace -- examples/tb.vcd \
  --signal-path examples/signal_paths.json \
  --output my_trace.txt
```

`--signal-path` JSON example (all keys optional, only override what you need):
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
### Signal definitions

| Signal Name                    | Definition                                                         |
|--------------------------------|--------------------------------------------------------------|
| clk                            | The CPU clock          |
| rst_n                          | The CPU reset (active low)   |
| inst_retired                   | instruction completion signal |
| instruction                    | The instruction being executed |
| pc                             | The counter program |
| rf_we_qual                     | The write enable signal to the register file |
| rf_we_lsu                      | An alternate  write enable signal to the register file |
| rd_addr                        | read register address |
| rd_wdata                       | write register address (used when rf_we_qual is high ) |
| rd_wdata_lsu                   | write register address (used when rf_we_lsu is high ) |





## Tests

Run the test suite from the crate root:

```sh
cargo test
```

Tests use `tests/waveform.vcd` as the reference VCD fixture.

## Notes

- Large VCDs are streamed from disk; header scope names are sanitized to tolerate special characters (matches Python behavior).
- Release builds (`cargo build --release`) provide the best performance. For example, listing filtered signals in `examples/printf_test.vcd` was ~6× faster than the Python CLI in local measurements.***
