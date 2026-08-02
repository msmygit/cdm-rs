//! Raw, undeserialized access to a result row.
//!
//! This is the primitive that zero-copy passthrough (`MIG-040`) is built on. When an origin
//! column and its target column have identical CQL types and no feature transforms the column,
//! the correct and fastest thing to do is copy the serialized bytes straight from the read frame
//! into the write bind buffer — no deserialization into an owned value, no re-serialization.
//!
//! [`RawRow`] implements [`DeserializeRow`] without deserializing anything: it captures each
//! column's frame slice and specification and hands them back untouched.
//!
//! # Specification
//!
//! - `MIG-040` — zero-copy passthrough is the default fast path
//! - `CDC-010` — `ConversionPlan::Passthrough`
//! - `TST-030` — passthrough must be provably lossless

use scylla::deserialize::row::{ColumnIterator, DeserializeRow};
use scylla::deserialize::{DeserializationError, FrameSlice, TypeCheckError};
use scylla::frame::response::result::{ColumnSpec, ColumnType};

/// One column of a row, still in its wire representation.
#[derive(Clone, Copy, Debug)]
pub struct RawCell<'frame, 'metadata> {
    /// Position of the column in the row, matching the query's projection order.
    pub index: usize,
    /// The column's name and CQL type, as reported by the server.
    pub spec: &'metadata ColumnSpec<'metadata>,
    /// The serialized value, or `None` when the column is null.
    ///
    /// Note that a null column and a zero-length value are different things: `Some(&[])` is an
    /// empty blob or empty string, `None` is null. Conflating them is how tombstones get written
    /// by accident (see `MIG-012`).
    pub bytes: Option<&'frame [u8]>,
}

impl<'metadata> RawCell<'_, 'metadata> {
    /// The column's CQL type.
    pub fn typ(&self) -> &'metadata ColumnType<'metadata> {
        self.spec.typ()
    }

    /// The column's name.
    pub fn name(&self) -> &'metadata str {
        self.spec.name()
    }

    /// Whether the column is null.
    pub fn is_null(&self) -> bool {
        self.bytes.is_none()
    }

    /// The serialized length in bytes, or `0` for a null column.
    ///
    /// Used by the guardrail job (`GRD-002`) to size a column without deserializing it.
    pub fn byte_len(&self) -> usize {
        self.bytes.map_or(0, <[u8]>::len)
    }
}

/// A result row exposed as its raw column slices.
///
/// Borrows from the response frame, so it lives only as long as the frame does. Cheap to
/// construct: no allocation beyond one `Vec` of thin descriptors, and no value decoding.
#[derive(Clone, Debug)]
pub struct RawRow<'frame, 'metadata> {
    cells: Vec<RawCell<'frame, 'metadata>>,
}

impl<'frame, 'metadata> RawRow<'frame, 'metadata> {
    /// The row's columns, in projection order.
    pub fn cells(&self) -> &[RawCell<'frame, 'metadata>] {
        &self.cells
    }

    /// The column at `index`, or `None` if the row is narrower than that.
    pub fn cell(&self, index: usize) -> Option<&RawCell<'frame, 'metadata>> {
        self.cells.get(index)
    }

    /// Number of columns in the row.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the row has no columns.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

impl<'frame, 'metadata> DeserializeRow<'frame, 'metadata> for RawRow<'frame, 'metadata> {
    /// Accepts any row shape.
    ///
    /// Raw access makes no assumptions about column types — checking them is the job of the
    /// conversion planner (`CDC-010`), which does it once at startup rather than per row.
    fn type_check(_specs: &[ColumnSpec<'_>]) -> Result<(), TypeCheckError> {
        Ok(())
    }

    fn deserialize(row: ColumnIterator<'frame, 'metadata>) -> Result<Self, DeserializationError> {
        let mut cells = Vec::with_capacity(row.columns_remaining());
        for column in row {
            let column = column?;
            cells.push(RawCell {
                index: column.index,
                spec: column.spec,
                bytes: column.slice.as_ref().map(FrameSlice::as_slice),
            });
        }
        Ok(Self { cells })
    }
}
