# M4 T01 protocol validation

Base commit: `5341f2c` (`design-db`)  
Worktree: uncommitted RQ-M4-T01 implementation

## Focused protocol suite

```sh
cargo test --test protocol_v1 -- --test-threads=1
```

Result after the golden stream-lifecycle corrections: 15 passed, 0 failed.

The final focused corrections make `end.stats.encoded_bytes` equal the cumulative encoded JSONL bytes of begin, chunks, and stabilized end (including every newline), and require every streamed fixture ID to have `begin(seq=0)`, contiguous optional chunks, and exactly one terminal end. Both complete and incomplete stream lifecycles have dedicated assertions.

Coverage includes exact request/response/error golden JSONL, every v1 method, both streaming schemas, all value variants, all frozen error codes, legal complete/incomplete per-ID stream lifecycles, authoritative cumulative encoded-byte statistics, partial/multiple frames, malformed JSON, invalid UTF-8, recursive duplicate keys, unknown fields, active request IDs, canonical decimals, overflow, invalid windows/occurrence, response reserve, generated 1 MiB oversize input, and bounded terminal errors.

## Complete suites

Run once before the final bounded-error focused test:

```sh
cargo check --all-targets
cargo test --all-targets -- --test-threads=1
cargo test --release --all-targets -- --test-threads=1
cargo doc --no-deps
```

Debug: 188 passed, 4 ignored, 0 failed.  
Release: 188 passed, 4 ignored, 0 failed.  
Documentation build: passed.

## Release-target source/type checks

```sh
cargo check --all-targets --target x86_64-unknown-linux-gnu
cargo check --all-targets --target x86_64-pc-windows-msvc
cargo check --all-targets --target aarch64-unknown-linux-gnu
cargo check --all-targets --target x86_64-apple-darwin
cargo check --all-targets --target aarch64-apple-darwin
```

All five passed before the final focused golden-lifecycle corrections; the host `cargo check --all-targets` passed again after those corrections.

## Formatting and lint scope

Changed protocol Rust files pass direct `rustfmt --check`; `git diff --check` passes. Project-wide clippy with warnings denied remains blocked by pre-existing accepted lints in legacy/query files and reported no protocol-specific compiler warning under `cargo check --all-targets`.
