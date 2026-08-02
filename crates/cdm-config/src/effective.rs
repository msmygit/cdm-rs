//! The immutable, hashed result of loading and validating (`CFG-023`, `CFG-037`, `DST-003`).
//!
//! [`EffectiveConfig`] is what the engine plans from. It differs from [`CdmConfig`] in that every
//! rule that *resolves* rather than rejects has already been applied:
//!
//! * an unset target keyspace and table has become the origin's (`CFG-023`);
//! * an explicit TTL or writetime column list has turned the corresponding automatic mode off
//!   (`CFG-037`);
//! * the defaults that are computed rather than constant — worker count, node identity, whether
//!   Prometheus is served — have been decided.
//!
//! Doing this once, here, is what keeps `ARCHITECTURE.md` §5.5 true: nothing downstream has to
//! re-derive a default, so nothing downstream can derive it differently.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};

use cdm_core::TableRef;

use crate::model::CdmConfig;
use crate::validate::parse_keyspace_table;

/// A configuration with every derived value resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveConfig {
    config: CdmConfig,
    origin_table: Option<TableRef>,
    target_table: Option<TableRef>,
    workers: u32,
    node_id: String,
    config_hash: String,
}

impl EffectiveConfig {
    /// Resolves a validated configuration.
    ///
    /// Call this only on a configuration that [`Validator`](crate::Validator) accepted: the
    /// resolution rules assume the values are well formed, and quietly leave a malformed one
    /// alone rather than inventing a plausible substitute for it.
    pub fn resolve(mut config: CdmConfig) -> Self {
        // CFG-023: the target defaults to the origin.
        if config
            .schema
            .target
            .keyspace_table
            .as_deref()
            .is_none_or(str::is_empty)
        {
            config
                .schema
                .target
                .keyspace_table
                .clone_from(&config.schema.origin.keyspace_table);
        }

        // CFG-037: an explicit list wins over the automatic mode, as Java CDM resolves it.
        if !config.schema.origin.ttl.names.is_empty() {
            config.schema.origin.ttl.automatic = false;
        }
        if !config.schema.origin.writetime.names.is_empty() {
            config.schema.origin.writetime.automatic = false;
        }

        let origin_table = config
            .schema
            .origin
            .keyspace_table
            .as_deref()
            .and_then(parse_keyspace_table);
        let target_table = config
            .schema
            .target
            .keyspace_table
            .as_deref()
            .and_then(parse_keyspace_table);

        let workers = config
            .perfops
            .workers
            .unwrap_or_else(|| u32::try_from(num_cpus::get()).unwrap_or(u32::MAX).max(1));
        let node_id = config
            .cluster
            .node_id
            .clone()
            .unwrap_or_else(default_node_id);
        let config_hash = hash(&config);

        Self {
            config,
            origin_table,
            target_table,
            workers,
            node_id,
            config_hash,
        }
    }

    /// The resolved configuration.
    pub fn config(&self) -> &CdmConfig {
        &self.config
    }

    /// The origin table, absent only if `schema.origin.keyspace_table` was malformed.
    pub fn origin_table(&self) -> Option<&TableRef> {
        self.origin_table.as_ref()
    }

    /// The target table, which `CFG-023` defaults to the origin's.
    pub fn target_table(&self) -> Option<&TableRef> {
        self.target_table.as_ref()
    }

    /// The number of range workers, resolved from `num_cpus` when unset.
    pub fn workers(&self) -> u32 {
        self.workers
    }

    /// Whether the Prometheus endpoint is served, which defaults to whether the server is on.
    pub fn prometheus_enabled(&self) -> bool {
        self.config
            .metrics
            .prometheus
            .enabled
            .unwrap_or(self.config.server.enabled)
    }

    /// This node's identity in distributed mode, defaulted to the host name and process id.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// A stable digest of the configuration, **excluding secrets** (`DST-003`).
    ///
    /// Secrets are excluded structurally rather than by an exclusion list: the digest is taken
    /// over the serialised form, and [`Secret`](crate::Secret) serialises as `***`. Two nodes
    /// with the same plan and different credentials therefore agree, which is what distributed
    /// mode needs. The digest is the standard library's hasher, chosen for stability across the
    /// runs of one build rather than for cryptographic strength; it is a consistency check, not
    /// a signature.
    pub fn config_hash(&self) -> &str {
        &self.config_hash
    }

    /// The flat, string-valued view that `cdm-core`'s plugin traits speak in (`PLG-013`).
    ///
    /// Keys are the legacy `spark.cdm.*` names where a property has one, so a plugin ported from
    /// Java reads the key it already knows; properties that are new to cdm-rs appear under their
    /// canonical name. Credentials are omitted entirely (`SEC-001`).
    pub fn to_core(&self) -> cdm_core::EffectiveConfig {
        let json = serde_json::to_value(&self.config).unwrap_or(serde_json::Value::Null);
        let mut out = cdm_core::EffectiveConfig::new();
        for meta in crate::PropertyRegistry::global().all() {
            if meta.secret {
                continue;
            }
            let Some(value) = lookup(&json, &meta.canonical) else {
                continue;
            };
            let rendered = match value {
                serde_json::Value::Null => continue,
                serde_json::Value::String(text) => text.clone(),
                serde_json::Value::Array(items) => items
                    .iter()
                    .map(|item| match item {
                        serde_json::Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(","),
                other => other.to_string(),
            };
            let key = meta
                .legacy
                .first()
                .cloned()
                .unwrap_or_else(|| meta.canonical.clone());
            out.insert(key, rendered);
        }
        out
    }
}

/// Follows a dotted canonical path through a JSON tree.
fn lookup<'a>(tree: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    path.split('.')
        .try_fold(tree, |node, segment| node.get(segment))
}

/// `hostname-pid`, or `unknown-pid` where the host name is not exposed to the process.
fn default_node_id() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_owned());
    format!("{host}-{}", std::process::id())
}

/// Digests the serialised configuration, in which secrets are already redacted.
fn hash(config: &CdmConfig) -> String {
    let mut hasher = DefaultHasher::new();
    serde_json::to_string(config)
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
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
    use crate::secret::Secret;

    fn base() -> CdmConfig {
        let mut config = CdmConfig::default();
        config.schema.origin.keyspace_table = Some("ks.src".to_owned());
        config
    }

    #[test]
    fn cfg_023_an_unset_target_table_defaults_to_the_origin_table() {
        let effective = EffectiveConfig::resolve(base());
        assert_eq!(
            effective.target_table().map(ToString::to_string),
            Some("ks.src".to_owned())
        );
        assert_eq!(
            effective.config().schema.target.keyspace_table.as_deref(),
            Some("ks.src")
        );

        let mut explicit = base();
        explicit.schema.target.keyspace_table = Some("other.dst".to_owned());
        let effective = EffectiveConfig::resolve(explicit);
        assert_eq!(
            effective.target_table().map(ToString::to_string),
            Some("other.dst".to_owned())
        );
        assert_eq!(
            effective.origin_table().map(ToString::to_string),
            Some("ks.src".to_owned())
        );

        // An empty string is as good as unset, which is how a `.properties` file spells "unset".
        let mut empty = base();
        empty.schema.target.keyspace_table = Some(String::new());
        assert_eq!(
            EffectiveConfig::resolve(empty)
                .target_table()
                .map(ToString::to_string),
            Some("ks.src".to_owned())
        );
    }

    #[test]
    fn cfg_037_explicit_column_names_switch_the_automatic_mode_off() {
        let mut config = base();
        config.schema.origin.ttl.names = vec!["data".to_owned()];
        assert!(config.schema.origin.ttl.automatic, "the default is on");

        let effective = EffectiveConfig::resolve(config);
        assert!(!effective.config().schema.origin.ttl.automatic);
        // The writetime side is untouched, because no writetime columns were named.
        assert!(effective.config().schema.origin.writetime.automatic);

        let mut both = base();
        both.schema.origin.writetime.names = vec!["data".to_owned()];
        let effective = EffectiveConfig::resolve(both);
        assert!(!effective.config().schema.origin.writetime.automatic);
    }

    #[test]
    fn cfg_200_computed_defaults_are_decided_once() {
        let effective = EffectiveConfig::resolve(base());
        assert!(effective.workers() >= 1);
        assert!(effective.node_id().contains('-'));
        // Prometheus follows the server unless it is set explicitly.
        assert!(!effective.prometheus_enabled());

        let mut serving = base();
        serving.server.enabled = true;
        assert!(EffectiveConfig::resolve(serving).prometheus_enabled());

        let mut explicit = base();
        explicit.server.enabled = true;
        explicit.metrics.prometheus.enabled = Some(false);
        assert!(!EffectiveConfig::resolve(explicit).prometheus_enabled());

        let mut pinned = base();
        pinned.perfops.workers = Some(3);
        pinned.cluster.node_id = Some("node-a".to_owned());
        let effective = EffectiveConfig::resolve(pinned);
        assert_eq!(effective.workers(), 3);
        assert_eq!(effective.node_id(), "node-a");
    }

    #[test]
    fn cfg_001_the_config_hash_ignores_credentials_but_not_the_plan() {
        let a = EffectiveConfig::resolve(base());

        let mut different_password = base();
        different_password.connect.origin.password = Secret::new("something-else");
        let b = EffectiveConfig::resolve(different_password);
        assert_eq!(
            a.config_hash(),
            b.config_hash(),
            "credentials must not change the hash (DST-003)"
        );

        let mut different_plan = base();
        different_plan.perfops.num_parts = 9;
        let c = EffectiveConfig::resolve(different_plan);
        assert_ne!(a.config_hash(), c.config_hash());
        assert_eq!(a.config_hash().len(), 16);
    }

    #[test]
    fn cfg_001_the_flat_plugin_view_uses_the_java_key_where_there_is_one() {
        let mut config = base();
        config.perfops.num_parts = 42;
        config.schema.origin.column.skip = vec!["a".to_owned(), "b".to_owned()];
        let flat = EffectiveConfig::resolve(config).to_core();

        assert_eq!(flat.get("spark.cdm.perfops.numParts"), Some("42"));
        assert_eq!(
            flat.get("spark.cdm.schema.origin.keyspaceTable"),
            Some("ks.src")
        );
        assert_eq!(flat.get("spark.cdm.schema.origin.column.skip"), Some("a,b"));
        // A property with no Java name appears under its canonical one.
        assert_eq!(flat.get("logging.level"), Some("info"));
        // SEC-001: no credential is in the flat view at all, redacted or otherwise.
        assert!(flat.get("spark.cdm.connect.origin.password").is_none());
    }
}
