//! The property keys the features read, and the typed accessors they share.
//!
//! # Why the keys live in one place
//!
//! Every feature in this crate is configured from the same flat property map (`CFG-100`..`CFG-200`,
//! reachable here as [`EffectiveConfig`]), and every key has two spellings: the canonical cdm-rs
//! name and the Java `spark.cdm.*` alias that `CFG-011` requires cdm-rs to keep accepting. Spreading
//! that pairing across five feature modules would guarantee that one of them eventually drifts, so
//! the whole §3.5 slice this crate cares about is declared once, here, and
//! [`registry`] lets a test assert it against the specification table.
//!
//! # Precedence
//!
//! The canonical name wins when both spellings are present. That is the same rule the configuration
//! loader applies when it folds a `.properties` file into the typed model (`CFG-010`), and repeating
//! it here means a feature behaves identically whether it is driven by a loaded `CdmConfig` or by a
//! hand-built [`EffectiveConfig`] in a unit test.

use cdm_core::{CdmError, EffectiveConfig, ErrorKind};

/// One configuration property, in both of its spellings (`CFG-011`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PropertyKey {
    canonical: &'static str,
    legacy: &'static str,
}

impl PropertyKey {
    /// Declares a key. `legacy` is the Java `spark.cdm.*` name exactly as `KnownProperties` spells
    /// it, which is what makes an existing `.properties` file work unchanged.
    const fn new(canonical: &'static str, legacy: &'static str) -> Self {
        Self { canonical, legacy }
    }

    /// The cdm-rs name, e.g. `feature.constant_columns.names`.
    pub const fn canonical(&self) -> &'static str {
        self.canonical
    }

    /// The Java name, e.g. `spark.cdm.feature.constantColumns.names`.
    pub const fn legacy(&self) -> &'static str {
        self.legacy
    }
}

/// `feature.constant_columns.names` (`FEA-010`).
pub const CONSTANT_COLUMN_NAMES: PropertyKey = PropertyKey::new(
    "feature.constant_columns.names",
    "spark.cdm.feature.constantColumns.names",
);
/// `feature.constant_columns.values` (`FEA-010`).
pub const CONSTANT_COLUMN_VALUES: PropertyKey = PropertyKey::new(
    "feature.constant_columns.values",
    "spark.cdm.feature.constantColumns.values",
);
/// `feature.constant_columns.split_regex` (`FEA-010`).
pub const CONSTANT_COLUMN_SPLIT_REGEX: PropertyKey = PropertyKey::new(
    "feature.constant_columns.split_regex",
    "spark.cdm.feature.constantColumns.splitRegex",
);
/// `feature.explode_map.origin_column` (`FEA-020`).
pub const EXPLODE_MAP_ORIGIN_COLUMN: PropertyKey = PropertyKey::new(
    "feature.explode_map.origin_column",
    "spark.cdm.feature.explodeMap.origin.name",
);
/// `feature.explode_map.target_key_column` (`FEA-020`).
pub const EXPLODE_MAP_TARGET_KEY_COLUMN: PropertyKey = PropertyKey::new(
    "feature.explode_map.target_key_column",
    "spark.cdm.feature.explodeMap.target.name.key",
);
/// `feature.explode_map.target_value_column` (`FEA-020`).
pub const EXPLODE_MAP_TARGET_VALUE_COLUMN: PropertyKey = PropertyKey::new(
    "feature.explode_map.target_value_column",
    "spark.cdm.feature.explodeMap.target.name.value",
);
/// `feature.extract_json.origin_column` (`FEA-030`).
pub const EXTRACT_JSON_ORIGIN_COLUMN: PropertyKey = PropertyKey::new(
    "feature.extract_json.origin_column",
    "spark.cdm.feature.extractJson.originColumn",
);
/// `feature.extract_json.property_mapping` (`FEA-030`).
pub const EXTRACT_JSON_PROPERTY_MAPPING: PropertyKey = PropertyKey::new(
    "feature.extract_json.property_mapping",
    "spark.cdm.feature.extractJson.propertyMapping",
);
/// `feature.extract_json.overwrite` (`FEA-032`).
pub const EXTRACT_JSON_OVERWRITE: PropertyKey = PropertyKey::new(
    "feature.extract_json.overwrite",
    "spark.cdm.feature.extractJson.overwrite",
);
/// `feature.extract_json.exclusive` (`FEA-033`).
pub const EXTRACT_JSON_EXCLUSIVE: PropertyKey = PropertyKey::new(
    "feature.extract_json.exclusive",
    "spark.cdm.feature.extractJson.exclusive",
);
/// `schema.origin.ttl.automatic` (`FEA-042`).
pub const ORIGIN_TTL_AUTOMATIC: PropertyKey = PropertyKey::new(
    "schema.origin.ttl.automatic",
    "spark.cdm.schema.origin.column.ttl.automatic",
);
/// `schema.origin.ttl.names` (`FEA-042`).
pub const ORIGIN_TTL_NAMES: PropertyKey = PropertyKey::new(
    "schema.origin.ttl.names",
    "spark.cdm.schema.origin.column.ttl.names",
);
/// `schema.origin.writetime.automatic` (`FEA-042`).
pub const ORIGIN_WRITETIME_AUTOMATIC: PropertyKey = PropertyKey::new(
    "schema.origin.writetime.automatic",
    "spark.cdm.schema.origin.column.writetime.automatic",
);
/// `schema.origin.writetime.names` (`FEA-042`).
pub const ORIGIN_WRITETIME_NAMES: PropertyKey = PropertyKey::new(
    "schema.origin.writetime.names",
    "spark.cdm.schema.origin.column.writetime.names",
);
/// `schema.ttl_writetime.use_collections` (`FEA-041`).
pub const TTL_WRITETIME_USE_COLLECTIONS: PropertyKey = PropertyKey::new(
    "schema.ttl_writetime.use_collections",
    "spark.cdm.schema.ttlwritetime.calc.useCollections",
);
/// `transform.custom_writetime` (`FEA-044`).
pub const TRANSFORM_CUSTOM_WRITETIME: PropertyKey = PropertyKey::new(
    "transform.custom_writetime",
    "spark.cdm.transform.custom.writetime",
);
/// `transform.custom_writetime_increment` (`FEA-040`).
pub const TRANSFORM_CUSTOM_WRITETIME_INCREMENT: PropertyKey = PropertyKey::new(
    "transform.custom_writetime_increment",
    "spark.cdm.transform.custom.writetime.incrementBy",
);
/// `transform.custom_ttl` (`FEA-044`).
pub const TRANSFORM_CUSTOM_TTL: PropertyKey =
    PropertyKey::new("transform.custom_ttl", "spark.cdm.transform.custom.ttl");
/// `filter.cql_where` (`FEA-050`).
pub const FILTER_CQL_WHERE: PropertyKey = PropertyKey::new(
    "filter.cql_where",
    "spark.cdm.filter.cassandra.whereCondition",
);
/// `filter.writetime.min` (`FEA-051`).
pub const FILTER_WRITETIME_MIN: PropertyKey = PropertyKey::new(
    "filter.writetime.min",
    "spark.cdm.filter.java.writetime.min",
);
/// `filter.writetime.max` (`FEA-051`).
pub const FILTER_WRITETIME_MAX: PropertyKey = PropertyKey::new(
    "filter.writetime.max",
    "spark.cdm.filter.java.writetime.max",
);
/// `filter.column.name` (`FEA-052`).
pub const FILTER_COLUMN_NAME: PropertyKey =
    PropertyKey::new("filter.column.name", "spark.cdm.filter.java.column.name");
/// `filter.column.value` (`FEA-052`).
pub const FILTER_COLUMN_VALUE: PropertyKey =
    PropertyKey::new("filter.column.value", "spark.cdm.filter.java.column.value");
/// `filter.token.min` (`FEA-053`).
pub const FILTER_TOKEN_MIN: PropertyKey = PropertyKey::new(
    "filter.token.min",
    "spark.cdm.filter.cassandra.partition.min",
);
/// `filter.token.max` (`FEA-053`).
pub const FILTER_TOKEN_MAX: PropertyKey = PropertyKey::new(
    "filter.token.max",
    "spark.cdm.filter.cassandra.partition.max",
);

/// Every property this crate reads, in specification order.
///
/// Exhaustive by construction: a key that is declared above but missing here is caught by
/// `fea_010_the_property_registry_matches_the_specification`, which is the only place the §3.5
/// spellings are asserted.
pub const fn registry() -> &'static [PropertyKey] {
    &[
        CONSTANT_COLUMN_NAMES,
        CONSTANT_COLUMN_VALUES,
        CONSTANT_COLUMN_SPLIT_REGEX,
        EXPLODE_MAP_ORIGIN_COLUMN,
        EXPLODE_MAP_TARGET_KEY_COLUMN,
        EXPLODE_MAP_TARGET_VALUE_COLUMN,
        EXTRACT_JSON_ORIGIN_COLUMN,
        EXTRACT_JSON_PROPERTY_MAPPING,
        EXTRACT_JSON_OVERWRITE,
        EXTRACT_JSON_EXCLUSIVE,
        ORIGIN_TTL_AUTOMATIC,
        ORIGIN_TTL_NAMES,
        ORIGIN_WRITETIME_AUTOMATIC,
        ORIGIN_WRITETIME_NAMES,
        TTL_WRITETIME_USE_COLLECTIONS,
        TRANSFORM_CUSTOM_WRITETIME,
        TRANSFORM_CUSTOM_WRITETIME_INCREMENT,
        TRANSFORM_CUSTOM_TTL,
        FILTER_CQL_WHERE,
        FILTER_WRITETIME_MIN,
        FILTER_WRITETIME_MAX,
        FILTER_COLUMN_NAME,
        FILTER_COLUMN_VALUE,
        FILTER_TOKEN_MIN,
        FILTER_TOKEN_MAX,
    ]
}

/// The value of a property under either spelling, exactly as configured.
///
/// An empty value reads as absent. Java's `PropertyHelper` treats `""` and "not set" alike for every
/// property this crate reads, and `CFG-027` makes an explicitly empty list an error at load time, so
/// collapsing them here cannot mask a configuration the loader would have accepted.
pub(crate) fn raw(config: &EffectiveConfig, key: PropertyKey) -> Option<&str> {
    config
        .get(key.canonical())
        .or_else(|| config.get(key.legacy()))
        .filter(|value| !value.is_empty())
}

/// A property with surrounding whitespace removed, which is what column names and CQL fragments
/// want.
pub(crate) fn trimmed(config: &EffectiveConfig, key: PropertyKey) -> Option<String> {
    raw(config, key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// A comma-separated list property, trimmed element-wise with empty elements dropped.
pub(crate) fn list(config: &EffectiveConfig, key: PropertyKey) -> Vec<String> {
    raw(config, key).map_or_else(Vec::new, |value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|element| !element.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    })
}

/// A boolean property, defaulting when unset.
///
/// Only `true` (in any case) is true, matching Java's `Boolean.parseBoolean`. A typo therefore reads
/// as `false` rather than as an error — the loader's Tier-1 pass (`CFG-020`) is where a malformed
/// boolean is rejected, and duplicating that judgement here would make a feature disagree with the
/// diagnostic the operator was already shown.
pub(crate) fn boolean(config: &EffectiveConfig, key: PropertyKey, default: bool) -> bool {
    raw(config, key).map_or(default, |value| value.trim().eq_ignore_ascii_case("true"))
}

/// An integer property.
///
/// # Errors
///
/// Returns [`ErrorKind::Config`] naming the canonical key, because a writetime or TTL that does not
/// parse must stop the run before any data moves rather than silently become zero.
pub(crate) fn integer(config: &EffectiveConfig, key: PropertyKey) -> Result<Option<i64>, CdmError> {
    let Some(value) = trimmed(config, key) else {
        return Ok(None);
    };
    value.parse::<i64>().map(Some).map_err(|e| {
        CdmError::new(
            ErrorKind::Config,
            format!("`{}` must be an integer, got `{value}`", key.canonical()),
        )
        .with_context(|c| c.with_config_key(key.canonical()))
        .with_source(e)
    })
}

/// A 128-bit integer property, which is what a Random-partitioner token bound needs (`TOK-002`).
///
/// # Errors
///
/// As [`integer`].
pub(crate) fn integer_128(
    config: &EffectiveConfig,
    key: PropertyKey,
) -> Result<Option<i128>, CdmError> {
    let Some(value) = trimmed(config, key) else {
        return Ok(None);
    };
    value.parse::<i128>().map(Some).map_err(|e| {
        CdmError::new(
            ErrorKind::Config,
            format!("`{}` must be an integer, got `{value}`", key.canonical()),
        )
        .with_context(|c| c.with_config_key(key.canonical()))
        .with_source(e)
    })
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

    fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
        pairs.iter().copied().collect()
    }

    #[test]
    fn fea_010_the_property_registry_matches_the_specification() {
        // Spot-checks every §3.5 pairing this crate owns. A drift in either spelling breaks an
        // operator's existing `.properties` file, so the table is asserted rather than trusted.
        for key in registry() {
            assert!(
                key.legacy().starts_with("spark.cdm."),
                "{} has a non-Java legacy alias",
                key.canonical()
            );
            assert!(!key.canonical().starts_with("spark."));
        }
        assert_eq!(registry().len(), 25);
        assert_eq!(
            CONSTANT_COLUMN_NAMES.legacy(),
            "spark.cdm.feature.constantColumns.names"
        );
        assert_eq!(
            TTL_WRITETIME_USE_COLLECTIONS.legacy(),
            "spark.cdm.schema.ttlwritetime.calc.useCollections"
        );
        assert_eq!(
            FILTER_TOKEN_MIN.legacy(),
            "spark.cdm.filter.cassandra.partition.min"
        );
    }

    #[test]
    fn fea_010_the_canonical_spelling_wins_over_the_legacy_alias() {
        let config = config(&[
            ("feature.constant_columns.values", "canonical"),
            ("spark.cdm.feature.constantColumns.values", "legacy"),
        ]);
        assert_eq!(raw(&config, CONSTANT_COLUMN_VALUES), Some("canonical"));

        let legacy_only = config_legacy_only();
        assert_eq!(raw(&legacy_only, CONSTANT_COLUMN_VALUES), Some("legacy"));
    }

    fn config_legacy_only() -> EffectiveConfig {
        config(&[("spark.cdm.feature.constantColumns.values", "legacy")])
    }

    #[test]
    fn fea_010_an_empty_value_reads_as_absent() {
        let config = config(&[("feature.constant_columns.values", "")]);
        assert_eq!(raw(&config, CONSTANT_COLUMN_VALUES), None);
        assert_eq!(trimmed(&config, CONSTANT_COLUMN_VALUES), None);
        assert!(list(&config, CONSTANT_COLUMN_NAMES).is_empty());
    }

    #[test]
    fn fea_042_typed_accessors_parse_lists_booleans_and_integers() {
        let config = config(&[
            ("schema.origin.ttl.names", " a , ,b "),
            ("schema.origin.ttl.automatic", "FALSE"),
            ("transform.custom_ttl", " 42 "),
            ("filter.token.min", "-9223372036854775809"),
        ]);
        assert_eq!(list(&config, ORIGIN_TTL_NAMES), vec!["a", "b"]);
        assert!(!boolean(&config, ORIGIN_TTL_AUTOMATIC, true));
        assert!(boolean(&config, ORIGIN_WRITETIME_AUTOMATIC, true));
        assert_eq!(integer(&config, TRANSFORM_CUSTOM_TTL).unwrap(), Some(42));
        assert_eq!(
            integer_128(&config, FILTER_TOKEN_MIN).unwrap(),
            Some(-9_223_372_036_854_775_809_i128)
        );
        assert_eq!(integer(&config, TRANSFORM_CUSTOM_WRITETIME).unwrap(), None);
    }

    #[test]
    fn fea_044_an_unparsable_integer_is_a_config_error_naming_the_canonical_key() {
        let config = config(&[("transform.custom_writetime", "soon")]);
        let error = integer(&config, TRANSFORM_CUSTOM_WRITETIME).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert_eq!(
            error.context().config_key.as_deref(),
            Some("transform.custom_writetime")
        );
        assert!(integer_128(&config, TRANSFORM_CUSTOM_WRITETIME).is_err());
    }
}
