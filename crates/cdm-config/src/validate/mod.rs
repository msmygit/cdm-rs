//! Three-tier configuration validation (`CFG-020`..`CFG-040`).
//!
//! | Tier | Name | Needs a cluster? | Checks |
//! |---|---|---|---|
//! | 1 | [`Tier::Syntactic`] | no | types, ranges, enum values, mutually-required groups |
//! | 2 | [`Tier::Semantic`] | no | the cross-field rules of `SPEC` §3.4 |
//! | 3 | [`Tier::SchemaBound`] | yes | keyspace, table, column and counter-table rules |
//!
//! Every tier reports **every** violation it finds; nothing fails fast, because an operator who
//! has to fix one property per run of `cdm validate-config` will give up before the tenth
//! (`CFG-021`). Each finding is a [`Diagnostic`] naming the offending key, the supplied value,
//! the rule and a suggested fix.
//!
//! # Why the cluster is behind a trait
//!
//! Tier 3 needs the live schema, but `cdm-config` must not depend on `cdm-cql`
//! (`ARCHITECTURE.md` §3.2). It is therefore expressed as the [`SchemaProvider`] trait, defined
//! here and implemented in `cdm-cql` (PR #9) over the schema snapshot it takes at start-up. The
//! trait is deliberately **synchronous**: schema is fetched once before any range is planned
//! (`ARCHITECTURE.md` §5.5), so a caller has the answers in hand before it validates, and
//! `cdm-config` stays free of an async runtime.
//!
//! # Not yet covered
//!
//! `CFG-020`'s Tier-3 list ends with "codec availability for every mapped column pair". The
//! codec registry lives in `cdm-codec`, which does not exist until PR #11 and which `cdm-config`
//! may not depend on; that check belongs to the conversion planner (`CDC-010`) and is enforced
//! there.

mod tier1;
mod tier2;
mod tier3;

use cdm_core::{CdmError, ColumnRef, Diagnostic, Severity, TableRef};

use crate::loader::CODE;
use crate::model::CdmConfig;

/// One of the three escalating validation tiers (`CFG-020`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Types, ranges, enumerations and mutually-required groups. Needs nothing but the config.
    Syntactic,
    /// The cross-field rules of `SPEC` §3.4. Needs nothing but the config.
    Semantic,
    /// Keyspace, table, column and counter-table rules. Needs the live schema.
    SchemaBound,
}

impl Tier {
    /// The tier number as `SPEC` §3.3 writes it.
    pub const fn number(self) -> u8 {
        match self {
            Self::Syntactic => 1,
            Self::Semantic => 2,
            Self::SchemaBound => 3,
        }
    }

    /// The tier's name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Syntactic => "syntactic",
            Self::Semantic => "semantic",
            Self::SchemaBound => "schema-bound",
        }
    }
}

/// Knobs that change what validation considers acceptable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ValidationOptions {
    /// Restore Java behaviours that cdm-rs otherwise improves on (`CFG-161`).
    pub compat_java: bool,
}

/// What validation found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidationReport {
    /// Every finding, in the order the tiers produced them.
    pub diagnostics: Vec<Diagnostic>,
    /// The highest tier that actually ran.
    pub tiers_run: Vec<Tier>,
}

impl ValidationReport {
    /// Whether the run may proceed.
    pub fn is_valid(&self) -> bool {
        !self.diagnostics.iter().any(Diagnostic::is_blocking)
    }

    /// The blocking findings.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.of_severity(Severity::Error)
    }

    /// The non-blocking findings the operator should read.
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.of_severity(Severity::Warning)
    }

    /// The informational findings.
    pub fn notices(&self) -> impl Iterator<Item = &Diagnostic> {
        self.of_severity(Severity::Info)
    }

    fn of_severity(&self, severity: Severity) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(move |d| d.severity == severity)
    }

    /// Whether any finding cites a given rule, which is how callers assert on a requirement.
    pub fn cites(&self, rule: &str) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.rule.as_deref() == Some(rule))
    }

    fn absorb(&mut self, tier: Tier, diagnostics: Vec<Diagnostic>) {
        self.tiers_run.push(tier);
        self.diagnostics.extend(diagnostics);
    }
}

/// One column of a table, as Tier 3 needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnDescription {
    /// The column's name and `system_schema`-spelled CQL type.
    pub column: ColumnRef,
    /// Whether the column is part of the partition key.
    pub partition_key: bool,
    /// Whether the column is a clustering column.
    pub clustering_key: bool,
    /// Whether the column is static.
    pub is_static: bool,
}

impl ColumnDescription {
    /// A plain, non-key column.
    pub fn new(name: impl Into<String>, cql_type: impl Into<String>) -> Self {
        Self {
            column: ColumnRef::new(name, cql_type),
            partition_key: false,
            clustering_key: false,
            is_static: false,
        }
    }

    /// Marks the column as part of the partition key.
    #[must_use]
    pub fn partition_key(mut self) -> Self {
        self.partition_key = true;
        self
    }

    /// Marks the column as a clustering column.
    #[must_use]
    pub fn clustering_key(mut self) -> Self {
        self.clustering_key = true;
        self
    }

    /// Marks the column as static.
    #[must_use]
    pub fn r#static(mut self) -> Self {
        self.is_static = true;
        self
    }

    /// The column's name.
    pub fn name(&self) -> &str {
        self.column.name()
    }

    /// The column's CQL type, exactly as `system_schema.columns.type` spells it.
    pub fn cql_type(&self) -> &str {
        self.column.cql_type()
    }

    /// Whether the column belongs to the primary key.
    pub fn is_key(&self) -> bool {
        self.partition_key || self.clustering_key
    }

    /// Whether the column is a counter, which forbids TTL and writetime (`CFG-036`).
    pub fn is_counter(&self) -> bool {
        self.cql_type().eq_ignore_ascii_case("counter")
    }

    /// Whether the column is a list, set or map, frozen or not.
    pub fn is_collection(&self) -> bool {
        let ty = self.unfrozen_type();
        ["list<", "set<", "map<"]
            .iter()
            .any(|prefix| ty.starts_with(prefix))
    }

    /// Whether the column is an unfrozen `list`, which `CASSANDRA-11368` makes unsafe to rerun
    /// with a writetime increment of zero (`CFG-039`).
    pub fn is_unfrozen_list(&self) -> bool {
        let ty = self.cql_type().trim().to_ascii_lowercase();
        ty.starts_with("list<")
    }

    /// Whether the column is a user-defined or tuple type.
    pub fn is_udt(&self) -> bool {
        let ty = self.unfrozen_type();
        ty.starts_with("tuple<")
            || !(ty.starts_with("list<")
                || ty.starts_with("set<")
                || ty.starts_with("map<")
                || is_primitive(&ty))
    }

    /// Whether the column can hold a large object, which changes batching advice (`UI-004`).
    pub fn is_lob(&self) -> bool {
        matches!(
            self.cql_type().trim().to_ascii_lowercase().as_str(),
            "blob" | "text" | "varchar" | "ascii"
        )
    }

    /// The type with one layer of `frozen<>` removed.
    fn unfrozen_type(&self) -> String {
        let ty = self.cql_type().trim().to_ascii_lowercase();
        ty.strip_prefix("frozen<")
            .and_then(|inner| inner.strip_suffix('>'))
            .map_or(ty.clone(), str::to_owned)
    }
}

/// Whether a CQL type name is one of the primitives, i.e. not a UDT.
fn is_primitive(ty: &str) -> bool {
    matches!(
        ty,
        "ascii"
            | "bigint"
            | "blob"
            | "boolean"
            | "counter"
            | "date"
            | "decimal"
            | "double"
            | "duration"
            | "float"
            | "inet"
            | "int"
            | "smallint"
            | "text"
            | "time"
            | "timestamp"
            | "timeuuid"
            | "tinyint"
            | "uuid"
            | "varchar"
            | "varint"
    ) || ty.starts_with("vector<")
}

/// A table as Tier 3 needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDescription {
    /// Which table this describes.
    pub table: TableRef,
    /// Its columns, in schema order.
    pub columns: Vec<ColumnDescription>,
}

impl TableDescription {
    /// Builds a description.
    pub fn new(table: TableRef, columns: Vec<ColumnDescription>) -> Self {
        Self { table, columns }
    }

    /// The named column. Comparison is exact; identifier folding is `cdm-cql`'s job (`SCH-010`).
    pub fn column(&self, name: &str) -> Option<&ColumnDescription> {
        self.columns.iter().find(|column| column.name() == name)
    }

    /// The partition key columns, in order.
    pub fn partition_key(&self) -> impl Iterator<Item = &ColumnDescription> {
        self.columns.iter().filter(|column| column.partition_key)
    }

    /// The primary key columns, in order.
    pub fn primary_key(&self) -> impl Iterator<Item = &ColumnDescription> {
        self.columns.iter().filter(|column| column.is_key())
    }

    /// Whether this is a counter table (`CFG-036`, `MIG-030`).
    pub fn is_counter_table(&self) -> bool {
        self.columns.iter().any(ColumnDescription::is_counter)
    }

    /// Whether the primary key is the partition key, i.e. there are no clustering columns.
    ///
    /// Every row is then its own partition, so batching across rows spans partitions
    /// (`UI-004`).
    pub fn is_partition_key_only(&self) -> bool {
        !self.columns.iter().any(|column| column.clustering_key)
    }

    /// The non-key columns eligible to supply TTL and writetime.
    ///
    /// Cassandra reports no TTL or writetime for a collection column unless the operator opts
    /// in with `schema.ttl_writetime.use_collections`.
    pub fn writetime_candidates(
        &self,
        use_collections: bool,
    ) -> impl Iterator<Item = &ColumnDescription> {
        self.columns.iter().filter(move |column| {
            !column.is_key() && !column.is_counter() && (use_collections || !column.is_collection())
        })
    }
}

/// The live cluster schema, as Tier 3 sees it (`CFG-020`).
///
/// Implemented by `cdm-cql` over the schema snapshot taken at start-up. Returning `Ok(None)`
/// means "the table does not exist", which is a diagnostic; returning `Err` means the schema
/// could not be read at all, which is a connection problem and is reported as one.
pub trait SchemaProvider {
    /// Describes a table, or reports that it does not exist.
    fn table(&self, table: &TableRef) -> Result<Option<TableDescription>, CdmError>;

    /// Whether a keyspace exists. Distinguishing this from a missing table turns "no such table"
    /// into the far more useful "no such keyspace" when the operator mistyped the keyspace.
    fn keyspace_exists(&self, keyspace: &str) -> Result<bool, CdmError>;
}

/// Runs the three tiers of `CFG-020`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Validator {
    options: ValidationOptions,
}

impl Validator {
    /// A validator with the default options.
    pub fn new() -> Self {
        Self::default()
    }

    /// A validator with explicit options.
    pub fn with_options(options: ValidationOptions) -> Self {
        Self { options }
    }

    /// Tier 1: types, ranges, enumerations and mutually-required groups.
    pub fn tier1(&self, config: &CdmConfig) -> Vec<Diagnostic> {
        tier1::check(config, self.options)
    }

    /// Tier 2: the cross-field rules of `SPEC` §3.4.
    pub fn tier2(&self, config: &CdmConfig) -> Vec<Diagnostic> {
        tier2::check(config, self.options)
    }

    /// Tier 3: the rules that need the live schema.
    pub fn tier3(&self, config: &CdmConfig, schema: &dyn SchemaProvider) -> Vec<Diagnostic> {
        tier3::check(config, schema, self.options)
    }

    /// Runs every tier that can run, in order, collecting all violations (`CFG-021`).
    ///
    /// Tier 3 is skipped, with a notice, when no [`SchemaProvider`] is supplied — which is what
    /// `cdm validate-config --offline` does.
    pub fn validate(
        &self,
        config: &CdmConfig,
        schema: Option<&dyn SchemaProvider>,
    ) -> ValidationReport {
        let mut report = ValidationReport::default();
        report.absorb(Tier::Syntactic, self.tier1(config));
        report.absorb(Tier::Semantic, self.tier2(config));
        match schema {
            Some(schema) => report.absorb(Tier::SchemaBound, self.tier3(config, schema)),
            None => report.diagnostics.push(
                Diagnostic::info(CODE, "schema-bound validation was skipped")
                    .with_detail("no cluster schema was supplied, so tier 3 could not run")
                    .with_rule("CFG-020"),
            ),
        }
        report
    }
}

/// Splits a `keyspace.table` property, if it is well formed.
pub(crate) fn parse_keyspace_table(value: &str) -> Option<TableRef> {
    let (keyspace, table) = value.trim().split_once('.')?;
    if keyspace.is_empty() || table.is_empty() || table.contains('.') {
        return None;
    }
    Some(TableRef::new(keyspace, table))
}

/// Builds an error diagnostic for a property.
pub(crate) fn error(key: &str, title: impl Into<String>, rule: &str) -> Diagnostic {
    Diagnostic::error(CODE, title)
        .with_location(key.to_owned())
        .with_rule(rule.to_owned())
}

/// Builds a warning diagnostic for a property.
pub(crate) fn warning(key: &str, title: impl Into<String>, rule: &str) -> Diagnostic {
    Diagnostic::warning(CODE, title)
        .with_location(key.to_owned())
        .with_rule(rule.to_owned())
}

/// Builds an informational diagnostic for a property.
pub(crate) fn notice(key: &str, title: impl Into<String>, rule: &str) -> Diagnostic {
    Diagnostic::info(CODE, title)
        .with_location(key.to_owned())
        .with_rule(rule.to_owned())
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
pub(crate) mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// A [`SchemaProvider`] backed by a map, for tests and for `cdm config check --schema-file`.
    #[derive(Debug, Default)]
    pub(crate) struct FakeSchema {
        tables: BTreeMap<String, TableDescription>,
        broken: bool,
    }

    impl FakeSchema {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        pub(crate) fn with(mut self, table: TableDescription) -> Self {
            self.tables.insert(table.table.to_string(), table);
            self
        }

        pub(crate) fn broken() -> Self {
            Self {
                tables: BTreeMap::new(),
                broken: true,
            }
        }
    }

    impl SchemaProvider for FakeSchema {
        fn table(&self, table: &TableRef) -> Result<Option<TableDescription>, CdmError> {
            if self.broken {
                return Err(CdmError::new(cdm_core::ErrorKind::Connect, "no session"));
            }
            Ok(self.tables.get(&table.to_string()).cloned())
        }

        fn keyspace_exists(&self, keyspace: &str) -> Result<bool, CdmError> {
            if self.broken {
                return Err(CdmError::new(cdm_core::ErrorKind::Connect, "no session"));
            }
            Ok(self
                .tables
                .keys()
                .any(|name| name.starts_with(&format!("{keyspace}."))))
        }
    }

    /// The canonical two-column origin table used across the tier-3 tests.
    pub(crate) fn origin_table() -> TableDescription {
        TableDescription::new(
            TableRef::new("ks", "src"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("bucket", "int").clustering_key(),
                ColumnDescription::new("data", "text"),
                ColumnDescription::new("tags", "list<text>"),
            ],
        )
    }

    #[test]
    fn cfg_020_the_tiers_are_numbered_and_named_as_the_spec_says() {
        assert_eq!(Tier::Syntactic.number(), 1);
        assert_eq!(Tier::Semantic.number(), 2);
        assert_eq!(Tier::SchemaBound.number(), 3);
        assert_eq!(Tier::SchemaBound.as_str(), "schema-bound");
        assert!(Tier::Syntactic < Tier::SchemaBound);
    }

    #[test]
    fn cfg_020_each_tier_is_independently_invocable() {
        let mut config = CdmConfig::default();
        config.schema.origin.keyspace_table = Some("ks.src".to_owned());
        let validator = Validator::new();
        assert!(validator.tier1(&config).iter().all(|d| !d.is_blocking()));
        assert!(validator.tier2(&config).is_empty());
        let schema = FakeSchema::new().with(origin_table());
        assert!(validator
            .tier3(&config, &schema)
            .iter()
            .all(|d| !d.is_blocking()));
    }

    #[test]
    fn cfg_020_tier_three_is_skipped_with_a_notice_when_there_is_no_cluster() {
        let mut config = CdmConfig::default();
        config.schema.origin.keyspace_table = Some("ks.src".to_owned());
        let report = Validator::new().validate(&config, None);
        assert_eq!(report.tiers_run, [Tier::Syntactic, Tier::Semantic]);
        assert!(report.notices().any(|d| d.title.contains("skipped")));
        assert!(report.is_valid());
    }

    #[test]
    fn cfg_021_every_violation_is_reported_at_once() {
        // Six independent mistakes; a fail-fast validator would show one.
        let mut config = CdmConfig::default();
        config.schema.origin.keyspace_table = None; // CFG-022
        config.connect.origin.host = String::new(); // CFG-024
        config.perfops.num_parts = 0; // range
        config.filter.token_coverage_percent = 200; // range
        config.feature.guardrail.column_size_kb = -1.0; // CFG-035
        config.feature.explode_map.origin_column = Some("m".to_owned()); // CFG-031
        config.transform.custom_writetime_increment = -5; // CFG-033

        let report = Validator::new().validate(&config, None);
        assert!(!report.is_valid());
        for rule in [
            "CFG-022", "CFG-024", "CFG-020", "CFG-035", "CFG-031", "CFG-033",
        ] {
            assert!(report.cites(rule), "{rule} was not reported: {report:#?}");
        }
        assert!(report.errors().count() >= 6, "{report:#?}");
    }

    #[test]
    fn cfg_020_the_report_partitions_findings_by_severity() {
        let mut report = ValidationReport::default();
        report.absorb(
            Tier::Syntactic,
            vec![
                error("a", "e", "CFG-022"),
                warning("b", "w", "CFG-026"),
                notice("c", "n", "CFG-037"),
            ],
        );
        assert_eq!(report.errors().count(), 1);
        assert_eq!(report.warnings().count(), 1);
        assert_eq!(report.notices().count(), 1);
        assert!(!report.is_valid());
        assert!(report.cites("CFG-026"));
        assert!(!report.cites("CFG-999"));
    }

    #[test]
    fn cfg_020_keyspace_table_values_are_parsed_strictly() {
        assert_eq!(
            parse_keyspace_table(" ks.tbl ").map(|t| t.to_string()),
            Some("ks.tbl".to_owned())
        );
        assert!(parse_keyspace_table("tbl").is_none());
        assert!(parse_keyspace_table("ks.").is_none());
        assert!(parse_keyspace_table(".tbl").is_none());
        assert!(parse_keyspace_table("a.b.c").is_none());
    }

    #[test]
    fn cfg_036_column_predicates_classify_cql_types() {
        let counter = ColumnDescription::new("c", "counter");
        assert!(counter.is_counter());
        assert!(!counter.is_collection());

        let list = ColumnDescription::new("l", "list<text>");
        assert!(list.is_collection());
        assert!(list.is_unfrozen_list());

        let frozen = ColumnDescription::new("f", "frozen<list<text>>");
        assert!(frozen.is_collection());
        assert!(!frozen.is_unfrozen_list());

        assert!(ColumnDescription::new("u", "my_udt").is_udt());
        assert!(!ColumnDescription::new("i", "int").is_udt());
        assert!(!ColumnDescription::new("v", "vector<float, 3>").is_udt());
        assert!(ColumnDescription::new("b", "blob").is_lob());
        assert!(!ColumnDescription::new("i", "int").is_lob());

        let key = ColumnDescription::new("k", "int").partition_key();
        assert!(key.is_key());
        assert!(ColumnDescription::new("s", "int").r#static().is_static);
        assert_eq!(key.name(), "k");
    }

    #[test]
    fn cfg_036_table_predicates_describe_the_shape_of_a_table() {
        let table = origin_table();
        assert!(!table.is_counter_table());
        assert!(!table.is_partition_key_only());
        assert_eq!(table.partition_key().count(), 1);
        assert_eq!(table.primary_key().count(), 2);
        assert_eq!(
            table.column("data").map(ColumnDescription::cql_type),
            Some("text")
        );
        assert!(table.column("nope").is_none());
        // `tags` is a collection, so it only counts when collections are opted in.
        assert_eq!(table.writetime_candidates(false).count(), 1);
        assert_eq!(table.writetime_candidates(true).count(), 2);

        let counters = TableDescription::new(
            TableRef::new("ks", "counts"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("hits", "counter"),
            ],
        );
        assert!(counters.is_counter_table());
        assert!(counters.is_partition_key_only());
    }
}
