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
    legacy: Option<&'static str>,
}

impl PropertyKey {
    /// Declares a key. `legacy` is the Java `spark.cdm.*` name exactly as `KnownProperties` spells
    /// it, which is what makes an existing `.properties` file work unchanged.
    const fn new(canonical: &'static str, legacy: &'static str) -> Self {
        Self {
            canonical,
            legacy: Some(legacy),
        }
    }

    /// Declares a key that is new in cdm-rs (`CFG-200`) and therefore has no Java spelling.
    ///
    /// There is nothing to fall back to for one of these: an existing `.properties` file cannot
    /// contain it, because Java has no such setting to have written.
    const fn new_in_cdm_rs(canonical: &'static str) -> Self {
        Self {
            canonical,
            legacy: None,
        }
    }

    /// The cdm-rs name, e.g. `feature.constant_columns.names`.
    pub const fn canonical(&self) -> &'static str {
        self.canonical
    }

    /// The Java name, e.g. `spark.cdm.feature.constantColumns.names`, or `None` for a property
    /// that is new in cdm-rs.
    pub const fn legacy(&self) -> Option<&'static str> {
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
/// `feature.guardrail.column_size_kb` (`GRD-002`).
pub const GUARDRAIL_COLUMN_SIZE_KB: PropertyKey = PropertyKey::new(
    "feature.guardrail.column_size_kb",
    "spark.cdm.feature.guardrail.colSizeInKB",
);
/// `feature.guardrail.mode` (`GRD-004`). New in cdm-rs: Java's guardrail is a job of its own and
/// never runs alongside a migration, so it has no inline mode to spell.
pub const GUARDRAIL_MODE: PropertyKey = PropertyKey::new_in_cdm_rs("feature.guardrail.mode");

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
        GUARDRAIL_COLUMN_SIZE_KB,
        GUARDRAIL_MODE,
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
        .or_else(|| key.legacy().and_then(|legacy| config.get(legacy)))
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

/// A property naming a **column**, read back from its CQL spelling to the internal name
/// (`SCH-002`).
///
/// An operator writes a column name the way they would write it in cqlsh, so a hyphenated or
/// case-sensitive name arrives quoted: `feature.explode_map.origin_column="fruit-map"`. Every
/// schema lookup in this crate — [`crate::schema::TableFacts::column`] and its callers — matches
/// on the *internal* name that `system_schema` stores, which for that column is `fruit-map`
/// without the quotes. Comparing the two spellings finds nothing, and the feature reports a column
/// that is plainly there as missing.
///
/// Normalising here, at the configuration boundary, is what keeps the crate on one spelling:
/// validation messages, the resolved plan and the target column names it hands back to the caller
/// then all agree with the schema, and with the column mapping `cdm-cql` builds — via
/// `identifier::unformat` — from these same properties.
///
/// The rule is `SCH-002`'s, whose canonical implementation is `cdm_cql::schema::identifier`. See
/// [`unformat`] for why it is restated here rather than called.
pub(crate) fn column_name(config: &EffectiveConfig, key: PropertyKey) -> Option<String> {
    trimmed(config, key)
        .map(|name| unformat(&name))
        .filter(|name| !name.is_empty())
}

/// Reads a CQL identifier back to its internal form: strips the surrounding quotes and undoubles
/// an embedded `""` (`SCH-002`).
///
/// # Why this is not `cdm_cql::schema::identifier::unformat`
///
/// It is the same rule, deliberately kept behaviourally identical, because `cdm-feature` does not
/// depend on `cdm-cql` and should not: every feature here is expressed against the
/// driver-independent `CqlTypeInfo`/`RawCell`/`Record` types precisely so that it stays
/// unit-testable without a cluster (SPEC §11, `docs/ARCHITECTURE.md` §3). Taking that dependency
/// edge for fifteen lines of string handling would trade a structural guarantee for very little.
/// `SCH-002` is a property of CQL rather than of the driver, so the longer-term home for it is
/// `cdm-core`, which both crates already depend on; until it moves there, the parity test in this
/// module pins the two implementations to the same answers.
///
/// An unquoted name is returned unchanged rather than folded, exactly as Java's `unFormatName`
/// does: `system_schema` stores internal names, so folding would make a column created as
/// `"MyColumn"` unfindable.
fn unformat(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    // Java's `^"[^\s]*"$`: a well-formed quoted identifier, which contains no whitespace.
    let well_formed_quoted = name.len() >= 2
        && name.starts_with('"')
        && name.ends_with('"')
        && !name.chars().any(char::is_whitespace);
    if well_formed_quoted {
        return name
            .get(1..name.len() - 1)
            .unwrap_or_default()
            .replace("\"\"", "\"");
    }
    if name.contains('"') || name.chars().any(char::is_whitespace) {
        // Not a well-formed quoted name and not a bare one — a value such as `"a b"`, which is a
        // perfectly legal column name and exactly the kind that has to be quoted to be written at
        // all. Strip the outer quotes when they are there, and otherwise leave the name alone
        // rather than mangling it.
        let trimmed = name.trim();
        if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
            return trimmed
                .get(1..trimmed.len() - 1)
                .unwrap_or_default()
                .replace("\"\"", "\"");
        }
        return name.to_owned();
    }
    name.to_owned()
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

/// A floating-point property, which is what the guardrail threshold needs (`GRD-002`).
///
/// A fraction is accepted where Java's `Long.parseLong` would reject it, which is the whole of
/// `docs/MIGRATION_FROM_JAVA.md` item 12. Non-finite values are refused: `inf` would disable a
/// guardrail the operator switched on, and `NaN` would make every comparison false, both of which
/// look exactly like a working configuration from the outside.
///
/// # Errors
///
/// Returns [`ErrorKind::Config`] naming the canonical key.
pub(crate) fn float(config: &EffectiveConfig, key: PropertyKey) -> Result<Option<f64>, CdmError> {
    let Some(value) = trimmed(config, key) else {
        return Ok(None);
    };
    let parsed = value.parse::<f64>().map_err(|e| {
        CdmError::new(
            ErrorKind::Config,
            format!("`{}` must be a number, got `{value}`", key.canonical()),
        )
        .with_context(|c| c.with_config_key(key.canonical()))
        .with_source(e)
    })?;
    if !parsed.is_finite() {
        return Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "`{}` must be a finite number, got `{value}`",
                key.canonical()
            ),
        )
        .with_context(|c| c.with_config_key(key.canonical())));
    }
    Ok(Some(parsed))
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
            if let Some(legacy) = key.legacy() {
                assert!(
                    legacy.starts_with("spark.cdm."),
                    "{} has a non-Java legacy alias",
                    key.canonical()
                );
            }
            assert!(!key.canonical().starts_with("spark."));
        }
        assert_eq!(registry().len(), 27);
        assert_eq!(
            CONSTANT_COLUMN_NAMES.legacy(),
            Some("spark.cdm.feature.constantColumns.names")
        );
        assert_eq!(
            TTL_WRITETIME_USE_COLLECTIONS.legacy(),
            Some("spark.cdm.schema.ttlwritetime.calc.useCollections")
        );
        assert_eq!(
            FILTER_TOKEN_MIN.legacy(),
            Some("spark.cdm.filter.cassandra.partition.min")
        );
        assert_eq!(
            GUARDRAIL_COLUMN_SIZE_KB.legacy(),
            Some("spark.cdm.feature.guardrail.colSizeInKB")
        );
        // GRD-004's mode is new in cdm-rs, so there is nothing for it to fall back to.
        assert_eq!(GUARDRAIL_MODE.legacy(), None);
        assert_eq!(
            registry()
                .iter()
                .filter(|key| key.legacy().is_none())
                .count(),
            1
        );
    }

    #[test]
    fn sch_002_a_column_name_property_is_read_back_to_its_internal_form() {
        let settings = config(&[
            ("feature.explode_map.origin_column", "  \"fruit-map\"  "),
            ("feature.explode_map.target_key_column", "fruit"),
            ("feature.explode_map.target_value_column", "\"\"\""),
        ]);
        assert_eq!(
            column_name(&settings, EXPLODE_MAP_ORIGIN_COLUMN).as_deref(),
            Some("fruit-map")
        );
        // A name that never needed quoting is untouched.
        assert_eq!(
            column_name(&settings, EXPLODE_MAP_TARGET_KEY_COLUMN).as_deref(),
            Some("fruit")
        );
        // `"""` is the CQL spelling of the one-character name `"`.
        assert_eq!(
            column_name(&settings, EXPLODE_MAP_TARGET_VALUE_COLUMN).as_deref(),
            Some("\"")
        );
        assert_eq!(column_name(&config(&[]), EXPLODE_MAP_ORIGIN_COLUMN), None);
        // A quoted empty name is no name at all, and must not read as one.
        let empty = config(&[("feature.explode_map.origin_column", "\"\"")]);
        assert_eq!(column_name(&empty, EXPLODE_MAP_ORIGIN_COLUMN), None);
    }

    #[test]
    fn sch_002_unformatting_answers_exactly_as_cdm_cqls_canonical_rule_does() {
        // These are the documented answers of `cdm_cql::schema::identifier::unformat`, which this
        // crate cannot call (no `cdm-cql` dependency) but must not disagree with: `cdm-cql` builds
        // the column mapping from the same properties, so any divergence puts a feature's plan and
        // the statement's bind list on two different spellings of one column.
        assert_eq!(unformat("\"Data\""), "Data");
        assert_eq!(unformat("Data"), "Data");
        assert_eq!(unformat("\"we\"\"ird\""), "we\"ird");
        assert_eq!(unformat("DATA"), "DATA");
        assert_eq!(unformat("Reserved_Words"), "Reserved_Words");
        assert_eq!(unformat("\"two words\""), "two words");
        assert_eq!(unformat("\"fruit-map\""), "fruit-map");
        assert_eq!(unformat(""), "");
        assert_eq!(unformat("\""), "\"");
        // Already-internal names pass through unchanged, so normalising twice is harmless: the
        // mapping in `cdm-cql` unformats these properties again on its own side.
        for internal in ["fruit-map", "Data", "two words", "we\"ird", "data"] {
            assert_eq!(unformat(internal), internal, "{internal}");
        }
    }

    #[test]
    fn grd_002_the_threshold_accessor_takes_fractions_and_refuses_nonsense() {
        let settings = config(&[
            ("feature.guardrail.column_size_kb", " 0.5 "),
            ("feature.guardrail.mode", "inf"),
        ]);
        assert_eq!(
            float(&settings, GUARDRAIL_COLUMN_SIZE_KB).unwrap(),
            Some(0.5_f64)
        );
        assert_eq!(float(&settings, FILTER_TOKEN_MIN).unwrap(), None);
        // `inf` parses as a float but is not a threshold; borrowing the mode key here keeps the
        // case to one config map.
        assert!(float(&settings, GUARDRAIL_MODE).is_err());
        assert!(float(
            &config(&[("feature.guardrail.mode", "big")]),
            GUARDRAIL_MODE
        )
        .is_err());
    }

    #[test]
    fn grd_004_a_new_in_cdm_rs_key_is_read_from_its_canonical_name_only() {
        let settings = config(&[("feature.guardrail.mode", "block")]);
        assert_eq!(raw(&settings, GUARDRAIL_MODE), Some("block"));
        assert_eq!(raw(&config(&[]), GUARDRAIL_MODE), None);
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
