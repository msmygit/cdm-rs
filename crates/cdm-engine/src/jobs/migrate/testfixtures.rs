//! Schemas, mappings and rows the migrate unit tests share.
//!
//! Everything here is `#[cfg(test)]`: the migrate job's own tests need a binder and a handful of
//! rows, and building them inline in five modules would let the five drift apart.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use cdm_codec::{CodecRegistry, Planner, PlannerOptions};
use cdm_core::{RawCell, Row};
use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
use cdm_cql::statement::{
    Binder, ColumnMapping, MappingOptions, MissingKeyPolicy, StatementOptions, TargetUpsert,
};

/// A column of the shape `cdm-cql`'s introspection produces.
pub(crate) fn column(name: &str, cql_type: &str, kind: ColumnKind, position: i32) -> ColumnMeta {
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

/// `id int PRIMARY KEY, data text` — the simplest table that can prove anything.
pub(crate) fn plain_schema() -> TableSchema {
    TableSchema {
        keyspace: "ks".to_owned(),
        table: "t".to_owned(),
        columns: vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("data", "text", ColumnKind::Regular, -1),
        ],
        is_materialized_view: false,
    }
}

/// `id int, cc text, n counter, PRIMARY KEY (id, cc)`.
pub(crate) fn counter_schema() -> TableSchema {
    TableSchema {
        keyspace: "ks".to_owned(),
        table: "hits".to_owned(),
        columns: vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("cc", "text", ColumnKind::Clustering, 0),
            column("n", "counter", ColumnKind::Regular, -1),
        ],
        is_materialized_view: false,
    }
}

pub(crate) fn plain_mapping() -> ColumnMapping {
    let schema = plain_schema();
    ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap()
}

pub(crate) fn counter_mapping() -> ColumnMapping {
    let schema = counter_schema();
    ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap()
}

pub(crate) fn planner() -> Planner {
    Planner::new(
        CodecRegistry::with_builtins(&[], None).unwrap(),
        PlannerOptions::default(),
    )
}

/// A binder for the plain table, with no transforms.
pub(crate) fn binder() -> Binder {
    let mapping = plain_mapping();
    let statement = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
    Binder::new(
        &mapping,
        statement,
        &planner(),
        MissingKeyPolicy::default(),
        false,
    )
    .unwrap()
}

/// One row of the plain table.
pub(crate) fn row_of(id: i32, data: &str) -> Row {
    Row::new(vec![
        RawCell::new(id.to_be_bytes().to_vec()),
        RawCell::new(data.as_bytes().to_vec()),
    ])
}
