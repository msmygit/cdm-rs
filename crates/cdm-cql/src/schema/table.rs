//! The table metadata cdm-rs works from (`SCH-001`, `SCH-010`).
//!
//! Deliberately *not* the driver's [`Table`](scylla::cluster::metadata::Table): that type carries
//! a `HashMap<String, Column>` with no ordering, no clustering direction and a `ColumnType` whose
//! rendering is the driver's rather than `system_schema`'s. `SCH-001` asks for columns, CQL types
//! *as the schema spells them*, partition key order, clustering order **and direction**, and
//! whether the table is a counter table — so the metadata is read from `system_schema` directly
//! (see [`super::introspect`]) into the shape below.

use cdm_config::{ColumnDescription, TableDescription};
use cdm_core::{CdmError, ErrorKind, Side, TableRef};

use crate::schema::identifier;

/// What role a column plays in its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ColumnKind {
    /// Part of the partition key.
    PartitionKey,
    /// A clustering column.
    Clustering,
    /// A static column: one value per partition.
    Static,
    /// An ordinary column.
    Regular,
}

impl ColumnKind {
    /// Parses the `kind` column of `system_schema.columns`.
    pub fn parse(kind: &str) -> Self {
        match kind {
            "partition_key" => Self::PartitionKey,
            "clustering" => Self::Clustering,
            "static" => Self::Static,
            _ => Self::Regular,
        }
    }

    /// Whether the column is part of the primary key.
    pub fn is_key(self) -> bool {
        matches!(self, Self::PartitionKey | Self::Clustering)
    }
}

/// The direction a clustering column is stored in (`SCH-001`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClusteringOrder {
    /// Ascending, the default.
    Asc,
    /// Descending.
    Desc,
    /// Not a clustering column.
    None,
}

impl ClusteringOrder {
    /// Parses the `clustering_order` column of `system_schema.columns`.
    pub fn parse(order: &str) -> Self {
        match order.to_ascii_lowercase().as_str() {
            "asc" => Self::Asc,
            "desc" => Self::Desc,
            _ => Self::None,
        }
    }

    /// The direction as CQL writes it, for `WITH CLUSTERING ORDER BY`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
            Self::None => "",
        }
    }
}

/// One column of a table (`SCH-001`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMeta {
    /// The column's name, in its internal (unquoted) form.
    pub name: String,
    /// The CQL type, spelled exactly as `system_schema.columns.type` spells it — including
    /// `frozen<…>`, `vector<float, 3>` and UDT names.
    pub cql_type: String,
    /// The column's role.
    pub kind: ColumnKind,
    /// Its position within the partition or clustering key; `-1` for other columns.
    pub position: i32,
    /// The clustering direction, for clustering columns.
    pub clustering_order: ClusteringOrder,
}

impl ColumnMeta {
    /// The name as it must be written in CQL (`SCH-002`).
    pub fn quoted_name(&self) -> String {
        identifier::format(&self.name)
    }

    /// Whether this is a counter column, which makes the table a counter table (`SCH-005`).
    pub fn is_counter(&self) -> bool {
        self.unfrozen().eq_ignore_ascii_case("counter")
    }

    /// Whether the column is a list, set or map, frozen or not.
    pub fn is_collection(&self) -> bool {
        let ty = self.unfrozen().to_ascii_lowercase();
        ["list<", "set<", "map<"].iter().any(|p| ty.starts_with(p))
    }

    /// Whether the declared type is wrapped in `frozen<…>`.
    pub fn is_frozen(&self) -> bool {
        self.cql_type
            .trim()
            .to_ascii_lowercase()
            .starts_with("frozen<")
    }

    /// Whether the column is a tuple.
    pub fn is_tuple(&self) -> bool {
        self.unfrozen().to_ascii_lowercase().starts_with("tuple<")
    }

    /// Whether the column is a `vector<t, n>` (`CDC-004`).
    pub fn is_vector(&self) -> bool {
        self.unfrozen().to_ascii_lowercase().starts_with("vector<")
    }

    /// The type with one layer of `frozen<>` removed.
    pub fn unfrozen(&self) -> &str {
        let ty = self.cql_type.trim();
        if ty.len() > 8 && ty.to_ascii_lowercase().starts_with("frozen<") && ty.ends_with('>') {
            return ty.get(7..ty.len() - 1).unwrap_or(ty).trim();
        }
        ty
    }
}

/// A table, or a materialized view, as `system_schema` describes it (`SCH-001`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSchema {
    /// The keyspace, internal form.
    pub keyspace: String,
    /// The table, internal form.
    pub table: String,
    /// Every column, partition key first (in key order), then clustering columns (in key order),
    /// then the rest in `system_schema` order.
    pub columns: Vec<ColumnMeta>,
    /// Whether this is a materialized view rather than a table (`SCH-010`).
    pub is_materialized_view: bool,
}

impl TableSchema {
    /// The table as a [`TableRef`].
    pub fn table_ref(&self) -> TableRef {
        TableRef::new(self.keyspace.clone(), self.table.clone())
    }

    /// The `keyspace.table` reference, quoted as CQL requires (`SCH-002`).
    pub fn quoted_name(&self) -> String {
        identifier::qualified(&self.keyspace, &self.table)
    }

    /// The named column, matched on the internal form.
    pub fn column(&self, name: &str) -> Option<&ColumnMeta> {
        self.columns.iter().find(|column| column.name == name)
    }

    /// The partition key columns, in key order.
    pub fn partition_key(&self) -> Vec<&ColumnMeta> {
        self.of_kind(ColumnKind::PartitionKey)
    }

    /// The clustering columns, in key order, each carrying its direction.
    pub fn clustering_columns(&self) -> Vec<&ColumnMeta> {
        self.of_kind(ColumnKind::Clustering)
    }

    /// The primary key columns: partition key then clustering columns.
    pub fn primary_key(&self) -> Vec<&ColumnMeta> {
        let mut key = self.partition_key();
        key.extend(self.clustering_columns());
        key
    }

    /// The columns that are not part of the primary key.
    pub fn regular_columns(&self) -> Vec<&ColumnMeta> {
        self.columns
            .iter()
            .filter(|column| !column.kind.is_key())
            .collect()
    }

    /// Whether any column is a counter (`SCH-005`, `MIG-030`).
    pub fn is_counter_table(&self) -> bool {
        self.columns.iter().any(ColumnMeta::is_counter)
    }

    /// Rejects a materialized view used as a target (`SCH-010`).
    ///
    /// A view is maintained by Cassandra from its base table. Writing to one is not merely
    /// unsupported — the server refuses it — so cdm-rs says why while it is still a
    /// configuration problem.
    pub fn reject_if_materialized_view(&self, side: Side) -> Result<(), CdmError> {
        if !self.is_materialized_view {
            return Ok(());
        }
        Err(CdmError::new(
            ErrorKind::SchemaMismatch,
            format!(
                "{} is a materialized view, which cannot be a migration target: Cassandra \
                 maintains a view from its base table and rejects writes to it. Set \
                 schema.target.keyspace_table to the base table instead (SCH-010).",
                self.quoted_name()
            ),
        )
        .with_context(|c| c.with_side(side).with_table(self.table_ref())))
    }

    /// The description Tier-3 configuration validation consumes (`CFG-020`).
    pub fn to_description(&self) -> TableDescription {
        let columns = self
            .columns
            .iter()
            .map(|column| {
                let mut description = ColumnDescription::new(&column.name, &column.cql_type);
                description.partition_key = column.kind == ColumnKind::PartitionKey;
                description.clustering_key = column.kind == ColumnKind::Clustering;
                description.is_static = column.kind == ColumnKind::Static;
                description
            })
            .collect();
        TableDescription::new(self.table_ref(), columns)
    }

    fn of_kind(&self, kind: ColumnKind) -> Vec<&ColumnMeta> {
        let mut columns: Vec<&ColumnMeta> = self
            .columns
            .iter()
            .filter(|column| column.kind == kind)
            .collect();
        columns.sort_by_key(|column| column.position);
        columns
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
pub(crate) mod tests {
    use super::*;

    pub(crate) fn column(
        name: &str,
        cql_type: &str,
        kind: ColumnKind,
        position: i32,
    ) -> ColumnMeta {
        ColumnMeta {
            name: name.to_owned(),
            cql_type: cql_type.to_owned(),
            kind,
            position,
            clustering_order: if kind == ColumnKind::Clustering {
                ClusteringOrder::Asc
            } else {
                ClusteringOrder::None
            },
        }
    }

    pub(crate) fn table() -> TableSchema {
        TableSchema {
            keyspace: "ks".to_owned(),
            table: "tbl".to_owned(),
            columns: vec![
                column("data", "text", ColumnKind::Regular, -1),
                column("c2", "text", ColumnKind::Clustering, 1),
                column("pk2", "int", ColumnKind::PartitionKey, 1),
                column("c1", "timestamp", ColumnKind::Clustering, 0),
                column("pk1", "uuid", ColumnKind::PartitionKey, 0),
            ],
            is_materialized_view: false,
        }
    }

    #[test]
    fn sch_001_key_columns_come_back_in_key_order() {
        let table = table();
        let partition: Vec<&str> = table
            .partition_key()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(partition, vec!["pk1", "pk2"]);

        let clustering: Vec<&str> = table
            .clustering_columns()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(clustering, vec!["c1", "c2"]);

        let primary: Vec<&str> = table
            .primary_key()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(primary, vec!["pk1", "pk2", "c1", "c2"]);

        let regular: Vec<&str> = table
            .regular_columns()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(regular, vec!["data"]);
    }

    #[test]
    fn sch_001_clustering_direction_is_carried() {
        let mut table = table();
        table.columns[1].clustering_order = ClusteringOrder::Desc;
        assert_eq!(
            table.column("c2").unwrap().clustering_order,
            ClusteringOrder::Desc
        );
        assert_eq!(ClusteringOrder::parse("DESC"), ClusteringOrder::Desc);
        assert_eq!(ClusteringOrder::parse("asc"), ClusteringOrder::Asc);
        assert_eq!(ClusteringOrder::parse("none"), ClusteringOrder::None);
        assert_eq!(ClusteringOrder::Desc.as_str(), "DESC");
        assert_eq!(ClusteringOrder::None.as_str(), "");
    }

    #[test]
    fn sch_001_column_kinds_are_parsed_from_system_schema() {
        assert_eq!(ColumnKind::parse("partition_key"), ColumnKind::PartitionKey);
        assert_eq!(ColumnKind::parse("clustering"), ColumnKind::Clustering);
        assert_eq!(ColumnKind::parse("static"), ColumnKind::Static);
        assert_eq!(ColumnKind::parse("regular"), ColumnKind::Regular);
        assert!(ColumnKind::PartitionKey.is_key());
        assert!(ColumnKind::Clustering.is_key());
        assert!(!ColumnKind::Static.is_key());
    }

    #[test]
    fn sch_001_the_type_taxonomy_is_read_from_the_declared_type() {
        let cases = [
            ("text", false, false, false, false),
            ("counter", false, false, false, false),
            ("list<text>", true, false, false, false),
            ("frozen<list<text>>", true, true, false, false),
            ("map<text, frozen<address>>", true, false, false, false),
            ("frozen<tuple<int, text>>", false, true, true, false),
            ("vector<float, 3>", false, false, false, true),
        ];
        for (ty, collection, frozen, tuple, vector) in cases {
            let column = column("c", ty, ColumnKind::Regular, -1);
            assert_eq!(column.is_collection(), collection, "{ty}");
            assert_eq!(column.is_frozen(), frozen, "{ty}");
            assert_eq!(column.is_tuple(), tuple, "{ty}");
            assert_eq!(column.is_vector(), vector, "{ty}");
        }
    }

    #[test]
    fn sch_005_a_counter_table_is_detected() {
        let mut table = table();
        assert!(!table.is_counter_table());
        table
            .columns
            .push(column("hits", "counter", ColumnKind::Regular, -1));
        assert!(table.is_counter_table());
    }

    #[test]
    fn sch_002_the_table_name_is_quoted_when_it_must_be() {
        let mut table = table();
        assert_eq!(table.quoted_name(), "ks.tbl");
        table.table = "Select".to_owned();
        assert_eq!(table.quoted_name(), "ks.\"Select\"");
        assert_eq!(
            column("My Col", "text", ColumnKind::Regular, -1).quoted_name(),
            "\"My Col\""
        );
    }

    #[test]
    fn sch_010_a_materialized_view_is_rejected_as_a_target() {
        let mut table = table();
        table.is_materialized_view = true;
        let err = table.reject_if_materialized_view(Side::Target).unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::SchemaMismatch);
        assert!(err.to_string().contains("materialized view"), "{err}");
        assert!(err.to_string().contains("base table"), "{err}");
        assert!(err.to_string().contains("target"), "{err}");
    }

    #[test]
    fn sch_010_a_table_is_not_rejected() {
        assert!(table().reject_if_materialized_view(Side::Target).is_ok());
    }

    #[test]
    fn sch_001_the_tier3_description_carries_the_key_flags() {
        let mut table = table();
        table
            .columns
            .push(column("s", "text", ColumnKind::Static, -1));
        let description = table.to_description();
        assert_eq!(description.table, TableRef::new("ks", "tbl"));
        assert!(description.column("pk1").unwrap().partition_key);
        assert!(description.column("c1").unwrap().clustering_key);
        assert!(description.column("s").unwrap().is_static);
        assert!(!description.column("data").unwrap().is_key());
        assert_eq!(description.column("data").unwrap().cql_type(), "text");
    }
}
