# vcd_tools_rs

Fast streaming waveform analysis tool and Python library, written in Rust.

The current query engine provides commands to list signals, query metadata, extract time/value pairs, count toggles, search for value occurrences, and diff two VCD files — all with a memory-efficient streaming parser.

## Current format support

| Surface | VCD | FST |
|---------|-----|-----|
| Rust format detection API | Supported | Supported |
| Native CLI queries (`list`, `meta`, `extract`, `toggle`, `find`) | Supported | Supported |
| Python query functions | Supported | Supported |
| Unix `serve` mode | Supported | Supported |
| `compare` | Supported | Supported |
| `vcd2trace` | Supported | Not supported |

FST read/query support uses the pure-Rust `fst-reader` backend and content-based format detection. Supported FST surfaces include native CLI and Python list/metadata/extract/find/toggle operations, same- or cross-format comparison when physical timescales match, and Unix server mode. `vcd2trace` remains VCD-only. The implementation includes:

- the pure-Rust `fst-reader` 0.17 backend dependency;
- content-based VCD/FST format detection through the Rust API;
- explicit format assertions and deterministic mismatch errors;
- parser-panic containment, bounded error text, an 8 GiB file limit, and rejection of whole-file gzip-wrapped FST;
- bounded FST hierarchy depth, declaration count, retained name bytes, and catalog ownership, with compressed hierarchy expansion preflight before decompression;
- callback-based streaming for native CLI and Unix server extraction;
- a reproducible generated FST fixture with a committed semantic oracle.

The CLI, Python query functions, comparison engine, and Unix server auto-detect VCD/FST content. Native CLI users may assert the input with `--format auto|vcd|fst`; a mismatch fails deterministically. A `.fst` extension is not required. Whole-file gzip-wrapped FST and incomplete FST requiring an external `.hier` file are intentionally unsupported; `vcd2trace` remains VCD-only.

The established VCD-named Rust functions remain VCD-only. Format-neutral Rust callers use `OpenedWaveform`; `OpenedFst` exposes project-owned metadata and signal types without leaking `fst-reader` types.


Current additive Rust API:

```rust
use vcd_tools_rs::{detect_waveform_format, WaveformFormatHint};

let format = detect_waveform_format("simulation.fst", WaveformFormatHint::Auto)?;
println!("{format}"); // "fst" or "vcd"
```

Detection uses file contents, not the extension. It classifies and validates the input at detection time; a later consumer must still open and validate its own file generation. See [`FST_PLAN.md`](FST_PLAN.md) and the [FST architecture](docs/design/fst-support-architecture.md) for implementation status and supported-subset decisions.

## Installation

### Option 1: pip (Python users)

Installs the `vcd_tools_rs` command and the `vcd_tools` Python module:

```sh
pip install vcd-tools
```

The `vcd_tools_rs` CLI is backed by the compiled Rust extension, so performance is equivalent to the native binary.

### Option 2: Precompiled binaries

Download for your platform from [GitHub Releases](https://github.com/rbarzic/vcd_tools_rs/releases):

| Platform | Archive |
|----------|---------|
| Linux x64 | `vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | `vcd_tools_rs-vX.X.X-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Intel | `vcd_tools_rs-vX.X.X-x86_64-apple-darwin.tar.gz` |
| macOS Apple Silicon | `vcd_tools_rs-vX.X.X-aarch64-apple-darwin.tar.gz` |
| Windows x64 | `vcd_tools_rs-vX.X.X-x86_64-pc-windows-msvc.zip` |

```sh
tar xzf vcd_tools_rs-vX.X.X-x86_64-unknown-linux-gnu.tar.gz
./vcd_tools_rs --help
```

### Option 3: Build from source

```sh
cargo build --release
# optional: install into ~/.cargo/bin
cargo install --path .
```

---

## CLI usage

```sh
# List signals (optional substring filter)
vcd_tools_rs list simulation.vcd [--filter clk]

# File metadata (signal count, timescale, time bounds)
vcd_tools_rs meta simulation.vcd [--pretty]

# Extract time/value pairs for selected signals
vcd_tools_rs extract simulation.vcd \
  --signal "tb.clk" --signal "tb.rst_n" \
  --start 0 --end 1000000 \
  [--signals-file signals.txt] [--pretty]

# Count value transitions per signal
vcd_tools_rs toggle simulation.vcd \
  --signal "tb.clk" --signal "tb.counter[7:0]" \
  [--start 0 --end 500000]

# Find the Nth occurrence of a signal value
vcd_tools_rs find simulation.vcd \
  --signal "tb.clk" --value 1 --occurrence 3

# Compare two VCD files
vcd_tools_rs compare reference.vcd actual.vcd \
  [--max-mismatches 10] [--ignore-unknown] [--output json|compact]

# Experimental local VCD query server (Unix native build or Unix PyPI wheel)
mkdir -m 700 /tmp/vcd-tools-$USER
vcd_tools_rs serve simulation.vcd \
  --socket /tmp/vcd-tools-$USER/server.sock
```

Global flags: `--pretty` (table output), `--log-level` (`error`/`warn`/`info`/`debug`).

### Experimental server availability

Server mode is available from native Unix builds, Linux/macOS release archives, and Unix PyPI wheels. It accepts one VCD or FST selected at startup and reports `source_format` from `describe`. The server binds that waveform to an owner-only Unix-domain socket and supports `ping`, `describe`, `list`, `metadata`, `extract`, `find`, `toggles`, and `cancel` using protocol v1 JSON Lines.

After `pip install vcd-tools`, the same command is available on Unix:

```sh
mkdir -m 700 /tmp/vcd-tools-$USER
vcd_tools_rs serve simulation.vcd --socket /tmp/vcd-tools-$USER/server.sock
```

The socket parent directory must already exist, be owned by the current user, and have no group/other permissions. Windows does not expose this Unix socket mode.

See [the protocol contract](docs/design/unix-json-protocol-v1.md) for request schemas and limits.

See [CLI_REFERENCE.md](CLI_REFERENCE.md) for the complete option reference.

### Signals file

One signal name per line. Lines starting with `#` and inline `#` comments are ignored:

```text
# signals.txt
tb.U_CHIP.counter[7:0]
tb.U_CHIP.enable
tb.clk          # clock signal
```

---

## Python API

After `pip install vcd-tools`:

```python
import vcd_tools

# List signals
signals = vcd_tools.list_signals("simulation.vcd")
signals = vcd_tools.list_signals("simulation.vcd", filter="clk")

# Metadata
meta = vcd_tools.metadata("simulation.vcd")
# → {"signal_count": 42, "timescale": "1 ns", "start_time": 0, "end_time": 1000000}

# Extract time/value pairs
values = vcd_tools.extract("simulation.vcd",
                           signals=["tb.clk", "tb.rst_n"],
                           start=0, end=500000)
# → [{"signal": "tb.clk", "time": 0, "value": "0"}, ...]

# Count toggles
counts = vcd_tools.toggles("simulation.vcd", signals=["tb.clk"])
# → {"tb.clk": 500}

# Find Nth occurrence of a value
result = vcd_tools.find("simulation.vcd",
                        signal="tb.clk", value="1", occurrence=3)
# → {"found": True, "signal": "tb.clk", "time": 30, "value": "1"}

# Compare two VCD files
diff = vcd_tools.compare("reference.vcd", "actual.vcd",
                         max_mismatches=10, ignore_unknown=True)
# → {"passed": False, "total_mismatches": 3,
#    "mismatches": [{"signal": ..., "time": ..., "value1": ..., "value2": ...}], ...}
```

All query functions auto-detect VCD or FST content and raise `RuntimeError` on parse errors or missing signals. `compare` supports VCD/VCD, FST/FST, and VCD/FST when physical timescales match.

---

## vcd2trace (RISC-V trace extraction)

`vcd2trace` recreates a RISC-V architectural trace from a VCD file (Aldebaran-style format).

```sh
# Default signal paths
vcd2trace simulation.vcd --output trace.txt

# Override paths via JSON
vcd2trace simulation.vcd \
  --signal-path examples/signal_paths.json \
  --output trace.txt
```

See [CLI_REFERENCE.md](CLI_REFERENCE.md#vcd2trace) for the full signal path reference.

---

## Tests

```sh
cargo test
```

Tests use `tests/waveform.vcd` as the reference fixture.

## Notes

- Large VCDs are streamed from disk; header scope names are sanitized to tolerate special characters.
- Release builds (`cargo build --release`) provide the best performance (~6× faster than Python VCD parsers in local measurements).
- The pip-installed `vcd_tools_rs` command is a Python entry point backed by the same compiled Rust code; there is no performance difference vs. the native binary.
