//! The production [`OriginRows`], over the paged range scan of `FEA-060` (`GRD-001`, `ENG-003`).
//!
//! # What was actually missing
//!
//! Not a pager. `cdm_cql::exec` has paged the origin since `FEA-060` landed: it holds the paging
//! state, sets the page size from `perfops.fetch_size`, retries a page in place with `CON-011`'s
//! backoff, and hands each page back as a frame that rows are *lent* out of rather than copied
//! from. What was missing was the fifty lines that let the guardrail job say so — an
//! [`OriginRows`] that turns that scan into the one thing the job consumes, a [`RowSizes`] at a
//! time. Until this file existed the only implementation of the trait was a fixture in
//! `guardrail_it` that read a whole range unpaged, and `cdm guardrail` was unwirable because
//! wiring it would have meant shipping that fixture.
//!
//! ```text
//!   OriginRows::scan(range, fetch_size)
//!        └─ OriginReader::scan  ──►  OwnedRangeScan          one request in flight
//!                                        │
//!             next_row() ◄── RowSizes ◄── page ── RawRow ──  byte_len() per cell
//!                                                            key cells copied, values never
//! ```
//!
//! # Where the memory bound comes from (`NFR-003`)
//!
//! Two claims, and neither is about the size of the range.
//!
//! The scan holds one [`Page`](cdm_cql::exec::Page) at a time, which is at most `fetch_size` rows
//! of frame; the borrow checker enforces it, because a `RawRow` cannot outlive the page it borrows
//! from. This reader then reduces that page to at most `fetch_size` [`RowSizes`] and drops the
//! frame before asking for the next one. A `RowSizes` is a `Vec<usize>` and a primary key, so the
//! buffer is a few hundred bytes per row *regardless of how wide the rows are* — which matters
//! precisely here, since a guardrail run exists to find multi-megabyte columns and would otherwise
//! be at its most memory-hungry exactly when it succeeds.
//!
//! Peak residency is therefore `fetch_size × (one frame row + one RowSizes)` per in-flight range,
//! and `ENG-007` bounds the in-flight ranges. Both factors are configuration.
//!
//! # No value is ever copied (`SEC-002`, `MIG-040`)
//!
//! [`RawCell::byte_len`] is a length taken off the frame, not a decode, so measuring a 40 MB blob
//! costs a `usize`. The only bytes this file copies are the primary-key cells, because the key is
//! what a finding names. Every other cell contributes its length and is dropped with the frame it
//! came in on.
//!
//! # Specification
//!
//! - `GRD-001` — an origin reader and no target, all the way down to the session
//! - `GRD-002` — every column is measured, as its frame length
//! - `ENG-003` — the scan is paged at `perfops.fetch_size` and never materialised
//! - `FEA-060` — the statement paged is the origin range select
//! - `SEC-002`, `MIG-040` — lengths, not values

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{CdmError, ErrorKind, PrimaryKey, RawCell, Side, TokenRange};
use cdm_cql::exec::{OriginReader, OwnedRangeScan};
use cdm_cql::raw::RawRow;
use cdm_cql::schema::TableSchema;
use cdm_cql::statement::OriginProjection;
use cdm_feature::RowSizes;

use super::{OriginRows, RowSizeStream};

/// The guardrail's origin reader: a paged range scan reduced to row sizes (`GRD-001`, `ENG-003`).
///
/// Built once per run and shared by every worker. It holds an
/// [`OriginReader`], which holds an origin session and the range select and
/// nothing else — so the structural argument `GRD-001` rests on is unbroken from the job down to
/// the `Session`, rather than stopping at this crate's boundary.
#[derive(Debug)]
pub struct CqlOriginRows {
    reader: Arc<OriginReader>,
    key_indices: Arc<[usize]>,
}

impl CqlOriginRows {
    /// Builds a reader that takes the row's primary key from the given projection positions.
    #[must_use]
    pub fn new(reader: Arc<OriginReader>, key_indices: impl IntoIterator<Item = usize>) -> Self {
        Self {
            reader,
            key_indices: key_indices.into_iter().collect(),
        }
    }

    /// Builds a reader, resolving the key positions from the schema and the projection.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] if a primary-key column is not in the projection, which would
    /// leave findings identified by a partial key. It is a startup failure rather than a per-row
    /// one for the usual reason: a key that can never be built must not be discovered on the first
    /// row of the first range.
    pub fn resolve(
        reader: Arc<OriginReader>,
        table: &TableSchema,
        projection: &OriginProjection,
    ) -> Result<Self, CdmError> {
        Ok(Self::new(reader, key_indices_of(table, projection)?))
    }

    /// The projection positions the primary key occupies, in key order.
    #[must_use]
    pub fn key_indices(&self) -> &[usize] {
        &self.key_indices
    }
}

#[async_trait]
impl OriginRows for CqlOriginRows {
    async fn scan(
        &self,
        range: TokenRange,
        fetch_size: u32,
    ) -> Result<Box<dyn RowSizeStream>, CdmError> {
        Ok(Box::new(PagedRowSizes {
            scan: self.reader.scan(range, fetch_size),
            key_indices: Arc::clone(&self.key_indices),
            buffer: VecDeque::new(),
        }))
    }
}

/// One range's rows, a page at a time (`ENG-003`, `NFR-003`).
///
/// The buffer holds the sizes of one page and no more: `next_row` only asks for another page once
/// it has handed the previous one out row by row, so a caller that stops early — the cancellation
/// check in `GuardrailJob::process`, say — leaves the rest of the range unread rather than merely
/// unexamined.
struct PagedRowSizes {
    scan: OwnedRangeScan,
    key_indices: Arc<[usize]>,
    buffer: VecDeque<RowSizes>,
}

#[async_trait]
impl RowSizeStream for PagedRowSizes {
    async fn next_row(&mut self) -> Result<Option<RowSizes>, CdmError> {
        loop {
            if let Some(row) = self.buffer.pop_front() {
                return Ok(Some(row));
            }
            // A page can legitimately arrive empty — the server may return no rows and still hand
            // back a paging state — so this loops rather than returning `None` on the first one.
            let Some(page) = self.scan.next_page().await? else {
                return Ok(None);
            };
            self.buffer.reserve(page.len());
            for row in page.rows()? {
                self.buffer.push_back(measure(&row?, &self.key_indices));
            }
            // `page` is dropped here, with the frame every `RawRow` borrowed from. What survives
            // the iteration is one `usize` per column plus the copied key: `NFR-003`'s bound is
            // this line as much as it is the page size.
        }
    }
}

/// Reduces one frame row to its lengths and its key (`GRD-002`, `SEC-002`).
fn measure(row: &RawRow<'_, '_>, key_indices: &[usize]) -> RowSizes {
    let key = primary_key(key_indices, |index| {
        row.cell(index)
            .and_then(|cell| cell.bytes)
            .map(<[u8]>::to_vec)
    });
    RowSizes::new(key, row.cells().iter().map(cdm_cql::raw::RawCell::byte_len))
}

/// Assembles a primary key from the cells at `key_indices`, in key order.
///
/// A position the row does not have yields `NULL` rather than an error: a `NULL` cannot occur in a
/// Cassandra primary key, so a key that contains one describes a row nothing can look up, and a
/// finding identified by a partial key is still more useful to an operator than a failed range
/// would be. Written over an accessor rather than over the row so that the indexing rule of
/// `ERR-004` is discharged by the closure and the ordering is testable without a frame.
fn primary_key(key_indices: &[usize], cell: impl Fn(usize) -> Option<Vec<u8>>) -> PrimaryKey {
    PrimaryKey::new(
        key_indices
            .iter()
            .map(|index| cell(*index).map_or(RawCell::NULL, RawCell::new))
            .collect(),
    )
}

/// The projection positions of the table's primary key, in key order (`SCH-004`).
fn key_indices_of(
    table: &TableSchema,
    projection: &OriginProjection,
) -> Result<Vec<usize>, CdmError> {
    let entries = projection.entries();
    let mut indices = Vec::new();
    for column in table.primary_key() {
        let quoted = column.quoted_name();
        let position = entries.iter().position(|entry| *entry == quoted);
        match position {
            Some(index) => indices.push(index),
            None => {
                return Err(CdmError::new(
                    ErrorKind::SchemaMismatch,
                    format!(
                        "the origin primary-key column `{}` is not in the guardrail projection, so \
                         a finding could not name the row it found (GRD-003, SCH-004)",
                        column.name
                    ),
                )
                .with_context(|c| c.with_side(Side::Origin).with_column(column.name.clone())));
            }
        }
    }
    Ok(indices)
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

    use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta};

    fn column(name: &str, kind: ColumnKind, position: i32) -> ColumnMeta {
        ColumnMeta {
            name: name.to_owned(),
            cql_type: "text".to_owned(),
            kind,
            position,
            clustering_order: if kind == ColumnKind::Clustering {
                ClusteringOrder::Asc
            } else {
                ClusteringOrder::None
            },
        }
    }

    /// `part` (partition), `payload` (regular), `clust` (clustering) — deliberately *not* in key
    /// order, so that a test which passed by accident on a well-ordered table fails here.
    fn table() -> TableSchema {
        TableSchema {
            keyspace: "ks".to_owned(),
            table: "t".to_owned(),
            columns: vec![
                column("part", ColumnKind::PartitionKey, 0),
                column("payload", ColumnKind::Regular, -1),
                column("clust", ColumnKind::Clustering, 0),
            ],
            is_materialized_view: false,
        }
    }

    #[test]
    fn grd_002_the_key_positions_are_where_the_key_sits_in_the_projection_not_in_the_key() {
        let table = table();
        let projection = OriginProjection::new(&table.columns, &[]);
        // `part` is projected first and `clust` third; the key order is (part, clust), so the
        // answer is [0, 2] rather than [0, 1].
        assert_eq!(key_indices_of(&table, &projection).unwrap(), vec![0, 2]);
    }

    #[test]
    fn grd_002_the_virtual_ttl_and_writetime_columns_do_not_move_the_key_positions() {
        let table = table();
        let virtuals = vec!["TTL(payload)".to_owned(), "WRITETIME(payload)".to_owned()];
        let projection = OriginProjection::new(&table.columns, &virtuals);
        assert_eq!(key_indices_of(&table, &projection).unwrap(), vec![0, 2]);
        assert_eq!(projection.width(), 5);
    }

    #[test]
    fn grd_003_a_primary_key_column_missing_from_the_projection_is_a_startup_error() {
        let table = table();
        let projection = OriginProjection::new(&table.columns[1..], &[]);
        let error = key_indices_of(&table, &projection).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::SchemaMismatch);
        assert!(error.to_string().contains("part"), "{error}");
        assert_eq!(error.context().side, Some(Side::Origin));
    }

    #[test]
    fn grd_002_the_primary_key_is_assembled_in_key_order_from_the_projection_positions() {
        let key = primary_key(&[2, 0], |index| match index {
            0 => Some(vec![0xAA]),
            2 => Some(vec![0xBB, 0xCC]),
            _ => None,
        });
        assert_eq!(key.len(), 2);
        assert_eq!(key.values()[0].bytes().unwrap().as_ref(), &[0xBB, 0xCC]);
        assert_eq!(key.values()[1].bytes().unwrap().as_ref(), &[0xAA]);
    }

    #[test]
    fn err_004_a_key_position_the_row_does_not_have_is_null_rather_than_a_panic() {
        let key = primary_key(&[0, 9], |index| (index == 0).then(|| vec![0x01]));
        assert_eq!(key.len(), 2);
        assert!(key.values()[1].bytes().is_none());
    }

    #[test]
    fn grd_001_the_reader_has_no_field_that_could_reach_a_target() {
        // The same sweep `cdm_cql::exec::origin` makes of `OriginReader`, made again of the type
        // the job actually holds: `GRD-001` is only worth anything if it holds at every layer.
        let fields = include_str!("origin.rs")
            .split("pub struct CqlOriginRows {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("the struct definition is in this file");
        assert!(!fields.to_lowercase().contains("target"), "{fields}");
        assert!(!fields.contains("Sink"), "{fields}");
    }
}
