use std::collections::HashMap;
use std::mem::size_of;
use std::sync::Arc;

use vcd::{IdCode, ScopeItem, Var, VarType};

use crate::{Result, Signal, SignalIndex, VcdError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SignalKey(u32);

impl SignalKey {
    fn from_len(len: usize) -> Result<Self> {
        let value = u32::try_from(len).map_err(|_| catalog_capacity_error())?;
        Ok(Self(value))
    }

    fn index(self) -> usize {
        self.0 as usize
    }
}

fn catalog_capacity_error() -> VcdError {
    // Capacity exhaustion is an internal representation limit. Keep it behind
    // the crate's established parse-error boundary instead of adding a public
    // exhaustive VcdError variant before the compact catalog is public.
    VcdError::Parse("VCD signal catalog exceeds supported size".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ScopeKey(u32);

impl ScopeKey {
    fn from_len(len: usize) -> Result<Self> {
        let value = u32::try_from(len).map_err(|_| catalog_capacity_error())?;
        Ok(Self(value))
    }

    fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug)]
struct ScopeNode {
    component: Arc<str>,
    parent: Option<ScopeKey>,
}

#[derive(Debug)]
pub(crate) struct SignalMeta {
    pub(crate) id_code: IdCode,
    pub(crate) full_name: Arc<str>,
    pub(crate) size: u32,
    pub(crate) type_: VarType,
    scope: Option<ScopeKey>,
}

/// Compact internal signal representation.
///
/// Names are allocated once and shared with `by_name`; aliases contain numeric
/// keys rather than cloned `Signal` values; scopes are stored once as an arena.
#[derive(Debug)]
#[allow(dead_code)] // Lookup tables become active when `OpenedVcd` is introduced in RQ-M1-T04.
pub(crate) struct SignalCatalog {
    signals: Box<[SignalMeta]>,
    by_name: HashMap<Arc<str>, SignalKey>,
    aliases_by_id: HashMap<IdCode, Box<[SignalKey]>>,
    scopes: Box<[ScopeNode]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Exposed through `OpenedVcd` diagnostics in the next M1 slice.
pub(crate) struct CatalogMemoryEstimate {
    pub(crate) total_bytes: usize,
    pub(crate) signal_bytes: usize,
    pub(crate) name_bytes: usize,
    pub(crate) scope_bytes: usize,
    pub(crate) alias_bytes: usize,
    pub(crate) lookup_bytes: usize,
}

struct CatalogBuilder {
    signals: Vec<SignalMeta>,
    by_name: HashMap<Arc<str>, SignalKey>,
    aliases_by_id: HashMap<IdCode, Vec<SignalKey>>,
    scopes: Vec<ScopeNode>,
}

impl CatalogBuilder {
    fn new() -> Self {
        Self {
            signals: Vec::new(),
            by_name: HashMap::new(),
            aliases_by_id: HashMap::new(),
            scopes: Vec::new(),
        }
    }

    fn add_scope(&mut self, component: Arc<str>, parent: Option<ScopeKey>) -> Result<ScopeKey> {
        let key = ScopeKey::from_len(self.scopes.len())?;
        self.scopes.push(ScopeNode { component, parent });
        Ok(key)
    }

    fn add_signal(&mut self, scope: Option<ScopeKey>, prefix: &str, var: Var) -> Result<()> {
        let key = SignalKey::from_len(self.signals.len())?;
        let id_code = var.code;
        let size = var.size;
        let type_ = var.var_type;
        let reference = var.reference;
        let index = var.index.map(|index| index.to_string());
        let mut full_name = String::with_capacity(
            prefix.len()
                + usize::from(!prefix.is_empty())
                + reference.len()
                + index.as_ref().map_or(0, String::len),
        );
        if !prefix.is_empty() {
            full_name.push_str(prefix);
            full_name.push('.');
        }
        full_name.push_str(&reference);
        if let Some(index) = index {
            full_name.push_str(&index);
        }
        let full_name: Arc<str> = Arc::from(full_name);

        if self.by_name.contains_key(full_name.as_ref()) {
            return Err(VcdError::DuplicateSignal(full_name.to_string()));
        }

        self.signals.push(SignalMeta {
            id_code,
            full_name: Arc::clone(&full_name),
            size,
            type_,
            scope,
        });
        self.by_name.insert(full_name, key);
        self.aliases_by_id.entry(id_code).or_default().push(key);
        Ok(())
    }

    fn collect(
        &mut self,
        items: Vec<ScopeItem>,
        parent: Option<ScopeKey>,
        prefix: &mut String,
    ) -> Result<()> {
        // vcd 0.7 exposes only a fully materialized Header tree. Consume that
        // tree here so processed ScopeItems and Vars are released/moved as the
        // compact catalog is built instead of retaining the entire tree.
        for item in items {
            match item {
                ScopeItem::Scope(scope) => {
                    let component: Arc<str> = Arc::from(scope.identifier);
                    let children = scope.items;
                    let key = self.add_scope(Arc::clone(&component), parent)?;
                    let old_len = prefix.len();
                    if !prefix.is_empty() {
                        prefix.push('.');
                    }
                    prefix.push_str(&component);
                    self.collect(children, Some(key), prefix)?;
                    prefix.truncate(old_len);
                }
                ScopeItem::Var(var) => self.add_signal(parent, prefix, var)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn finish(self) -> SignalCatalog {
        SignalCatalog {
            signals: self.signals.into_boxed_slice(),
            by_name: self.by_name,
            aliases_by_id: self
                .aliases_by_id
                .into_iter()
                .map(|(id, keys)| (id, keys.into_boxed_slice()))
                .collect(),
            scopes: self.scopes.into_boxed_slice(),
        }
    }
}

#[allow(dead_code)] // Several lookup methods are foundations for `OpenedVcd` in RQ-M1-T04.
impl SignalCatalog {
    pub(crate) fn from_scope_items(items: Vec<ScopeItem>) -> Result<Self> {
        let mut builder = CatalogBuilder::new();
        builder.collect(items, None, &mut String::new())?;
        Ok(builder.finish())
    }

    pub(crate) fn len(&self) -> usize {
        self.signals.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }

    pub(crate) fn signal(&self, key: SignalKey) -> &SignalMeta {
        &self.signals[key.index()]
    }

    pub(crate) fn key_by_name(&self, name: &str) -> Option<SignalKey> {
        self.by_name.get(name).copied()
    }

    pub(crate) fn aliases(&self, id: IdCode) -> &[SignalKey] {
        self.aliases_by_id.get(&id).map_or(&[], Box::as_ref)
    }

    pub(crate) fn names(&self) -> impl ExactSizeIterator<Item = &str> {
        self.signals.iter().map(|signal| signal.full_name.as_ref())
    }

    fn scope_components(&self, scope: Option<ScopeKey>) -> Vec<String> {
        let mut keys = Vec::new();
        let mut current = scope;
        while let Some(key) = current {
            keys.push(key);
            current = self.scopes[key.index()].parent;
        }
        keys.reverse();
        keys.into_iter()
            .map(|key| self.scopes[key.index()].component.to_string())
            .collect()
    }

    pub(crate) fn to_owned_signals(&self) -> Vec<Signal> {
        self.signals
            .iter()
            .map(|signal| Signal {
                name: signal.full_name.to_string(),
                id_code: signal.id_code,
                size: signal.size,
                type_: signal.type_,
                scope: self.scope_components(signal.scope),
            })
            .collect()
    }

    pub(crate) fn to_compatibility_parts(&self) -> Result<(Vec<Signal>, SignalIndex)> {
        let signals = self.to_owned_signals();
        let index = SignalIndex::build(&signals)?;
        Ok((signals, index))
    }

    pub(crate) fn into_compatibility_parts(self) -> Result<(Vec<Signal>, SignalIndex)> {
        let SignalCatalog {
            signals,
            by_name,
            aliases_by_id,
            scopes,
        } = self;
        // Compatibility maps clone public Signals. Release compact lookup maps
        // first so path-based wrappers do not retain both map representations.
        drop(by_name);
        drop(aliases_by_id);

        let scope_components = |scope: Option<ScopeKey>| {
            let mut keys = Vec::new();
            let mut current = scope;
            while let Some(key) = current {
                keys.push(key);
                current = scopes[key.index()].parent;
            }
            keys.reverse();
            keys.into_iter()
                .map(|key| scopes[key.index()].component.to_string())
                .collect()
        };
        let signals = Vec::from(signals)
            .into_iter()
            .map(|signal| Signal {
                name: signal.full_name.to_string(),
                id_code: signal.id_code,
                size: signal.size,
                type_: signal.type_,
                scope: scope_components(signal.scope),
            })
            .collect::<Vec<_>>();
        let index = SignalIndex::build(&signals)?;
        Ok((signals, index))
    }

    /// Estimate bytes owned by the compact catalog.
    ///
    /// Hash-table control bytes and allocator bookkeeping are not available
    /// through stable Rust, so `lookup_bytes` uses entry payload sizes and map
    /// capacity. The estimate is intentionally suitable for before/after trend
    /// evidence, not as an allocator-exact memory limit.
    pub(crate) fn estimated_owned_bytes(&self) -> CatalogMemoryEstimate {
        let signal_bytes = self.signals.len() * size_of::<SignalMeta>();
        let name_bytes = self
            .signals
            .iter()
            .map(|signal| signal.full_name.len())
            .sum();
        let scope_bytes = self.scopes.len() * size_of::<ScopeNode>()
            + self
                .scopes
                .iter()
                .map(|scope| scope.component.len())
                .sum::<usize>();
        let alias_bytes = self
            .aliases_by_id
            .values()
            .map(|keys| keys.len() * size_of::<SignalKey>())
            .sum();
        let lookup_bytes = self.by_name.capacity() * size_of::<(Arc<str>, SignalKey)>()
            + self.aliases_by_id.capacity() * size_of::<(IdCode, Box<[SignalKey]>)>();
        CatalogMemoryEstimate {
            total_bytes: signal_bytes + name_bytes + scope_bytes + alias_bytes + lookup_bytes,
            signal_bytes,
            name_bytes,
            scope_bytes,
            alias_bytes,
            lookup_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::catalog_capacity_error;
    use crate::VcdError;
    use crate::header::{parse_compact_header, read_compact_header};

    const SEMANTICS: &str = "tests/fixtures/query_semantics.vcd";

    #[test]
    fn catalog_shares_names_and_uses_numeric_aliases() {
        let parsed = read_compact_header(SEMANTICS).expect("header");
        let catalog = parsed.catalog;
        assert_eq!(catalog.len(), 7);
        assert!(!catalog.is_empty());
        assert_eq!(catalog.names().next(), Some("top.a"));

        let a = catalog.key_by_name("top.a").expect("a");
        let alias = catalog.key_by_name("top.alias_a").expect("alias");
        assert_eq!(catalog.signal(a).id_code, catalog.signal(alias).id_code);
        assert_eq!(catalog.aliases(catalog.signal(a).id_code), &[a, alias]);
    }

    #[test]
    fn capacity_limit_uses_existing_parse_error_boundary() {
        let error = catalog_capacity_error();
        assert!(matches!(error, VcdError::Parse(_)));
        assert_eq!(
            error.to_string(),
            "unexpected parse error: VCD signal catalog exceeds supported size"
        );
    }

    #[test]
    fn nested_scope_arena_reconstructs_compatibility_scope() {
        let bytes = b"$scope module outer $end\n\
$scope module inner $end\n\
$var wire 1 ! value $end\n\
$upscope $end\n\
$upscope $end\n\
$enddefinitions $end\n";
        let parsed = parse_compact_header(bytes, bytes.len() as u64).expect("nested header");
        assert_eq!(
            parsed.catalog.names().collect::<Vec<_>>(),
            ["outer.inner.value"]
        );
        let (signals, _) = parsed
            .catalog
            .into_compatibility_parts()
            .expect("compatibility parts");
        assert_eq!(signals[0].scope, ["outer", "inner"]);
    }

    #[test]
    fn compatibility_materialization_preserves_scope_and_order() {
        let parsed = read_compact_header(SEMANTICS).expect("header");
        let (signals, index) = parsed
            .catalog
            .into_compatibility_parts()
            .expect("compatibility parts");
        assert_eq!(signals[0].name, "top.a");
        assert_eq!(signals[0].scope, vec!["top"]);
        assert_eq!(index.by_name["top.alias_a"].scope, vec!["top"]);
        assert_eq!(index.by_id[&signals[0].id_code].len(), 2);
    }

    fn diagnostic_vcd() -> String {
        std::env::var("VCD_CATALOG_PROBE").unwrap_or_else(|_| "tests/waveform.vcd".to_string())
    }

    #[test]
    #[ignore = "release-only diagnostic; run through scripts/measure-catalog-memory.sh"]
    fn probe_compact_peak_rss() {
        let parsed = read_compact_header(diagnostic_vcd()).expect("large header");
        let signal_count = parsed.catalog.len();
        let estimate = parsed.catalog.estimated_owned_bytes();
        eprintln!(
            "mode=compact signals={signal_count} estimated_catalog_bytes={} estimated_bytes_per_signal={:.2}",
            estimate.total_bytes,
            estimate.total_bytes as f64 / signal_count as f64
        );
        std::hint::black_box(&parsed);
    }

    #[test]
    #[ignore = "release-only diagnostic; run through scripts/measure-catalog-memory.sh"]
    fn probe_compatibility_peak_rss() {
        let parsed = read_compact_header(diagnostic_vcd()).expect("large header");
        let signal_count = parsed.catalog.len();
        let compatibility = parsed
            .catalog
            .into_compatibility_parts()
            .expect("compatibility materialization");
        eprintln!(
            "mode=compatibility signals={signal_count} owned_signals={} by_name={} by_id={}",
            compatibility.0.len(),
            compatibility.1.by_name.len(),
            compatibility.1.by_id.len()
        );
        std::hint::black_box(&compatibility);
    }

    #[test]
    fn memory_estimate_has_each_component() {
        let parsed = read_compact_header(SEMANTICS).expect("header");
        let estimate = parsed.catalog.estimated_owned_bytes();
        assert!(estimate.total_bytes > 0);
        assert!(estimate.signal_bytes > 0);
        assert!(estimate.name_bytes > 0);
        assert!(estimate.scope_bytes > 0);
        assert!(estimate.alias_bytes > 0);
        assert!(estimate.lookup_bytes > 0);
        assert_eq!(
            estimate.total_bytes,
            estimate.signal_bytes
                + estimate.name_bytes
                + estimate.scope_bytes
                + estimate.alias_bytes
                + estimate.lookup_bytes
        );
    }
}
