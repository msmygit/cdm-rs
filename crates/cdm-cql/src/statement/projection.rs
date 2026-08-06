//! The origin `SELECT` list, real columns and virtual ones alike (`SCH-004`, `SCH-007`).
//!
//! # Why virtual columns are part of the projection
//!
//! `TTL(data)` and `WRITETIME(data)` are not columns of the table, but they occupy positions in the
//! result row exactly as columns do, and every consumer downstream addresses cells by index rather
//! than by name (`ARCHITECTURE.md` §5.5). Modelling them as *appended* projection entries — which
//! is what Java's `CqlTable.extendColumns` does — means the TTL/writetime feature can say "my
//! values are at positions 4 and 5" once at startup and never look a name up again.
//!
//! The append order is fixed and matches Java: the table's own columns, then every `TTL(…)`, then
//! every `WRITETIME(…)`. `cdm-feature`'s `WritetimeTtlPlan::resolve` computes its indices on
//! exactly that assumption, so changing the order here would silently mis-read every row's
//! writetime rather than fail.

use crate::schema::{identifier, ColumnMeta};

use super::join;

/// The origin projection: the columns the range scan reads, in row order (`SCH-004`, `SCH-007`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginProjection {
    entries: Vec<String>,
    base_columns: usize,
}

impl OriginProjection {
    /// Builds a projection from the mapped origin columns and any virtual expressions appended
    /// after them.
    ///
    /// `virtual_expressions` are taken verbatim — they are CQL source such as `TTL(data)`, already
    /// produced with the right quoting by whoever owns the feature. [`identifier::format`] passes a
    /// function form through untouched, so running them through the same formatter as real columns
    /// is safe and keeps the two paths from drifting.
    pub fn new(columns: &[ColumnMeta], virtual_expressions: &[String]) -> Self {
        let mut entries: Vec<String> = columns.iter().map(ColumnMeta::quoted_name).collect();
        let base_columns = entries.len();
        entries.extend(
            virtual_expressions
                .iter()
                .map(|expression| identifier::format(expression)),
        );
        Self {
            entries,
            base_columns,
        }
    }

    /// The projection as it appears after `SELECT`, comma-joined without spaces.
    pub fn cql(&self) -> String {
        join(&self.entries)
    }

    /// The projection entries, in row order.
    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// How many cells a row from this projection has.
    pub fn width(&self) -> usize {
        self.entries.len()
    }

    /// Whether the projection selects nothing, which cannot produce a usable row.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many of the entries are real table columns; the rest are virtual (`SCH-007`).
    pub const fn base_columns(&self) -> usize {
        self.base_columns
    }

    /// The row position of the `n`th virtual expression (`SCH-007`).
    ///
    /// The identity `virtual_index(n) == base_columns() + n` is the contract
    /// `cdm-feature`'s TTL/writetime plan resolves its own indices against; it is asserted rather
    /// than merely documented, in `sch_007_virtual_columns_are_addressable_by_index`.
    pub fn virtual_index(&self, nth: usize) -> Option<usize> {
        let index = self.base_columns.checked_add(nth)?;
        (index < self.entries.len()).then_some(index)
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
    use crate::schema::table::tests::column;
    use crate::schema::ColumnKind;

    fn columns() -> Vec<ColumnMeta> {
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("data", "text", ColumnKind::Regular, -1),
        ]
    }

    #[test]
    fn sch_007_virtual_columns_are_appended_and_addressable_by_index() {
        let virtuals = vec!["TTL(data)".to_owned(), "WRITETIME(data)".to_owned()];
        let projection = OriginProjection::new(&columns(), &virtuals);

        assert_eq!(projection.cql(), "id,data,TTL(data),WRITETIME(data)");
        assert_eq!(projection.width(), 4);
        assert_eq!(projection.base_columns(), 2);
        assert_eq!(projection.virtual_index(0), Some(2));
        assert_eq!(projection.virtual_index(1), Some(3));
        assert_eq!(projection.virtual_index(2), None);
        assert!(!projection.is_empty());
    }

    #[test]
    fn sch_007_a_virtual_column_over_a_quoted_column_keeps_its_quoting() {
        let mut columns = columns();
        columns.push(column("My Col", "text", ColumnKind::Regular, -1));
        let virtuals = vec!["WRITETIME(\"My Col\")".to_owned()];
        let projection = OriginProjection::new(&columns, &virtuals);
        assert_eq!(
            projection.cql(),
            "id,data,\"My Col\",WRITETIME(\"My Col\")",
            "a function form passes through the formatter untouched (SCH-002)"
        );
    }

    #[test]
    fn sch_004_a_projection_with_no_virtual_columns_is_just_the_columns() {
        let projection = OriginProjection::new(&columns(), &[]);
        assert_eq!(projection.cql(), "id,data");
        assert_eq!(projection.entries(), ["id", "data"]);
        assert_eq!(projection.virtual_index(0), None);
        assert!(OriginProjection::new(&[], &[]).is_empty());
    }
}
