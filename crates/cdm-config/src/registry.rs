//! The property registry: the projection of [`CdmConfig`] that untyped
//! sources are resolved against (`CFG-001`, `CFG-011`).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::meta::{MetaContext, Properties, PropertyMeta};
use crate::model::CdmConfig;

/// Every property cdm-rs knows about, indexed by canonical and legacy name.
///
/// Built once from [`CdmConfig`]; there is no hand-maintained list anywhere in
/// the repository (`CFG-001`).
///
/// ```
/// let registry = cdm_config::PropertyRegistry::global();
/// let host = registry.by_legacy("spark.cdm.connect.origin.host").unwrap();
/// assert_eq!(host.canonical, "connect.origin.host");
/// assert_eq!(host.default_value.as_deref(), Some("localhost"));
/// ```
#[derive(Debug)]
pub struct PropertyRegistry {
    properties: Vec<PropertyMeta>,
    by_canonical: BTreeMap<String, usize>,
    by_legacy: BTreeMap<String, usize>,
}

/// The process-wide registry, built on first use.
static GLOBAL: OnceLock<PropertyRegistry> = OnceLock::new();

impl PropertyRegistry {
    /// The process-wide registry.
    pub fn global() -> &'static Self {
        GLOBAL.get_or_init(Self::build)
    }

    /// Builds a registry by walking the configuration model.
    fn build() -> Self {
        let properties = CdmConfig::properties(&MetaContext::root());
        let mut by_canonical = BTreeMap::new();
        let mut by_legacy = BTreeMap::new();
        for (index, meta) in properties.iter().enumerate() {
            by_canonical.insert(meta.canonical.clone(), index);
            for alias in &meta.legacy {
                by_legacy.insert(alias.clone(), index);
            }
        }
        Self {
            properties,
            by_canonical,
            by_legacy,
        }
    }

    /// Every property, in declaration order.
    pub fn all(&self) -> &[PropertyMeta] {
        &self.properties
    }

    /// Looks a property up by its canonical cdm-rs name.
    pub fn by_canonical(&self, name: &str) -> Option<&PropertyMeta> {
        self.by_canonical
            .get(name)
            .and_then(|index| self.properties.get(*index))
    }

    /// Looks a property up by a legacy `spark.cdm.*` name.
    pub fn by_legacy(&self, name: &str) -> Option<&PropertyMeta> {
        self.by_legacy
            .get(name)
            .and_then(|index| self.properties.get(*index))
    }

    /// Looks a property up by either spelling.
    pub fn resolve(&self, name: &str) -> Option<&PropertyMeta> {
        self.by_canonical(name).or_else(|| self.by_legacy(name))
    }

    /// The known name closest to `name`, for the "did you mean" half of `CFG-011`.
    ///
    /// Candidates are drawn from the same namespace as the input — a misspelled `spark.cdm.*`
    /// key is only ever compared against legacy names — because suggesting a canonical name to
    /// someone editing a Java properties file is not a useful suggestion. The similarity floor
    /// keeps a wholly unrelated key from attracting a nonsense suggestion.
    ///
    /// The metric is normalised Levenshtein, as `CFG-011` specifies. Jaro-Winkler would be the
    /// obvious alternative but it weights a shared prefix so heavily that every key beginning
    /// `spark.cdm.` looks like every other one.
    pub fn closest_match(&self, name: &str) -> Option<&str> {
        const MINIMUM_SIMILARITY: f64 = 0.6;

        let candidates: Vec<&str> = if name.starts_with("spark.") {
            self.by_legacy.keys().map(String::as_str).collect()
        } else {
            self.by_canonical.keys().map(String::as_str).collect()
        };

        candidates
            .into_iter()
            .map(|candidate| (candidate, strsim::normalized_levenshtein(name, candidate)))
            .filter(|(_, score)| *score >= MINIMUM_SIMILARITY)
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(candidate, _)| candidate)
    }

    /// Every property whose value is a credential (`CFG-012`).
    pub fn secrets(&self) -> impl Iterator<Item = &PropertyMeta> {
        self.properties.iter().filter(|meta| meta.secret)
    }
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn cfg_001_the_registry_is_derived_from_the_model_and_has_no_duplicates() {
        let registry = PropertyRegistry::global();
        assert!(registry.all().len() > 80, "{}", registry.all().len());

        let mut canonical: Vec<&str> = registry
            .all()
            .iter()
            .map(|meta| meta.canonical.as_str())
            .collect();
        let total = canonical.len();
        canonical.sort_unstable();
        canonical.dedup();
        assert_eq!(canonical.len(), total, "canonical names must be unique");

        let mut legacy: Vec<&str> = registry
            .all()
            .iter()
            .flat_map(|meta| meta.legacy.iter().map(String::as_str))
            .collect();
        let legacy_total = legacy.len();
        legacy.sort_unstable();
        legacy.dedup();
        assert_eq!(legacy.len(), legacy_total, "legacy aliases must be unique");
    }

    #[test]
    fn cfg_011_lookup_accepts_both_spellings() {
        let registry = PropertyRegistry::global();
        let by_legacy = registry.by_legacy("spark.cdm.perfops.numParts").unwrap();
        let by_canonical = registry.by_canonical("perfops.num_parts").unwrap();
        assert_eq!(by_legacy, by_canonical);
        assert_eq!(registry.resolve("perfops.num_parts"), Some(by_legacy));
        assert_eq!(
            registry.resolve("spark.cdm.perfops.numParts"),
            Some(by_legacy)
        );
        assert!(registry.resolve("spark.cdm.nope").is_none());
    }

    #[test]
    fn cfg_011_a_misspelled_key_gets_the_closest_known_name() {
        let registry = PropertyRegistry::global();
        assert_eq!(
            registry.closest_match("spark.cdm.connect.orgin.host"),
            Some("spark.cdm.connect.origin.host")
        );
        assert_eq!(
            registry.closest_match("spark.cdm.perfops.numPart"),
            Some("spark.cdm.perfops.numParts")
        );
        assert_eq!(
            registry.closest_match("perfops.numparts"),
            Some("perfops.num_parts")
        );
        // A `spark.cdm.*` key is never told to use a canonical name.
        assert!(registry
            .closest_match("spark.cdm.connect.orgin.host")
            .is_some_and(|s| s.starts_with("spark.")));
        // Nothing remotely similar produces no suggestion at all.
        assert_eq!(registry.closest_match("spark.cdm.zzzzzzzzzzzzzz"), None);
    }

    #[test]
    fn cfg_012_every_credential_is_marked_secret() {
        let registry = PropertyRegistry::global();
        let mut secrets: Vec<&str> = registry
            .secrets()
            .map(|meta| meta.canonical.as_str())
            .collect();
        secrets.sort_unstable();
        assert_eq!(
            secrets,
            [
                "connect.origin.password",
                "connect.origin.tls.keystore.password",
                "connect.origin.tls.truststore.password",
                "connect.target.password",
                "connect.target.tls.keystore.password",
                "connect.target.tls.truststore.password",
            ]
        );
        // SPEC §3.3 names `*.password`, `*.token`, `*.keyStore.password` and
        // `*.trustStore.password`; the Astra token is carried by `*.password`, exactly as Java
        // CDM does (`KnownProperties` comment above `ORIGIN_ASTRA_DATABASE_ID`).
        for meta in registry.all() {
            let looks_secret =
                meta.canonical.ends_with("password") || meta.canonical.ends_with("token");
            assert_eq!(
                looks_secret, meta.secret,
                "{} is inconsistently marked",
                meta.canonical
            );
        }
    }
}
