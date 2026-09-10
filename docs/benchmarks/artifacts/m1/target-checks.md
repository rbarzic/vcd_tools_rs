# M1 installed-target source checks

Status: passing source/type checks; native release linking remains CI-owned.

Environment:

- host: `x86_64-unknown-linux-gnu`
- rustc: `1.93.1 (01f6ddf75 2026-02-11)`
- branch: `design-db`
- implementation base commit: `d6231fb`
- worktree: dirty with uncommitted RQ-M1-T08 portability/test changes

Commands:

```sh
cargo check --lib
cargo check --lib --target x86_64-pc-windows-msvc
cargo check --lib --target aarch64-unknown-linux-gnu
cargo check --lib --target x86_64-apple-darwin
cargo check --lib --target aarch64-apple-darwin
```

Result: all five commands passed.

The first Windows attempt with default `blake3` features failed in the dependency build script because a Linux host does not provide MSVC `ml64.exe`. Enabling `blake3`'s `pure` feature selects its portable Rust implementation; subsequent checks passed for all installed targets. The complete host debug and release test suites also passed with that feature, including every identity/fingerprint test, demonstrating unchanged digest-based behavior at this API boundary.

The Windows check also found that stable Rust still gates `MetadataExt::volume_serial_number` and `file_index` behind `windows_by_handle`. M1 therefore uses stable Unix device/inode identity only on Unix. On Windows and other targets, generation identity uses file length/mtime, complete raw-header BLAKE3, and bounded beginning/end BLAKE3 samples (plus optional strict full-content BLAKE3). Native file-ID support can be added later through platform APIs without changing the public opaque identity.

These commands type-check target-gated Rust source and dependencies. They do not prove native linking, archive construction, runtime filesystem semantics, or Python wheels; the existing native release CI matrix retains responsibility for those checks.
