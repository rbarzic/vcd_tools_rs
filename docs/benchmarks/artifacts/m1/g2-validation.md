# G2 validation evidence

Candidate commit: `d420347707b5c3a3e8e56582e82d64a98f332ac1`  
Tree state during commands: clean  
Host: Linux x86_64, Rust 1.93.1

## All-target source/type checks

All commands completed successfully from the clean candidate commit:

```sh
cargo check --all-targets --target x86_64-unknown-linux-gnu
cargo check --all-targets --target x86_64-pc-windows-msvc
cargo check --all-targets --target aarch64-unknown-linux-gnu
cargo check --all-targets --target x86_64-apple-darwin
cargo check --all-targets --target aarch64-apple-darwin
```

Per CR-002, these prove source/type compatibility for all current release targets. Native/cross linking, archives, wheels, and platform runtime behavior remain G8/release-CI requirements.

## Debug and release suites

```sh
cargo test --all-targets -- --test-threads=1
cargo test --release --all-targets -- --test-threads=1
python3 -m unittest discover -s tests/python -p 'test_*.py' -v
cargo doc --no-deps
```

Results for both debug and release Rust suites:

- library/unit: 37 passed, 3 release-only diagnostics ignored;
- native CLI compatibility: 14 passed;
- existing integration: 57 passed;
- public `OpenedVcd`: 4 passed;
- semantic characterization: 21 passed;
- total: 133 passed, 0 failed, 3 ignored.

Python console compatibility: 3 passed, 0 failed. Documentation build passed.

## Clean performance evidence

- [`catalog-memory-A-g2.tsv`](catalog-memory-A-g2.tsv): five alternating compact/opened/compatibility samples, `evidence_status=g2_candidate`, `git_dirty=false`, exact commit, and verified VCD-A SHA-256.
- [`path-list-A-g2.tsv`](path-list-A-g2.tsv): five path-list samples with zero exit status and accepted M0 output SHA-256 verified on every run.

VCD-A SHA-256:

```text
8b5a3791ad8d4232e637fd16edba9618cd0370eb66d3d7e1d780f82531c8f4ac
```

Native release-link and platform filesystem behavior are not claimed by this document.
