//! The schema facts a feature needs in order to validate and to resolve itself.
//!
//! # Why not `SchemaPair`
//!
//! `cdm-core`'s [`TableView`] lists a table's columns and their types, which is enough for a plugin
//! to decide whether it applies. The features in this crate need three facts it does not carry:
//! which columns form the primary key (`FEA-012`, `FEA-022`, `FEA-033`), which columns are counters
//! (`FEA-045`), and the parsed type tree rather than the `system_schema` spelling (`FEA-011`,
//! `FEA-021`, `FEA-041`). [`TableFacts`] adds exactly those and nothing else.
//!
//! It is a placeholder for `cdm-cql`'s `TableSchema` (`SCH-001`), in the same sense and for the same
//! reason as the placeholders in `cdm-core`: a feature that took the driver's metadata directly
//! could not be unit-tested without a cluster, which SPEC §11 requires of every feature here.

use cdm_codec::CqlTypeInfo;
use cdm_core::{CdmError, ColumnRef, ErrorKind, TableRef, TableView};

/// One column, with its type already parsed and its role in the table known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnFacts {
    name: String,
    cql_type: CqlTypeInfo,
    key: bool,
}

impl ColumnFacts {
    /// Declares a column.
    pub fn new(name: impl Into<String>, cql_type: CqlTypeInfo, key: bool) -> Self {
        Self {
            name: name.into(),
            cql_type,
            key,
        }
    }

    /// The column name, unquoted.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The parsed column type.
    pub const fn cql_type(&self) -> &CqlTypeInfo {
        &self.cql_type
    }

    /// Whether the column is part of the partition or clustering key.
    pub const fn is_key(&self) -> bool {
        self.key
    }

    /// Whether the column can carry a `WRITETIME` or `TTL` (`FEA-041`).
    ///
    /// Key columns never can — Cassandra rejects the function outright. Primitives, tuples and
    /// frozen values always can. An unfrozen collection can only when the operator opts in with
    /// `schema.ttl_writetime.use_collections`, because the function then returns a *list* of
    /// per-element values (`FEA-043`) whose cost is proportional to the collection's size.
    pub fn can_carry_ttl_or_writetime(&self, use_collections: bool) -> bool {
        if self.key {
            return false;
        }
        if self.cql_type.is_primitive() || matches!(self.cql_type, CqlTypeInfo::Tuple { .. }) {
            return true;
        }
        if self.cql_type.is_frozen() {
            return true;
        }
        use_collections && self.cql_type.is_collection()
    }
}

/// One side's table, as the features see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableFacts {
    table: TableRef,
    columns: Vec<ColumnFacts>,
}

impl TableFacts {
    /// Declares a table from columns whose types are already parsed.
    pub fn new(table: TableRef, columns: Vec<ColumnFacts>) -> Self {
        Self { table, columns }
    }

    /// Parses a [`TableView`], naming which of its columns form the primary key.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::SchemaMismatch`] when a column's `system_schema` type does not parse, or
    /// when a named key column is not on the table — both of which mean the caller and the cluster
    /// disagree about the table, and no feature can be validated against it.
    pub fn from_view(view: &TableView, primary_key: &[&str]) -> Result<Self, CdmError> {
        let mut columns = Vec::with_capacity(view.columns.len());
        for column in &view.columns {
            let cql_type = CqlTypeInfo::parse(column.cql_type()).map_err(|e| {
                e.with_context(|c| {
                    c.with_table(view.table.clone())
                        .with_column(column.name().to_owned())
                })
            })?;
            columns.push(ColumnFacts::new(
                column.name(),
                cql_type,
                primary_key.contains(&column.name()),
            ));
        }
        for key in primary_key {
            if !columns.iter().any(|column| column.name() == *key) {
                return Err(CdmError::new(
                    ErrorKind::SchemaMismatch,
                    format!("primary-key column `{key}` is not on table {}", view.table),
                )
                .with_context(|c| c.with_table(view.table.clone())));
            }
        }
        Ok(Self::new(view.table.clone(), columns))
    }

    /// The table this describes.
    pub const fn table(&self) -> &TableRef {
        &self.table
    }

    /// Its columns, in schema order.
    pub fn columns(&self) -> &[ColumnFacts] {
        &self.columns
    }

    /// The named column, if the table has one.
    pub fn column(&self, name: &str) -> Option<&ColumnFacts> {
        self.columns.iter().find(|column| column.name() == name)
    }

    /// The position of the named column in schema order, which is also its position in a projection
    /// that selects every column (`ARCHITECTURE.md` §5.5).
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|column| column.name() == name)
    }

    /// The primary-key column names, in schema order.
    pub fn primary_key(&self) -> Vec<&str> {
        self.columns
            .iter()
            .filter(|column| column.is_key())
            .map(ColumnFacts::name)
            .collect()
    }

    /// Whether the table has a `counter` column, which disables TTL and writetime entirely
    /// (`FEA-045`) and forbids batching (`MIG-032`).
    pub fn is_counter_table(&self) -> bool {
        self.columns
            .iter()
            .any(|column| matches!(column.cql_type(), CqlTypeInfo::Counter))
    }

    /// The columns that can supply a `WRITETIME` or `TTL` (`FEA-041`), in schema order.
    pub fn ttl_writetime_columns(&self, use_collections: bool) -> Vec<&str> {
        self.columns
            .iter()
            .filter(|column| column.can_carry_ttl_or_writetime(use_collections))
            .map(ColumnFacts::name)
            .collect()
    }
}

/// The origin and target tables a run reconciles, as the features see them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureSchema {
    /// The origin table.
    pub origin: TableFacts,
    /// The target table.
    pub target: TableFacts,
}

impl FeatureSchema {
    /// Pairs two tables.
    pub const fn new(origin: TableFacts, target: TableFacts) -> Self {
        Self { origin, target }
    }
}

/// A [`TableView`] built from `(name, type)` pairs, for callers that already have them.
///
/// Exists so that a test — and `cdm-cql`'s introspection, which produces the same shape — can build
/// a view without hand-rolling the `Vec<ColumnRef>` every time.
pub fn table_view(table: TableRef, columns: &[(&str, &str)]) -> TableView {
    TableView::new(
        table,
        columns
            .iter()
            .map(|(name, cql_type)| ColumnRef::new(*name, *cql_type))
            .collect(),
    )
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

    fn facts() -> TableFacts {
        TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "tbl"),
                &[
                    ("id", "int"),
                    ("cc", "text"),
                    ("v", "text"),
                    ("t", "tuple<int, text>"),
                    ("fz", "frozen<list<int>>"),
                    ("l", "list<int>"),
                ],
            ),
            &["id", "cc"],
        )
        .unwrap()
    }

    #[test]
    fn fea_041_key_columns_can_never_carry_a_ttl_or_writetime() {
        let facts = facts();
        assert_eq!(facts.primary_key(), vec!["id", "cc"]);
        assert!(!facts.column("id").unwrap().can_carry_ttl_or_writetime(true));
        assert!(!facts.column("cc").unwrap().can_carry_ttl_or_writetime(true));
    }

    #[test]
    fn fea_041_primitives_tuples_and_frozen_values_are_eligible_collections_only_on_request() {
        let facts = facts();
        assert_eq!(facts.ttl_writetime_columns(false), vec!["v", "t", "fz"]);
        assert_eq!(
            facts.ttl_writetime_columns(true),
            vec!["v", "t", "fz", "l"],
            "an unfrozen collection joins the set only when use_collections is on"
        );
    }

    #[test]
    fn fea_045_a_counter_column_marks_the_table_as_a_counter_table() {
        assert!(!facts().is_counter_table());
        let counters = TableFacts::from_view(
            &table_view(TableRef::new("ks", "c"), &[("id", "int"), ("n", "counter")]),
            &["id"],
        )
        .unwrap();
        assert!(counters.is_counter_table());
    }

    #[test]
    fn fea_041_column_lookup_is_positional_and_by_name() {
        let facts = facts();
        assert_eq!(facts.index_of("v"), Some(2));
        assert_eq!(facts.index_of("nope"), None);
        assert!(facts.column("nope").is_none());
        assert_eq!(facts.table().to_string(), "ks.tbl");
        assert_eq!(facts.columns().len(), 6);
    }

    #[test]
    fn fea_041_an_unparsable_type_or_a_phantom_key_column_is_a_schema_mismatch() {
        let bad_type = TableFacts::from_view(
            &table_view(TableRef::new("ks", "t"), &[("id", "list<")]),
            &[],
        )
        .unwrap_err();
        assert_eq!(bad_type.kind(), ErrorKind::SchemaMismatch);

        let phantom = TableFacts::from_view(
            &table_view(TableRef::new("ks", "t"), &[("id", "int")]),
            &["nope"],
        )
        .unwrap_err();
        assert_eq!(phantom.kind(), ErrorKind::SchemaMismatch);
        assert!(phantom.message().contains("nope"));
    }

    #[test]
    fn fea_020_a_schema_pairs_two_sides() {
        let schema = FeatureSchema::new(facts(), facts());
        assert_eq!(schema.origin, schema.target);
        assert_eq!(
            ColumnFacts::new("x", CqlTypeInfo::Int, false).cql_type(),
            &CqlTypeInfo::Int
        );
        assert!(!ColumnFacts::new("x", CqlTypeInfo::Int, false).is_key());
    }
}
