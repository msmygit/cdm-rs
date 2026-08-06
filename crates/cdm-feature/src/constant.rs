//! Constant columns — target columns written with a fixed literal (`FEA-010`..`FEA-014`).
//!
//! # What the feature is for
//!
//! A migration frequently needs a column the origin does not have: a tenant discriminator, a
//! `source = 'legacy'` marker, a partition-key component introduced by the new model. Constant
//! columns supply those without a transformation step, and — because the value is CQL *source* and
//! not a bind — the same constant can appear both in an `INSERT` column list and in the `WHERE`
//! clause of the validate job's target lookup (`FEA-012`).
//!
//! # Java parity
//!
//! `com.datastax.cdm.feature.ConstantColumns` reads three properties, splits the values with a
//! configurable regex, and type-checks each value with the driver's literal parser at startup. All
//! three behaviours are reproduced, including the two failure modes that surprise operators: values
//! configured without a split regex are an error rather than a single value (Java throws), and a
//! names/values length mismatch disables the feature rather than truncating it.
//!
//! `spark.cdm.feature.constantColumns.types` is deliberately absent: it appears in Java's
//! `cdm-detailed.properties` but in neither `KnownProperties` nor Java's source, so it has never had
//! an effect. Types come from the target schema (SPEC §3.5.10).

use cdm_codec::CqlTypeInfo;
use cdm_core::{
    BindingBuilder, CdmError, CompareHook, Diagnostic, EffectiveConfig, ErrorKind, FeaturePlugin,
    Plugin, RawCell, SchemaPair,
};
use regex::Regex;

use crate::literal::parse_literal;
use crate::properties::{
    self, CONSTANT_COLUMN_NAMES, CONSTANT_COLUMN_SPLIT_REGEX, CONSTANT_COLUMN_VALUES,
};
use crate::schema::{FeatureSchema, TableFacts};
use crate::{diagnostic, PROVIDER};

/// The default value separator, matching Java's `spark.cdm.feature.constantColumns.splitRegex`.
const DEFAULT_SPLIT_REGEX: &str = ",";

/// Where a target column's value comes from once constant columns are applied (`FEA-014`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnSource {
    /// A configured constant. Wins over an origin column of the same name, which is what makes an
    /// origin constant *replaceable* by a different target constant.
    Constant,
    /// The origin column of the same name.
    Origin,
    /// Neither — the column is left unset, so no tombstone is written (`MIG-012`).
    Absent,
}

/// One constant column, resolved against the target schema (`FEA-011`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConstant {
    name: String,
    literal: String,
    value: RawCell,
    cql_type: CqlTypeInfo,
    key: bool,
}

impl ResolvedConstant {
    /// The target column name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The literal as the operator wrote it, which is what is spliced into generated CQL.
    pub fn literal(&self) -> &str {
        &self.literal
    }

    /// The serialised value, for the binding path.
    pub const fn value(&self) -> &RawCell {
        &self.value
    }

    /// The target column's type.
    pub const fn cql_type(&self) -> &CqlTypeInfo {
        &self.cql_type
    }

    /// Whether the column is part of the target primary key (`FEA-012`).
    pub const fn is_key(&self) -> bool {
        self.key
    }
}

/// The constant-columns feature (`FEA-010`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConstantColumns {
    names: Vec<String>,
    values: Vec<String>,
}

impl ConstantColumns {
    /// Reads the feature's configuration (`FEA-010`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] when values are configured with an empty split regex, or when
    /// that regex does not compile. Java throws in the first case and would throw in the second; a
    /// value string that cannot be split is not recoverable, because guessing a separator would
    /// silently write the wrong constant into every row.
    pub fn load(config: &EffectiveConfig) -> Result<Self, CdmError> {
        let names = properties::list(config, CONSTANT_COLUMN_NAMES);
        let Some(values) = properties::raw(config, CONSTANT_COLUMN_VALUES) else {
            return Ok(Self {
                names,
                values: Vec::new(),
            });
        };

        let pattern = properties::raw(config, CONSTANT_COLUMN_SPLIT_REGEX)
            .unwrap_or(DEFAULT_SPLIT_REGEX)
            .to_owned();
        if pattern.is_empty() {
            return Err(CdmError::new(
                ErrorKind::Config,
                format!(
                    "`{}` is set but `{}` is empty; cdm-rs will not guess how to split the values",
                    CONSTANT_COLUMN_VALUES.canonical(),
                    CONSTANT_COLUMN_SPLIT_REGEX.canonical()
                ),
            )
            .with_context(|c| c.with_config_key(CONSTANT_COLUMN_SPLIT_REGEX.canonical())));
        }
        let separator = Regex::new(&pattern).map_err(|e| {
            CdmError::new(
                ErrorKind::Config,
                format!("`{pattern}` is not a valid regular expression"),
            )
            .with_context(|c| c.with_config_key(CONSTANT_COLUMN_SPLIT_REGEX.canonical()))
            .with_source(e)
        })?;

        Ok(Self {
            names,
            values: separator
                .split(values)
                .map(|value| value.trim().to_owned())
                .collect(),
        })
    }

    /// Whether any constant column is configured.
    pub fn is_enabled(&self) -> bool {
        !self.names.is_empty()
    }

    /// The configured column names, in declaration order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The literal configured for a column, if it is a constant column.
    pub fn literal(&self, column: &str) -> Option<&str> {
        let index = self.names.iter().position(|name| name == column)?;
        self.values.get(index).map(String::as_str)
    }

    /// Whether the named target column is supplied as a constant.
    pub fn is_constant(&self, column: &str) -> bool {
        self.names.iter().any(|name| name == column)
    }

    /// Validates the configuration against the target schema (`FEA-010`, `FEA-011`).
    ///
    /// Only the target side is consulted: a constant column exists on the target by definition, and
    /// what the origin happens to call the same name is `FEA-014`'s business, not validation's.
    /// Every finding is returned, not just the first, so that an operator with three malformed
    /// constants fixes all three in one pass (`CFG-021`).
    pub fn validate(&self, target: &TableFacts) -> Vec<Diagnostic> {
        let mut findings = self.validate_cardinality();
        if !findings.is_empty() || !self.is_enabled() {
            return findings;
        }
        for (name, literal) in self.names.iter().zip(&self.values) {
            let Some(column) = target.column(name) else {
                findings.push(
                    diagnostic::schema_error(format!(
                        "constant column `{name}` is not on the target table {}",
                        target.table()
                    ))
                    .with_rule("FEA-010")
                    .with_suggestion(
                        "add the column to the target, or drop it from the names list",
                    ),
                );
                continue;
            };
            if literal.is_empty() {
                findings.push(
                    diagnostic::config_error(format!("constant column `{name}` has no value"))
                        .with_rule("FEA-011"),
                );
                continue;
            }
            if let Err(error) = parse_literal(literal, column.cql_type()) {
                findings.push(
                    diagnostic::config_error(format!(
                        "constant column `{name}` cannot be parsed as {}",
                        column.cql_type()
                    ))
                    .with_detail(error.message().to_owned())
                    .with_value(literal.clone())
                    .with_rule("FEA-011"),
                );
            }
        }
        findings
    }

    /// The names/values cardinality rule of `FEA-010` and `CFG-030`.
    fn validate_cardinality(&self) -> Vec<Diagnostic> {
        if self.names.is_empty() && self.values.is_empty() {
            return Vec::new();
        }
        if self.names.len() == self.values.len() && !self.names.is_empty() {
            return Vec::new();
        }
        vec![diagnostic::config_error(format!(
            "constant column names ({}) and values ({}) are of different sizes",
            self.names.len(),
            self.values.len()
        ))
        .with_rule("FEA-010")
        .with_suggestion(
            "the values string is split by `feature.constant_columns.split_regex`; check that the \
             separator does not appear inside a value",
        )]
    }

    /// Resolves every constant against the target schema (`FEA-011`, `FEA-012`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if a constant does not resolve. Callers are expected to have
    /// run [`ConstantColumns::validate`] first and reported its diagnostics; this is the fail-safe
    /// that keeps an unvalidated caller from writing an unparsable constant.
    pub fn resolve(&self, target: &TableFacts) -> Result<Vec<ResolvedConstant>, CdmError> {
        let mut resolved = Vec::with_capacity(self.names.len());
        for (name, literal) in self.names.iter().zip(&self.values) {
            let column = target.column(name).ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "constant column `{name}` is not on the target table {}",
                        target.table()
                    ),
                )
                .with_context(|c| c.with_column(name.clone()))
            })?;
            let value = parse_literal(literal, column.cql_type()).map_err(|e| {
                CdmError::new(
                    ErrorKind::Config,
                    format!("constant column `{name}`: {}", e.message()),
                )
                .with_context(|c| c.with_column(name.clone()))
            })?;
            resolved.push(ResolvedConstant {
                name: name.clone(),
                literal: literal.clone(),
                value,
                cql_type: column.cql_type().clone(),
                key: column.is_key(),
            });
        }
        Ok(resolved)
    }

    /// The `col=<literal>` terms a constant primary-key column contributes to a target `WHERE`
    /// clause (`FEA-012`).
    ///
    /// The literal is spliced, not bound, exactly as Java splices `targetDefaultValueStrings`: a
    /// constant is by definition the same for every row, so binding it would add a parameter to
    /// every statement for no benefit — and the prepared statement's cache key stays stable.
    ///
    /// # Errors
    ///
    /// As [`ConstantColumns::resolve`].
    pub fn where_clause_terms(&self, target: &TableFacts) -> Result<Vec<String>, CdmError> {
        Ok(self
            .resolve(target)?
            .into_iter()
            .filter(ResolvedConstant::is_key)
            .map(|constant| format!("{}={}", constant.name(), constant.literal()))
            .collect())
    }

    /// Where a target column's value comes from (`FEA-014`).
    ///
    /// A constant wins over an origin column of the same name, which is what "origin constants MUST
    /// be replaceable by different target constants" means in practice: the origin column is read
    /// and ignored, and the target receives the configured literal.
    pub fn target_column_source(&self, column: &str, origin: &TableFacts) -> ColumnSource {
        if self.is_constant(column) {
            ColumnSource::Constant
        } else if origin.column(column).is_some() {
            ColumnSource::Origin
        } else {
            ColumnSource::Absent
        }
    }
}

/// Origin columns with no target counterpart, which are therefore dropped (`FEA-014`).
///
/// Dropping is what makes a constant column *removable* on the target side: the origin keeps the
/// column it was migrated with, the target does not declare one, and the run proceeds rather than
/// failing on a column it cannot place. It is a free function because the answer depends only on the
/// two schemas — a constant column can only ever name a column the target *does* have.
pub fn dropped_origin_columns(schema: &FeatureSchema) -> Vec<&str> {
    schema
        .origin
        .columns()
        .iter()
        .map(crate::schema::ColumnFacts::name)
        .filter(|name| schema.target.column(name).is_none())
        .collect()
}

impl CompareHook for ConstantColumns {
    /// Constant columns are excluded from validate comparison (`FEA-013`).
    ///
    /// There is nothing on the origin to compare them against: a mismatch would be reported for
    /// every row of every run, which is exactly the false positive that trains operators to ignore
    /// the validate report.
    fn skips_column(&self, column: &str) -> bool {
        self.is_constant(column)
    }
}

impl Plugin for ConstantColumns {
    fn name(&self) -> &'static str {
        "constant-columns"
    }

    fn provider(&self) -> &'static str {
        PROVIDER
    }
}

impl FeaturePlugin for ConstantColumns {
    fn is_enabled(&self, _config: &EffectiveConfig) -> bool {
        Self::is_enabled(self)
    }

    /// The schema-bound checks reachable from a [`SchemaPair`].
    ///
    /// A `SchemaPair` says which columns exist and how the schema spells their types, but not which
    /// of them form the primary key, so this entry point runs every check except the ones that need
    /// that fact. The engine calls [`ConstantColumns::validate`] with a
    /// [`FeatureSchema`](crate::FeatureSchema) instead, which carries it.
    fn validate(&self, _config: &EffectiveConfig, schema: &SchemaPair) -> Vec<Diagnostic> {
        match TableFacts::from_view(&schema.target, &[]) {
            Ok(target) => Self::validate(self, &target),
            Err(error) => vec![error.to_diagnostic()],
        }
    }

    /// Contributes each constant as a literal binding (`FEA-010`).
    fn extend_target_binding(&self, binding: &mut BindingBuilder) {
        for (name, literal) in self.names.iter().zip(&self.values) {
            binding.add_literal(name.clone(), literal.clone());
        }
    }

    fn compare_hook(&self) -> Option<&dyn CompareHook> {
        Some(self)
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
    use crate::schema::table_view;
    use cdm_core::TableRef;

    fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
        pairs.iter().copied().collect()
    }

    fn schema() -> FeatureSchema {
        let origin = TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "src"),
                &[("id", "int"), ("v", "text"), ("legacy_only", "text")],
            ),
            &["id"],
        )
        .unwrap();
        let target = TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "dst"),
                &[
                    ("id", "int"),
                    ("tenant", "text"),
                    ("v", "text"),
                    ("n", "int"),
                ],
            ),
            &["id", "tenant"],
        )
        .unwrap();
        FeatureSchema::new(origin, target)
    }

    #[test]
    fn fea_010_names_and_values_are_split_by_the_configured_regex() {
        let feature = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "tenant,n"),
            ("feature.constant_columns.values", "'acme'%1234"),
            ("feature.constant_columns.split_regex", "%"),
        ]))
        .unwrap();
        assert!(feature.is_enabled());
        assert_eq!(feature.names(), ["tenant", "n"]);
        assert_eq!(feature.literal("tenant"), Some("'acme'"));
        assert_eq!(feature.literal("n"), Some("1234"));
        assert_eq!(feature.literal("absent"), None);
        assert!(feature.validate(&schema().target).is_empty());
    }

    #[test]
    fn fea_010_the_split_regex_defaults_to_a_comma_and_the_feature_defaults_to_off() {
        let feature = ConstantColumns::load(&config(&[
            ("spark.cdm.feature.constantColumns.names", "tenant,n"),
            ("spark.cdm.feature.constantColumns.values", "'acme',1234"),
        ]))
        .unwrap();
        assert_eq!(feature.literal("n"), Some("1234"));

        let off = ConstantColumns::load(&EffectiveConfig::new()).unwrap();
        assert!(!off.is_enabled());
        assert!(off.validate(&schema().target).is_empty());
        assert!(off.resolve(&schema().target).unwrap().is_empty());
    }

    #[test]
    fn fea_010_mismatched_cardinality_is_reported_rather_than_truncated() {
        let feature = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "tenant,n"),
            ("feature.constant_columns.values", "'acme'"),
        ]))
        .unwrap();
        let findings = feature.validate(&schema().target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule.as_deref(), Some("FEA-010"));
        assert!(findings[0].is_blocking());
    }

    #[test]
    fn fea_010_values_without_a_split_regex_are_a_config_error() {
        let error = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "tenant"),
            ("feature.constant_columns.values", "'acme'"),
            ("feature.constant_columns.split_regex", " "),
        ]));
        // A whitespace regex is legal; an *empty* one is not, and neither is a malformed one.
        assert!(error.is_ok());
        let bad = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "tenant"),
            ("feature.constant_columns.values", "'acme'"),
            ("feature.constant_columns.split_regex", "(["),
        ]))
        .unwrap_err();
        assert_eq!(bad.kind(), ErrorKind::Config);
    }

    #[test]
    fn fea_011_values_are_type_checked_against_the_target_column() {
        let feature = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "tenant,n"),
            ("feature.constant_columns.values", "'acme',not-a-number"),
        ]))
        .unwrap();
        let findings = feature.validate(&schema().target);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule.as_deref(), Some("FEA-011"));
        assert!(findings[0].title.contains("`n` cannot be parsed as int"));

        let resolved = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "n"),
            ("feature.constant_columns.values", "1234"),
        ]))
        .unwrap()
        .resolve(&schema().target)
        .unwrap();
        assert_eq!(
            resolved[0].value(),
            &RawCell::new(1234_i32.to_be_bytes().to_vec())
        );
        assert_eq!(resolved[0].cql_type(), &CqlTypeInfo::Int);
        assert_eq!(resolved[0].name(), "n");
        assert!(!resolved[0].is_key());
    }

    #[test]
    fn fea_011_an_unknown_target_column_is_reported_and_refuses_to_resolve() {
        let feature = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "nope"),
            ("feature.constant_columns.values", "'x'"),
        ]))
        .unwrap();
        let findings = feature.validate(&schema().target);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].title.contains("not on the target table"));
        assert_eq!(
            feature.resolve(&schema().target).unwrap_err().kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn fea_012_a_constant_primary_key_column_appears_as_a_literal_in_the_where_clause() {
        let feature = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "tenant,n"),
            ("feature.constant_columns.values", "'acme',1234"),
        ]))
        .unwrap();
        let target = schema().target;
        assert_eq!(
            feature.where_clause_terms(&target).unwrap(),
            ["tenant='acme'"]
        );
        let resolved = feature.resolve(&target).unwrap();
        assert!(resolved[0].is_key(), "tenant is part of the target PK");
        assert!(!resolved[1].is_key());
    }

    #[test]
    fn fea_013_constant_columns_are_excluded_from_validate_comparison() {
        let feature = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "tenant"),
            ("feature.constant_columns.values", "'acme'"),
        ]))
        .unwrap();
        assert!(feature.skips_column("tenant"));
        assert!(!feature.skips_column("v"));
        assert!(FeaturePlugin::compare_hook(&feature)
            .unwrap()
            .skips_column("tenant"));
    }

    #[test]
    fn fea_014_a_constant_replaces_an_origin_column_and_a_missing_target_column_is_dropped() {
        let schema = schema();
        let feature = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "v"),
            ("feature.constant_columns.values", "'replaced'"),
        ]))
        .unwrap();
        assert_eq!(
            feature.target_column_source("v", &schema.origin),
            ColumnSource::Constant,
            "an origin constant is replaceable by a different target constant"
        );
        assert_eq!(
            feature.target_column_source("id", &schema.origin),
            ColumnSource::Origin
        );
        assert_eq!(
            feature.target_column_source("n", &schema.origin),
            ColumnSource::Absent
        );
        assert_eq!(dropped_origin_columns(&schema), ["legacy_only"]);
    }

    #[test]
    fn fea_010_the_feature_registers_as_a_plugin_and_contributes_its_literals() {
        let feature = ConstantColumns::load(&config(&[
            ("feature.constant_columns.names", "tenant"),
            ("feature.constant_columns.values", "'acme'"),
        ]))
        .unwrap();
        assert_eq!(Plugin::name(&feature), "constant-columns");
        assert_eq!(Plugin::provider(&feature), PROVIDER);
        assert!(FeaturePlugin::is_enabled(&feature, &EffectiveConfig::new()));

        let mut binding = BindingBuilder::new();
        feature.extend_target_binding(&mut binding);
        assert_eq!(
            binding.bindings(),
            [("tenant".to_owned(), "'acme'".to_owned())]
        );

        let pair = SchemaPair::new(
            table_view(TableRef::new("ks", "src"), &[("id", "int")]),
            table_view(TableRef::new("ks", "dst"), &[("tenant", "text")]),
        );
        assert!(FeaturePlugin::validate(&feature, &EffectiveConfig::new(), &pair).is_empty());
    }
}
