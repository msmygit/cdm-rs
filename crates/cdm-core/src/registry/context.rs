//! Supporting types the plugin traits exchange (`PLG-001`..`PLG-007`).
//!
//! # Why these live here
//!
//! Several of these types are the *minimal placeholder* form of something a later crate owns —
//! `cdm-config`'s `CdmConfig`, `cdm-cql`'s `TableSchema`, `cdm-codec`'s `CqlTypeInfo`,
//! `cdm-metrics`' `Counters`. The plugin traits are declared in `cdm-core` and `cdm-core` may not
//! depend on any of those crates (`ARCHITECTURE.md` §3.2), so the trait signatures speak in terms
//! of these driver-independent, allocation-only structures instead. Each one documents the type it
//! stands in for and the PR that introduces the real thing.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::domain::{ColumnRef, JobKind, RawCell, Record, RunId, RunStatus, TableRef, TokenRange};
use crate::error::CdmError;

/// The fully resolved configuration of a run, flattened to string-valued property keys.
///
/// A placeholder for `cdm-config`'s `CdmConfig` (PR #4). The flattened form is not a compromise:
/// `CFG-100`..`CFG-200` define the property registry in exactly these terms, and Java CDM's
/// `.properties` files are string-valued too, so a plugin that reads `spark.cdm.perfops.numParts`
/// sees the same key here as in the file. Multi-valued properties are comma-joined, as in Java.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveConfig {
    values: BTreeMap<String, String>,
}

impl EffectiveConfig {
    /// An empty configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// The value of a property, if set.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Whether a property is set.
    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// Sets a property, returning the previous value.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) -> Option<String> {
        self.values.insert(key.into(), value.into())
    }

    /// Every property, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for EffectiveConfig {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        Self {
            values: iter
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
}

/// One side's table and its columns.
///
/// A placeholder for `cdm-cql`'s `TableSchema` (PR #9), reduced to what a plugin needs in order to
/// decide whether it applies: which columns exist and how `system_schema` spells their types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableView {
    /// The table this view describes.
    pub table: TableRef,
    /// Its columns, in schema order.
    pub columns: Vec<ColumnRef>,
}

impl TableView {
    /// Creates a view.
    pub fn new(table: TableRef, columns: Vec<ColumnRef>) -> Self {
        Self { table, columns }
    }

    /// The named column, if the table has one. Comparison is exact: CQL identifier folding is
    /// `cdm-cql`'s business (`SCH-010`).
    pub fn column(&self, name: &str) -> Option<&ColumnRef> {
        self.columns.iter().find(|c| c.name() == name)
    }
}

/// The origin and target schemas a run reconciles.
///
/// A placeholder for the schema pair `cdm-cql` resolves at startup (PR #9); it is what Tier-3
/// validation (`CFG-020`) is evaluated against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaPair {
    /// The origin table.
    pub origin: TableView,
    /// The target table.
    pub target: TableView,
}

impl SchemaPair {
    /// Creates a pair.
    pub fn new(origin: TableView, target: TableView) -> Self {
        Self { origin, target }
    }
}

/// A pair of CQL types a codec converts between (`PLG-001`).
///
/// Types are named as `system_schema.columns.type` spells them. A placeholder for `cdm-codec`'s
/// `CqlTypeInfo` pair (PR #11), which parses those strings into a type tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypePair {
    /// The origin CQL type, e.g. `text`.
    pub origin: String,
    /// The target CQL type, e.g. `int`.
    pub target: String,
}

impl TypePair {
    /// Creates a pair.
    pub fn new(origin: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            target: target.into(),
        }
    }
}

/// Accumulates extra expressions a feature needs in the origin `SELECT` (`FEA-060`, `FEA-030`).
///
/// A placeholder for `cdm-cql`'s statement builder (PR #18). Expressions are appended verbatim, in
/// registration order, so their positions in the resulting [`Row`](crate::Row) are deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectionBuilder {
    expressions: Vec<String>,
}

impl ProjectionBuilder {
    /// An empty projection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an expression, e.g. `WRITETIME(col)`, returning its position in the row.
    pub fn add(&mut self, expression: impl Into<String>) -> usize {
        self.expressions.push(expression.into());
        self.expressions.len() - 1
    }

    /// The accumulated expressions, in registration order.
    pub fn expressions(&self) -> &[String] {
        &self.expressions
    }
}

/// Accumulates extra target columns a feature binds (`FEA-010`, constant columns).
///
/// A placeholder for `cdm-cql`'s statement builder (PR #18). The literal is CQL source, bound into
/// the statement text rather than as a value, which is how Java CDM implements constant columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingBuilder {
    bindings: Vec<(String, String)>,
}

impl BindingBuilder {
    /// An empty binding set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds a target column to a CQL literal.
    pub fn add_literal(&mut self, column: impl Into<String>, literal: impl Into<String>) {
        self.bindings.push((column.into(), literal.into()));
    }

    /// The accumulated `(column, literal)` pairs, in registration order.
    pub fn bindings(&self) -> &[(String, String)] {
        &self.bindings
    }
}

/// Where a feature emits the records it produces (`PLG-002`).
///
/// A sink rather than a return value because one origin row may become many records (`FEA-020`,
/// explode map) or none (a filter), and because the engine reuses one buffer per worker rather
/// than allocating a `Vec` per row (`ARCHITECTURE.md` §5.5).
pub trait RecordSink {
    /// Emits one record.
    ///
    /// # Errors
    ///
    /// Returns whatever error the downstream consumer produces; a feature should propagate it
    /// rather than swallow it.
    fn emit(&mut self, record: Record) -> Result<(), CdmError>;
}

impl RecordSink for Vec<Record> {
    fn emit(&mut self, record: Record) -> Result<(), CdmError> {
        self.push(record);
        Ok(())
    }
}

/// A feature's participation in the validate job's comparison (`PLG-002`, `VAL-004`).
///
/// The default implementations opt out, so a hook only overrides what it actually cares about.
pub trait CompareHook: Send + Sync {
    /// Whether the named target column should be excluded from comparison — what constant columns
    /// (`FEA-011`) and TTL/writetime columns need.
    fn skips_column(&self, _column: &str) -> bool {
        false
    }

    /// Compares one cell, overriding the default byte comparison.
    ///
    /// `Some(true)` means equal, `Some(false)` means different, and `None` defers to the default.
    fn compare_cell(&self, _column: &str, _origin: &RawCell, _target: &RawCell) -> Option<bool> {
        None
    }
}

/// What a job made of one range (`PLG-004`).
///
/// A placeholder for the counter snapshot `cdm-metrics` will carry (PR #19): `MET-004` requires
/// per-range interim counts to be folded into run totals, and `MET-005` requires `info` to be the
/// exact Java metrics string, e.g. `Read: 10; Write: 9; Skipped: 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeOutcome {
    /// The range that was processed.
    pub range: TokenRange,
    /// Its terminal status (`TRK-021`).
    pub status: RunStatus,
    /// The metrics string recorded in `cdm_run_details.run_info`.
    pub info: String,
}

/// A point-in-time view of a run's counters, handed to every [`MetricsExporter`] (`PLG-006`).
///
/// A placeholder for `cdm-metrics`' `Counters` (PR #19). Counter names are the `MET-001` set:
/// `READ`, `WRITE`, `MISMATCH`, `CORRECTED_MISMATCH`, `MISSING`, `CORRECTED_MISSING`, `VALID`,
/// `SKIPPED`, `LARGE`, `ERROR`, `UNFLUSHED`, `PARTITIONS_PASSED`, `PARTITIONS_FAILED`. Which of
/// them a run registers depends on its job (`MET-002`), so a snapshot holds only those.
///
/// [`MetricsExporter`]: crate::MetricsExporter
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshot {
    /// The run these counters belong to.
    pub run_id: RunId,
    /// The job that registered them.
    pub job: JobKind,
    /// When the snapshot was taken. RFC 3339 UTC on the wire (`NFR-007`).
    pub taken_at: DateTime<Utc>,
    /// Counter name to value, for the counters this job registered.
    pub counters: BTreeMap<String, u64>,
}

impl MetricsSnapshot {
    /// The value of a counter, or zero if this job did not register it.
    pub fn counter(&self, name: &str) -> u64 {
        self.counters.get(name).copied().unwrap_or(0)
    }
}

/// The `cdm_run_info` row of one run (`TRK-010`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    /// The run id.
    pub run_id: RunId,
    /// The run this one resumes, if any (`TRK-031`).
    pub previous_run_id: Option<RunId>,
    /// The table being processed, whose name is part of the partition key.
    pub table: TableRef,
    /// The job type, written to `run_type`.
    pub job: JobKind,
    /// The current status.
    pub status: RunStatus,
    /// When the run started.
    pub started_at: Option<DateTime<Utc>>,
    /// When the run ended (`TRK-022`).
    pub ended_at: Option<DateTime<Utc>>,
    /// The aggregate metrics string (`MET-005`).
    pub info: Option<String>,
}

/// The `cdm_run_details` row of one range within a run (`TRK-010`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeRecord {
    /// The range, whose lower bound is the clustering key of the row.
    pub range: TokenRange,
    /// The current status.
    pub status: RunStatus,
    /// When a worker claimed the range (`TRK-021`).
    pub started_at: Option<DateTime<Utc>>,
    /// The interim metrics string for this range (`MET-004`).
    pub info: Option<String>,
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
    use crate::domain::{PrimaryKey, Row};

    #[test]
    fn plg_013_effective_config_is_a_flat_property_map() {
        let mut config: EffectiveConfig = [("spark.cdm.perfops.numParts", "5000")]
            .into_iter()
            .collect();
        assert_eq!(config.get("spark.cdm.perfops.numParts"), Some("5000"));
        assert!(config.contains("spark.cdm.perfops.numParts"));
        assert_eq!(config.get("spark.cdm.missing"), None);
        assert_eq!(
            config.insert("spark.cdm.perfops.numParts", "10000"),
            Some("5000".to_owned())
        );
        assert_eq!(config.iter().count(), 1);
        assert!(EffectiveConfig::new().iter().next().is_none());
    }

    #[test]
    fn plg_002_schema_pair_exposes_columns_by_name() {
        let origin = TableView::new(
            TableRef::new("ks", "src"),
            vec![ColumnRef::new("id", "int"), ColumnRef::new("v", "text")],
        );
        let target = TableView::new(
            TableRef::new("ks", "dst"),
            vec![ColumnRef::new("id", "int")],
        );
        let pair = SchemaPair::new(origin, target);
        assert_eq!(
            pair.origin.column("v").map(ColumnRef::cql_type),
            Some("text")
        );
        assert!(pair.target.column("v").is_none());
        assert_eq!(pair.origin.table.to_string(), "ks.src");
    }

    #[test]
    fn plg_002_projection_and_binding_builders_preserve_registration_order() {
        let mut projection = ProjectionBuilder::new();
        assert_eq!(projection.add("WRITETIME(v)"), 0);
        assert_eq!(projection.add("TTL(v)"), 1);
        assert_eq!(projection.expressions(), ["WRITETIME(v)", "TTL(v)"]);

        let mut binding = BindingBuilder::new();
        binding.add_literal("const_a", "'x'");
        binding.add_literal("const_b", "1");
        assert_eq!(
            binding.bindings(),
            [
                ("const_a".to_owned(), "'x'".to_owned()),
                ("const_b".to_owned(), "1".to_owned())
            ]
        );
    }

    #[test]
    fn plg_002_a_vec_is_a_record_sink() {
        let mut sink: Vec<Record> = Vec::new();
        let record = Record::new(PrimaryKey::default(), Row::default());
        sink.emit(record.clone()).unwrap();
        sink.emit(record).unwrap();
        assert_eq!(sink.len(), 2);
    }

    #[test]
    fn plg_002_compare_hook_defaults_to_no_opinion() {
        struct Silent;
        impl CompareHook for Silent {}
        let hook = Silent;
        assert!(!hook.skips_column("anything"));
        assert_eq!(hook.compare_cell("c", &RawCell::NULL, &RawCell::NULL), None);
    }

    #[test]
    fn plg_006_metrics_snapshot_reports_unregistered_counters_as_zero() {
        let snapshot = MetricsSnapshot {
            run_id: RunId::from_raw(7),
            job: JobKind::Migrate,
            taken_at: DateTime::UNIX_EPOCH,
            counters: [("READ".to_owned(), 10)].into_iter().collect(),
        };
        assert_eq!(snapshot.counter("READ"), 10);
        assert_eq!(snapshot.counter("MISMATCH"), 0);
    }
}
