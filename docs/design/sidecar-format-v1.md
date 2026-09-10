# Sparse Sidecar Format v1

Status: **DRAFT — NOT IMPLEMENTATION-APPROVED**  
Plan: **VCD-RQ-001**  
Blocked by: parser offset/resume spike (`RQ-M3-T02`)

## 1. Scope

The v1 sidecar is a disposable sparse timestamp-to-byte-offset cache. It accelerates queries with a late `start` by skipping a validated prefix of the VCD body.

It is not:

- a complete event database;
- a snapshot of all signal values;
- portable proof of VCD contents;
- authoritative data;
- required to read a VCD;
- modified in place after publication.

Deletion or rejection of a sidecar always falls back to normal streaming unless policy says indexing is required.

## 2. Correctness precondition

Before implementing this format, an executable spike must prove that the selected offset is the logical start of a timestamp command and that a fresh parser can resume there.

Required experiment:

1. Before each parser `next()`, obtain the logical stream position from the parser's buffered reader.
2. Parse the command.
3. When it is a timestamp, record the prior position and timestamp.
4. For every recorded checkpoint, create a fresh reader at that position.
5. Compare all subsequent commands against a full parse, including command kind, ID, value, timestamp, and order.

Fixtures must include:

- LF and CRLF;
- blank lines and varied whitespace;
- `$dumpvars` and changes before the first timestamp;
- multiple timestamp commands and repeated timestamps;
- scalar/vector/real/string changes;
- large vectors and X/Z;
- comments and dump-control commands in the body;
- unusual formatting around `$enddefinitions`;
- parser buffer boundaries;
- both supplied large VCDs using sampled checkpoints.

Do not use the underlying `File` cursor position: buffered read-ahead makes it unsuitable. If logical positions cannot be proven safe, either implement a dedicated byte-level checkpoint scanner with differential tests or defer sparse seek from v1.

## 3. Checkpoint semantics

Every checkpoint is:

```text
(timestamp, file_offset)
```

`file_offset` points to the beginning of the corresponding timestamp command, not the first value change after it. Parsing the timestamp restores the current time naturally.

The index also contains a mandatory body-start checkpoint:

```text
(timestamp = 0, file_offset = body_offset, kind = BODY_START)
```

This ensures pre-first-timestamp changes and initial dump blocks remain reachable for windows starting at zero.

For `window.start = t`:

1. choose the greatest timestamp checkpoint with `checkpoint.timestamp <= t`;
2. when equal timestamps have multiple checkpoint candidates, choose the earliest safe offset unless equivalence tests prove another choice retains all same-time changes;
3. seek an independently validated reader to the checkpoint;
4. parse normally and apply the existing inclusive time filter;
5. stop after observing a timestamp greater than `window.end`.

Because current extraction and toggle semantics do not synthesize state active before `start`, v1 stores no signal snapshot. Any future semantic mode requiring active values must use a new format/flag and must not reinterpret v1 records.

## 4. Checkpoint selection

The builder supports:

- minimum byte distance from the previous checkpoint;
- optional minimum timestamp distance;
- a mandatory first/body-start record;
- a mandatory last useful timestamp record if it improves tail access.

A timestamp is recorded when either enabled threshold is crossed. Default thresholds are **TBD at Gate G4**. The initial benchmark candidate is a 64 MiB byte stride; timestamp units are file-specific and therefore must not be the only default criterion.

Target sidecar size: no more than 0.5% of the VCD under approved defaults.

## 5. File naming

Default adjacent path:

```text
<source-vcd>.vcdidx-v1
```

An explicit cache directory may instead derive a collision-resistant filename from canonical display path plus file identity. The source path is metadata, not identity.

Unknown future versions are left untouched. v1 code must never overwrite a file it cannot identify as its own disposable v1 cache.

## 6. Logical binary layout

All integer fields are little-endian. The final fixed byte layout must be frozen with golden fixtures before Gate G6; this table defines required information rather than granting permission to implement unreviewed offsets.

### 6.1 Header

| Field | Type | Requirement |
|---|---|---|
| Magic | 8 bytes | `VCDIDX01` |
| Format version | `u16` | `1` |
| Header length | `u16` | Allows readers to skip compatible extensions |
| Flags | `u32` | Unknown required flags reject; optional flags may be ignored |
| Record length | `u16` | Reject unsupported record encoding |
| Reserved | fixed bytes | Must be zero when written |
| Source size | `u64` | Snapshotted length |
| Source mtime seconds | signed fixed integer | Platform-normalized |
| Source mtime nanoseconds | `u32` | 0..999,999,999 |
| Source device | `u64` | Zero if unavailable |
| Source inode/file ID parts | fixed integers | Zero if unavailable |
| Body offset | `u64` | Must match parsed header generation |
| Start time | `u64` | Exact metadata bound from build scan |
| End time | `u64` | Exact metadata bound from build scan |
| Checkpoint count | `u64` | Bounded before allocation |
| Byte stride | `u64` | Builder setting |
| Time stride | `u64` | Zero when disabled |
| Header/content sample hash | 32 bytes | BLAKE3 unless changed before format freeze |
| Records length | `u64` | Checked against file length |
| Header checksum | 32 bytes | Covers header with checksum field zeroed |

### 6.2 Checkpoint record

| Field | Type | Requirement |
|---|---|---|
| Kind | `u8` | body-start or timestamp |
| Reserved | fixed bytes | zero |
| Timestamp | `u64` | non-decreasing |
| File offset | `u64` | strictly increasing and inside source snapshot |

### 6.3 Trailer

| Field | Type | Requirement |
|---|---|---|
| Records checksum | 32 bytes | Covers encoded records exactly |
| End magic | 8 bytes | Detect truncation/misframing |

The decoder uses checked arithmetic and validates lengths before allocation. It rejects trailing bytes unless the frozen format explicitly defines an extension area.

## 7. Source fingerprint

The sidecar is accepted only when all available identity checks match policy:

- source length;
- nanosecond mtime;
- device/inode or platform file ID;
- body offset;
- BLAKE3 of the complete parsed header bytes;
- bounded samples at the beginning and end of the snapshotted content;
- optional strict full-file hash.

Size and mtime alone are not sufficient for correctness. The builder and reader use identity from opened file handles to reduce path races.

## 8. Decoder validation

Reject the sidecar when any check fails:

- wrong magic or unsupported version/required flags;
- invalid header/record length;
- arithmetic overflow;
- truncated file or unexpected trailing data;
- checksum mismatch;
- source fingerprint mismatch;
- body offset mismatch;
- missing body-start checkpoint;
- non-monotonic timestamps;
- non-increasing offsets;
- offset before body start or outside source snapshot;
- timestamp metadata outside recorded start/end bounds;
- excessive checkpoint count under configured limits.

Decoder failure never causes a panic and never changes the VCD. In best-effort policy it logs/reports the rejection and streams from the source.

## 9. Build and publication lifecycle

1. Validate/open one VCD generation.
2. Acquire an inter-process builder lease.
3. Build to a unique temporary file in the destination directory.
4. Parse the complete body once, calculating metadata and checkpoints.
5. Check cancellation/deadline periodically.
6. Validate generated records in memory or by rereading the temporary file.
7. Flush and `sync_all` the temporary file.
8. Atomically rename it to the final path.
9. Sync the parent directory where supported.
10. Release the lease and notify in-process waiters.

A cancelled, failed, or panicking build removes its temporary file when possible and must clear in-process `Building` state through an RAII guard.

Publication must never expose a partial file. Concurrent processes may wait, stream without an index, or fail when indexing is explicitly required. They may not race-write the same final file.

The exact lease mechanism is selected by spike:

- preferred: advisory OS lock on a dedicated lock file with process-lifetime release;
- alternative: create-new lease containing PID/start time with conservative stale recovery.

Never delete a lease solely because a wall-clock timeout elapsed without verifying ownership/liveness as supported by the platform.

## 10. Policies

```rust
enum SidecarPolicy {
    Off,
    ReadOnly,
    BuildIfMissing,
    Require,
}
```

- `Off`: ignore sidecars and stream from body start.
- `ReadOnly`: use a valid sidecar; otherwise stream.
- `BuildIfMissing`: single-flight build; policy determines whether the initiating query waits or streams.
- `Require`: fail if no valid index can be obtained.

Initial release default: `Off` or `ReadOnly`, decided at Gate G6. Automatic building is not the first stable default.

## 11. Corruption and compatibility

- Sidecars are caches, so rebuilding is preferred to repair-in-place.
- A corrupt known-v1 sidecar may be renamed with a `.corrupt-<unique>` suffix before rebuild, subject to a retention policy.
- Unknown versions remain untouched.
- Writers never change v1 meaning. Incompatible changes use new magic/version and filename.
- Golden format fixtures test decoding across future releases.
- Fuzzing covers decoder lengths, checksums, counts, and records.

## 12. Required tests

1. Full-stream versus checkpoint-resume command equivalence.
2. Query equivalence for early/middle/late windows and exact boundaries.
3. Repeated timestamp and same-time ordering.
4. Empty body and no-timestamp body.
5. Every supported value type.
6. LF/CRLF and buffer-boundary offsets.
7. Invalid magic/version/flags/lengths/checksums/counts.
8. Non-monotonic/out-of-range records.
9. Append, truncate, touch, atomic replace, and in-place sample modification.
10. Two builders, killed builder, stale lease, and interrupted publication.
11. Cancellation at multiple build points leaves no accepted partial sidecar.
12. Golden v1 fixture decode.
13. Fuzz decoder and differential query backend.

## 13. Acceptance targets

Gate G6 requires:

- exact semantic equivalence with streaming backend;
- no accepted stale/corrupt sidecar in mutation tests;
- no decoder panic under fuzzing;
- sidecar at or below 0.5% of source size with approved stride;
- at least 5× median speedup for representative queries beginning in the final 10% where checkpoint density permits;
- build throughput no more than 15% slower than the existing metadata-only scan unless justified;
- cancellation and interrupted publication leave a recoverable state;
- deleting the sidecar fully restores ordinary streaming behavior.
