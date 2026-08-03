//! The [`SchemaProvider`] implementation that lets Tier-3 validation run (`CFG-020`, `SCH-001`).
//!
//! `cdm-config` defines Tier-3 — the rules that need the live schema — against a trait rather
//! than against this crate, so that configuration parsing stays testable without a cluster and
//! the dependency graph stays acyclic (`ARCHITECTURE.md` §3.2). The trait is **synchronous**,
//! because schema is fetched once before any range is planned. [`SchemaSnapshot`] is that fetch:
//! an immutable picture of the tables a run touches, taken at start-up and then answered from
//! memory.
//!
//! Until this existed, `Validator::validate` was always called with `None` and Tier 3 never ran.

use std::collections::{BTreeMap, BTreeSet};

use cdm_config::{
    CdmConfig, SchemaProvider, TableDescription, ValidationOptions, ValidationReport, Validator,
};
use cdm_core::{CdmError, Side, TableRef};
use scylla::client::session::Session;

use crate::schema::introspect;
use crate::schema::table::TableSchema;

/// The schema of the tables a run touches, read once (`SCH-001`).
#[derive(Debug, Clone, Default)]
pub struct SchemaSnapshot {
    tables: BTreeMap<String, TableSchema>,
    keyspaces: BTreeSet<String>,
}

impl SchemaSnapshot {
    /// An empty snapshot, which reports every table as missing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a table, and its keyspace, to the snapshot.
    #[must_use]
    pub fn with_table(mut self, schema: TableSchema) -> Self {
        self.keyspaces.insert(schema.keyspace.clone());
        self.tables.insert(key(&schema.table_ref()), schema);
        self
    }

    /// Records that a keyspace exists even though none of its tables were read.
    #[must_use]
    pub fn with_keyspace(mut self, keyspace: impl Into<String>) -> Self {
        self.keyspaces.insert(keyspace.into());
        self
    }

    /// Reads the named tables, and the existence of their keyspaces, from a cluster.
    ///
    /// A table that does not exist is not an error here: Tier 3 reports it as a diagnostic
    /// naming the property, which is far more useful than a connection-level failure.
    pub async fn fetch(
        side: Side,
        session: &Session,
        tables: &[TableRef],
    ) -> Result<Self, CdmError> {
        let mut snapshot = Self::new();
        for table in tables {
            if introspect::keyspace_exists(side, session, table.keyspace()).await? {
                snapshot.keyspaces.insert(table.keyspace().to_owned());
            }
            if let Some(schema) = introspect::fetch_table(side, session, table).await? {
                snapshot.tables.insert(key(table), schema);
            }
        }
        Ok(snapshot)
    }

    /// The tables in the snapshot.
    pub fn tables(&self) -> impl Iterator<Item = &TableSchema> {
        self.tables.values()
    }

    /// One table's full metadata, as opposed to the reduced Tier-3 description.
    pub fn schema(&self, table: &TableRef) -> Option<&TableSchema> {
        self.tables.get(&key(table))
    }

    /// Merges another snapshot into this one, which is how the origin and target snapshots become
    /// the single provider Tier 3 takes.
    #[must_use]
    pub fn merged(mut self, other: Self) -> Self {
        self.keyspaces.extend(other.keyspaces);
        self.tables.extend(other.tables);
        self
    }

    /// Runs every validation tier, with Tier 3 finally able to run (`CFG-020`).
    pub fn validate(&self, config: &CdmConfig, options: ValidationOptions) -> ValidationReport {
        Validator::with_options(options).validate(config, Some(self))
    }
}

impl SchemaProvider for SchemaSnapshot {
    fn table(&self, table: &TableRef) -> Result<Option<TableDescription>, CdmError> {
        Ok(self
            .tables
            .get(&key(table))
            .map(TableSchema::to_description))
    }

    fn keyspace_exists(&self, keyspace: &str) -> Result<bool, CdmError> {
        Ok(self.keyspaces.contains(keyspace))
    }
}

/// Snapshot keys are the internal, case-exact `keyspace.table` form (`SCH-002`).
fn key(table: &TableRef) -> String {
    format!("{}.{}", table.keyspace(), table.table())
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
    use crate::schema::table::tests::table;

    #[test]
    fn sch_001_a_snapshot_answers_for_the_tables_it_holds() {
        let snapshot = SchemaSnapshot::new().with_table(table());
        let reference = TableRef::new("ks", "tbl");

        let description = snapshot.table(&reference).unwrap().unwrap();
        assert_eq!(description.table, reference);
        assert!(description.column("pk1").unwrap().partition_key);
        assert!(snapshot.keyspace_exists("ks").unwrap());
        assert!(snapshot.schema(&reference).is_some());
        assert_eq!(snapshot.tables().count(), 1);
    }

    #[test]
    fn sch_001_an_unknown_table_is_reported_as_missing_not_as_an_error() {
        let snapshot = SchemaSnapshot::new().with_table(table());
        assert!(snapshot
            .table(&TableRef::new("ks", "other"))
            .unwrap()
            .is_none());
        assert!(!snapshot.keyspace_exists("other_ks").unwrap());
    }

    #[test]
    fn sch_002_table_lookup_is_case_exact() {
        // `TBL` and `tbl` are different tables when the first was created quoted.
        let snapshot = SchemaSnapshot::new().with_table(table());
        assert!(snapshot
            .table(&TableRef::new("ks", "TBL"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn sch_001_snapshots_merge() {
        let mut second = table();
        second.keyspace = "ks2".to_owned();
        second.table = "other".to_owned();

        let merged = SchemaSnapshot::new()
            .with_table(table())
            .merged(SchemaSnapshot::new().with_table(second));
        assert_eq!(merged.tables().count(), 2);
        assert!(merged.keyspace_exists("ks").unwrap());
        assert!(merged.keyspace_exists("ks2").unwrap());
    }

    #[test]
    fn cfg_020_tier_three_finally_runs_against_a_snapshot() {
        let mut config = CdmConfig::default();
        config.schema.origin.keyspace_table = Some("ks.tbl".to_owned());
        config.schema.target.keyspace_table = Some("ks.missing".to_owned());

        let report = SchemaSnapshot::new()
            .with_table(table())
            .validate(&config, ValidationOptions::default());

        assert!(
            report.tiers_run.contains(&cdm_config::Tier::SchemaBound),
            "tier 3 must have run"
        );
        assert!(
            !report.is_valid(),
            "the target table does not exist and must be reported"
        );
        assert!(
            report
                .errors()
                .any(|d| d.title == "the table does not exist"),
            "expected a Tier-3 diagnostic naming the absent target table, got {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn cfg_020_a_keyspace_can_be_recorded_without_its_tables() {
        let snapshot = SchemaSnapshot::new().with_keyspace("ks");
        assert!(snapshot.keyspace_exists("ks").unwrap());
        assert!(snapshot
            .table(&TableRef::new("ks", "tbl"))
            .unwrap()
            .is_none());
    }
}
