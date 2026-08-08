//! Rows, cells, primary keys and the [`Record`] that flows through the engine.
//!
//! # Placeholder types
//!
//! `cdm-core` must not depend on a CQL driver (`ARCHITECTURE.md` §3.2), so a cell here is an
//! opaque, driver-independent byte buffer rather than a typed value. Interpreting those bytes is
//! `cdm-codec`'s job, and it does so against a `CqlTypeInfo` it owns. That split is also what
//! makes the zero-copy passthrough of `MIG-040` expressible: a migrate job that needs no
//! conversion moves [`RawCell`]s from origin to target without ever decoding them.

use std::fmt;

use bytes::Bytes;

use crate::error::{CdmError, ErrorKind};

/// One CQL cell, as the wire represents it.
///
/// Three states, which are not the same thing and must not be conflated (`MIG-012`):
///
/// * `Some(bytes)` — a value, possibly empty (an empty `blob` or `text` is a legitimate value);
/// * `None` — CQL `NULL`;
/// * absent from the binding entirely — `UNSET`, which is what a migrate job binds for a null or
///   empty collection so that no tombstone is created. `UNSET` is a property of a *binding*, not
///   of a cell, so it has no representation here.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RawCell(Option<Bytes>);

impl RawCell {
    /// A cell holding CQL `NULL`.
    pub const NULL: Self = Self(None);

    /// A cell holding the given serialised value.
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Self(Some(bytes.into()))
    }

    /// A cell borrowing a `'static` buffer, which avoids a copy for literals and test fixtures.
    pub const fn from_static(bytes: &'static [u8]) -> Self {
        Self(Some(Bytes::from_static(bytes)))
    }

    /// The serialised value, or `None` for CQL `NULL`.
    pub const fn bytes(&self) -> Option<&Bytes> {
        self.0.as_ref()
    }

    /// Whether the cell holds CQL `NULL`.
    pub const fn is_null(&self) -> bool {
        self.0.is_none()
    }

    /// The serialised length in bytes; zero for `NULL`. Used by the guardrail job's size checks
    /// (`GRD-001`).
    pub fn len(&self) -> usize {
        self.0.as_ref().map_or(0, Bytes::len)
    }

    /// Whether the cell is `NULL` or holds a zero-length value.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Display for RawCell {
    /// Renders as hex, never as text. `SEC-002` forbids logging row values outside the validate
    /// diff path; hex at least makes an accidental leak obviously non-quotable, and it is the
    /// representation `ARCHITECTURE.md` §13 expects for the primary key of a failing row.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            None => f.write_str("null"),
            Some(bytes) => {
                f.write_str("0x")?;
                for byte in bytes {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
        }
    }
}

/// A keyspace-qualified table name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableRef {
    keyspace: String,
    table: String,
}

impl TableRef {
    /// Creates a reference. Identifiers are stored exactly as given; quoting for CQL is
    /// `cdm-cql`'s business (`SCH-010`).
    pub fn new(keyspace: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            keyspace: keyspace.into(),
            table: table.into(),
        }
    }

    /// The keyspace.
    pub fn keyspace(&self) -> &str {
        &self.keyspace
    }

    /// The table.
    pub fn table(&self) -> &str {
        &self.table
    }
}

impl fmt::Display for TableRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.keyspace, self.table)
    }
}

/// A column, with its CQL type rendered as the string `system_schema.columns` reports.
///
/// A placeholder for `cdm-codec`'s `CqlTypeInfo`, which parses that string into a type tree.
/// Plugins that only need to know *which* columns exist can work with this; plugins that need the
/// type structure will take `CqlTypeInfo` once `cdm-codec` exists (PR #11).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColumnRef {
    name: String,
    cql_type: String,
}

impl ColumnRef {
    /// Creates a column reference, e.g. `("addresses", "list<frozen<address>>")`.
    pub fn new(name: impl Into<String>, cql_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            cql_type: cql_type.into(),
        }
    }

    /// The column name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The CQL type as `system_schema.columns.type` spells it.
    pub fn cql_type(&self) -> &str {
        &self.cql_type
    }
}

impl fmt::Display for ColumnRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.cql_type)
    }
}

/// The primary key of one row: partition-key columns followed by clustering columns, in schema
/// order.
///
/// Ordering and hashing are structural, over the serialised bytes, which is what lets the validate
/// job index target rows by key without decoding them.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrimaryKey {
    values: Vec<RawCell>,
}

impl PrimaryKey {
    /// Creates a key from its component cells, in schema order.
    pub fn new(values: Vec<RawCell>) -> Self {
        Self { values }
    }

    /// The component cells, in schema order.
    pub fn values(&self) -> &[RawCell] {
        &self.values
    }

    /// The number of components.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the key has no components, which only happens for a default-constructed value.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl fmt::Display for PrimaryKey {
    /// Renders as `(0x01, 0x02)`, hex per [`RawCell`]'s `Display`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(")?;
        for (index, value) in self.values.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{value}")?;
        }
        f.write_str(")")
    }
}

/// A row, as an ordered list of cells matching the projection that produced it.
///
/// Cells are positional, not named: the projection is fixed once at startup into the
/// `ExecutionPlan` (`ARCHITECTURE.md` §5.5), so the hot path indexes rather than looks up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    cells: Vec<RawCell>,
}

impl Row {
    /// Creates a row from its cells, in projection order.
    pub fn new(cells: Vec<RawCell>) -> Self {
        Self { cells }
    }

    /// The cells, in projection order.
    pub fn cells(&self) -> &[RawCell] {
        &self.cells
    }

    /// The cell at `index`, or `None` if the projection is shorter than that.
    pub fn get(&self, index: usize) -> Option<&RawCell> {
        self.cells.get(index)
    }

    /// The number of cells.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the row has no cells.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// One exploded map entry, converted to the target columns' types (`FEA-020`, `FEA-021`).
///
/// The conversion lives in `cdm-feature`, which owns the explode-map plan; the *result* is two
/// plain cells and is carried here so that the three places that need it — the primary key a
/// target row is looked up by, the values an autocorrect write binds, and the two columns a
/// comparison cannot obtain from the origin row — all read the same entry rather than each
/// re-deriving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplodedEntry {
    /// The map key, as the target key column's type.
    pub key: RawCell,
    /// The map value, as the target value column's type.
    pub value: RawCell,
}

/// One unit of work as it flows through the engine: an origin row, the target primary key derived
/// from it, and — when validating — the corresponding target row (`SPEC.md` §2).
///
/// A feature plugin may turn one record into several (`FEA-020`, explode map), which is why
/// [`FeaturePlugin::transform`](crate::FeaturePlugin::transform) emits into a
/// [`RecordSink`](crate::RecordSink) rather than returning a single value. Each of those records
/// stands for one map entry, and carries it — see [`Record::exploded`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    key: PrimaryKey,
    origin: Row,
    target: Option<Row>,
    token: Option<i128>,
    exploded: Option<ExplodedEntry>,
}

impl Record {
    /// Creates a record from the origin row and the target primary key derived from it.
    pub fn new(key: PrimaryKey, origin: Row) -> Self {
        Self {
            key,
            origin,
            target: None,
            token: None,
            exploded: None,
        }
    }

    /// The target primary key derived from the origin row.
    pub fn key(&self) -> &PrimaryKey {
        &self.key
    }

    /// The origin row.
    pub fn origin(&self) -> &Row {
        &self.origin
    }

    /// The corresponding target row, present only once a validate job has fetched it.
    pub fn target(&self) -> Option<&Row> {
        self.target.as_ref()
    }

    /// The partition token of the origin row, if the projection selected it.
    pub fn token(&self) -> Option<i128> {
        self.token
    }

    /// Attaches the target row fetched for comparison.
    #[must_use]
    pub fn with_target(mut self, target: Row) -> Self {
        self.target = Some(target);
        self
    }

    /// Attaches the partition token of the origin row.
    #[must_use]
    pub fn with_token(mut self, token: i128) -> Self {
        self.token = Some(token);
        self
    }

    /// Replaces the origin row, keeping the key — how an exploded record is derived from its
    /// parent.
    #[must_use]
    pub fn with_origin(mut self, origin: Row) -> Self {
        self.origin = origin;
        self
    }

    /// Replaces the target primary key, keeping everything else — how the record for one exploded
    /// map entry is derived from its parent, whose key is the same row's but incomplete
    /// (`FEA-022`).
    #[must_use]
    pub fn with_key(mut self, key: PrimaryKey) -> Self {
        self.key = key;
        self
    }

    /// The exploded map entry this record stands for (`FEA-020`, `FEA-022`).
    ///
    /// `None` for every record of a run with no explode map, which is the overwhelmingly common
    /// case. `Some` means this record is *one entry* of one origin row: its
    /// [`key`](Record::key) was derived for that entry, an autocorrect write binds the entry's two
    /// halves into the target's key and value columns, and a comparison reads them for the two
    /// target columns the origin row has no cell for.
    pub fn exploded(&self) -> Option<&ExplodedEntry> {
        self.exploded.as_ref()
    }

    /// Attaches the exploded map entry this record stands for (`FEA-020`).
    #[must_use]
    pub fn with_exploded(mut self, entry: ExplodedEntry) -> Self {
        self.exploded = Some(entry);
        self
    }

    /// The origin cell at `index`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Internal`] if the index is outside the projection, carrying the
    /// record's primary key so the offending row is identifiable (`ARCHITECTURE.md` §13). A
    /// mismatch between the plan and the row it produced is a bug, not bad data — but `ERR-004`
    /// forbids expressing that as a panic on the hot path.
    pub fn origin_cell(&self, index: usize) -> Result<&RawCell, CdmError> {
        self.origin.get(index).ok_or_else(|| {
            CdmError::new(
                ErrorKind::Internal,
                format!(
                    "origin projection has {} columns; column {index} was requested",
                    self.origin.len()
                ),
            )
            .with_context(|c| c.with_primary_key(self.key.clone()))
        })
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

    fn cell(bytes: &'static [u8]) -> RawCell {
        RawCell::from_static(bytes)
    }

    #[test]
    fn plg_005_a_null_cell_is_distinct_from_an_empty_one() {
        let null = RawCell::NULL;
        let empty = RawCell::new(Vec::new());
        assert!(null.is_null());
        assert!(!empty.is_null());
        assert!(null.is_empty() && empty.is_empty());
        assert_ne!(null, empty);
        assert_eq!(null, RawCell::default());
        assert_eq!(null.bytes(), None);
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn plg_005_cells_render_as_hex_never_as_text() {
        assert_eq!(cell(b"hi").to_string(), "0x6869");
        assert_eq!(RawCell::NULL.to_string(), "null");
        assert_eq!(RawCell::new(vec![0u8, 255]).to_string(), "0x00ff");
        assert_eq!(cell(&[0x0a]).len(), 1);
    }

    #[test]
    fn plg_005_primary_keys_compare_and_render_structurally() {
        let key = PrimaryKey::new(vec![cell(&[1]), cell(&[2])]);
        assert_eq!(key.len(), 2);
        assert!(!key.is_empty());
        assert_eq!(key.to_string(), "(0x01, 0x02)");
        assert_eq!(key, PrimaryKey::new(vec![cell(&[1]), cell(&[2])]));
        assert!(key > PrimaryKey::new(vec![cell(&[1]), cell(&[1])]));
        assert!(PrimaryKey::default().is_empty());
        assert_eq!(key.values()[0], cell(&[1]));
    }

    #[test]
    fn plg_005_rows_are_positional() {
        let row = Row::new(vec![cell(&[1]), RawCell::NULL]);
        assert_eq!(row.len(), 2);
        assert!(!row.is_empty());
        assert_eq!(row.get(1), Some(&RawCell::NULL));
        assert_eq!(row.get(2), None);
        assert_eq!(row.cells().len(), 2);
        assert!(Row::default().is_empty());
    }

    #[test]
    fn plg_002_a_record_carries_the_origin_row_key_token_and_optional_target() {
        let key = PrimaryKey::new(vec![cell(&[7])]);
        let record = Record::new(key.clone(), Row::new(vec![cell(&[7]), cell(b"v")]));
        assert_eq!(record.key(), &key);
        assert!(record.target().is_none());
        assert!(record.token().is_none());

        let record = record
            .with_target(Row::new(vec![cell(&[7]), cell(b"w")]))
            .with_token(-42);
        assert_eq!(record.token(), Some(-42));
        assert_eq!(record.target().unwrap().get(1), Some(&cell(b"w")));
        assert_eq!(record.origin_cell(1).unwrap(), &cell(b"v"));
    }

    #[test]
    fn plg_002_replacing_the_origin_row_keeps_the_key() {
        let key = PrimaryKey::new(vec![cell(&[7])]);
        let exploded = Record::new(key.clone(), Row::new(vec![cell(b"a")]))
            .with_origin(Row::new(vec![cell(b"b")]));
        assert_eq!(exploded.key(), &key);
        assert_eq!(exploded.origin().get(0), Some(&cell(b"b")));
    }

    #[test]
    fn err_004_an_out_of_range_cell_is_an_error_not_a_panic() {
        let record = Record::new(
            PrimaryKey::new(vec![cell(&[7])]),
            Row::new(vec![cell(b"only")]),
        );
        let err = record.origin_cell(9).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert_eq!(err.context().primary_key.as_ref(), Some(record.key()));
        assert!(err.to_string().contains("column 9 was requested"));
    }

    #[test]
    fn err_001_table_and_column_references_render_for_diagnostics() {
        let table = TableRef::new("ks", "tbl");
        assert_eq!(table.to_string(), "ks.tbl");
        assert_eq!(table.keyspace(), "ks");
        assert_eq!(table.table(), "tbl");

        let column = ColumnRef::new("addresses", "list<frozen<address>>");
        assert_eq!(column.to_string(), "addresses list<frozen<address>>");
        assert_eq!(column.name(), "addresses");
        assert_eq!(column.cql_type(), "list<frozen<address>>");
    }
}
