//! The comparison engine of the Java-parity differential harness (`TST-020`, `MET-006`).
//!
//! Two comparisons, and nothing else:
//!
//! 1. [`compare_target_state`] — the target keyspace Java CDM wrote against the target keyspace
//!    cdm-rs wrote, cell by cell, **as serialised bytes**, with writetime and TTL;
//! 2. [`compare_counter_blocks`] — the two final metrics blocks (`MET-006`), field by field.
//!
//! # Why this is not `cdm validate`
//!
//! cdm-rs already has a comparator: the validate job. Using it to decide whether cdm-rs migrated
//! correctly is circular — a comparator and a writer that share a conversion plan, a codec
//! registry and a bind order agree with each other by construction, including where both are
//! wrong. The differential harness exists precisely to catch the bugs that survive that
//! agreement, so its comparison must be independent of both writers. Nothing in this module
//! decodes a value, consults a codec, or knows what CQL type a column has.
//!
//! # Why bytes, and not decoded values
//!
//! A decoded comparison can call two different byte sequences equal. `decimal` with a different
//! scale, a `map` whose entries arrived in a different order, `0.0` against `-0.0`, a `varint`
//! with a redundant leading zero byte, a `set` serialised with a different element count prefix —
//! each of these is one value to a decoder and two rows on disk, and each is exactly the class of
//! bug this harness is for. So the unit of comparison is [`RawCell`]: the bytes the server sent
//! back, compared with `==`, with `NULL` (`None`) distinguished from an empty value (`Some(&[])`)
//! because `MIG-012` turns on that distinction.
//!
//! # Why writetime and TTL are part of "identical"
//!
//! `MIG-020`/`MIG-021` and `FEA-040`–`FEA-046` carry the origin's writetime and TTL onto the
//! target. Two rows whose values match but whose writetimes differ are **not** identical target
//! state: the next migration into that table, or any writetime filter, will behave differently.
//! So [`CellSnapshot`] carries all three, and a writetime difference is reported as loudly as a
//! value difference.
//!
//! TTL is the one quantity that cannot be compared by exact equality without qualification, and
//! this is not leniency: `TTL(col)` is the *remaining* lifetime at the instant of the read, so two
//! reads of two clusters taken seconds apart differ by seconds no matter how identical the
//! writes were. [`TtlPolicy`] therefore makes the comparison state its terms, and defaults to
//! [`TtlPolicy::Exact`]. A harness that reads both targets at the same moment should leave it
//! there; one that cannot should say by how much, in the test, where a reviewer can see it.
//!
//! # Why row order is not compared
//!
//! Two clusters return a full-table scan in token order, and token order depends on the
//! partitioner and the ring — facts about the fixture, not about the migration. Rows are
//! therefore keyed by primary key ([`TargetSnapshot`] is a `BTreeMap` over [`PrimaryKey`], whose
//! ordering is structural over the key's serialised bytes) and compared by key. A row present on
//! one side only is a [`Difference::RowMissing`], never a silently absent comparison.
//!
//! # This is a diff path, deliberately (`SEC-002`)
//!
//! `SEC-002` forbids logging row values outside the validate discrepancy detail. A byte-diff
//! report is that kind of path — a diff a human can act on is the entire deliverable, and a
//! boolean would make the nightly run useless — so this module renders values, and says so here
//! rather than leaving it to be discovered. Two things bound it:
//!
//! * values are rendered as hex, never as text, so nothing leaks in a quotable form;
//! * [`Redaction::Hashed`] renders a digest and a length instead, mirroring
//!   `validate.report.redact_values`, for a harness pointed at data that is not synthetic.
//!
//! The corpus of `TST-020` is generated from a seed and contains no real data, so
//! [`Redaction::Hex`] is the default. Nothing here ever renders a credential: it never sees one.
//!
//! # Reading a target over CQL
//!
//! [`snapshot_target`] is behind the off-by-default `differential` feature, for the reason
//! `ARCHITECTURE.md` §3.3 gives for `macrobench`: reading a target needs `cdm-cql`, and
//! `cdm-testkit` may not have that edge in a default build. Everything above — the snapshot
//! types, both comparisons, the whole report — is driver-free and compiles with no features at
//! all, which is also what makes it unit-testable against synthesised inputs.
//!
//! # Specification
//!
//! - `TST-020` — [`compare_target_state`], [`compare_counter_blocks`], [`DifferentialReport`]
//! - `MET-006` — [`FinalBlock`], [`compare_counter_blocks`]
//! - `MET-005` — [`compare_metrics_strings`]
//! - `MIG-012` — `NULL` and an empty value are different cells
//! - `SEC-002` — [`Redaction`]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use cdm_core::{CdmError, ErrorKind, PrimaryKey, RawCell};

/// Which implementation a snapshot, a block or a difference came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Implementation {
    /// The Java `cassandra-data-migrator`, the parity baseline.
    Java,
    /// This implementation.
    Rust,
}

impl Implementation {
    /// The other one.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Java => Self::Rust,
            Self::Rust => Self::Java,
        }
    }
}

impl fmt::Display for Implementation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Java => "Java CDM",
            Self::Rust => "cdm-rs",
        })
    }
}

/// How a cell's bytes are rendered in a report (`SEC-002`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Redaction {
    /// `0x0000002a` — the bytes themselves, hex-encoded.
    ///
    /// The default, because the corpus of `TST-020` is generated from a seed: there is nothing in
    /// it to protect, and a report that does not show the bytes cannot be acted on.
    #[default]
    Hex,
    /// `<8 bytes, h=3c9e0f…>` — a length and a digest, and no value.
    ///
    /// The equivalent of `validate.report.redact_values` for a harness pointed at data that is
    /// not synthetic. Two identical values still render identically, so a report stays readable;
    /// the value itself does not appear.
    Hashed,
}

/// A 64-bit FNV-1a digest of a cell's bytes.
///
/// Identification, not cryptography: it exists so that [`Redaction::Hashed`] can show that two
/// cells differ without showing what either one is, and so that the same value renders the same
/// way twice in one report. Hand-written rather than pulled in, because a hash crate would be a
/// dependency of the whole workspace for four lines used only when rendering a report.
fn digest(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Renders one cell for a report, under the given redaction.
fn render_cell(cell: &RawCell, redaction: Redaction) -> String {
    match (redaction, cell.bytes()) {
        (_, None) => "null".to_owned(),
        (Redaction::Hex, Some(_)) => cell.to_string(),
        (Redaction::Hashed, Some(bytes)) => {
            format!("<{} bytes, h={:016x}>", bytes.len(), digest(bytes))
        }
    }
}

/// Renders a primary key for a report, under the given redaction.
///
/// A key column is a row value too, so it is redacted on the same terms as any other cell. It is
/// also the one thing a human needs in order to go and look at the offending row, which is why
/// `ARCHITECTURE.md` §13 has diagnostics carry it at all.
fn render_key(key: &PrimaryKey, redaction: Redaction) -> String {
    let rendered: Vec<String> = key
        .values()
        .iter()
        .map(|value| render_cell(value, redaction))
        .collect();
    format!("({})", rendered.join(", "))
}

/// A writetime or TTL as the server reported it, or the fact that it was not asked for.
///
/// Three states, and conflating any two of them hides a difference:
///
/// * [`CellTime::NotSelected`] — the projection did not ask. Not every column can be asked:
///   `WRITETIME` is rejected for a primary-key column and, on most server versions, for a
///   non-frozen collection. The snapshot records that it did not ask rather than inventing a
///   value;
/// * [`CellTime::Absent`] — asked, and the server answered `null`: the cell is null, or has no
///   TTL;
/// * [`CellTime::Value`] — asked, and answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CellTime<T> {
    /// The projection did not select this quantity for this column.
    #[default]
    NotSelected,
    /// Selected, and the server returned `null`.
    Absent,
    /// Selected, and the server returned this.
    Value(T),
}

impl<T: fmt::Display> fmt::Display for CellTime<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSelected => f.write_str("not selected"),
            Self::Absent => f.write_str("null"),
            Self::Value(value) => write!(f, "{value}"),
        }
    }
}

/// One column of one target row: its bytes, its writetime and its TTL.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CellSnapshot {
    /// The serialised value, or `NULL`. Never decoded.
    pub value: RawCell,
    /// `WRITETIME(col)`, in microseconds since the epoch.
    pub writetime: CellTime<i64>,
    /// `TTL(col)`, in seconds remaining at the instant of the read.
    pub ttl: CellTime<i32>,
}

impl CellSnapshot {
    /// A cell with no writetime or TTL selected.
    #[must_use]
    pub fn new(value: RawCell) -> Self {
        Self {
            value,
            writetime: CellTime::NotSelected,
            ttl: CellTime::NotSelected,
        }
    }

    /// The same cell with a writetime.
    #[must_use]
    pub const fn with_writetime(mut self, writetime: CellTime<i64>) -> Self {
        self.writetime = writetime;
        self
    }

    /// The same cell with a TTL.
    #[must_use]
    pub const fn with_ttl(mut self, ttl: CellTime<i32>) -> Self {
        self.ttl = ttl;
        self
    }
}

/// One target row: its primary key, and its columns by name.
///
/// By name rather than by position, because the two implementations are read by two `SELECT`s
/// that this harness builds from the same [`SnapshotSpec`] but that the two servers are free to
/// answer in any column order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowSnapshot {
    /// The primary key, in schema order — the join key of the whole comparison.
    pub key: PrimaryKey,
    /// The non-key columns, by name.
    pub cells: BTreeMap<String, CellSnapshot>,
}

impl RowSnapshot {
    /// An empty row with the given key.
    #[must_use]
    pub fn new(key: PrimaryKey) -> Self {
        Self {
            key,
            cells: BTreeMap::new(),
        }
    }

    /// The same row with one more column.
    #[must_use]
    pub fn with_cell(mut self, column: impl Into<String>, cell: CellSnapshot) -> Self {
        self.cells.insert(column.into(), cell);
        self
    }
}

/// Every row of one target keyspace's table, keyed by primary key.
///
/// Materialised in memory on purpose: a comparison keyed by primary key cannot stream both sides,
/// and a differential corpus is sized to be compared rather than to be a load test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetSnapshot {
    table: String,
    key_columns: Vec<String>,
    rows: BTreeMap<PrimaryKey, RowSnapshot>,
}

impl TargetSnapshot {
    /// An empty snapshot of `table`, whose rows are keyed by `key_columns` in schema order.
    #[must_use]
    pub fn new(table: impl Into<String>, key_columns: Vec<String>) -> Self {
        Self {
            table: table.into(),
            key_columns,
            rows: BTreeMap::new(),
        }
    }

    /// Adds a row.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if a row with the same primary key is already present. A table
    /// cannot hold two rows with one key, so a duplicate means the snapshot's key columns are not
    /// the table's — and a comparison keyed by a non-key would silently compare one arbitrary row
    /// of each group and call the rest identical.
    pub fn insert(&mut self, row: RowSnapshot) -> Result<(), CdmError> {
        if self.rows.contains_key(&row.key) {
            return Err(CdmError::new(
                ErrorKind::Internal,
                format!(
                    "two rows of `{}` share the primary key {}: the snapshot's key columns \
                     {:?} are not this table's primary key",
                    self.table,
                    render_key(&row.key, Redaction::Hashed),
                    self.key_columns,
                ),
            ));
        }
        self.rows.insert(row.key.clone(), row);
        Ok(())
    }

    /// The table this is a snapshot of.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The key columns, in schema order.
    pub fn key_columns(&self) -> &[String] {
        &self.key_columns
    }

    /// The rows, by primary key.
    pub fn rows(&self) -> &BTreeMap<PrimaryKey, RowSnapshot> {
        &self.rows
    }

    /// How many rows the snapshot holds.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the snapshot holds no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// How TTLs are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TtlPolicy {
    /// Equal to the second. The default.
    #[default]
    Exact,
    /// Equal to within `n` seconds.
    ///
    /// For a harness that cannot read both targets at the same instant. `TTL(col)` counts down
    /// from the moment of the read, so a skew of a few seconds between two reads is a property of
    /// the harness and not of the migration — but the tolerance has to be stated, in the test,
    /// where a reviewer can see how much slack was bought. Whether a TTL exists at all is never
    /// subject to the tolerance.
    WithinSeconds(u32),
}

impl TtlPolicy {
    /// Whether two selected TTLs are equal under this policy.
    const fn admits(self, java: i32, rust: i32) -> bool {
        match self {
            Self::Exact => java == rust,
            Self::WithinSeconds(slack) => java.abs_diff(rust) <= slack,
        }
    }
}

impl fmt::Display for TtlPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact => f.write_str("exact"),
            Self::WithinSeconds(slack) => write!(f, "within {slack}s"),
        }
    }
}

/// How many differences a report holds before it stops recording them.
///
/// A corpus that diverges everywhere would otherwise produce a report nobody can read and a CI
/// log nobody can download. The *count* stays exact — [`TargetStateDiff::total`] counts every
/// difference found — so truncation never turns a failure into a pass.
pub const DEFAULT_MAX_DIFFERENCES: usize = 100;

/// What [`compare_target_state`] is allowed to overlook, and how it renders what it finds.
///
/// Every field defaults to the strict setting. Relaxing one is a decision a test makes explicitly
/// and a reviewer can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompareOptions {
    /// How TTLs are compared. Defaults to [`TtlPolicy::Exact`].
    pub ttl: TtlPolicy,
    /// How cell bytes are rendered. Defaults to [`Redaction::Hex`].
    pub redaction: Redaction,
    /// How many differences to record. Defaults to [`DEFAULT_MAX_DIFFERENCES`].
    pub max_differences: usize,
    /// Columns excluded from the comparison, each with the `MIGRATION_FROM_JAVA.md` row that
    /// records the divergence. Empty by default; see [`CompareOptions::excluding_column`].
    excluded: BTreeMap<String, u32>,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            ttl: TtlPolicy::Exact,
            redaction: Redaction::Hex,
            max_differences: DEFAULT_MAX_DIFFERENCES,
            excluded: BTreeMap::new(),
        }
    }
}

impl CompareOptions {
    /// The strict defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Compares TTLs to within `slack` seconds rather than exactly.
    #[must_use]
    pub const fn ttl_within(mut self, slack: u32) -> Self {
        self.ttl = TtlPolicy::WithinSeconds(slack);
        self
    }

    /// Renders cell bytes as a digest rather than as hex (`SEC-002`).
    #[must_use]
    pub const fn redacted(mut self) -> Self {
        self.redaction = Redaction::Hashed;
        self
    }

    /// Records at most `max` differences.
    #[must_use]
    pub const fn max_differences(mut self, max: usize) -> Self {
        self.max_differences = max;
        self
    }

    /// Excludes one column from the comparison, citing the `MIGRATION_FROM_JAVA.md` row that
    /// records the divergence.
    ///
    /// The row number is not decoration and there is no exclusion without one. cdm-rs's
    /// intentional differences from Java are enumerated in that document as a hard requirement —
    /// "a PR that introduces a difference without adding it here fails review" — so a column that
    /// legitimately differs has a row, and a column that has no row does not legitimately differ:
    /// it is a finding, and the correct response is to report it rather than to exclude it.
    ///
    /// Every exclusion is reproduced in the report, so a run that passes says which columns it
    /// did not look at and on whose authority.
    #[must_use]
    pub fn excluding_column(mut self, column: impl Into<String>, migration_row: u32) -> Self {
        self.excluded.insert(column.into(), migration_row);
        self
    }

    /// The excluded columns and the rows that justify them.
    pub fn excluded(&self) -> &BTreeMap<String, u32> {
        &self.excluded
    }
}

/// One way the two targets differ.
///
/// Structured rather than rendered: a caller that wants to assert on a specific difference, count
/// them by kind, or serialise the report can, and [`fmt::Display`] is for the human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Difference {
    /// The two snapshots are of differently named tables, so nothing below them is comparable.
    Table {
        /// The Java side's table.
        java: String,
        /// The cdm-rs side's table.
        rust: String,
    },
    /// The two snapshots are keyed by different columns.
    KeyColumns {
        /// The Java side's key columns.
        java: Vec<String>,
        /// The cdm-rs side's key columns.
        rust: Vec<String>,
    },
    /// The two targets hold different numbers of rows.
    RowCount {
        /// Rows in the Java target.
        java: usize,
        /// Rows in the cdm-rs target.
        rust: usize,
    },
    /// A row is absent from one target.
    RowMissing {
        /// The primary key of the row.
        key: PrimaryKey,
        /// The implementation whose target does not have it.
        from: Implementation,
    },
    /// A row exists on both sides, but one of them has no such column.
    ColumnMissing {
        /// The primary key of the row.
        key: PrimaryKey,
        /// The column.
        column: String,
        /// The implementation whose row does not have it.
        from: Implementation,
    },
    /// A cell's serialised bytes differ. The difference this harness exists for.
    Value {
        /// The primary key of the row.
        key: PrimaryKey,
        /// The column.
        column: String,
        /// The bytes Java CDM wrote.
        java: RawCell,
        /// The bytes cdm-rs wrote.
        rust: RawCell,
    },
    /// A cell's writetime differs (`MIG-020`, `FEA-040`).
    Writetime {
        /// The primary key of the row.
        key: PrimaryKey,
        /// The column.
        column: String,
        /// The writetime Java CDM wrote.
        java: CellTime<i64>,
        /// The writetime cdm-rs wrote.
        rust: CellTime<i64>,
    },
    /// A cell's TTL differs by more than [`TtlPolicy`] admits (`FEA-040`).
    Ttl {
        /// The primary key of the row.
        key: PrimaryKey,
        /// The column.
        column: String,
        /// The TTL the Java target reported.
        java: CellTime<i32>,
        /// The TTL the cdm-rs target reported.
        rust: CellTime<i32>,
    },
}

impl Difference {
    /// The primary key this difference is about, when it is about one row.
    #[must_use]
    pub const fn key(&self) -> Option<&PrimaryKey> {
        match self {
            Self::Table { .. } | Self::KeyColumns { .. } | Self::RowCount { .. } => None,
            Self::RowMissing { key, .. }
            | Self::ColumnMissing { key, .. }
            | Self::Value { key, .. }
            | Self::Writetime { key, .. }
            | Self::Ttl { key, .. } => Some(key),
        }
    }

    /// The column this difference is about, when it is about one column.
    #[must_use]
    pub fn column(&self) -> Option<&str> {
        match self {
            Self::Table { .. }
            | Self::KeyColumns { .. }
            | Self::RowCount { .. }
            | Self::RowMissing { .. } => None,
            Self::ColumnMissing { column, .. }
            | Self::Value { column, .. }
            | Self::Writetime { column, .. }
            | Self::Ttl { column, .. } => Some(column),
        }
    }

    /// Renders the difference under an explicit redaction (`SEC-002`).
    #[must_use]
    pub fn render(&self, redaction: Redaction) -> String {
        match self {
            Self::Table { java, rust } => {
                format!("table: Java CDM snapshot is of `{java}`, cdm-rs of `{rust}`")
            }
            Self::KeyColumns { java, rust } => {
                format!("key columns: Java CDM {java:?}, cdm-rs {rust:?}")
            }
            Self::RowCount { java, rust } => {
                format!("row count: Java CDM {java}, cdm-rs {rust}")
            }
            Self::RowMissing { key, from } => format!(
                "row {}: present in {}, absent from {}",
                render_key(key, redaction),
                from.other(),
                from
            ),
            Self::ColumnMissing { key, column, from } => format!(
                "row {} column `{column}`: absent from {}",
                render_key(key, redaction),
                from
            ),
            Self::Value {
                key,
                column,
                java,
                rust,
            } => format!(
                "row {} column `{column}`: Java CDM {}, cdm-rs {}",
                render_key(key, redaction),
                render_cell(java, redaction),
                render_cell(rust, redaction),
            ),
            Self::Writetime {
                key,
                column,
                java,
                rust,
            } => format!(
                "row {} column `{column}` WRITETIME: Java CDM {java}, cdm-rs {rust}",
                render_key(key, redaction),
            ),
            Self::Ttl {
                key,
                column,
                java,
                rust,
            } => format!(
                "row {} column `{column}` TTL: Java CDM {java}, cdm-rs {rust}",
                render_key(key, redaction),
            ),
        }
    }
}

impl fmt::Display for Difference {
    /// Hex, because that is the default of [`Redaction`]; use [`Difference::render`] to choose.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render(Redaction::Hex))
    }
}

/// The result of comparing two target keyspaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetStateDiff {
    table: String,
    differences: Vec<Difference>,
    total: usize,
    java_rows: usize,
    rust_rows: usize,
    options: CompareOptions,
}

impl TargetStateDiff {
    /// Whether the two targets are byte-identical, which is what `TST-020` requires.
    #[must_use]
    pub const fn is_identical(&self) -> bool {
        self.total == 0
    }

    /// The recorded differences, at most [`CompareOptions::max_differences`] of them.
    pub fn differences(&self) -> &[Difference] {
        &self.differences
    }

    /// How many differences were found, including any not recorded.
    #[must_use]
    pub const fn total(&self) -> usize {
        self.total
    }

    /// Whether more differences were found than were recorded.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.total > self.differences.len()
    }

    /// Rows in each target.
    #[must_use]
    pub const fn row_counts(&self) -> (usize, usize) {
        (self.java_rows, self.rust_rows)
    }

    /// Turns the diff into a result, so a test can `?` it.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] rendering the whole report, when the targets are not identical.
    pub fn into_result(self) -> Result<(), CdmError> {
        if self.is_identical() {
            return Ok(());
        }
        Err(CdmError::new(ErrorKind::Internal, self.to_string()))
    }
}

impl fmt::Display for TargetStateDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_identical() {
            return write!(
                f,
                "target state is byte-identical: {} rows of `{}` (TST-020)",
                self.java_rows, self.table
            );
        }
        writeln!(
            f,
            "target state differs (TST-020 requires byte-identical target state)\n  \
             table: {}\n  rows: Java CDM {}, cdm-rs {}\n  differences: {}\n  TTL policy: {}",
            self.table, self.java_rows, self.rust_rows, self.total, self.options.ttl
        )?;
        for (column, row) in &self.options.excluded {
            writeln!(
                f,
                "  excluded: `{column}` (MIGRATION_FROM_JAVA.md row {row})"
            )?;
        }
        for difference in &self.differences {
            writeln!(f, "  - {}", difference.render(self.options.redaction))?;
        }
        if self.truncated() {
            write!(
                f,
                "  … and {} more (raise CompareOptions::max_differences to see them)",
                self.total - self.differences.len()
            )?;
        }
        Ok(())
    }
}

/// Accumulates differences up to the cap while counting all of them.
#[derive(Debug)]
struct Collector {
    recorded: Vec<Difference>,
    total: usize,
    max: usize,
}

impl Collector {
    const fn new(max: usize) -> Self {
        Self {
            recorded: Vec::new(),
            total: 0,
            max,
        }
    }

    fn push(&mut self, difference: Difference) {
        self.total += 1;
        if self.recorded.len() < self.max {
            self.recorded.push(difference);
        }
    }
}

/// Compares two target keyspaces cell by cell, as bytes (`TST-020`).
///
/// The two snapshots are whatever [`snapshot_target`] — or a test — produced; this function does
/// not know which cluster either came from, and that independence from both writers is the point.
/// See the module documentation.
///
/// Everything is reported, not just the first thing: a run that differs in one column of every
/// row and a run that lost a row are different situations, and a comparison that stopped at the
/// first difference could not tell them apart.
#[must_use]
pub fn compare_target_state(
    java: &TargetSnapshot,
    rust: &TargetSnapshot,
    options: &CompareOptions,
) -> TargetStateDiff {
    let mut found = Collector::new(options.max_differences);

    if java.table != rust.table {
        found.push(Difference::Table {
            java: java.table.clone(),
            rust: rust.table.clone(),
        });
    }
    if java.key_columns != rust.key_columns {
        found.push(Difference::KeyColumns {
            java: java.key_columns.clone(),
            rust: rust.key_columns.clone(),
        });
    }
    if java.rows.len() != rust.rows.len() {
        found.push(Difference::RowCount {
            java: java.rows.len(),
            rust: rust.rows.len(),
        });
    }

    // The union of the keys, so that a row present on either side is accounted for. `BTreeMap`
    // gives it in key order, which makes a report of a failing run reproducible between runs.
    let keys: BTreeSet<&PrimaryKey> = java.rows.keys().chain(rust.rows.keys()).collect();
    for key in keys {
        match (java.rows.get(key), rust.rows.get(key)) {
            (Some(java_row), Some(rust_row)) => {
                compare_row(key, java_row, rust_row, options, &mut found);
            }
            (Some(_), None) => found.push(Difference::RowMissing {
                key: key.clone(),
                from: Implementation::Rust,
            }),
            (None, Some(_)) => found.push(Difference::RowMissing {
                key: key.clone(),
                from: Implementation::Java,
            }),
            // Unreachable: `key` came from one of the two maps.
            (None, None) => {}
        }
    }

    TargetStateDiff {
        table: java.table.clone(),
        differences: found.recorded,
        total: found.total,
        java_rows: java.rows.len(),
        rust_rows: rust.rows.len(),
        options: options.clone(),
    }
}

/// Compares one row's columns.
fn compare_row(
    key: &PrimaryKey,
    java: &RowSnapshot,
    rust: &RowSnapshot,
    options: &CompareOptions,
    found: &mut Collector,
) {
    let columns: BTreeSet<&String> = java.cells.keys().chain(rust.cells.keys()).collect();
    for column in columns {
        if options.excluded.contains_key(column.as_str()) {
            continue;
        }
        let (java_cell, rust_cell) = match (java.cells.get(column), rust.cells.get(column)) {
            (Some(java_cell), Some(rust_cell)) => (java_cell, rust_cell),
            (Some(_), None) => {
                found.push(Difference::ColumnMissing {
                    key: key.clone(),
                    column: column.clone(),
                    from: Implementation::Rust,
                });
                continue;
            }
            (None, Some(_)) => {
                found.push(Difference::ColumnMissing {
                    key: key.clone(),
                    column: column.clone(),
                    from: Implementation::Java,
                });
                continue;
            }
            (None, None) => continue,
        };

        // `RawCell`'s equality is over the bytes, and distinguishes `NULL` from an empty value
        // (`MIG-012`). Nothing is decoded, so two encodings of one logical value differ here —
        // which is the whole reason this comparison is not `cdm validate`.
        if java_cell.value != rust_cell.value {
            found.push(Difference::Value {
                key: key.clone(),
                column: column.clone(),
                java: java_cell.value.clone(),
                rust: rust_cell.value.clone(),
            });
        }
        if java_cell.writetime != rust_cell.writetime {
            found.push(Difference::Writetime {
                key: key.clone(),
                column: column.clone(),
                java: java_cell.writetime,
                rust: rust_cell.writetime,
            });
        }
        if !ttl_matches(java_cell.ttl, rust_cell.ttl, options.ttl) {
            found.push(Difference::Ttl {
                key: key.clone(),
                column: column.clone(),
                java: java_cell.ttl,
                rust: rust_cell.ttl,
            });
        }
    }
}

/// Whether two TTLs agree under a policy.
///
/// The tolerance applies only between two *values*. A TTL that exists on one side and not the
/// other is a difference at any tolerance: it is the difference between a row that expires and a
/// row that does not.
const fn ttl_matches(java: CellTime<i32>, rust: CellTime<i32>, policy: TtlPolicy) -> bool {
    match (java, rust) {
        (CellTime::Value(java), CellTime::Value(rust)) => policy.admits(java, rust),
        (CellTime::NotSelected, CellTime::NotSelected) | (CellTime::Absent, CellTime::Absent) => {
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Counter blocks (MET-005, MET-006)
// ---------------------------------------------------------------------------------------------

/// One `Final … Count: N` line of a final block, or one entry of a metrics string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterLine {
    /// The label, exactly as printed: `Final Read Record Count`, or `Read` in a metrics string.
    pub label: String,
    /// The value.
    pub value: u64,
    /// The line as printed, with any log-framework prefix removed.
    pub text: String,
}

impl CounterLine {
    /// The label with runs of whitespace collapsed, which is what two blocks are matched on.
    ///
    /// Matching on the collapsed label and then comparing [`CounterLine::text`] is what turns a
    /// whitespace change into a [`CounterDifference::LineFormat`] rather than into a pair of
    /// "missing" and "extra" counters that reads as though a counter had been renamed.
    #[must_use]
    pub fn key(&self) -> String {
        self.label.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// One line of a final block, in the order it was printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockLine {
    /// A `#` banner, and how many `#` it had. Java prints ninety-six.
    Rule(usize),
    /// The `RunId:` line, and its value — which is *not* compared.
    RunId(String),
    /// A counter line.
    Counter(CounterLine),
    /// A line inside the banners that is none of the above.
    Other(String),
}

/// The marker that opens a counter line of `MET-006`.
const FINAL_PREFIX: &str = "Final ";

/// The marker that opens the run-id line of `MET-006`.
const RUN_ID_PREFIX: &str = "RunId:";

/// The shortest run of `#` that counts as a banner rather than as a comment.
const MIN_RULE_WIDTH: usize = 8;

/// A parsed `MET-006` final block.
///
/// Structure, not text: the banners and their widths, the presence of the `RunId` line, and the
/// counter lines in the order they were printed. Everything `MET-006` fixes is here, so that
/// [`compare_counter_blocks`] can compare it field by field rather than as one string — a string
/// comparison would report "the blocks differ" and leave the reader to find out how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalBlock {
    lines: Vec<BlockLine>,
}

impl FinalBlock {
    /// Parses a final block out of a run's output.
    ///
    /// Tolerant about *where* the block is — a whole log can be passed in, as `cdm-assert.sh`
    /// does — and about one thing only in *how* it is printed: a leading log-framework prefix.
    /// Java emits the block through log4j and cdm-rs through `tracing`, so a line arrives as
    /// `12:00:01 INFO  JobCounter - Final Read Record Count: 7`. The timestamp and the logger name
    /// are the logging configuration's, not `MET-006`'s, so the parser takes each line from its
    /// marker (`#`, `RunId:` or `Final `) onwards. Everything from the marker to the end of the
    /// line is compared verbatim, including spacing, so nothing `MET-006` actually fixes is
    /// normalised away.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the text contains no counter lines at all. A run that printed
    /// no block must fail loudly: comparing two absent blocks would otherwise pass, which is how
    /// a parity assertion comes to mean nothing (the same reasoning as `ENG-008`).
    pub fn parse(text: &str) -> Result<Self, CdmError> {
        let mut lines = Vec::new();
        let mut inside = false;
        for raw in text.lines() {
            if let Some(width) = rule_width(raw) {
                lines.push(BlockLine::Rule(width));
                inside = !inside;
                continue;
            }
            if let Some(index) = raw.find(RUN_ID_PREFIX) {
                let value = raw
                    .get(index + RUN_ID_PREFIX.len()..)
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                lines.push(BlockLine::RunId(value));
                continue;
            }
            if let Some(index) = raw.find(FINAL_PREFIX) {
                if let Some(line) = raw.get(index..).map(str::trim_end) {
                    if let Some(counter) = counter_line(line) {
                        lines.push(BlockLine::Counter(counter));
                        continue;
                    }
                }
            }
            // Ordinary log output either side of the block is not the block's business. A line
            // *between* the banners is: `MET-006` says what the block contains, and an extra line
            // in it is a parity failure that a lenient parser would drop on the floor.
            if inside {
                lines.push(BlockLine::Other(raw.trim().to_owned()));
            }
        }

        if !lines
            .iter()
            .any(|line| matches!(line, BlockLine::Counter(_)))
        {
            return Err(CdmError::new(
                ErrorKind::Internal,
                "no `Final … Count:` lines found; was the final block (MET-006) printed at all?"
                    .to_owned(),
            ));
        }
        Ok(Self { lines })
    }

    /// Parses a metrics string such as `Read: 10; Write: 9; Skipped: 1` (`MET-005`).
    ///
    /// The same structure with no banners and no run id, so that both formats compare through
    /// [`compare_counter_blocks`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if a segment is not `<Name>: <number>`. Nothing is skipped here:
    /// the metrics string is a whole value rather than a line in a log, so an unparsable segment
    /// means the format changed, which is what `MET-005` and `COMPAT-004` forbid.
    pub fn parse_metrics_string(metrics: &str) -> Result<Self, CdmError> {
        let mut lines = Vec::new();
        for segment in metrics.split(';') {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }
            let counter = counter_line(segment).ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!("`{segment}` is not a `<Name>: <number>` entry of MET-005"),
                )
            })?;
            lines.push(BlockLine::Counter(counter));
        }
        Ok(Self { lines })
    }

    /// The lines, in the order they were printed.
    pub fn lines(&self) -> &[BlockLine] {
        &self.lines
    }

    /// The widths of the `#` banners, in order.
    #[must_use]
    pub fn rules(&self) -> Vec<usize> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                BlockLine::Rule(width) => Some(*width),
                _ => None,
            })
            .collect()
    }

    /// The `RunId` line's value, when the block has one.
    ///
    /// Reported so that a harness can record which run it compared. Never compared: `MET-006`
    /// prints the run id of *this* run, and two runs necessarily have two.
    #[must_use]
    pub fn run_id(&self) -> Option<&str> {
        self.lines.iter().find_map(|line| match line {
            BlockLine::RunId(value) => Some(value.as_str()),
            _ => None,
        })
    }

    /// The counter lines, in the order they were printed.
    #[must_use]
    pub fn counters(&self) -> Vec<&CounterLine> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                BlockLine::Counter(counter) => Some(counter),
                _ => None,
            })
            .collect()
    }

    /// The lines inside the banners that are neither a banner, a run id nor a counter.
    #[must_use]
    pub fn unrecognised(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                BlockLine::Other(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The block as text, with the run id replaced by a placeholder.
    ///
    /// The comparison's safety net: if two blocks' normalisations differ but no structured
    /// difference was produced, [`compare_counter_blocks`] reports the raw texts rather than
    /// passing. That is what makes "identical" mean identical — every character of the block
    /// except the run id's value is either compared as a field or caught here.
    #[must_use]
    pub fn normalised(&self) -> Vec<String> {
        self.lines
            .iter()
            .map(|line| match line {
                BlockLine::Rule(width) => "#".repeat(*width),
                BlockLine::RunId(_) => format!("{RUN_ID_PREFIX} <ignored>"),
                BlockLine::Counter(counter) => counter.text.clone(),
                BlockLine::Other(text) => text.clone(),
            })
            .collect()
    }
}

/// The width of the `#` banner on a line, if it is one.
///
/// Searches for the run of `#` rather than requiring the line to start with one, for the same
/// reason [`FinalBlock::parse`] takes each line from its marker: a log framework may have put a
/// timestamp in front of it.
fn rule_width(line: &str) -> Option<usize> {
    let trimmed = line.trim_end();
    let start = trimmed.find('#')?;
    let rest = trimmed.get(start..)?;
    let width = rest.chars().take_while(|c| *c == '#').count();
    if width >= MIN_RULE_WIDTH && rest.chars().skip(width).all(char::is_whitespace) {
        Some(width)
    } else {
        None
    }
}

/// Parses `<label>: <number>` into a counter line.
fn counter_line(text: &str) -> Option<CounterLine> {
    let (label, value) = text.rsplit_once(':')?;
    let value: u64 = value.trim().parse().ok()?;
    Some(CounterLine {
        label: label.to_owned(),
        value,
        text: text.to_owned(),
    })
}

/// One way two counter blocks differ (`MET-005`, `MET-006`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CounterDifference {
    /// The `#` banners differ in number or in width.
    Banner {
        /// The Java block's banner widths.
        java: Vec<usize>,
        /// The cdm-rs block's banner widths.
        rust: Vec<usize>,
    },
    /// One block has a `RunId` line and the other does not.
    ///
    /// The value is never compared; its *presence* is part of the format, and Java prints it
    /// exactly when run tracking is enabled.
    RunIdPresence {
        /// Whether the Java block had one.
        java: bool,
        /// Whether the cdm-rs block had one.
        rust: bool,
    },
    /// A counter one block prints and the other does not (`MET-002`).
    CounterMissing {
        /// The counter's label.
        label: String,
        /// The implementation that does not print it.
        from: Implementation,
        /// The value the other one printed.
        value: u64,
    },
    /// A counter both blocks print, with different values.
    Value {
        /// The counter's label.
        label: String,
        /// Java CDM's value.
        java: u64,
        /// cdm-rs's value.
        rust: u64,
    },
    /// The same counters in a different order.
    Order {
        /// Java CDM's order.
        java: Vec<String>,
        /// cdm-rs's order.
        rust: Vec<String>,
    },
    /// The same counter and value, printed differently.
    LineFormat {
        /// The counter's label.
        label: String,
        /// The line Java CDM printed.
        java: String,
        /// The line cdm-rs printed.
        rust: String,
    },
    /// A line inside the banners that is not part of `MET-006`.
    Unrecognised {
        /// The line.
        line: String,
        /// The implementation that printed it.
        from: Implementation,
    },
    /// The blocks differ in a way none of the above named.
    ///
    /// The safety net of [`FinalBlock::normalised`]. Reaching it means the field-by-field
    /// comparison has a gap, so it prints both blocks whole and fails.
    Rendering {
        /// Java CDM's block.
        java: String,
        /// cdm-rs's block.
        rust: String,
    },
}

impl fmt::Display for CounterDifference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Banner { java, rust } => write!(
                f,
                "banner: Java CDM printed {java:?} `#` rules, cdm-rs {rust:?}"
            ),
            Self::RunIdPresence { java, rust } => write!(
                f,
                "RunId line: Java CDM {}, cdm-rs {} (the value is not compared, the line is)",
                present(*java),
                present(*rust)
            ),
            Self::CounterMissing { label, from, value } => write!(
                f,
                "`{label}`: {} printed {value}, {from} does not print this counter at all",
                from.other()
            ),
            Self::Value { label, java, rust } => {
                write!(f, "`{label}`: Java CDM {java}, cdm-rs {rust}")
            }
            Self::Order { java, rust } => {
                write!(f, "counter order: Java CDM {java:?}, cdm-rs {rust:?}")
            }
            Self::LineFormat { label, java, rust } => write!(
                f,
                "`{label}` is formatted differently: Java CDM `{java}`, cdm-rs `{rust}`"
            ),
            Self::Unrecognised { line, from } => {
                write!(f, "{from} printed `{line}`, which MET-006 does not define")
            }
            Self::Rendering { java, rust } => write!(
                f,
                "the blocks differ in a way no field comparison named:\n\
                 --- Java CDM ---\n{java}\n--- cdm-rs ---\n{rust}"
            ),
        }
    }
}

/// "a line" / "no line", for a message.
const fn present(value: bool) -> &'static str {
    if value {
        "prints one"
    } else {
        "does not"
    }
}

/// The result of comparing two counter blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterBlockDiff {
    source: &'static str,
    differences: Vec<CounterDifference>,
}

impl CounterBlockDiff {
    /// Whether the two blocks are identical but for the run id.
    #[must_use]
    pub fn is_identical(&self) -> bool {
        self.differences.is_empty()
    }

    /// The differences.
    pub fn differences(&self) -> &[CounterDifference] {
        &self.differences
    }

    /// Which contract was compared: `MET-006`'s block or `MET-005`'s string.
    #[must_use]
    pub const fn source(&self) -> &'static str {
        self.source
    }

    /// Turns the diff into a result, so a test can `?` it.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] naming the requirement and every difference, when the blocks are
    /// not identical. A difference here is a parity failure of a `[P]` requirement, not a
    /// cosmetic one: `COMPAT-004` makes these strings a public contract, and users' scripts parse
    /// them.
    pub fn into_result(self) -> Result<(), CdmError> {
        if self.is_identical() {
            return Ok(());
        }
        Err(CdmError::new(ErrorKind::Internal, self.to_string()))
    }
}

impl fmt::Display for CounterBlockDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_identical() {
            return write!(f, "{} is identical (TST-020)", self.source);
        }
        writeln!(
            f,
            "{} differs between Java CDM and cdm-rs; this is a parity failure of a [P] \
             requirement, which COMPAT-004 makes a public contract:",
            self.source
        )?;
        for difference in &self.differences {
            writeln!(f, "  - {difference}")?;
        }
        Ok(())
    }
}

/// Compares two `MET-006` final blocks field by field (`TST-020`, `MET-006`).
///
/// Everything the format fixes is compared: the banners and their widths, the presence of the
/// `RunId` line, which counters appear, their order, their values, and the exact text of each
/// line. Only the run id's *value* is ignored, because two runs have two run ids and no
/// comparison of them could ever be meaningful.
///
/// A counter cdm-rs emits and Java does not — or a line formatted differently — is reported as a
/// [`CounterDifference`] and fails the comparison. `MET-002` fixes the registration per job and
/// `MET-006` fixes the rendering, so an extra line is a parity defect and not an improvement.
#[must_use]
pub fn compare_counter_blocks(java: &FinalBlock, rust: &FinalBlock) -> CounterBlockDiff {
    CounterBlockDiff {
        source: "the final counter block (MET-006)",
        differences: block_differences(java, rust),
    }
}

/// Compares two `MET-005` metrics strings field by field (`TST-020`, `MET-005`).
///
/// The same comparison over `Read: 10; Write: 9; Skipped: 1`, which `TRK-021` stores in
/// `cdm_run_info.run_info` where a Java run may read it back.
#[must_use]
pub fn compare_metrics_strings(java: &FinalBlock, rust: &FinalBlock) -> CounterBlockDiff {
    CounterBlockDiff {
        source: "the metrics string (MET-005)",
        differences: block_differences(java, rust),
    }
}

/// Every way two blocks differ.
fn block_differences(java: &FinalBlock, rust: &FinalBlock) -> Vec<CounterDifference> {
    let mut differences = Vec::new();

    let (java_rules, rust_rules) = (java.rules(), rust.rules());
    if java_rules != rust_rules {
        differences.push(CounterDifference::Banner {
            java: java_rules,
            rust: rust_rules,
        });
    }
    if java.run_id().is_some() != rust.run_id().is_some() {
        differences.push(CounterDifference::RunIdPresence {
            java: java.run_id().is_some(),
            rust: rust.run_id().is_some(),
        });
    }
    for (side, block) in [(Implementation::Java, java), (Implementation::Rust, rust)] {
        for line in block.unrecognised() {
            differences.push(CounterDifference::Unrecognised {
                line: line.to_owned(),
                from: side,
            });
        }
    }

    let java_counters: BTreeMap<String, &CounterLine> = java
        .counters()
        .into_iter()
        .map(|line| (line.key(), line))
        .collect();
    let rust_counters: BTreeMap<String, &CounterLine> = rust
        .counters()
        .into_iter()
        .map(|line| (line.key(), line))
        .collect();

    let labels: BTreeSet<&String> = java_counters.keys().chain(rust_counters.keys()).collect();
    for label in labels {
        match (java_counters.get(label), rust_counters.get(label)) {
            (Some(java_line), Some(rust_line)) => {
                if java_line.value == rust_line.value {
                    if java_line.text != rust_line.text {
                        differences.push(CounterDifference::LineFormat {
                            label: label.clone(),
                            java: java_line.text.clone(),
                            rust: rust_line.text.clone(),
                        });
                    }
                } else {
                    differences.push(CounterDifference::Value {
                        label: label.clone(),
                        java: java_line.value,
                        rust: rust_line.value,
                    });
                }
            }
            (Some(java_line), None) => differences.push(CounterDifference::CounterMissing {
                label: label.clone(),
                from: Implementation::Rust,
                value: java_line.value,
            }),
            (None, Some(rust_line)) => differences.push(CounterDifference::CounterMissing {
                label: label.clone(),
                from: Implementation::Java,
                value: rust_line.value,
            }),
            (None, None) => {}
        }
    }

    // Order matters only when both blocks print the same counters; otherwise the missing-counter
    // differences above already say what happened, and an order difference on top of them would
    // be noise.
    let java_order: Vec<String> = java.counters().iter().map(|line| line.key()).collect();
    let rust_order: Vec<String> = rust.counters().iter().map(|line| line.key()).collect();
    if java_counters.keys().eq(rust_counters.keys()) && java_order != rust_order {
        differences.push(CounterDifference::Order {
            java: java_order,
            rust: rust_order,
        });
    }

    // The net. If the two blocks render differently and nothing above noticed, the field-by-field
    // comparison has a gap and the honest thing is to fail with both blocks in the message.
    if differences.is_empty() {
        let (java_text, rust_text) = (java.normalised(), rust.normalised());
        if java_text != rust_text {
            differences.push(CounterDifference::Rendering {
                java: java_text.join("\n"),
                rust: rust_text.join("\n"),
            });
        }
    }

    differences
}

/// Both halves of one differential run's verdict (`TST-020`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DifferentialReport {
    /// The byte-level target-state comparison.
    pub state: TargetStateDiff,
    /// The counter-block comparison.
    pub counters: CounterBlockDiff,
}

impl DifferentialReport {
    /// Whether both comparisons found the two implementations identical.
    #[must_use]
    pub fn is_identical(&self) -> bool {
        self.state.is_identical() && self.counters.is_identical()
    }

    /// Turns the report into a result, so a test can `?` it.
    ///
    /// Both halves are reported even when both failed: "the counters differ *and* so does the
    /// target" and "only the counters differ" point at different bugs.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] rendering both halves, when either differs.
    pub fn into_result(self) -> Result<(), CdmError> {
        if self.is_identical() {
            return Ok(());
        }
        Err(CdmError::new(ErrorKind::Internal, self.to_string()))
    }
}

impl fmt::Display for DifferentialReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.state)?;
        write!(f, "{}", self.counters)
    }
}

// ---------------------------------------------------------------------------------------------
// Reading a target over CQL
// ---------------------------------------------------------------------------------------------

/// One non-key column of a snapshot, and whether its writetime and TTL can be selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueColumn {
    /// The column name.
    pub name: String,
    /// Whether to select `WRITETIME` and `TTL` for it.
    ///
    /// Not every column can be asked: the server rejects `WRITETIME` for a primary-key column, for
    /// a counter column, and — before Cassandra 4.1 — for a non-frozen collection. The caller
    /// declares which columns are eligible, and the *same* declaration is used for both targets,
    /// so it can never hide a difference between them: it can only decline to look at a quantity
    /// the server would not report for either.
    pub timestamps: bool,
}

/// What to read from a target, and how.
///
/// Deliberately explicit rather than introspected: the two targets have the same schema by
/// construction (the corpus creates both), and a spec built once and used for both sides is what
/// guarantees the two `SELECT`s project the same columns in the same order. A spec introspected
/// per side could differ per side, and a comparison whose two halves asked different questions
/// cannot be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSpec {
    keyspace: String,
    table: String,
    key_columns: Vec<String>,
    value_columns: Vec<ValueColumn>,
}

impl SnapshotSpec {
    /// A spec for `keyspace.table` with no columns yet.
    #[must_use]
    pub fn new(keyspace: impl Into<String>, table: impl Into<String>) -> Self {
        Self {
            keyspace: keyspace.into(),
            table: table.into(),
            key_columns: Vec::new(),
            value_columns: Vec::new(),
        }
    }

    /// Adds a primary-key column, in schema order.
    #[must_use]
    pub fn key_column(mut self, name: impl Into<String>) -> Self {
        self.key_columns.push(name.into());
        self
    }

    /// Adds a value column whose writetime and TTL are compared.
    #[must_use]
    pub fn value_column(mut self, name: impl Into<String>) -> Self {
        self.value_columns.push(ValueColumn {
            name: name.into(),
            timestamps: true,
        });
        self
    }

    /// Adds a value column whose writetime and TTL the server will not report.
    ///
    /// See [`ValueColumn::timestamps`] for when that is the case, and why declaring it is not a
    /// relaxation of the comparison.
    #[must_use]
    pub fn value_column_without_timestamps(mut self, name: impl Into<String>) -> Self {
        self.value_columns.push(ValueColumn {
            name: name.into(),
            timestamps: false,
        });
        self
    }

    /// The keyspace.
    pub fn keyspace(&self) -> &str {
        &self.keyspace
    }

    /// The table, unqualified.
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The key columns, in schema order.
    pub fn key_columns(&self) -> &[String] {
        &self.key_columns
    }

    /// The value columns, in projection order.
    pub fn value_columns(&self) -> &[ValueColumn] {
        &self.value_columns
    }

    /// The `SELECT` that reads one target, in the projection order the reader assumes.
    ///
    /// Keys first in schema order, then, per value column, the value followed by its `WRITETIME`
    /// and `TTL` when they are selectable. No `ALLOW FILTERING`, no `LIMIT`, no ordering: it is a
    /// full-table scan of a corpus, and the comparison is keyed by primary key precisely so that
    /// the order rows come back in does not matter.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the spec has no key columns. A snapshot with no key has no join
    /// key, and every row of it would collide on the empty key.
    pub fn select_statement(&self) -> Result<String, CdmError> {
        if self.key_columns.is_empty() {
            return Err(CdmError::new(
                ErrorKind::Internal,
                format!(
                    "the snapshot spec for {}.{} declares no key columns, so its rows could not \
                     be paired between the two targets",
                    self.keyspace, self.table
                ),
            ));
        }
        let mut projection: Vec<String> = self.key_columns.iter().map(|c| quote_ident(c)).collect();
        for column in &self.value_columns {
            let quoted = quote_ident(&column.name);
            projection.push(quoted.clone());
            if column.timestamps {
                projection.push(format!("WRITETIME({quoted})"));
                projection.push(format!("TTL({quoted})"));
            }
        }
        Ok(format!(
            "SELECT {} FROM {}.{}",
            projection.join(", "),
            quote_ident(&self.keyspace),
            quote_ident(&self.table),
        ))
    }

    /// How many columns [`SnapshotSpec::select_statement`] projects.
    #[must_use]
    pub fn projected_columns(&self) -> usize {
        self.key_columns.len()
            + self
                .value_columns
                .iter()
                .map(|column| if column.timestamps { 3 } else { 1 })
                .sum::<usize>()
    }

    /// An empty snapshot shaped by this spec.
    #[must_use]
    pub fn empty_snapshot(&self) -> TargetSnapshot {
        TargetSnapshot::new(
            format!("{}.{}", self.keyspace, self.table),
            self.key_columns.clone(),
        )
    }
}

/// Quotes a CQL identifier, doubling any embedded quote (`SCH-010`).
///
/// Always quoted, never conditionally: the corpus of `TST-020` covers quoted, hyphenated and
/// case-sensitive identifiers, and a reader that quoted only when it thought it had to would be
/// one more thing that has to be right for the comparison to mean anything.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Decodes a `bigint` cell — a `WRITETIME` — from its wire bytes.
#[cfg(any(feature = "differential", test))]
fn writetime_of(bytes: Option<&[u8]>) -> Result<CellTime<i64>, CdmError> {
    match bytes {
        None => Ok(CellTime::Absent),
        Some(raw) => {
            let array: [u8; 8] = raw.try_into().map_err(|_| {
                CdmError::new(
                    ErrorKind::Read,
                    format!("a WRITETIME came back as {} bytes, not 8", raw.len()),
                )
            })?;
            Ok(CellTime::Value(i64::from_be_bytes(array)))
        }
    }
}

/// Decodes an `int` cell — a `TTL` — from its wire bytes.
#[cfg(any(feature = "differential", test))]
fn ttl_of(bytes: Option<&[u8]>) -> Result<CellTime<i32>, CdmError> {
    match bytes {
        None => Ok(CellTime::Absent),
        Some(raw) => {
            let array: [u8; 4] = raw.try_into().map_err(|_| {
                CdmError::new(
                    ErrorKind::Read,
                    format!("a TTL came back as {} bytes, not 4", raw.len()),
                )
            })?;
            Ok(CellTime::Value(i32::from_be_bytes(array)))
        }
    }
}

/// Opens a session to one target node over the native protocol (`TST-020`).
///
/// The counterpart of [`snapshot_target`], and the reason it exists here rather than in the caller:
/// `xtask` orchestrates the differential run but may not depend on `cdm-cql` — `ARCHITECTURE.md` §3
/// keeps the driver behind that one crate — so the crate that owns the snapshot owns the connection
/// as well. Behind the same off-by-default `differential` feature, on the same terms.
///
/// A default [`CdmConfig`](cdm_config::model::CdmConfig) with only the address changed: no
/// authentication and no TLS, because this connects to the containerised node that
/// `bench/java-comparison/environment/` starts, and a knob here that the two halves could be given
/// differently is a knob that could explain a difference the harness then reports as a defect.
///
/// # Errors
///
/// Whatever [`cdm_cql::connect::connect`] reports: an unreachable node, or a configuration the
/// driver rejects.
#[cfg(feature = "differential")]
// The driver's `build()` future is large and `connect` awaits it, exactly as in `macrobench`.
#[allow(clippy::large_futures)]
pub async fn connect_target(
    host: &str,
    port: u16,
) -> Result<cdm_cql::connect::ClusterSession, CdmError> {
    let mut config = cdm_config::model::CdmConfig::default();
    config.connect.target.host = host.to_owned();
    config.connect.target.port = port;
    cdm_cql::connect::connect(&config, cdm_core::Side::Target).await
}

/// Reads a whole target table into a [`TargetSnapshot`] (`TST-020`).
///
/// Behind the off-by-default `differential` feature, because it needs `cdm-cql` — see the module
/// documentation and `ARCHITECTURE.md` §3.3.
///
/// The rows come back as [`cdm_cql::raw::RawRow`], the same undeserialised view the zero-copy
/// passthrough of `MIG-040` uses, so the bytes that reach the comparison are the bytes the server
/// sent: no codec, no `CqlValue`, nothing that could make two different encodings look like one
/// value.
///
/// # Errors
///
/// [`ErrorKind::Read`] if the `SELECT` fails or a row cannot be read, [`ErrorKind::Internal`] if
/// a row has fewer columns than the spec projected, or if two rows share a primary key.
#[cfg(feature = "differential")]
pub async fn snapshot_target(
    session: &cdm_cql::connect::ClusterSession,
    spec: &SnapshotSpec,
) -> Result<TargetSnapshot, CdmError> {
    use cdm_cql::raw::RawRow;

    let statement = spec.select_statement()?;
    let result = session
        .session()
        .query_unpaged(statement.clone(), &[])
        .await
        .map_err(|error| {
            CdmError::new(
                ErrorKind::Read,
                format!("the target snapshot query failed: {error}"),
            )
        })?;
    let rows = result.into_rows_result().map_err(|error| {
        CdmError::new(
            ErrorKind::Read,
            format!("the target snapshot query returned no rows result: {error}"),
        )
    })?;
    let typed = rows.rows::<RawRow<'_, '_>>().map_err(|error| {
        CdmError::new(
            ErrorKind::Read,
            format!("the target snapshot page could not be typed: {error}"),
        )
    })?;

    let mut snapshot = spec.empty_snapshot();
    for row in typed {
        let row = row.map_err(|error| {
            CdmError::new(
                ErrorKind::Read,
                format!("a target row could not be read: {error}"),
            )
        })?;
        let cells = row.cells();
        if cells.len() < spec.projected_columns() {
            return Err(CdmError::new(
                ErrorKind::Internal,
                format!(
                    "`{statement}` projected {} columns but a row came back with {}",
                    spec.projected_columns(),
                    cells.len()
                ),
            ));
        }

        let mut index = 0;
        let mut key_values = Vec::with_capacity(spec.key_columns.len());
        for _ in &spec.key_columns {
            key_values.push(owned_cell(cells.get(index).and_then(|cell| cell.bytes)));
            index += 1;
        }
        let mut snapshot_row = RowSnapshot::new(PrimaryKey::new(key_values));

        for column in &spec.value_columns {
            let value = owned_cell(cells.get(index).and_then(|cell| cell.bytes));
            index += 1;
            let mut cell = CellSnapshot::new(value);
            if column.timestamps {
                cell = cell
                    .with_writetime(writetime_of(cells.get(index).and_then(|cell| cell.bytes))?)
                    .with_ttl(ttl_of(cells.get(index + 1).and_then(|cell| cell.bytes))?);
                index += 2;
            }
            snapshot_row.cells.insert(column.name.clone(), cell);
        }

        snapshot.insert(snapshot_row)?;
    }
    Ok(snapshot)
}

/// Copies a frame slice into an owned cell.
///
/// The copy is unavoidable and deliberate: the snapshot outlives the response frame, and a
/// comparison keyed by primary key has to hold both sides at once anyway.
#[cfg(feature = "differential")]
fn owned_cell(bytes: Option<&[u8]>) -> RawCell {
    bytes.map_or(RawCell::NULL, |bytes| RawCell::new(bytes.to_vec()))
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

    /// A key of one text component.
    fn key(value: &str) -> PrimaryKey {
        PrimaryKey::new(vec![RawCell::new(value.as_bytes().to_vec())])
    }

    /// A one-column snapshot of `ks.t`, keyed by `pk`.
    fn snapshot(rows: Vec<RowSnapshot>) -> TargetSnapshot {
        let mut snapshot = TargetSnapshot::new("ks.t", vec!["pk".to_owned()]);
        for row in rows {
            snapshot.insert(row).unwrap();
        }
        snapshot
    }

    fn row(pk: &str, cell: CellSnapshot) -> RowSnapshot {
        RowSnapshot::new(key(pk)).with_cell("v", cell)
    }

    // --- target state -------------------------------------------------------------------------

    #[test]
    fn tst_020_two_identical_targets_compare_identical() {
        let cell = CellSnapshot::new(RawCell::new(vec![0, 0, 0, 1]))
            .with_writetime(CellTime::Value(1_712_345_678_901_234))
            .with_ttl(CellTime::Value(60));
        let diff = compare_target_state(
            &snapshot(vec![row("a", cell.clone()), row("b", cell.clone())]),
            &snapshot(vec![row("b", cell.clone()), row("a", cell)]),
            &CompareOptions::new(),
        );
        assert!(diff.is_identical(), "{diff}");
        assert_eq!(diff.row_counts(), (2, 2));
    }

    #[test]
    fn tst_020_two_byte_sequences_that_decode_equal_are_not_equal() {
        // `varint` 1 is `0x01`; `0x0001` is the same integer with a redundant leading byte, and
        // every decoder in existence calls them equal. This is the bug class the harness is for:
        // a comparator that decoded would pass this.
        let java = snapshot(vec![row("a", CellSnapshot::new(RawCell::new(vec![0x01])))]);
        let rust = snapshot(vec![row(
            "a",
            CellSnapshot::new(RawCell::new(vec![0x00, 0x01])),
        )]);

        let diff = compare_target_state(&java, &rust, &CompareOptions::new());
        assert!(!diff.is_identical());
        assert_eq!(diff.total(), 1);
        match &diff.differences()[0] {
            Difference::Value {
                key,
                column,
                java,
                rust,
            } => {
                assert_eq!(key.values().len(), 1);
                assert_eq!(column, "v");
                assert_eq!(java.bytes().map(|b| &b[..]), Some([0x01].as_slice()));
                assert_eq!(rust.bytes().map(|b| &b[..]), Some([0x00, 0x01].as_slice()));
            }
            other => panic!("expected a value difference, got {other:?}"),
        }
        // The report names the key, the column and both byte strings.
        let rendered = diff.to_string();
        assert!(rendered.contains("column `v`"), "{rendered}");
        assert!(rendered.contains("0x01"), "{rendered}");
        assert!(rendered.contains("0x0001"), "{rendered}");
    }

    #[test]
    fn tst_020_a_null_cell_and_an_empty_cell_are_not_equal() {
        // MIG-012 turns on exactly this distinction: binding NULL where the value is empty writes
        // a tombstone.
        let java = snapshot(vec![row("a", CellSnapshot::new(RawCell::NULL))]);
        let rust = snapshot(vec![row("a", CellSnapshot::new(RawCell::new(Vec::new())))]);
        let diff = compare_target_state(&java, &rust, &CompareOptions::new());
        assert_eq!(diff.total(), 1);
        assert!(diff.to_string().contains("null"), "{diff}");
    }

    #[test]
    fn tst_020_a_missing_row_is_reported_by_key_and_side() {
        let cell = CellSnapshot::new(RawCell::new(vec![7]));
        let java = snapshot(vec![row("a", cell.clone()), row("b", cell.clone())]);
        let rust = snapshot(vec![row("a", cell)]);

        let diff = compare_target_state(&java, &rust, &CompareOptions::new());
        assert!(!diff.is_identical());
        // The row count and the missing row are both reported: they are different facts.
        assert_eq!(diff.total(), 2);
        assert!(diff.differences().contains(&Difference::RowMissing {
            key: key("b"),
            from: Implementation::Rust,
        }));
        assert!(diff.to_string().contains("absent from cdm-rs"), "{diff}");
    }

    #[test]
    fn tst_020_an_extra_row_in_the_rust_target_is_reported_too() {
        let cell = CellSnapshot::new(RawCell::new(vec![7]));
        let diff = compare_target_state(
            &snapshot(vec![row("a", cell.clone())]),
            &snapshot(vec![row("a", cell.clone()), row("z", cell)]),
            &CompareOptions::new(),
        );
        assert!(diff.differences().contains(&Difference::RowMissing {
            key: key("z"),
            from: Implementation::Java,
        }));
    }

    #[test]
    fn tst_020_identical_values_with_different_writetimes_are_not_identical_target_state() {
        let value = RawCell::new(vec![1, 2, 3]);
        let java = snapshot(vec![row(
            "a",
            CellSnapshot::new(value.clone()).with_writetime(CellTime::Value(1_000)),
        )]);
        let rust = snapshot(vec![row(
            "a",
            CellSnapshot::new(value).with_writetime(CellTime::Value(1_001)),
        )]);

        let diff = compare_target_state(&java, &rust, &CompareOptions::new());
        assert_eq!(diff.total(), 1);
        assert!(matches!(
            diff.differences()[0],
            Difference::Writetime { .. }
        ));
        assert!(diff.to_string().contains("WRITETIME"), "{diff}");
    }

    #[test]
    fn tst_020_a_ttl_that_exists_on_one_side_only_differs_at_any_tolerance() {
        let value = RawCell::new(vec![1]);
        let java = snapshot(vec![row(
            "a",
            CellSnapshot::new(value.clone()).with_ttl(CellTime::Value(60)),
        )]);
        let rust = snapshot(vec![row(
            "a",
            CellSnapshot::new(value).with_ttl(CellTime::Absent),
        )]);

        let diff = compare_target_state(&java, &rust, &CompareOptions::new().ttl_within(86_400));
        assert_eq!(diff.total(), 1, "{diff}");
        assert!(matches!(diff.differences()[0], Difference::Ttl { .. }));
    }

    #[test]
    fn tst_020_a_ttl_skew_is_a_difference_unless_the_test_says_otherwise() {
        let value = RawCell::new(vec![1]);
        let java = snapshot(vec![row(
            "a",
            CellSnapshot::new(value.clone()).with_ttl(CellTime::Value(60)),
        )]);
        let rust = snapshot(vec![row(
            "a",
            CellSnapshot::new(value).with_ttl(CellTime::Value(58)),
        )]);

        // The default is exact, so a two-second skew fails.
        assert_eq!(
            compare_target_state(&java, &rust, &CompareOptions::new()).total(),
            1
        );
        // One second of slack is not enough for two.
        assert_eq!(
            compare_target_state(&java, &rust, &CompareOptions::new().ttl_within(1)).total(),
            1
        );
        assert!(
            compare_target_state(&java, &rust, &CompareOptions::new().ttl_within(2)).is_identical()
        );
    }

    #[test]
    fn tst_020_a_column_present_on_one_side_only_is_reported() {
        let java = snapshot(vec![RowSnapshot::new(key("a"))
            .with_cell("v", CellSnapshot::new(RawCell::new(vec![1])))
            .with_cell("extra", CellSnapshot::new(RawCell::new(vec![2])))]);
        let rust = snapshot(vec![row("a", CellSnapshot::new(RawCell::new(vec![1])))]);

        let diff = compare_target_state(&java, &rust, &CompareOptions::new());
        assert_eq!(diff.total(), 1);
        assert!(diff.differences().contains(&Difference::ColumnMissing {
            key: key("a"),
            column: "extra".to_owned(),
            from: Implementation::Rust,
        }));
    }

    #[test]
    fn tst_020_an_excluded_column_is_named_in_the_report_with_its_migration_row() {
        let java = snapshot(vec![row("a", CellSnapshot::new(RawCell::new(vec![1])))]);
        let rust = snapshot(vec![row("a", CellSnapshot::new(RawCell::new(vec![2])))]);

        // Without the exclusion the column differs, which is the point: the exclusion is doing
        // work, not describing something that already agreed.
        assert!(!compare_target_state(&java, &rust, &CompareOptions::new()).is_identical());

        let options = CompareOptions::new().excluding_column("v", 6);
        assert!(compare_target_state(&java, &rust, &options).is_identical());

        // And any report rendered under those options says what it did not look at, and on whose
        // authority, so an exclusion cannot quietly accumulate.
        let with_another_difference = compare_target_state(
            &java,
            &snapshot(vec![
                row("a", CellSnapshot::new(RawCell::new(vec![2]))),
                row("b", CellSnapshot::new(RawCell::new(vec![3]))),
            ]),
            &options,
        );
        assert!(
            with_another_difference
                .to_string()
                .contains("excluded: `v` (MIGRATION_FROM_JAVA.md row 6)"),
            "{with_another_difference}"
        );
    }

    #[test]
    fn tst_020_the_report_is_capped_but_the_count_is_not() {
        let mut java = TargetSnapshot::new("ks.t", vec!["pk".to_owned()]);
        let mut rust = TargetSnapshot::new("ks.t", vec!["pk".to_owned()]);
        for index in 0..50_u8 {
            java.insert(row(
                &format!("k{index}"),
                CellSnapshot::new(RawCell::new(vec![index])),
            ))
            .unwrap();
            rust.insert(row(
                &format!("k{index}"),
                CellSnapshot::new(RawCell::new(vec![index.wrapping_add(1)])),
            ))
            .unwrap();
        }

        let diff = compare_target_state(&java, &rust, &CompareOptions::new().max_differences(5));
        assert_eq!(diff.total(), 50);
        assert_eq!(diff.differences().len(), 5);
        assert!(diff.truncated());
        assert!(diff.to_string().contains("45 more"), "{diff}");
        assert!(diff.into_result().is_err());
    }

    #[test]
    fn tst_020_redaction_hides_the_value_and_still_shows_the_difference() {
        let java = snapshot(vec![row(
            "a",
            CellSnapshot::new(RawCell::new(b"hunter2".to_vec())),
        )]);
        let rust = snapshot(vec![row(
            "a",
            CellSnapshot::new(RawCell::new(b"hunter3".to_vec())),
        )]);

        let diff = compare_target_state(&java, &rust, &CompareOptions::new().redacted());
        let rendered = diff.to_string();
        assert!(!rendered.contains("hunter"), "{rendered}");
        assert!(rendered.contains("7 bytes"), "{rendered}");
        assert!(rendered.contains("column `v`"), "{rendered}");
    }

    #[test]
    fn tst_020_two_rows_with_one_key_are_rejected_rather_than_silently_collapsed() {
        let mut snapshot = TargetSnapshot::new("ks.t", vec!["pk".to_owned()]);
        snapshot
            .insert(row("a", CellSnapshot::new(RawCell::new(vec![1]))))
            .unwrap();
        let error = snapshot
            .insert(row("a", CellSnapshot::new(RawCell::new(vec![2]))))
            .unwrap_err();
        assert!(error.to_string().contains("share the primary key"));
        // The rejection message is a diff path too, so it redacts.
        assert!(!error.to_string().contains("0x61"), "{error}");
    }

    #[test]
    fn tst_020_a_differing_table_or_key_shape_is_reported_rather_than_assumed_away() {
        let java = TargetSnapshot::new("ks.t", vec!["pk".to_owned()]);
        let rust = TargetSnapshot::new("ks.other", vec!["pk".to_owned(), "ck".to_owned()]);
        let diff = compare_target_state(&java, &rust, &CompareOptions::new());
        assert_eq!(diff.total(), 2);
        assert!(diff.to_string().contains("ks.other"), "{diff}");
    }

    // --- counter blocks -----------------------------------------------------------------------

    const JAVA_BLOCK: &str = "\
################################################################################################
RunId: 1712345678901234
Final Read Record Count: 1000000
Final Write Record Count: 999998
Final Skipped Record Count: 2
Final Error Record Count: 0
################################################################################################";

    fn rust_block(body: &str) -> String {
        format!(
            "################################################################################################\n\
             RunId: 9999999999999999\n{body}\
             ################################################################################################"
        )
    }

    const RUST_BODY: &str = "\
Final Read Record Count: 1000000
Final Write Record Count: 999998
Final Skipped Record Count: 2
Final Error Record Count: 0
";

    #[test]
    fn met_006_two_identical_blocks_compare_identical_despite_different_run_ids() {
        let java = FinalBlock::parse(JAVA_BLOCK).unwrap();
        let rust = FinalBlock::parse(&rust_block(RUST_BODY)).unwrap();
        assert_ne!(java.run_id(), rust.run_id());

        let diff = compare_counter_blocks(&java, &rust);
        assert!(diff.is_identical(), "{diff}");
        assert!(diff.into_result().is_ok());
    }

    #[test]
    fn met_006_a_counter_cdm_rs_emits_and_java_does_not_is_a_parity_failure() {
        let java = FinalBlock::parse(JAVA_BLOCK).unwrap();
        let rust = FinalBlock::parse(&rust_block(&format!(
            "{RUST_BODY}Final Partitions Passed: 5000\n"
        )))
        .unwrap();

        let diff = compare_counter_blocks(&java, &rust);
        assert!(!diff.is_identical());
        assert!(diff
            .differences()
            .contains(&CounterDifference::CounterMissing {
                label: "Final Partitions Passed".to_owned(),
                from: Implementation::Java,
                value: 5000,
            }));
        let rendered = diff.into_result().unwrap_err().to_string();
        assert!(rendered.contains("MET-006"), "{rendered}");
        assert!(rendered.contains("COMPAT-004"), "{rendered}");
    }

    #[test]
    fn met_006_a_differing_counter_value_is_reported_by_name() {
        let java = FinalBlock::parse(JAVA_BLOCK).unwrap();
        let rust = FinalBlock::parse(&rust_block(&RUST_BODY.replace("999998", "999997"))).unwrap();
        let diff = compare_counter_blocks(&java, &rust);
        assert!(diff.differences().contains(&CounterDifference::Value {
            label: "Final Write Record Count".to_owned(),
            java: 999_998,
            rust: 999_997,
        }));
    }

    #[test]
    fn met_006_a_line_formatted_differently_is_reported_as_a_format_difference() {
        // Same counter, same value, two spaces. `cdm-assert.sh` greps these lines; a script that
        // matches `Final Read Record Count: ` would stop matching.
        let java = FinalBlock::parse(JAVA_BLOCK).unwrap();
        let rust = FinalBlock::parse(&rust_block(
            &RUST_BODY.replace("Final Read Record Count", "Final  Read  Record Count"),
        ))
        .unwrap();

        let diff = compare_counter_blocks(&java, &rust);
        assert_eq!(diff.differences().len(), 1, "{diff}");
        assert!(matches!(
            diff.differences()[0],
            CounterDifference::LineFormat { .. }
        ));
    }

    #[test]
    fn met_006_counters_in_a_different_order_are_reported() {
        let java = FinalBlock::parse(JAVA_BLOCK).unwrap();
        let rust = FinalBlock::parse(&rust_block(
            "Final Write Record Count: 999998\n\
             Final Read Record Count: 1000000\n\
             Final Skipped Record Count: 2\n\
             Final Error Record Count: 0\n",
        ))
        .unwrap();

        let diff = compare_counter_blocks(&java, &rust);
        assert_eq!(diff.differences().len(), 1, "{diff}");
        assert!(matches!(
            diff.differences()[0],
            CounterDifference::Order { .. }
        ));
    }

    #[test]
    fn met_006_an_extra_line_inside_the_banners_is_reported() {
        let java = FinalBlock::parse(JAVA_BLOCK).unwrap();
        let rust = FinalBlock::parse(&rust_block(&format!("{RUST_BODY}Elapsed: 12s\n"))).unwrap();

        let diff = compare_counter_blocks(&java, &rust);
        assert!(diff
            .differences()
            .contains(&CounterDifference::Unrecognised {
                line: "Elapsed: 12s".to_owned(),
                from: Implementation::Rust,
            }));
    }

    #[test]
    fn met_006_a_narrower_banner_is_reported() {
        let java = FinalBlock::parse(JAVA_BLOCK).unwrap();
        let rust = FinalBlock::parse(&format!("##########\n{RUST_BODY}##########")).unwrap();
        let diff = compare_counter_blocks(&java, &rust);
        assert!(diff
            .differences()
            .iter()
            .any(|d| matches!(d, CounterDifference::Banner { .. })));
        // The missing RunId line is a separate, separately reported fact.
        assert!(diff
            .differences()
            .iter()
            .any(|d| matches!(d, CounterDifference::RunIdPresence { .. })));
    }

    #[test]
    fn met_006_a_log_prefix_is_the_only_thing_normalised_away() {
        let prefixed = "\
2026-08-14T12:00:00Z  INFO cdm::metrics: ################################################################################################
2026-08-14T12:00:00Z  INFO cdm::metrics: RunId: 42
2026-08-14T12:00:00Z  INFO cdm::metrics: Final Read Record Count: 1000000
2026-08-14T12:00:00Z  INFO cdm::metrics: Final Write Record Count: 999998
2026-08-14T12:00:00Z  INFO cdm::metrics: Final Skipped Record Count: 2
2026-08-14T12:00:00Z  INFO cdm::metrics: Final Error Record Count: 0
2026-08-14T12:00:00Z  INFO cdm::metrics: ################################################################################################";
        let diff = compare_counter_blocks(
            &FinalBlock::parse(JAVA_BLOCK).unwrap(),
            &FinalBlock::parse(prefixed).unwrap(),
        );
        assert!(diff.is_identical(), "{diff}");
    }

    #[test]
    fn met_006_a_run_that_printed_no_block_is_a_failure_not_an_empty_match() {
        // Two absent blocks must not compare equal: that is how a parity assertion silently stops
        // asserting anything (the reasoning of ENG-008).
        let error = FinalBlock::parse("the job died\n").unwrap_err();
        assert!(error.to_string().contains("MET-006"), "{error}");
    }

    #[test]
    fn met_005_the_metrics_string_is_compared_entry_by_entry() {
        let java = FinalBlock::parse_metrics_string("Read: 10; Write: 9; Skipped: 1").unwrap();
        let same = FinalBlock::parse_metrics_string("Read: 10; Write: 9; Skipped: 1").unwrap();
        assert!(compare_metrics_strings(&java, &same).is_identical());

        let different = FinalBlock::parse_metrics_string("Read: 10; Write: 8; Skipped: 1").unwrap();
        let diff = compare_metrics_strings(&java, &different);
        assert!(diff.differences().contains(&CounterDifference::Value {
            label: "Write".to_owned(),
            java: 9,
            rust: 8,
        }));
        assert!(diff.to_string().contains("MET-005"), "{diff}");
    }

    #[test]
    fn met_005_a_segment_that_is_not_a_counter_is_a_parse_failure() {
        let error = FinalBlock::parse_metrics_string("Read: 10; Write: nine").unwrap_err();
        assert!(error.to_string().contains("MET-005"), "{error}");
    }

    #[test]
    fn met_006_any_remaining_textual_difference_is_caught_by_the_net() {
        // Every field agrees — same banners, same counters, same values, same order, a RunId line
        // on both sides — and yet the blocks are not the same block: cdm-rs printed the RunId
        // after the counters instead of before them. Java prints it first, so this is a MET-006
        // difference, and it is the field comparison's blind spot by construction. The net exists
        // so that a blind spot fails rather than passes.
        let java = FinalBlock::parse(JAVA_BLOCK).unwrap();
        let rust = FinalBlock::parse(&format!(
            "################################################################################################\n\
             {RUST_BODY}RunId: 9999999999999999\n\
             ################################################################################################"
        ))
        .unwrap();

        let diff = compare_counter_blocks(&java, &rust);
        assert_eq!(diff.differences().len(), 1, "{diff}");
        assert!(matches!(
            diff.differences()[0],
            CounterDifference::Rendering { .. }
        ));
        assert!(
            diff.to_string().contains("no field comparison named"),
            "{diff}"
        );
    }

    // --- the whole verdict --------------------------------------------------------------------

    #[test]
    fn tst_020_the_report_fails_when_either_half_does() {
        let identical_state = compare_target_state(
            &snapshot(vec![row("a", CellSnapshot::new(RawCell::new(vec![1])))]),
            &snapshot(vec![row("a", CellSnapshot::new(RawCell::new(vec![1])))]),
            &CompareOptions::new(),
        );
        let differing_counters = compare_counter_blocks(
            &FinalBlock::parse(JAVA_BLOCK).unwrap(),
            &FinalBlock::parse(&rust_block(&RUST_BODY.replace("1000000", "999999"))).unwrap(),
        );

        let report = DifferentialReport {
            state: identical_state,
            counters: differing_counters,
        };
        assert!(!report.is_identical());
        let error = report.into_result().unwrap_err().to_string();
        assert!(error.contains("byte-identical"), "{error}");
        assert!(error.contains("Final Read Record Count"), "{error}");
    }

    // --- the SELECT the reader builds -----------------------------------------------------------

    #[test]
    fn tst_020_the_snapshot_select_projects_value_writetime_and_ttl_in_a_fixed_order() {
        let spec = SnapshotSpec::new("cdm_diff", "corpus")
            .key_column("pk")
            .key_column("ck")
            .value_column("v")
            .value_column_without_timestamps("collection");

        assert_eq!(
            spec.select_statement().unwrap(),
            "SELECT \"pk\", \"ck\", \"v\", WRITETIME(\"v\"), TTL(\"v\"), \"collection\" \
             FROM \"cdm_diff\".\"corpus\""
        );
        assert_eq!(spec.projected_columns(), 6);
        assert_eq!(spec.empty_snapshot().key_columns(), ["pk", "ck"]);
    }

    #[test]
    fn tst_020_a_quoted_identifier_survives_the_projection() {
        let spec = SnapshotSpec::new("ks", "My-Table")
            .key_column("Key\"1")
            .value_column_without_timestamps("v");
        assert_eq!(
            spec.select_statement().unwrap(),
            "SELECT \"Key\"\"1\", \"v\" FROM \"ks\".\"My-Table\""
        );
    }

    #[test]
    fn tst_020_a_spec_with_no_key_columns_is_rejected() {
        let error = SnapshotSpec::new("ks", "t")
            .value_column("v")
            .select_statement()
            .unwrap_err();
        assert!(error.to_string().contains("no key columns"), "{error}");
    }

    #[test]
    fn tst_020_writetime_and_ttl_decode_from_the_wire_form() {
        assert_eq!(
            writetime_of(Some(&1_712_345_678_901_234_i64.to_be_bytes())).unwrap(),
            CellTime::Value(1_712_345_678_901_234)
        );
        assert_eq!(writetime_of(None).unwrap(), CellTime::Absent);
        assert_eq!(
            ttl_of(Some(&60_i32.to_be_bytes())).unwrap(),
            CellTime::Value(60)
        );
        assert!(writetime_of(Some(&[0, 1])).is_err());
        assert!(ttl_of(Some(&[0, 1])).is_err());
    }
}
