//! Everything the migrate job resolves before the first row moves (`ARCHITECTURE.md` §5.5).
//!
//! # What "resolved once" buys
//!
//! The per-row path here does no name lookups, no type parsing, no `format!` and no plan
//! construction. Every one of those has already happened: the column mapping, one conversion plan
//! per column, the bind-slot order, the prepared statements, the filter chain, the TTL/writetime
//! positions, the counter columns, the positions of the partition key inside a bound write.
//!
//! That is not only a throughput argument. A decision taken per row is a decision that can be
//! taken *differently* on row 4,000,000 of a run, and there is no test that would catch it.
//!
//! # The one thing that is decided per row, and when it is not
//!
//! [`MigratePlan::needs_record`] asks whether any enabled feature has to see the row as owned
//! data. Filters (`FEA-050`..`FEA-054`) take a [`Record`](cdm_core::Record), and the TTL/writetime
//! plan (`FEA-040`) reads its virtual columns out of a [`Row`](cdm_core::Row) — both owned types,
//! so a run that uses either pays one materialisation per row.
//!
//! A run that uses neither — the common case, and the one the benchmarks are written against —
//! never builds one, and every bound value is the response frame's own bytes (`MIG-040`). The
//! flag is computed here, once, so that the row loop is a branch on a `bool` rather than five
//! predicates.

use cdm_codec::Planner;
use cdm_core::{CdmError, ErrorKind};
use cdm_cql::exec::RunExecutor;
use cdm_cql::statement::{
    Binder, ColumnMapping, KeyBinding, MissingKeyPolicy, OriginProjection, StatementOptions,
    TargetSelectByPk, TargetUpsert,
};
use cdm_feature::{ExplodePlan, ExtractJsonPlan, FilterChain, WritetimeTtlPlan};

use super::counter::CounterPlan;
use super::settings::MigrateSettings;

/// The features a run has switched on, handed to [`MigratePlan::resolve`] already resolved.
///
/// A struct rather than five parameters because the list will grow, and because a caller that
/// passes `None, None, chain, plan, false` in the wrong order compiles.
#[derive(Debug, Default)]
pub struct MigrateFeatures {
    /// The filter chain (`FEA-050`..`FEA-054`, `MIG-002`).
    pub filters: FilterChain,
    /// The TTL and writetime plan (`FEA-040`..`FEA-046`).
    pub writetime: WritetimeTtlPlan,
    /// The explode-map plan, when configured (`FEA-020`..`FEA-023`).
    pub explode: Option<ExplodePlan>,
    /// The extract-JSON plan, when configured (`FEA-030`..`FEA-035`).
    pub extract_json: Option<ExtractJsonPlan>,
    /// Whether a writetime filter is active, which coerces the batch size (`MIG-021`).
    pub writetime_filter: bool,
}

/// The immutable execution plan of one migrate run.
#[derive(Debug)]
pub struct MigratePlan {
    executor: RunExecutor,
    binder: Binder,
    key_binding: KeyBinding,
    settings: MigrateSettings,
    counters: CounterPlan,
    partition_positions: Vec<usize>,
    origin_key_indices: Vec<usize>,
    projection_width: usize,
    features: MigrateFeatures,
    needs_record: bool,
}

impl MigratePlan {
    /// Resolves the plan from both schemas and an executor that has already prepared its
    /// statements.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] for a target this run cannot write — a counter table with no
    /// counter column mapped from the origin (`MIG-030`), or a primary key the target lookup
    /// cannot address (`SCH-006`) — and [`ErrorKind::Internal`] if the statements and the mapping
    /// disagree, which means they were built from different schemas.
    pub fn resolve(
        executor: RunExecutor,
        mapping: &ColumnMapping,
        projection: &OriginProjection,
        planner: &Planner,
        settings: MigrateSettings,
        missing_key: MissingKeyPolicy,
        map_remove_null_value: bool,
        features: MigrateFeatures,
    ) -> Result<Self, CdmError> {
        let upsert = TargetUpsert::new(
            mapping,
            StatementOptions {
                using: using_clause(&features.writetime),
            },
        )?;
        let partition_positions = partition_positions(mapping, &upsert);
        let binder = Binder::new(mapping, upsert, planner, missing_key, map_remove_null_value)?;
        let key_binding = binder.key_binding(&TargetSelectByPk::new(mapping)?)?;
        let counters = CounterPlan::resolve(mapping)?;
        let origin_key_indices = origin_key_indices(mapping)?;

        // Every feature here is defined over an owned `Record` or `Row`, so switching any of them
        // on costs one materialisation per row. Deciding it once keeps the row loop a `bool`.
        let needs_record = !features.filters.is_empty()
            || features.writetime.has_writetime()
            || features.writetime.has_ttl()
            || features.explode.is_some()
            || features.extract_json.is_some();

        Ok(Self {
            executor,
            binder,
            key_binding,
            settings,
            counters,
            partition_positions,
            origin_key_indices,
            projection_width: projection.width(),
            features,
            needs_record,
        })
    }

    /// The prepared statements and the sessions they run against.
    #[must_use]
    pub const fn executor(&self) -> &RunExecutor {
        &self.executor
    }

    /// The binder every row goes through (`MIG-011`).
    #[must_use]
    pub const fn binder(&self) -> &Binder {
        &self.binder
    }

    /// The key columns the target lookup binds, for the counter delta (`MIG-031`).
    #[must_use]
    pub const fn key_binding(&self) -> &KeyBinding {
        &self.key_binding
    }

    /// The resolved settings (`MIG-004`, `MIG-020`..`MIG-022`, `MIG-041`).
    #[must_use]
    pub const fn settings(&self) -> MigrateSettings {
        self.settings
    }

    /// The counter columns, empty for an ordinary table (`MIG-030`).
    #[must_use]
    pub const fn counter_plan(&self) -> &CounterPlan {
        &self.counters
    }

    /// Where the target partition key sits inside a bound write (`MIG-022`).
    #[must_use]
    pub fn partition_positions(&self) -> &[usize] {
        &self.partition_positions
    }

    /// Where the origin's primary-key columns sit in the projection, for identifying a failing
    /// row without logging its values (`SEC-002`).
    #[must_use]
    pub fn origin_key_indices(&self) -> &[usize] {
        &self.origin_key_indices
    }

    /// How many cells a row from the origin projection has (`SCH-007`).
    #[must_use]
    pub const fn projection_width(&self) -> usize {
        self.projection_width
    }

    /// The features this run has switched on.
    #[must_use]
    pub const fn features(&self) -> &MigrateFeatures {
        &self.features
    }

    /// Whether any enabled feature forces the row to be materialised as owned data.
    ///
    /// `false` is the fast path of `MIG-040`: nothing is copied between the response frame and the
    /// write buffer.
    #[must_use]
    pub const fn needs_record(&self) -> bool {
        self.needs_record
    }

    /// Whether this run writes counters (`SCH-005`, `MIG-030`).
    #[must_use]
    pub fn is_counter_run(&self) -> bool {
        !self.counters.is_empty()
    }
}

/// The `USING` clause the write statement carries (`FEA-046`).
///
/// `cdm-feature` and `cdm-cql` each own a `UsingClause`, on opposite sides of the dependency edge
/// (`ARCHITECTURE.md` §3), so the two booleans cross as data. That is the same seam
/// `cdm-cql::statement`'s module documentation describes, applied once here rather than at every
/// call site.
fn using_clause(plan: &WritetimeTtlPlan) -> cdm_cql::statement::UsingClause {
    let feature = plan.using_clause();
    cdm_cql::statement::UsingClause {
        ttl: feature.ttl,
        timestamp: feature.timestamp,
    }
}

/// The positions, within a bound write, of the target partition-key columns (`MIG-022`).
///
/// A constant partition-key component is inlined into the statement (`FEA-012`) and so has no bind
/// position; it is also the same for every row, which means it can never distinguish two
/// partitions. Omitting it is therefore both necessary and harmless.
fn partition_positions(mapping: &ColumnMapping, upsert: &TargetUpsert) -> Vec<usize> {
    mapping
        .target_columns()
        .iter()
        .enumerate()
        .filter(|(_, column)| column.kind == cdm_cql::schema::ColumnKind::PartitionKey)
        .filter_map(|(index, _)| upsert.bind_position(index))
        .collect()
}

/// The origin projection positions of the origin's own primary-key columns.
fn origin_key_indices(mapping: &ColumnMapping) -> Result<Vec<usize>, CdmError> {
    mapping
        .origin_table()
        .primary_key()
        .iter()
        .map(|column| {
            mapping.origin_index_of(&column.name).ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!(
                        "the origin primary-key column `{}` is not in the projection, which \
                         `SCH-004` forbids skipping",
                        column.name
                    ),
                )
            })
        })
        .collect()
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
    use cdm_cql::statement::MappingOptions;

    use crate::migrate::testfixtures::{counter_mapping, plain_mapping, plain_schema};

    use super::*;

    #[test]
    fn mig_022_the_partition_positions_are_the_bind_slots_of_the_partition_key() {
        let mapping = plain_mapping();
        let upsert = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        // `id` is the only partition-key column, and it binds first.
        assert_eq!(partition_positions(&mapping, &upsert), vec![0]);
    }

    #[test]
    fn mig_022_a_constant_partition_key_component_has_no_bind_position() {
        let schema = plain_schema();
        let mut target = schema.clone();
        target.columns.push(cdm_cql::schema::ColumnMeta {
            name: "tenant".to_owned(),
            cql_type: "text".to_owned(),
            kind: cdm_cql::schema::ColumnKind::PartitionKey,
            position: 1,
            clustering_order: cdm_cql::schema::ClusteringOrder::None,
        });
        let options = MappingOptions {
            constants: vec![("tenant".to_owned(), "'acme'".to_owned())],
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&schema, &target, &options).unwrap();
        let upsert = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        assert_eq!(
            partition_positions(&mapping, &upsert),
            vec![0],
            "an inlined constant cannot distinguish two partitions, so it is not compared"
        );
    }

    #[test]
    fn mig_030_the_counter_plan_is_resolved_from_the_mapping() {
        assert!(CounterPlan::resolve(&plain_mapping()).unwrap().is_empty());
        assert_eq!(
            CounterPlan::resolve(&counter_mapping())
                .unwrap()
                .columns()
                .len(),
            1
        );
    }

    #[test]
    fn sch_004_the_origin_key_indices_are_projection_positions() {
        assert_eq!(origin_key_indices(&plain_mapping()).unwrap(), vec![0]);
        assert_eq!(origin_key_indices(&counter_mapping()).unwrap(), vec![0, 1]);
    }

    #[test]
    fn fea_046_the_using_clause_crosses_the_crate_boundary_as_data() {
        let disabled = using_clause(&WritetimeTtlPlan::disabled());
        assert!(!disabled.ttl && !disabled.timestamp);
    }
}
