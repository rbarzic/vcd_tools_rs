use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::Result;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt as UnixMetadataExt;

const CONTENT_SAMPLE_BYTES: u64 = 64 * 1024;
#[allow(dead_code)] // Allocated by `OpenedVcd::open` in RQ-M1-T04.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Process-local identity for one opened VCD generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GenerationId(u64);

impl GenerationId {
    #[allow(dead_code)] // Allocated by `OpenedVcd::open` in RQ-M1-T04.
    pub(crate) fn fresh() -> Self {
        let value = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        assert_ne!(value, u64::MAX, "VCD generation identifier space exhausted");
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// Opaque BLAKE3 fingerprint calculated from one opened file handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentFingerprint([u8; 32]);

impl ContentFingerprint {
    fn from_hash(hash: blake3::Hash) -> Self {
        Self(*hash.as_bytes())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Controls how much source content is read while creating a generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum FingerprintPolicy {
    /// Hash the complete header plus bounded 64 KiB beginning/end samples.
    #[default]
    MetadataAndSamples,
    /// Additionally hash the complete VCD. This deliberately performs a full scan.
    StrictFullContent,
}

/// Options shared by the future `OpenedVcd` constructor.
///
/// Fields are private and the type is non-exhaustive so sidecar/cache controls
/// can be added without breaking callers that use constructors and builders.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenOptions {
    fingerprint_policy: FingerprintPolicy,
}

impl OpenOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fingerprint_policy(&self) -> FingerprintPolicy {
        self.fingerprint_policy
    }

    pub fn with_fingerprint_policy(mut self, policy: FingerprintPolicy) -> Self {
        self.fingerprint_policy = policy;
        self
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            fingerprint_policy: FingerprintPolicy::MetadataAndSamples,
        }
    }
}

/// Identity of a VCD generation derived from one opened file handle.
///
/// Construction stays crate-private until `OpenedVcd::open` owns provenance.
/// Public accessors expose immutable evidence without allowing callers to
/// synthesize identities through struct literals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    len: u64,
    modified: Option<SystemTime>,
    body_offset: u64,
    header_fingerprint: ContentFingerprint,
    sample_fingerprint: ContentFingerprint,
    full_content_fingerprint: Option<ContentFingerprint>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileIdentity {
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    pub fn body_offset(&self) -> u64 {
        self.body_offset
    }

    pub fn header_fingerprint(&self) -> ContentFingerprint {
        self.header_fingerprint
    }

    pub fn sample_fingerprint(&self) -> ContentFingerprint {
        self.sample_fingerprint
    }

    pub fn full_content_fingerprint(&self) -> Option<ContentFingerprint> {
        self.full_content_fingerprint
    }

    #[cfg(unix)]
    pub fn device(&self) -> u64 {
        self.device
    }

    #[cfg(unix)]
    pub fn inode(&self) -> u64 {
        self.inode
    }

    pub(crate) fn matches_metadata(&self, metadata: &Metadata) -> bool {
        self.len == metadata.len()
            && self.modified == metadata.modified().ok()
            && matches_stored_platform_identity(self, metadata)
    }

    /// Validate a newly opened handle against this generation and leave it at
    /// the generation's body boundary, including after a rejected validation.
    pub(crate) fn validate_open_file(
        &self,
        file: &mut File,
        policy: FingerprintPolicy,
    ) -> Result<()> {
        let validation = (|| {
            let metadata_before = file.metadata()?;
            if !self.matches_metadata(&metadata_before) {
                return Err(generation_mismatch_error());
            }

            if !self.content_fingerprints_match(file, policy)? {
                return Err(generation_mismatch_error());
            }

            let metadata_after = file.metadata()?;
            if !self.matches_metadata(&metadata_after)
                || !same_platform_identity(&metadata_before, &metadata_after)
            {
                return Err(generation_mismatch_error());
            }
            Ok(())
        })();

        // Validation hashes move the cursor. Always restore the parser body
        // boundary before returning so successful callers have a deterministic
        // starting point and rejected handles are not left at a sampled offset.
        let restore = file.seek(SeekFrom::Start(self.body_offset));
        validation?;
        restore?;
        Ok(())
    }

    fn content_fingerprints_match(
        &self,
        file: &mut File,
        policy: FingerprintPolicy,
    ) -> Result<bool> {
        // Bounded beginning/end samples do not necessarily cover large VCD
        // headers. Re-hash the complete raw header region on every admission
        // and completion validation under both policies.
        if fingerprint_raw_region(file, 0, self.body_offset)? != self.header_fingerprint {
            return Ok(false);
        }
        if fingerprint_samples(file, self.len)? != self.sample_fingerprint {
            return Ok(false);
        }

        match (policy, self.full_content_fingerprint) {
            (FingerprintPolicy::MetadataAndSamples, None) => Ok(true),
            (FingerprintPolicy::StrictFullContent, Some(expected)) => {
                Ok(fingerprint_full_content(file)? == expected)
            }
            // OpenedVcd stores identity and options together. Any mismatched
            // pair is rejected rather than silently weakening validation.
            _ => Ok(false),
        }
    }

    pub(crate) fn from_open_file(
        file: &mut File,
        header: &[u8],
        body_offset: u64,
        policy: FingerprintPolicy,
    ) -> Result<Self> {
        let metadata = file.metadata()?;
        if body_offset > metadata.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VCD body offset is outside the opened file",
            )
            .into());
        }
        let header_fingerprint = ContentFingerprint::from_hash(blake3::hash(header));
        let sample_fingerprint = fingerprint_samples(file, metadata.len())?;
        let full_content_fingerprint = match policy {
            FingerprintPolicy::MetadataAndSamples => None,
            FingerprintPolicy::StrictFullContent => Some(fingerprint_full_content(file)?),
        };
        let metadata_after = file.metadata()?;
        if metadata.len() != metadata_after.len()
            || metadata.modified().ok() != metadata_after.modified().ok()
            || !same_platform_identity(&metadata, &metadata_after)
        {
            return Err(io::Error::other("VCD changed while calculating file identity").into());
        }
        // Leave the single provenance handle at the parser body boundary for
        // the future OpenedVcd constructor.
        file.seek(SeekFrom::Start(body_offset))?;

        Ok(Self::from_metadata(
            &metadata,
            body_offset,
            header_fingerprint,
            sample_fingerprint,
            full_content_fingerprint,
        ))
    }

    fn from_metadata(
        metadata: &Metadata,
        body_offset: u64,
        header_fingerprint: ContentFingerprint,
        sample_fingerprint: ContentFingerprint,
        full_content_fingerprint: Option<ContentFingerprint>,
    ) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            body_offset,
            header_fingerprint,
            sample_fingerprint,
            full_content_fingerprint,
            #[cfg(unix)]
            device: UnixMetadataExt::dev(metadata),
            #[cfg(unix)]
            inode: UnixMetadataExt::ino(metadata),
        }
    }
}

#[cfg(unix)]
fn matches_stored_platform_identity(identity: &FileIdentity, metadata: &Metadata) -> bool {
    identity.device == UnixMetadataExt::dev(metadata)
        && identity.inode == UnixMetadataExt::ino(metadata)
}

#[cfg(not(unix))]
fn matches_stored_platform_identity(_identity: &FileIdentity, _metadata: &Metadata) -> bool {
    // Stable std exposes device/inode identity on Unix. On other targets the
    // generation remains bound by metadata plus complete-header and content
    // sample fingerprints; stronger native file IDs can be added later without
    // changing the public identity shape.
    true
}

#[cfg(unix)]
fn same_platform_identity(before: &Metadata, after: &Metadata) -> bool {
    UnixMetadataExt::dev(before) == UnixMetadataExt::dev(after)
        && UnixMetadataExt::ino(before) == UnixMetadataExt::ino(after)
}

#[cfg(not(unix))]
fn same_platform_identity(_before: &Metadata, _after: &Metadata) -> bool {
    true
}

fn generation_mismatch_error() -> crate::VcdError {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "VCD source does not match the opened generation",
    )
    .into()
}

fn fingerprint_samples(file: &mut File, len: u64) -> Result<ContentFingerprint> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"vcd_tools_rs bounded samples v1\0");
    let beginning_len = len.min(CONTENT_SAMPLE_BYTES);
    hash_region(file, &mut hasher, 0, beginning_len)?;

    let end_start = len.saturating_sub(CONTENT_SAMPLE_BYTES);
    hasher.update(&end_start.to_le_bytes());
    hash_region(file, &mut hasher, end_start, len - end_start)?;
    Ok(ContentFingerprint::from_hash(hasher.finalize()))
}

fn fingerprint_raw_region(file: &mut File, offset: u64, len: u64) -> Result<ContentFingerprint> {
    file.seek(SeekFrom::Start(offset))?;
    let mut hasher = blake3::Hasher::new();
    let mut remaining = len;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let count = file.read(&mut buffer[..requested])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "VCD changed while calculating header fingerprint",
            )
            .into());
        }
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(ContentFingerprint::from_hash(hasher.finalize()))
}

fn fingerprint_full_content(file: &mut File) -> Result<ContentFingerprint> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"vcd_tools_rs full content v1\0");
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(ContentFingerprint::from_hash(hasher.finalize()))
}

fn hash_region(file: &mut File, hasher: &mut blake3::Hasher, offset: u64, len: u64) -> Result<()> {
    hasher.update(&offset.to_le_bytes());
    hasher.update(&len.to_le_bytes());
    file.seek(SeekFrom::Start(offset))?;
    let mut remaining = len;
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(buffer.len() as u64)).unwrap_or(buffer.len());
        let count = file.read(&mut buffer[..requested])?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "VCD changed while calculating content fingerprint",
            )
            .into());
        }
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, Write};

    use tempfile::NamedTempFile;

    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn foundations_are_send_and_sync() {
        assert_send_sync::<FileIdentity>();
        assert_send_sync::<GenerationId>();
        assert_send_sync::<OpenOptions>();
    }

    #[test]
    fn options_are_builder_based_and_default_to_bounded_samples() {
        let options = OpenOptions::new();
        assert_eq!(
            options.fingerprint_policy(),
            FingerprintPolicy::MetadataAndSamples
        );
        assert_eq!(
            options
                .with_fingerprint_policy(FingerprintPolicy::StrictFullContent)
                .fingerprint_policy(),
            FingerprintPolicy::StrictFullContent
        );
    }

    #[test]
    fn default_identity_hashes_header_and_bounded_content_without_full_hash() {
        let mut temp = NamedTempFile::new().expect("temp file");
        temp.write_all(b"header\nbody").expect("write");
        temp.flush().expect("flush");

        let identity = FileIdentity::from_open_file(
            temp.as_file_mut(),
            b"header\n",
            7,
            FingerprintPolicy::MetadataAndSamples,
        )
        .expect("identity");
        assert_eq!(identity.len(), 11);
        assert_eq!(identity.body_offset(), 7);
        assert!(!identity.is_empty());
        assert_eq!(identity.full_content_fingerprint(), None);
        assert_ne!(identity.header_fingerprint(), identity.sample_fingerprint());
        assert_eq!(temp.as_file_mut().stream_position().expect("position"), 7);
    }

    #[test]
    fn identity_rejects_body_offset_outside_same_handle() {
        let mut temp = NamedTempFile::new().expect("temp file");
        temp.write_all(b"short").expect("write");
        temp.flush().expect("flush");
        let error = FileIdentity::from_open_file(
            temp.as_file_mut(),
            b"short",
            6,
            FingerprintPolicy::MetadataAndSamples,
        )
        .expect_err("invalid offset");
        assert!(error.to_string().contains("body offset"));
    }

    #[test]
    fn strict_identity_adds_full_content_hash() {
        let mut temp = NamedTempFile::new().expect("temp file");
        temp.write_all(b"header\nbody").expect("write");
        temp.flush().expect("flush");
        let identity = FileIdentity::from_open_file(
            temp.as_file_mut(),
            b"header\n",
            7,
            FingerprintPolicy::StrictFullContent,
        )
        .expect("identity");
        assert!(identity.full_content_fingerprint().is_some());
        assert_eq!(temp.as_file_mut().stream_position().expect("position"), 7);
    }

    #[test]
    fn complete_header_validation_covers_regions_outside_bounded_samples() {
        const HEADER_LEN: usize = 96 * 1024;
        const TOTAL_LEN: usize = 256 * 1024;
        const MUTATION_OFFSET: u64 = 72 * 1024;

        let mut bytes = vec![b'h'; TOTAL_LEN];
        bytes[HEADER_LEN..].fill(b'b');
        let mut temp = NamedTempFile::new().expect("temp file");
        temp.write_all(&bytes).expect("write");
        temp.flush().expect("flush");

        let bounded = FileIdentity::from_open_file(
            temp.as_file_mut(),
            &bytes[..HEADER_LEN],
            HEADER_LEN as u64,
            FingerprintPolicy::MetadataAndSamples,
        )
        .expect("bounded identity");
        let strict = FileIdentity::from_open_file(
            temp.as_file_mut(),
            &bytes[..HEADER_LEN],
            HEADER_LEN as u64,
            FingerprintPolicy::StrictFullContent,
        )
        .expect("strict identity");

        temp.as_file_mut()
            .seek(SeekFrom::Start(MUTATION_OFFSET))
            .expect("seek mutation");
        temp.as_file_mut().write_all(b"X").expect("mutate header");
        temp.as_file_mut().flush().expect("flush mutation");

        // The mutation is beyond the first 64 KiB and before the final 64 KiB,
        // so the bounded whole-file samples intentionally remain unchanged.
        assert_eq!(
            fingerprint_samples(temp.as_file_mut(), TOTAL_LEN as u64).expect("samples"),
            bounded.sample_fingerprint()
        );
        assert_ne!(
            fingerprint_raw_region(temp.as_file_mut(), 0, HEADER_LEN as u64).expect("raw header"),
            bounded.header_fingerprint()
        );
        assert!(
            !bounded
                .content_fingerprints_match(
                    temp.as_file_mut(),
                    FingerprintPolicy::MetadataAndSamples,
                )
                .expect("bounded validation")
        );
        assert!(
            !strict
                .content_fingerprints_match(
                    temp.as_file_mut(),
                    FingerprintPolicy::StrictFullContent
                )
                .expect("strict validation")
        );
    }

    #[test]
    fn rejected_validation_restores_body_position() {
        let mut temp = NamedTempFile::new().expect("temp file");
        temp.write_all(b"header\nbody").expect("write");
        temp.flush().expect("flush");
        let identity = FileIdentity::from_open_file(
            temp.as_file_mut(),
            b"header\n",
            7,
            FingerprintPolicy::MetadataAndSamples,
        )
        .expect("identity");
        temp.as_file_mut().set_len(10).expect("truncate");
        assert!(
            identity
                .validate_open_file(temp.as_file_mut(), FingerprintPolicy::MetadataAndSamples)
                .is_err()
        );
        assert_eq!(temp.as_file_mut().stream_position().expect("position"), 7);
    }

    #[test]
    fn bounded_samples_detect_tail_changes() {
        let mut first = NamedTempFile::new().expect("first");
        let mut second = NamedTempFile::new().expect("second");
        first
            .write_all(b"same-header\nbody-a")
            .expect("write first");
        second
            .write_all(b"same-header\nbody-b")
            .expect("write second");
        first.flush().expect("flush first");
        second.flush().expect("flush second");
        let first_identity = FileIdentity::from_open_file(
            first.as_file_mut(),
            b"same-header\n",
            12,
            FingerprintPolicy::MetadataAndSamples,
        )
        .expect("first identity");
        let second_identity = FileIdentity::from_open_file(
            second.as_file_mut(),
            b"same-header\n",
            12,
            FingerprintPolicy::MetadataAndSamples,
        )
        .expect("second identity");
        assert_eq!(
            first_identity.header_fingerprint(),
            second_identity.header_fingerprint()
        );
        assert_ne!(
            first_identity.sample_fingerprint(),
            second_identity.sample_fingerprint()
        );
    }

    #[test]
    fn generations_are_unique_and_monotonic() {
        let first = GenerationId::fresh();
        let second = GenerationId::fresh();
        assert!(second.get() > first.get());
    }
}
