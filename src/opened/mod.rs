mod identity;

use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use vcd::{IdCode, Parser, VarType};

use crate::catalog::{CatalogMemoryEstimate, SignalCatalog, SignalKey};
use crate::header::read_identified_compact_header;
use crate::{Result, Signal, SignalIndex, Timescale};

pub use identity::{
    ContentFingerprint, FileIdentity, FingerprintPolicy, GenerationId, OpenOptions,
};

/// Memory owned by the compact signal catalog.
///
/// Hash-table control bytes and allocator metadata are estimated rather than
/// measured exactly. This structure is intended for diagnostics and regression
/// tracking, not enforcement of a hard allocator limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogMemoryUsage {
    pub total_bytes: usize,
    pub signal_bytes: usize,
    pub name_bytes: usize,
    pub scope_bytes: usize,
    pub alias_bytes: usize,
    pub lookup_bytes: usize,
}

impl From<CatalogMemoryEstimate> for CatalogMemoryUsage {
    fn from(value: CatalogMemoryEstimate) -> Self {
        Self {
            total_bytes: value.total_bytes,
            signal_bytes: value.signal_bytes,
            name_bytes: value.name_bytes,
            scope_bytes: value.scope_bytes,
            alias_bytes: value.alias_bytes,
            lookup_bytes: value.lookup_bytes,
        }
    }
}

/// Borrowed metadata for one signal in an [`OpenedVcd`].
///
/// The reference cannot outlive the opened catalog. Names and scope components
/// borrow the catalog's interned storage; converting to the legacy owned
/// [`Signal`] is explicit.
#[derive(Clone, Copy)]
pub struct SignalRef<'a> {
    catalog: &'a SignalCatalog,
    key: SignalKey,
}

impl fmt::Debug for SignalRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignalRef")
            .field("name", &self.name())
            .field("id_code", &self.id_code())
            .field("size", &self.size())
            .field("type_", &self.var_type())
            .finish()
    }
}

impl<'a> SignalRef<'a> {
    fn metadata(self) -> &'a crate::catalog::SignalMeta {
        self.catalog.signal(self.key)
    }

    pub fn name(self) -> &'a str {
        self.metadata().full_name.as_ref()
    }

    pub fn id_code(self) -> IdCode {
        self.metadata().id_code
    }

    pub fn size(self) -> u32 {
        self.metadata().size
    }

    pub fn var_type(self) -> VarType {
        self.metadata().type_
    }

    pub fn scope_components(self) -> impl ExactSizeIterator<Item = &'a str> {
        self.catalog.scope_component_refs(self.key).into_iter()
    }

    pub fn to_owned(self) -> Signal {
        Signal {
            name: self.name().to_string(),
            id_code: self.id_code(),
            size: self.size(),
            type_: self.var_type(),
            scope: self.scope_components().map(str::to_string).collect(),
        }
    }
}

#[derive(Debug)]
struct OpenedVcdInner {
    display_path: PathBuf,
    configured_path: PathBuf,
    canonical_target: PathBuf,
    generation: GenerationId,
    identity: FileIdentity,
    body_offset: u64,
    timescale: Option<Timescale>,
    catalog: SignalCatalog,
    options: OpenOptions,
}

/// One immutable VCD generation with a reusable compact signal catalog.
///
/// Cloning is cheap: clones share immutable state through an [`Arc`]. Body
/// queries open and validate independent file handles; no mutable parser or
/// shared seek cursor is stored here.
#[derive(Debug, Clone)]
pub struct OpenedVcd {
    inner: Arc<OpenedVcdInner>,
}

impl OpenedVcd {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_options(path, OpenOptions::default())
    }

    pub fn open_with_options(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let display_path = path.as_ref().to_path_buf();
        // Stabilize relative paths against later process cwd changes without
        // resolving the final symlink. Reopening this exact configured path is
        // part of generation validation: a retargeted symlink must not silently
        // keep serving the target resolved during open.
        let configured_path = if display_path.is_absolute() {
            display_path.clone()
        } else {
            std::env::current_dir()?.join(&display_path)
        };
        let identified = read_identified_compact_header(&configured_path, &options)?;
        // Canonical target is diagnostic only and is never used to reopen the
        // source. It may cease to exist or change after this generation opens.
        let canonical_target = std::fs::canonicalize(&configured_path)?;
        let header = identified.header;
        debug_assert_eq!(header.body_offset, identified.identity.body_offset());
        Ok(Self {
            inner: Arc::new(OpenedVcdInner {
                display_path,
                configured_path,
                canonical_target,
                generation: GenerationId::fresh(),
                identity: identified.identity,
                body_offset: header.body_offset,
                timescale: header.timescale,
                catalog: header.catalog,
                options,
            }),
        })
    }

    /// The path spelling supplied by the caller.
    pub fn path(&self) -> &Path {
        &self.inner.display_path
    }

    /// Absolute configured path used for every independent reopen. The final
    /// component is not canonicalized, so symlink retargeting is detectable.
    pub fn configured_path(&self) -> &Path {
        &self.inner.configured_path
    }

    /// Canonical target captured for diagnostics at open time. Readers never
    /// reopen this path; they use [`OpenedVcd::configured_path`].
    pub fn canonical_path(&self) -> &Path {
        &self.inner.canonical_target
    }

    pub fn generation(&self) -> GenerationId {
        self.inner.generation
    }

    pub fn identity(&self) -> &FileIdentity {
        &self.inner.identity
    }

    pub fn options(&self) -> &OpenOptions {
        &self.inner.options
    }

    pub fn body_offset(&self) -> u64 {
        self.inner.body_offset
    }

    pub fn timescale(&self) -> Option<&Timescale> {
        self.inner.timescale.as_ref()
    }

    pub fn signal_count(&self) -> usize {
        self.inner.catalog.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.catalog.is_empty()
    }

    pub fn signals(&self) -> impl ExactSizeIterator<Item = SignalRef<'_>> {
        self.inner.catalog.signal_keys().map(|key| SignalRef {
            catalog: &self.inner.catalog,
            key,
        })
    }

    pub fn signal(&self, name: &str) -> Option<SignalRef<'_>> {
        self.inner.catalog.key_by_name(name).map(|key| SignalRef {
            catalog: &self.inner.catalog,
            key,
        })
    }

    pub fn aliases(&self, id: IdCode) -> impl ExactSizeIterator<Item = SignalRef<'_>> {
        self.inner
            .catalog
            .aliases(id)
            .iter()
            .copied()
            .map(|key| SignalRef {
                catalog: &self.inner.catalog,
                key,
            })
    }

    pub fn signal_names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.inner.catalog.names()
    }

    pub fn list_signals(&self, filter: Option<&str>) -> Vec<String> {
        self.signal_names()
            .filter(|name| filter.is_none_or(|filter| name.contains(filter)))
            .map(str::to_string)
            .collect()
    }

    pub fn catalog_memory_usage(&self) -> CatalogMemoryUsage {
        self.inner.catalog.estimated_owned_bytes().into()
    }

    /// Materialize legacy owned signal metadata without changing its shape.
    pub fn to_owned_signals(&self) -> Vec<Signal> {
        self.inner.catalog.to_owned_signals()
    }

    /// Materialize the legacy clone-heavy signal index explicitly.
    pub fn to_compatibility_parts(&self) -> Result<(Vec<Signal>, SignalIndex)> {
        self.inner.catalog.to_compatibility_parts()
    }

    /// Open a new independently positioned reader after validating the source
    /// path against this generation.
    pub fn body_reader(&self) -> Result<OpenedBodyReader> {
        let mut file = File::open(&self.inner.configured_path)?;
        self.inner
            .identity
            .validate_open_file(&mut file, self.inner.options.fingerprint_policy())?;
        Ok(OpenedBodyReader {
            reader: BufReader::new(file),
            generation: Arc::clone(&self.inner),
        })
    }

    /// Create a forward-only VCD command parser backed by an independent,
    /// generation-validated reader.
    pub fn body_parser(&self) -> Result<Parser<OpenedBodyReader>> {
        Ok(Parser::new(self.body_reader()?))
    }

    /// Revalidate the configured source path without creating a parser.
    pub fn validate_source(&self) -> Result<()> {
        let mut file = File::open(&self.inner.configured_path)?;
        self.inner
            .identity
            .validate_open_file(&mut file, self.inner.options.fingerprint_policy())
    }
}

/// Independently positioned body reader for one [`OpenedVcd`] generation.
///
/// Call [`OpenedBodyReader::validate_complete`] before publishing a completed
/// result. A dropped reader cannot report a source mutation, so query engines
/// must make completion validation explicit.
pub struct OpenedBodyReader {
    reader: BufReader<File>,
    generation: Arc<OpenedVcdInner>,
}

impl fmt::Debug for OpenedBodyReader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenedBodyReader")
            .field("generation", &self.generation.generation)
            .field("path", &self.generation.display_path)
            .finish_non_exhaustive()
    }
}

impl OpenedBodyReader {
    pub fn generation(&self) -> GenerationId {
        self.generation.generation
    }

    /// Validate source identity after query execution while retaining the
    /// reader's logical cursor position.
    pub fn validate_complete(&mut self) -> Result<()> {
        let position = self.reader.stream_position()?;
        let validation = self.generation.identity.validate_open_file(
            self.reader.get_mut(),
            self.generation.options.fingerprint_policy(),
        );
        let restore = self.reader.seek(SeekFrom::Start(position));
        validation?;
        restore?;

        // The admitted handle can remain valid after a configured symlink is
        // retargeted. Reopen and validate the configured path as well so a
        // completed response is bound to the same path generation.
        let mut current = File::open(&self.generation.configured_path)?;
        self.generation
            .identity
            .validate_open_file(&mut current, self.generation.options.fingerprint_policy())?;
        Ok(())
    }
}

impl Read for OpenedBodyReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

impl BufRead for OpenedBodyReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.reader.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.reader.consume(amount);
    }
}

impl Seek for OpenedBodyReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.reader.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Seek as _, Write as _};
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;
    use vcd::Command;

    use super::*;

    const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";

    fn assert_send_sync<T: Send + Sync>() {}

    fn copy_fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("wave.vcd");
        std::fs::copy(SEMANTICS, &path).expect("copy fixture");
        (directory, path)
    }

    fn replace_file(replacement: &Path, destination: &Path) {
        #[cfg(windows)]
        std::fs::remove_file(destination).expect("remove destination");
        std::fs::rename(replacement, destination).expect("replace");
    }

    #[test]
    fn opened_vcd_is_send_sync_and_clone_shares_inner() {
        assert_send_sync::<OpenedVcd>();
        assert_send_sync::<OpenedBodyReader>();
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let cloned = opened.clone();
        assert!(Arc::ptr_eq(&opened.inner, &cloned.inner));
        assert_eq!(opened.generation(), cloned.generation());
    }

    #[test]
    fn open_exposes_compact_order_lookup_aliases_scope_and_diagnostics() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        assert_eq!(opened.path(), Path::new(SEMANTICS));
        assert!(opened.configured_path().is_absolute());
        assert!(opened.configured_path().ends_with(SEMANTICS));
        assert!(opened.canonical_path().is_absolute());
        assert_eq!(opened.signal_count(), 7);
        assert!(!opened.is_empty());
        assert_eq!(opened.signal_names().next(), Some("top.a"));
        assert_eq!(opened.list_signals(Some("vec")), vec!["top.vec4[3:0]"]);
        let signal = opened.signal("top.a").expect("signal");
        assert_eq!(signal.name(), "top.a");
        assert_eq!(signal.size(), 1);
        assert_eq!(signal.scope_components().collect::<Vec<_>>(), ["top"]);
        assert_eq!(signal.to_owned().scope, ["top"]);
        assert_eq!(
            opened
                .aliases(signal.id_code())
                .map(SignalRef::name)
                .collect::<Vec<_>>(),
            ["top.a", "top.alias_a"]
        );
        assert!(opened.catalog_memory_usage().total_bytes > 0);
    }

    #[test]
    fn legacy_materialization_preserves_order_aliases_and_scopes() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let (signals, index) = opened.to_compatibility_parts().expect("compatibility");
        assert_eq!(signals[0].name, "top.a");
        assert_eq!(signals[0].scope, ["top"]);
        assert_eq!(index.by_id[&signals[0].id_code].len(), 2);
        assert_eq!(opened.to_owned_signals(), signals);
    }

    #[test]
    fn body_readers_have_independent_cursors_and_generation() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut first = opened.body_reader().expect("first reader");
        let mut second = opened.body_reader().expect("second reader");
        assert_eq!(first.generation(), opened.generation());
        assert_eq!(
            first.stream_position().expect("position"),
            opened.body_offset()
        );
        assert_eq!(
            second.stream_position().expect("position"),
            opened.body_offset()
        );

        let mut byte = [0_u8; 1];
        first.read_exact(&mut byte).expect("read first");
        assert_eq!(
            second.stream_position().expect("second unchanged"),
            opened.body_offset()
        );
        first.validate_complete().expect("first complete");
        assert_eq!(
            first.stream_position().expect("restored"),
            opened.body_offset() + 1
        );
    }

    #[test]
    fn concurrent_parsers_are_independent_and_equivalent() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let workers = (0..8)
            .map(|_| {
                let opened = opened.clone();
                thread::spawn(move || {
                    let mut parser = opened.body_parser().expect("parser");
                    let mut timestamps = Vec::new();
                    for command in &mut parser {
                        if let Command::Timestamp(time) = command.expect("command") {
                            timestamps.push(time);
                        }
                    }
                    parser.reader().validate_complete().expect("complete");
                    timestamps
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            assert_eq!(worker.join().expect("worker"), [5, 10, 10, 15, 20, 25]);
        }
    }

    #[test]
    fn open_does_not_parse_the_body() {
        let opened =
            OpenedVcd::open("tests/fixtures/malformed_body.vcd").expect("header-only open");
        assert_eq!(opened.signal_count(), 1);
        let mut parser = opened.body_parser().expect("body parser");
        assert!(parser.any(|command| command.is_err()));
    }

    #[cfg(unix)]
    #[test]
    fn configured_symlink_retarget_rejects_admission_and_completion() {
        let directory = tempdir().expect("temp directory");
        let target_a = directory.path().join("a.vcd");
        let target_b = directory.path().join("b.vcd");
        let configured_link = directory.path().join("configured.vcd");
        std::fs::copy(SEMANTICS, &target_a).expect("copy target A");
        std::fs::copy(SEMANTICS, &target_b).expect("copy target B");
        symlink(&target_a, &configured_link).expect("create configured symlink");

        let opened = OpenedVcd::open(&configured_link).expect("open symlink generation");
        assert_eq!(opened.configured_path(), configured_link);
        assert_eq!(
            opened.canonical_path(),
            target_a.canonicalize().expect("target A")
        );
        let mut admitted = opened.body_reader().expect("admit target A");

        let replacement_link = directory.path().join("replacement-link");
        symlink(&target_b, &replacement_link).expect("create replacement symlink");
        std::fs::rename(&replacement_link, &configured_link).expect("retarget symlink");

        assert!(opened.body_reader().is_err(), "retargeted symlink admitted");
        assert!(
            opened.validate_source().is_err(),
            "retargeted symlink validated"
        );
        assert!(
            admitted.validate_complete().is_err(),
            "completion ignored configured symlink retarget"
        );
    }

    #[test]
    fn replacement_append_and_truncate_are_rejected_before_admission() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");

        let replacement = path.with_extension("replacement");
        std::fs::copy(SEMANTICS, &replacement).expect("copy replacement");
        replace_file(&replacement, &path);
        assert!(opened.body_reader().is_err(), "atomic replacement accepted");

        let opened = OpenedVcd::open(&path).expect("reopen replacement");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append open")
            .write_all(b"\n#999\n")
            .expect("append");
        assert!(opened.body_reader().is_err(), "append accepted");

        let opened = OpenedVcd::open(&path).expect("reopen appended");
        let length = std::fs::metadata(&path).expect("metadata").len();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("truncate open")
            .set_len(length - 4)
            .expect("truncate");
        assert!(opened.body_reader().is_err(), "truncate accepted");
    }

    #[test]
    fn sampled_in_place_mutation_is_rejected_before_admission() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("mutate open");
        file.seek(SeekFrom::Start(opened.body_offset()))
            .expect("seek");
        file.write_all(b"!").expect("mutate sample");
        file.flush().expect("flush");
        assert!(opened.body_reader().is_err(), "sampled mutation accepted");
    }

    #[test]
    fn completion_validation_rejects_mutation_after_admission() {
        let (_directory, path) = copy_fixture();
        let opened = OpenedVcd::open(&path).expect("open");
        let mut reader = opened.body_reader().expect("reader");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append open")
            .write_all(b"\n#999\n")
            .expect("append");
        assert!(reader.validate_complete().is_err());
    }

    #[test]
    fn strict_policy_is_retained_by_opened_generation() {
        let options =
            OpenOptions::new().with_fingerprint_policy(FingerprintPolicy::StrictFullContent);
        let opened = OpenedVcd::open_with_options(SEMANTICS, options).expect("strict open");
        assert_eq!(
            opened.options().fingerprint_policy(),
            FingerprintPolicy::StrictFullContent
        );
        assert!(opened.identity().full_content_fingerprint().is_some());
        opened.validate_source().expect("strict validation");
    }

    #[test]
    #[ignore = "release-only diagnostic; run through scripts/measure-catalog-memory.sh"]
    fn probe_opened_list_peak_rss() {
        let path =
            std::env::var("VCD_CATALOG_PROBE").unwrap_or_else(|_| "tests/waveform.vcd".to_string());
        let opened = OpenedVcd::open(path).expect("opened VCD");
        let signal_count = opened.signal_count();
        let listed_bytes = opened.signal_names().map(str::len).sum::<usize>();
        let first = opened.signals().next().expect("first signal");
        let lookup = opened.signal(first.name()).expect("lookup");
        let list_started = Instant::now();
        let repeated_listed_bytes = (0..100)
            .map(|_| opened.signal_names().map(str::len).sum::<usize>())
            .sum::<usize>();
        let repeated_list_us = list_started.elapsed().as_micros();
        let lookup_started = Instant::now();
        let repeated_lookup_size = (0..100_000)
            .map(|_| opened.signal(first.name()).expect("repeated lookup").size() as usize)
            .sum::<usize>();
        let repeated_lookup_us = lookup_started.elapsed().as_micros();
        let estimate = opened.catalog_memory_usage();
        eprintln!(
            "mode=opened signals={signal_count} listed_bytes={listed_bytes} lookup={} estimated_catalog_bytes={} repeated_list_100_us={repeated_list_us} repeated_lookup_100000_us={repeated_lookup_us}",
            lookup.name(),
            estimate.total_bytes
        );
        std::hint::black_box((
            opened,
            listed_bytes,
            repeated_listed_bytes,
            repeated_lookup_size,
        ));
    }

    #[test]
    fn body_parser_starts_at_body_without_reparsing_header() {
        let opened = OpenedVcd::open(SEMANTICS).expect("open");
        let mut parser = opened.body_parser().expect("parser");
        assert!(matches!(
            parser.next().expect("command").expect("valid"),
            Command::Begin(_)
        ));
    }
}
