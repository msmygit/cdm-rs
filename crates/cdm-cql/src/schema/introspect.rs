//! Reading table metadata out of `system_schema` (`SCH-001`, `SCH-010`).
//!
//! The driver already exposes cluster metadata, and it is not enough: its `Table` has an unordered
//! column map, no clustering direction, and types rendered in the driver's own vocabulary rather
//! than the schema's. `SCH-001` needs all three, so the queries are issued directly:
//!
//! ```text
//! SELECT keyspace_name FROM system_schema.keyspaces WHERE keyspace_name = ?
//! SELECT view_name     FROM system_schema.views     WHERE keyspace_name = ?
//! SELECT column_name, kind, position, clustering_order, type
//!   FROM system_schema.columns WHERE keyspace_name = ? AND table_name = ?
//! ```
//!
//! `system_schema` exists in Apache Cassandra 3.0 and later, in DSE, and in ScyllaDB, which is the
//! whole supported matrix (`TST-002`). A materialized view's columns live in
//! `system_schema.columns` under the view's name, so one query covers both and the view list only
//! decides which of the two a name is (`SCH-010`).
//!
//! # Identifiers
//!
//! Names go in and come out in their **internal** form — unquoted, case-exact — because that is
//! what `system_schema` stores. Quoting happens at the point a name is written into a statement
//! (`SCH-002`, [`super::identifier`]).

use cdm_core::{CdmError, Side, TableRef};
use scylla::client::session::Session;

use crate::errors::side_error_from;
use crate::schema::identifier;
use crate::schema::table::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};

/// The internal spellings to try for a configured identifier, best first (`SCH-002`).
///
/// A configured name is CQL: `"MyTable"` means the internal `MyTable`, while bare `MyTable` means
/// whatever cqlsh would resolve it to, which is `mytable`. The exact spelling is always tried
/// first — so a cluster holding both `MyTable` and `mytable` is never confused — and the folded
/// one is a fallback that turns "no such table" into the table the operator meant.
pub(crate) fn candidates(name: &str) -> Vec<String> {
    let exact = identifier::unformat(name);
    let folded = identifier::fold(&exact);
    if identifier::is_quoted(name) || folded == exact {
        vec![exact]
    } else {
        vec![exact, folded]
    }
}

/// Whether a keyspace exists (`SCH-001`).
///
/// Distinguishing this from a missing table turns "no such table" into the far more useful "no
/// such keyspace" when an operator mistyped the keyspace.
pub async fn keyspace_exists(
    side: Side,
    session: &Session,
    keyspace: &str,
) -> Result<bool, CdmError> {
    for candidate in candidates(keyspace) {
        let rows = session
            .query_unpaged(
                "SELECT keyspace_name FROM system_schema.keyspaces WHERE keyspace_name = ?",
                (candidate,),
            )
            .await
            .map_err(|e| schema_error(side, "system_schema.keyspaces", e))?
            .into_rows_result()
            .map_err(|e| schema_error(side, "system_schema.keyspaces", e))?;
        if rows.rows_num() > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The names of every keyspace in the cluster.
pub async fn keyspaces(side: Side, session: &Session) -> Result<Vec<String>, CdmError> {
    let rows = session
        .query_unpaged("SELECT keyspace_name FROM system_schema.keyspaces", &[])
        .await
        .map_err(|e| schema_error(side, "system_schema.keyspaces", e))?
        .into_rows_result()
        .map_err(|e| schema_error(side, "system_schema.keyspaces", e))?;
    let mut names = Vec::new();
    for row in rows
        .rows::<(String,)>()
        .map_err(|e| schema_error(side, "system_schema.keyspaces", e))?
    {
        let (name,) = row.map_err(|e| schema_error(side, "system_schema.keyspaces", e))?;
        names.push(name);
    }
    Ok(names)
}

/// Whether a name is a materialized view rather than a table (`SCH-010`).
///
/// `table` is a configured, CQL-spelled reference, so the same exact-then-folded resolution as
/// [`fetch_table`] applies.
pub async fn is_materialized_view(
    side: Side,
    session: &Session,
    table: &TableRef,
) -> Result<bool, CdmError> {
    for keyspace in candidates(table.keyspace()) {
        for name in candidates(table.table()) {
            if is_materialized_view_exact(side, session, &TableRef::new(&keyspace, &name)).await? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Whether an already-resolved internal name is a materialized view (`SCH-010`).
async fn is_materialized_view_exact(
    side: Side,
    session: &Session,
    table: &TableRef,
) -> Result<bool, CdmError> {
    let rows = session
        .query_unpaged(
            "SELECT view_name FROM system_schema.views WHERE keyspace_name = ? AND view_name = ?",
            (table.keyspace().to_owned(), table.table().to_owned()),
        )
        .await
        .map_err(|e| schema_error(side, "system_schema.views", e))?
        .into_rows_result()
        .map_err(|e| schema_error(side, "system_schema.views", e))?;
    Ok(rows.rows_num() > 0)
}

/// Reads one table's metadata, or reports that it does not exist (`SCH-001`).
pub async fn fetch_table(
    side: Side,
    session: &Session,
    table: &TableRef,
) -> Result<Option<TableSchema>, CdmError> {
    for keyspace in candidates(table.keyspace()) {
        for name in candidates(table.table()) {
            let columns = fetch_columns(side, session, &keyspace, &name).await?;
            if columns.is_empty() {
                continue;
            }
            let resolved = TableRef::new(&keyspace, &name);
            return Ok(Some(TableSchema {
                keyspace: keyspace.clone(),
                table: name,
                columns: order_columns(columns),
                is_materialized_view: is_materialized_view_exact(side, session, &resolved).await?,
            }));
        }
    }
    Ok(None)
}

/// The columns of one exactly-named table or view.
async fn fetch_columns(
    side: Side,
    session: &Session,
    keyspace: &str,
    table: &str,
) -> Result<Vec<ColumnMeta>, CdmError> {
    let rows = session
        .query_unpaged(
            "SELECT column_name, kind, position, clustering_order, type \
             FROM system_schema.columns WHERE keyspace_name = ? AND table_name = ?",
            (keyspace.to_owned(), table.to_owned()),
        )
        .await
        .map_err(|e| schema_error(side, "system_schema.columns", e))?
        .into_rows_result()
        .map_err(|e| schema_error(side, "system_schema.columns", e))?;

    let mut columns = Vec::new();
    for row in rows
        .rows::<(String, String, i32, Option<String>, String)>()
        .map_err(|e| schema_error(side, "system_schema.columns", e))?
    {
        let (column_name, kind, position, clustering_order, cql_type) =
            row.map_err(|e| schema_error(side, "system_schema.columns", e))?;
        columns.push(ColumnMeta {
            name: column_name,
            cql_type,
            kind: ColumnKind::parse(&kind),
            position,
            clustering_order: clustering_order
                .as_deref()
                .map_or(ClusteringOrder::None, ClusteringOrder::parse),
        });
    }
    Ok(columns)
}

/// Reads a table and refuses a materialized view (`SCH-010`).
///
/// This is the target-side entry point: `SCH-010` makes a view an error there, while reading
/// *from* a view is legitimate and stays available through [`fetch_table`].
pub async fn fetch_target_table(
    session: &Session,
    table: &TableRef,
) -> Result<Option<TableSchema>, CdmError> {
    let Some(schema) = fetch_table(Side::Target, session, table).await? else {
        return Ok(None);
    };
    schema.reject_if_materialized_view(Side::Target)?;
    Ok(Some(schema))
}

/// Puts columns in the order the rest of cdm-rs expects: partition key, clustering, then the rest.
///
/// `system_schema.columns` is clustered by column name, so it arrives alphabetically; the key
/// order lives in the `position` column. Sorting once here means every consumer can rely on
/// `columns` being in a meaningful order (`SCH-001`).
fn order_columns(mut columns: Vec<ColumnMeta>) -> Vec<ColumnMeta> {
    columns.sort_by(|a, b| {
        kind_rank(a.kind)
            .cmp(&kind_rank(b.kind))
            .then_with(|| a.position.cmp(&b.position))
            .then_with(|| a.name.cmp(&b.name))
    });
    columns
}

const fn kind_rank(kind: ColumnKind) -> u8 {
    match kind {
        ColumnKind::PartitionKey => 0,
        ColumnKind::Clustering => 1,
        ColumnKind::Static => 2,
        ColumnKind::Regular => 3,
    }
}

fn schema_error(side: Side, what: &str, cause: impl Into<crate::errors::Cause>) -> CdmError {
    side_error_from(
        cdm_core::ErrorKind::SchemaMismatch,
        side,
        format!("cannot read {what}"),
        cause,
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
    use crate::schema::table::tests::column;

    #[test]
    fn sch_001_columns_are_ordered_key_first_then_by_position() {
        // The order `system_schema.columns` returns: alphabetical by column name.
        let ordered = order_columns(vec![
            column("beta", "text", ColumnKind::Regular, -1),
            column("c2", "text", ColumnKind::Clustering, 1),
            column("alpha", "text", ColumnKind::Regular, -1),
            column("pk2", "int", ColumnKind::PartitionKey, 1),
            column("stat", "text", ColumnKind::Static, -1),
            column("c1", "int", ColumnKind::Clustering, 0),
            column("pk1", "uuid", ColumnKind::PartitionKey, 0),
        ]);
        let names: Vec<&str> = ordered.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["pk1", "pk2", "c1", "c2", "stat", "alpha", "beta"]
        );
    }

    #[test]
    fn sch_002_a_configured_name_resolves_exactly_first_then_folded() {
        // A quoted name is exact and nothing else: `"MyTable"` is not `mytable`.
        assert_eq!(candidates("\"MyTable\""), vec!["MyTable".to_owned()]);
        // A bare mixed-case name is tried as written — that is what `system_schema` stores for a
        // table created quoted — and then as cqlsh would resolve it.
        assert_eq!(
            candidates("MyTable"),
            vec!["MyTable".to_owned(), "mytable".to_owned()]
        );
        // An already-lowercase name has only one spelling.
        assert_eq!(candidates("mytable"), vec!["mytable".to_owned()]);
    }

    #[test]
    fn sch_001_the_kind_ranking_is_total() {
        let mut ranks = [
            kind_rank(ColumnKind::PartitionKey),
            kind_rank(ColumnKind::Clustering),
            kind_rank(ColumnKind::Static),
            kind_rank(ColumnKind::Regular),
        ];
        let unsorted = ranks;
        ranks.sort_unstable();
        assert_eq!(ranks, unsorted, "the ranking must already be in order");
    }
}
