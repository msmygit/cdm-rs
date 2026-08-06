//! Counter tables: the delta, the row that carries it, and why none of it is retried
//! (`MIG-030`..`MIG-032`, `CON-012`, `SCH-005`).
//!
//! # A counter migration is not a copy
//!
//! `UPDATE … SET c = c + ?` adds. It cannot assign. So migrating a counter column whose origin
//! value is `500` into a target that already holds `200` must write `+300`, not `+500` — which
//! means reading the target first, per row, immediately before the write (`MIG-031`). That read is
//! the reason counter migration is roughly an order of magnitude slower than an ordinary one, and
//! there is no way around it: the arithmetic is the storage engine's, not ours.
//!
//! # Everything about this is at-most-once
//!
//! The delta is only correct relative to the target value that was read a moment ago. If the write
//! is retried, or speculatively executed, or batched into something the coordinator retries, the
//! delta is applied twice and the counter is permanently wrong — silently, because a counter
//! carries no writetime and no version that could reveal it.
//!
//! cdm-rs therefore refuses at four independent levels, none of which is a runtime `if`:
//!
//! | Level | Mechanism | Requirement |
//! |---|---|---|
//! | Binding | a counter table produces [`CounterWrite`](cdm_cql::statement::CounterWrite), which does not implement the sealed `Idempotent` | `MIG-032` |
//! | Statement | the prepared upsert is marked non-idempotent, so the driver's retry policy declines it | `CON-012` |
//! | Execution | `TargetWriter::write_counter` has no retry loop | `CON-012` |
//! | Batching | `perfops.batch_size` is coerced to 1 | `MIG-021` |
//!
//! and a fifth outside this crate: `cdm-track`'s `RerunPolicy::rerunnable_statuses` returns
//! `[NotStarted]` for a counter table with a writing job, so a resume never replays a range that
//! may have half-applied (`DST-015`).
//!
//! # How the delta reaches the binder
//!
//! Not by a special case in the binder — the binder knows nothing about counters beyond which
//! statement shape to emit. [`CounterDeltas`] wraps the origin row and substitutes the counter
//! columns' cells with the computed deltas, so [`Binder::bind`](cdm_cql::statement::Binder::bind)
//! runs unchanged and every *other* column is still the frame slice it was (`MIG-040`).

use cdm_core::{CdmError, ErrorKind, Side};
use cdm_cql::statement::{ColumnMapping, SourceRow, TargetSource};

/// One counter column, resolved to the positions it occupies on both sides (`MIG-031`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterColumn {
    /// The column's position in the origin projection, which is where the binder reads it from.
    pub origin_index: usize,
    /// The column's position in the target projection, which is where the target lookup's row
    /// carries its current value.
    pub target_index: usize,
}

/// The counter columns of a run, resolved once (`MIG-030`, `MIG-031`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CounterPlan {
    columns: Vec<CounterColumn>,
}

impl CounterPlan {
    /// Resolves every counter column that the origin supplies.
    ///
    /// A counter column with no origin source is left alone: the binder will leave it `UNSET`, and
    /// an `UNSET` counter assignment adds nothing, which is the only safe reading of "the origin
    /// has no opinion about this counter".
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] if the target is a counter table but no counter column is
    /// derivable from the origin, which would make every write a no-op — indistinguishable, from
    /// the outside, from a migration that worked.
    pub fn resolve(mapping: &ColumnMapping) -> Result<Self, CdmError> {
        let mut columns = Vec::new();
        for (target_index, column) in mapping.target_columns().iter().enumerate() {
            if !column.is_counter() {
                continue;
            }
            if let Some(TargetSource::Origin(origin_index)) = mapping.source(target_index) {
                columns.push(CounterColumn {
                    origin_index: *origin_index,
                    target_index,
                });
            }
        }
        if mapping.target_is_counter() && columns.is_empty() {
            return Err(CdmError::new(
                ErrorKind::SchemaMismatch,
                format!(
                    "the target {} is a counter table but no counter column is mapped from the \
                     origin, so every generated UPDATE would add nothing (MIG-030, MIG-031).",
                    mapping.target_table().quoted_name()
                ),
            )
            .with_context(|c| {
                c.with_side(Side::Target)
                    .with_table(mapping.target_table().table_ref())
            }));
        }
        Ok(Self { columns })
    }

    /// The resolved counter columns.
    #[must_use]
    pub fn columns(&self) -> &[CounterColumn] {
        &self.columns
    }

    /// Whether this run writes counters at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// Computes `origin - current` for every counter column (`MIG-031`).
    ///
    /// `current` is the target row the lookup returned, indexed by target column position, or
    /// `None` when the target has no such row — which is a current value of zero, exactly as
    /// Java's `null == targetRow ? 0L` reads it.
    ///
    /// A counter column whose *origin* value is null produces no entry: the binder then leaves the
    /// marker `UNSET` and the `SET c = c + ?` clause adds nothing. Java reaches the same place by
    /// skipping the bind index.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TypeConversion`] if a counter cell is not eight bytes. A `counter` is a CQL
    /// `bigint` on the wire; anything else means the row does not match the schema the run planned
    /// against, and guessing would corrupt the target arithmetically.
    pub fn deltas<'o, 't, R, T>(
        &self,
        origin: &R,
        current: Option<&T>,
    ) -> Result<Vec<(usize, [u8; 8])>, CdmError>
    where
        R: SourceRow<'o>,
        T: SourceRow<'t>,
    {
        let mut deltas = Vec::with_capacity(self.columns.len());
        for column in &self.columns {
            let Some(Some(bytes)) = origin.cell(column.origin_index) else {
                continue;
            };
            let origin_value = counter_value(bytes, Side::Origin)?;
            let target_value = match current.map(|row| row.cell(column.target_index)) {
                // An absent cell and a null cell both mean "the target holds no value here",
                // which is zero for the purpose of a delta — Java's `null == targetRow ? 0L`.
                Some(Some(Some(bytes))) => counter_value(bytes, Side::Target)?,
                _ => 0,
            };
            deltas.push((
                column.origin_index,
                origin_value.wrapping_sub(target_value).to_be_bytes(),
            ));
        }
        Ok(deltas)
    }
}

/// The origin row with its counter columns replaced by the deltas that must be added (`MIG-031`).
///
/// Everything that is *not* a counter column is passed through untouched, so a counter table's
/// non-counter columns keep the zero-copy passthrough of `MIG-040`.
#[derive(Debug, Clone, Copy)]
pub struct CounterDeltas<'a, R> {
    inner: &'a R,
    deltas: &'a [(usize, [u8; 8])],
}

impl<'a, R> CounterDeltas<'a, R> {
    /// Wraps `inner`, substituting `deltas` by origin column position.
    ///
    /// `deltas` must outlive the binding, which it does: the caller keeps it on the stack for the
    /// duration of the counter write, and a counter write is awaited before the next row is read.
    #[must_use]
    pub const fn new(inner: &'a R, deltas: &'a [(usize, [u8; 8])]) -> Self {
        Self { inner, deltas }
    }
}

impl<'frame, R> SourceRow<'frame> for CounterDeltas<'frame, R>
where
    R: SourceRow<'frame>,
{
    fn cell(&self, index: usize) -> Option<Option<&'frame [u8]>> {
        if let Some((_, delta)) = self.deltas.iter().find(|(position, _)| *position == index) {
            return Some(Some(&delta[..]));
        }
        self.inner.cell(index)
    }

    fn width(&self) -> usize {
        self.inner.width()
    }
}

/// Reads an eight-byte counter off the wire.
fn counter_value(bytes: &[u8], side: Side) -> Result<i64, CdmError> {
    let array: [u8; 8] = bytes.try_into().map_err(|_| {
        CdmError::new(
            ErrorKind::TypeConversion,
            format!(
                "a counter cell on the {side} is {} bytes; a CQL counter is always eight \
                 (MIG-031).",
                bytes.len()
            ),
        )
        .with_context(|c| c.with_side(side))
    })?;
    Ok(i64::from_be_bytes(array))
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
    use cdm_core::{RawCell, Row};

    use crate::migrate::testfixtures::{counter_mapping, plain_mapping};

    use super::*;

    fn origin_row(values: Vec<Option<i64>>) -> Row {
        Row::new(
            values
                .into_iter()
                .map(|value| {
                    value.map_or(RawCell::NULL, |v| RawCell::new(v.to_be_bytes().to_vec()))
                })
                .collect(),
        )
    }

    #[test]
    fn mig_030_a_counter_table_resolves_its_counter_columns() {
        let plan = CounterPlan::resolve(&counter_mapping()).unwrap();
        assert_eq!(
            plan.columns(),
            [CounterColumn {
                origin_index: 2,
                target_index: 2
            }]
        );
        assert!(!plan.is_empty());
    }

    #[test]
    fn mig_030_a_plain_table_resolves_no_counter_columns() {
        let plan = CounterPlan::resolve(&plain_mapping()).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn mig_031_the_delta_is_the_origin_value_minus_the_current_target_value() {
        let plan = CounterPlan::resolve(&counter_mapping()).unwrap();
        let row = origin_row(vec![Some(1), Some(0), Some(500)]);
        let source: &Row = &row;

        let current = origin_row(vec![None, None, Some(200)]);
        let current: &Row = &current;
        let deltas = plan.deltas(&source, Some(&current)).unwrap();
        assert_eq!(deltas, vec![(2, 300i64.to_be_bytes())]);
    }

    #[test]
    fn mig_031_a_missing_target_row_makes_the_current_value_zero() {
        let plan = CounterPlan::resolve(&counter_mapping()).unwrap();
        let row = origin_row(vec![Some(1), Some(0), Some(500)]);
        let source: &Row = &row;
        let deltas = plan.deltas::<_, &Row>(&source, None).unwrap();
        assert_eq!(deltas, vec![(2, 500i64.to_be_bytes())]);
    }

    #[test]
    fn mig_031_a_target_ahead_of_the_origin_produces_a_negative_delta() {
        // The correct answer, and the one an operator will not expect: re-migrating a counter
        // that has moved on in the target *decrements* it back to the origin's value.
        let plan = CounterPlan::resolve(&counter_mapping()).unwrap();
        let row = origin_row(vec![Some(1), Some(0), Some(10)]);
        let source: &Row = &row;
        let current = origin_row(vec![None, None, Some(25)]);
        let current: &Row = &current;
        let deltas = plan.deltas(&source, Some(&current)).unwrap();
        assert_eq!(deltas, vec![(2, (-15i64).to_be_bytes())]);
    }

    #[test]
    fn mig_031_a_null_origin_counter_produces_no_delta_and_so_binds_unset() {
        let plan = CounterPlan::resolve(&counter_mapping()).unwrap();
        let row = origin_row(vec![Some(1), Some(0), None]);
        let source: &Row = &row;
        let current = origin_row(vec![None, None, Some(9)]);
        let current: &Row = &current;
        assert!(plan.deltas(&source, Some(&current)).unwrap().is_empty());
    }

    #[test]
    fn mig_031_a_counter_cell_of_the_wrong_width_is_a_record_error_not_a_panic() {
        let plan = CounterPlan::resolve(&counter_mapping()).unwrap();
        let row = Row::new(vec![
            RawCell::new(1i64.to_be_bytes().to_vec()),
            RawCell::new(vec![]),
            RawCell::new(vec![1, 2, 3]),
        ]);
        let source: &Row = &row;
        let error = plan.deltas::<_, &Row>(&source, None).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TypeConversion);
        assert!(error.message().contains("eight"), "{error}");
    }

    #[test]
    fn mig_031_the_delta_row_substitutes_only_the_counter_columns() {
        let row = Row::new(vec![
            RawCell::new(7i32.to_be_bytes().to_vec()),
            RawCell::new(b"payload".to_vec()),
            RawCell::new(500i64.to_be_bytes().to_vec()),
        ]);
        let source: &Row = &row;
        let deltas = [(2usize, 300i64.to_be_bytes())];
        let wrapped = CounterDeltas::new(&source, &deltas);

        assert_eq!(wrapped.width(), 3);
        assert_eq!(
            wrapped.cell(2),
            Some(Some(&300i64.to_be_bytes()[..])),
            "the counter column carries the delta"
        );
        assert_eq!(
            wrapped.cell(1).unwrap().unwrap().as_ptr(),
            row.get(1).unwrap().bytes().unwrap().as_ptr(),
            "MIG-040: every other column is still the row's own bytes"
        );
        assert_eq!(wrapped.cell(9), None);
    }

    #[test]
    fn mig_030_a_counter_target_with_nothing_mapped_from_the_origin_is_refused() {
        let mut mapping = counter_mapping();
        // Rebuild the mapping against an origin that does not carry the counter column at all.
        let mut origin = mapping.origin_table().clone();
        origin.columns.retain(|c| c.name != "n");
        mapping = ColumnMapping::resolve(
            &origin,
            mapping.target_table(),
            &cdm_cql::statement::MappingOptions::default(),
        )
        .unwrap();
        let error = CounterPlan::resolve(&mapping).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::SchemaMismatch);
        assert!(error.message().contains("add nothing"), "{error}");
    }
}
