# M2 / G3 candidate validation

Status: **candidate evidence pending commit and required review**

- branch: `design-db`
- base commit: `c4c2db0`
- worktree: dirty only with this final M2 test harness, validation artifact, and tracking updates
- production Rust source changes in this final batch: none
- Python tested: CPython 3.12.3 through an ABI3 Python >=3.8 wheel built by maturin 1.15.0

## Rust compatibility and query engine

Commands:

```sh
cargo test --all-targets -- --test-threads=1
cargo test --release --all-targets -- --test-threads=1
```

Both profiles passed the same matrix:

- library/unit: 69 passed, 4 ignored release-only diagnostics;
- native CLI compatibility: 14 passed;
- existing integration: 57 passed;
- public `OpenedVcd`: 4 passed;
- public query context: 5 passed;
- semantic characterization: 21 passed;
- reusable query API: 7 passed;
- total: **177 passed, 0 failed, 4 ignored**.

The matrix covers selected-ID-first conversion, command/row/logical-byte limits, cancellation/deadline precedence, alias and duplicate multiplicity, body ordering, generation validation at successful completion, reusable extract/find/toggle, provider-based comparison, legacy wrappers, and exact native CLI output.

Known retained limitation: comparison still materializes selected per-signal timelines before merging. This is explicitly tracked for future cached/provider work and is bounded by caller-selected signals/time windows rather than silently described as streaming comparison.

## Compiled Python extension

Harness:

```sh
PYTHON_TEST_VENV=<isolated-venv> scripts/test-python-extension.sh
```

The harness runs `maturin develop --release` with the project `python` feature and executes `tests/python/test_extension_compat.py`.

Result: **5 passed**.

Coverage:

- exact exported module-level function set and signatures;
- list/filter and metadata dictionary;
- extract alias/request ordering and row dictionary shape;
- toggle and find found/not-found shapes;
- compare dictionary and mismatch shape;
- `VcdError` remains `RuntimeError` with accepted messages.

The PyO3 functions continue calling the existing path compatibility wrappers in `src/lib.rs`; those wrappers now create temporary `OpenedVcd` instances. No persistent Python `Vcd` class or iterator was introduced; those remain M7 work.

Pure Python console characterization:

```sh
python3 -m unittest discover -s tests/python -p 'test_cli_compat.py' -v
```

Result: **3 passed**.

## vcd2trace disposition

Commands:

```sh
cargo build --release --bin vcd2trace
./target/release/vcd2trace --help
./target/release/vcd2trace tests/fixtures/query_semantics.vcd --output /tmp/vcd2trace-smoke.out
```

Build and help passed. The smoke invocation reached normal signal validation and exited 1 with the expected ten missing default signal paths.

Migration is **DEFERRED**:

- `vcd2trace` already parses its header once and streams its body once;
- moving it now offers little repeated-query benefit;
- its rising-edge/state-before-update behavior is specialized;
- no ten-signal trace fixture with byte-identical expected output exists yet;
- migration would add compatibility risk without advancing the server/query use case.

RQ-M2-T09 may be reconsidered only after the planned ten-signal byte-golden fixture exists.

## Portability and documentation

All commands passed:

```sh
cargo check --all-targets --target x86_64-unknown-linux-gnu
cargo check --all-targets --target x86_64-pc-windows-msvc
cargo check --all-targets --target aarch64-unknown-linux-gnu
cargo check --all-targets --target x86_64-apple-darwin
cargo check --all-targets --target aarch64-apple-darwin
cargo doc --no-deps
bash -n scripts/test-python-extension.sh
python3 -m py_compile tests/python/test_extension_compat.py
git diff --check
```

Per CR-002, native linking/archive/wheel matrices on every supported platform remain G8 release-CI obligations. This candidate proves all-target Rust source/type compatibility plus one Linux ABI3 compiled-extension build.
