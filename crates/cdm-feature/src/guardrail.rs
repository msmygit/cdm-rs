//! The column-size guardrail (`GRD-001`..`GRD-004`).
//!
//! One question, asked of every origin row: *is any single column bigger than the operator said it
//! should be?* A `yes` is a finding to report, never an error — the run carries on and the row is
//! counted `LARGE`.
//!
//! # kB, not KiB
//!
//! `feature.guardrail.column_size_kb` is multiplied by **1000**, not 1024. Java's
//! `Guardrail.BASE_FACTOR` is `1000` and `GRD-002` requires parity, so `column_size_kb = 1`
//! means *1000 bytes* and a 1024-byte column is over the limit. Operators comparing cdm-rs against
//! a KiB-based tool will see a 2.4% difference and it is this, not a rounding bug:
//! [`BYTES_PER_KB`] is the whole of it.
//!
//! # A size is a length, not a decoded value
//!
//! Java computes a column's size by decoding the cell into a Java object and re-encoding it
//! (`CqlTable.byteCount` → `codecFor(type).encode(object, …).remaining()`). Every driver codec
//! round-trips exactly, so the number that comes out is the length of the bytes that went in —
//! which cdm-rs reads directly off the frame. The answer is identical for every CQL type, and
//! obtaining it costs nothing on precisely the multi-megabyte rows this job exists to find.
//!
//! Two consequences worth stating, because they are what makes the shortcut safe:
//!
//! * the accounting is **uniform** across the type system. A `map<text,text>`, a `list<frozen<udt>>`,
//!   a `tuple<…>`, a `vector<float, 1536>` and a `blob` are all just a length, so there is no type
//!   for which the guardrail can be wrong, and no new CQL type can arrive and be measured wrongly;
//! * the size measured is that of the **whole column**, not of the largest element inside it. Java
//!   is the same: `SIT/features/05_guardrail` seeds a row whose *map key* is oversized and expects
//!   the whole `fruits` column to be reported.
//!
//! # `SEC-002` is structural here
//!
//! This module never sees a column value. [`RowSizes`] carries lengths and the primary key, and
//! nothing else; a [`Finding`] is built from a [`RowSizes`], so there is no value in scope that a
//! log statement could accidentally interpolate. That matters more here than anywhere else in
//! cdm-rs: the guardrail's entire output is a report about specific rows, which makes it the most
//! likely place for row data to reach a log. The primary key is rendered as hex by
//! [`PrimaryKey`]'s `Display`, which is the convention `ARCHITECTURE.md` §13 and the `ERR-005`
//! correction settled on for identifying a failing row.
//!
//! # This is a read-only feature
//!
//! Nothing here can write. The plan holds column names, a threshold and a mode; there is no
//! session, no statement and no sink anywhere in the type. `GRD-001`'s "origin only" is therefore
//! not a rule the guardrail job has to remember to follow — it is the only thing this type is
//! capable of.

use std::fmt;
use std::sync::Arc;

use cdm_core::{
    CdmError, Diagnostic, EffectiveConfig, ErrorKind, GuardrailPlugin, Plugin, PrimaryKey, Record,
    Row, TableRef,
};

use crate::properties::{self, GUARDRAIL_COLUMN_SIZE_KB, GUARDRAIL_MODE};
use crate::schema::TableFacts;
use crate::{diagnostic, PROVIDER};

/// The bytes-per-kilobyte factor `GRD-002` requires: Java's `Guardrail.BASE_FACTOR`.
///
/// A thousand, not 1024. See the module documentation.
pub const BYTES_PER_KB: f64 = 1000.0;

/// The registration key this guardrail is known by in the plugin registry (`PLG-003`).
pub const COLUMN_SIZE_GUARDRAIL: &str = "column-size";

/// The diagnostic code a guardrail finding carries (`ERR-003`).
///
/// Its own code rather than one of [`ErrorKind`]'s, because a finding is not an error of any kind:
/// nothing failed, the run is not degraded, and a caller filtering on `CDM-CONFIG` or `CDM-READ`
/// would be right to be surprised to see one.
pub const GUARDRAIL_DIAGNOSTIC_CODE: &str = "CDM-GUARDRAIL";

/// What an inline guardrail violation does to the row that caused it (`GRD-004`).
///
/// Mirrors `cdm-config`'s `GuardrailMode` value for value. It is restated here rather than
/// imported because `ARCHITECTURE.md` §3 has no `cdm-feature` → `cdm-config` edge: a feature is
/// configured from a flat [`EffectiveConfig`], which is what lets every one of them be unit-tested
/// without building a `CdmConfig`. [`GuardrailMode::parse`] accepts exactly the spellings the
/// configuration enum serialises to, and a test pins them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GuardrailMode {
    /// Count the row `LARGE`, report it at `ERROR`, and process it as normal. Java's behaviour,
    /// and the only one the standalone guardrail job can have.
    #[default]
    Check,
    /// As [`GuardrailMode::Check`], but the finding is reported at `WARN`.
    Warn,
    /// Count the row `LARGE` and skip it: nothing is written for it (`GRD-004`).
    Block,
}

impl GuardrailMode {
    /// Every accepted spelling, in declaration order.
    pub const VARIANTS: &'static [&'static str] = &["check", "warn", "block"];

    /// The configured spelling of this mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Warn => "warn",
            Self::Block => "block",
        }
    }

    /// Parses a configured mode, case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] naming the canonical key and every accepted spelling. An
    /// unrecognised mode is refused rather than coerced, because `block` and `check` differ in
    /// whether rows are written: guessing wrong either loses data or writes data the operator
    /// asked to have withheld.
    pub fn parse(value: &str) -> Result<Self, CdmError> {
        let trimmed = value.trim();
        for mode in [Self::Check, Self::Warn, Self::Block] {
            if trimmed.eq_ignore_ascii_case(mode.as_str()) {
                return Ok(mode);
            }
        }
        Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "`{}` must be one of {}, got `{trimmed}`",
                GUARDRAIL_MODE.canonical(),
                Self::VARIANTS.join(", ")
            ),
        )
        .with_context(|c| c.with_config_key(GUARDRAIL_MODE.canonical())))
    }

    /// Whether a row that violates the guardrail must be withheld from the target (`GRD-004`).
    #[must_use]
    pub const fn blocks(self) -> bool {
        matches!(self, Self::Block)
    }
}

impl fmt::Display for GuardrailMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The guardrail as configured, before it has met a schema (`GRD-002`, `GRD-004`).
///
/// The two-phase shape every feature in this crate has: this type holds configuration,
/// [`ColumnSizeGuardrail`] holds the resolved plan that the hot path uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guardrail {
    column_size_kb: f64,
    mode: GuardrailMode,
    compat_java: bool,
}

impl Default for Guardrail {
    /// Disabled, checking, and not in Java-compatibility mode — the state an unconfigured run has.
    fn default() -> Self {
        Self {
            column_size_kb: 0.0,
            mode: GuardrailMode::Check,
            compat_java: false,
        }
    }
}

impl Guardrail {
    /// Reads `feature.guardrail.column_size_kb` and `feature.guardrail.mode`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if the threshold does not parse as a number or the mode is
    /// not one of [`GuardrailMode::VARIANTS`]. A *negative* threshold is not rejected here: it is
    /// a [`Diagnostic`] from [`Guardrail::validate`] instead, so that `CFG-035` and every other
    /// finding reach the operator in one pass (`CFG-021`).
    pub fn load(config: &EffectiveConfig) -> Result<Self, CdmError> {
        let column_size_kb = properties::float(config, GUARDRAIL_COLUMN_SIZE_KB)?.unwrap_or(0.0);
        let mode = match properties::trimmed(config, GUARDRAIL_MODE) {
            Some(value) => GuardrailMode::parse(&value)?,
            None => GuardrailMode::default(),
        };
        Ok(Self {
            column_size_kb,
            mode,
            compat_java: false,
        })
    }

    /// Restores Java's truncated report format (`GRD-003`, `COMPAT-001`).
    #[must_use]
    pub const fn with_compat_java(mut self, compat_java: bool) -> Self {
        self.compat_java = compat_java;
        self
    }

    /// The configured threshold, in kilobytes of 1000 bytes.
    #[must_use]
    pub const fn column_size_kb(&self) -> f64 {
        self.column_size_kb
    }

    /// The configured inline mode (`GRD-004`).
    #[must_use]
    pub const fn mode(&self) -> GuardrailMode {
        self.mode
    }

    /// The threshold in bytes: the value a column's length must **exceed** to be reported.
    ///
    /// Kept as an `f64` rather than rounded to an integer so that the comparison is bit-for-bit
    /// the one Java makes (`colSize > colSizeInKB * BASE_FACTOR`, an `int` promoted to `double`).
    #[must_use]
    pub fn threshold_bytes(&self) -> f64 {
        self.column_size_kb * BYTES_PER_KB
    }

    /// Whether the guardrail does anything, matching Java's `isValid && colSizeInKB > 0`.
    ///
    /// `0` disables it (`CFG-035`), and so does a negative value — which is separately reported as
    /// a blocking diagnostic, so a run never silently proceeds with one.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.column_size_kb > 0.0
    }

    /// Validates the configuration (`CFG-035`).
    ///
    /// A negative threshold is blocking. A threshold of zero is not reported at all: `cdm-config`'s
    /// Tier-1 pass already says the guardrail is disabled, and saying it twice trains operators to
    /// scroll past diagnostics.
    #[must_use]
    pub fn validate(&self, origin: &TableFacts) -> Vec<Diagnostic> {
        let mut findings = Vec::new();
        if self.column_size_kb < 0.0 {
            findings.push(
                diagnostic::config_error("the guardrail column size may not be negative")
                    .with_location(GUARDRAIL_COLUMN_SIZE_KB.canonical())
                    .with_value(self.column_size_kb.to_string())
                    .with_suggestion("use 0 to disable the guardrail"),
            );
        }
        if self.is_enabled() && origin.columns().is_empty() {
            findings.push(
                diagnostic::schema_error(format!(
                    "the guardrail has no columns to measure on origin table {}",
                    origin.table()
                ))
                .with_location(GUARDRAIL_COLUMN_SIZE_KB.canonical()),
            );
        }
        findings
    }

    /// Resolves the guardrail against the **origin** table (`GRD-001`).
    ///
    /// The column names are captured once, in schema order, so that the per-row path indexes
    /// rather than looks up (`ARCHITECTURE.md` §5.5). Passing the target table here would be a
    /// category error — the guardrail measures what the origin holds, and a guardrail run never
    /// opens a target connection at all — which is why this takes one [`TableFacts`] and not a
    /// [`FeatureSchema`](crate::FeatureSchema).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if [`Guardrail::validate`] found anything blocking, so that a
    /// caller which skipped validation cannot end up with a plan built from a negative threshold.
    pub fn resolve(&self, origin: &TableFacts) -> Result<ColumnSizeGuardrail, CdmError> {
        if self.column_size_kb < 0.0 {
            return Err(CdmError::new(
                ErrorKind::Config,
                format!(
                    "`{}` must be greater than or equal to zero, but is {}",
                    GUARDRAIL_COLUMN_SIZE_KB.canonical(),
                    self.column_size_kb
                ),
            )
            .with_context(|c| c.with_config_key(GUARDRAIL_COLUMN_SIZE_KB.canonical())));
        }
        Ok(ColumnSizeGuardrail {
            table: origin.table().clone(),
            columns: origin
                .columns()
                .iter()
                .map(|column| column.name().to_owned())
                .collect(),
            threshold_bytes: self.threshold_bytes(),
            enabled: self.is_enabled(),
            mode: self.mode,
            compat_java: self.compat_java,
        })
    }
}

/// One origin row reduced to exactly what a size check needs (`GRD-002`, `SEC-002`).
///
/// A length per column, in projection order, plus the row's primary key. **No column value.** That
/// is not an optimisation — though it is also that, since a guardrail run that copied every
/// multi-megabyte cell it measured would spend all of its time on the rows it was sent to find —
/// it is what makes `SEC-002` structural: a value that is never in scope cannot be logged.
///
/// A `NULL` column has length `0`, which is what Java's `byteCount` returns for a null object.
/// Since the guardrail is only enabled for a threshold greater than zero, a null column can never
/// be reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RowSizes {
    key: PrimaryKey,
    lengths: Vec<usize>,
}

impl RowSizes {
    /// Builds a row's sizes from its primary key and its per-column lengths, in projection order.
    pub fn new(key: PrimaryKey, lengths: impl IntoIterator<Item = usize>) -> Self {
        Self {
            key,
            lengths: lengths.into_iter().collect(),
        }
    }

    /// Measures a decoded [`Row`], for the inline guardrail of `GRD-004`.
    ///
    /// A migrate or validate job already holds the row, so measuring it costs a walk over the
    /// cells and no copying: [`RawCell::len`](cdm_core::RawCell::len) is a length, not a clone.
    pub fn from_row(key: PrimaryKey, row: &Row) -> Self {
        Self::new(key, row.cells().iter().map(cdm_core::RawCell::len))
    }

    /// Measures the origin side of a [`Record`], for the inline guardrail of `GRD-004`.
    pub fn from_record(record: &Record) -> Self {
        Self::from_row(record.key().clone(), record.origin())
    }

    /// The row's primary key — the only thing that identifies it in a report.
    #[must_use]
    pub const fn key(&self) -> &PrimaryKey {
        &self.key
    }

    /// The per-column lengths, in projection order.
    #[must_use]
    pub fn lengths(&self) -> &[usize] {
        &self.lengths
    }
}

/// One column that exceeded the threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeColumn {
    /// The column's name, as the origin schema spells it.
    pub name: String,
    /// Its serialised length, in bytes.
    pub bytes: usize,
}

/// A row that violated the guardrail, and the columns that did it (`GRD-003`).
///
/// Carries no value, by construction: it is built from a [`RowSizes`], which has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    key: PrimaryKey,
    columns: Vec<LargeColumn>,
}

impl Finding {
    /// The primary key of the offending row.
    #[must_use]
    pub const fn key(&self) -> &PrimaryKey {
        &self.key
    }

    /// The oversized columns, in projection order.
    ///
    /// Order is load-bearing for reproducibility: Java accumulates into a `HashMap` and iterates
    /// it, so two runs over the same row can print the same columns in different orders. Projection
    /// order makes two runs' logs comparable, for the same reason `FEA-020` fixes the explode-map
    /// order.
    #[must_use]
    pub fn columns(&self) -> &[LargeColumn] {
        &self.columns
    }

    /// The largest column in the finding, which is the one an operator acts on first.
    #[must_use]
    pub fn largest(&self) -> Option<&LargeColumn> {
        self.columns.iter().max_by_key(|column| column.bytes)
    }
}

/// The resolved guardrail: everything the per-row check needs, and nothing else.
///
/// Cheap to clone — the column names are shared — so a scheduler may hand one to every worker.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnSizeGuardrail {
    table: TableRef,
    columns: Arc<[String]>,
    threshold_bytes: f64,
    enabled: bool,
    mode: GuardrailMode,
    compat_java: bool,
}

impl ColumnSizeGuardrail {
    /// The origin table this measures.
    #[must_use]
    pub const fn table(&self) -> &TableRef {
        &self.table
    }

    /// The column names, in projection order.
    #[must_use]
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// The threshold in bytes, which a column's length must exceed.
    #[must_use]
    pub const fn threshold_bytes(&self) -> f64 {
        self.threshold_bytes
    }

    /// Whether the guardrail is switched on.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The inline mode (`GRD-004`).
    #[must_use]
    pub const fn mode(&self) -> GuardrailMode {
        self.mode
    }

    /// Checks one row (`GRD-002`).
    ///
    /// Returns `None` for a clean row and for every row when the guardrail is disabled — which is
    /// what makes a disabled guardrail free rather than merely cheap.
    ///
    /// A row wider than the plan's column list is measured only as far as the list goes. That is
    /// deliberate: the extra positions are the virtual `TTL(col)` / `WRITETIME(col)` columns of
    /// `SCH-007`, which are a fixed four or eight bytes of cell metadata and not data the operator
    /// can do anything about. A row *narrower* than the list is measured as far as it goes, so a
    /// projection that skipped columns cannot make the check panic (`ERR-004`).
    #[must_use]
    pub fn check(&self, row: &RowSizes) -> Option<Finding> {
        if !self.enabled {
            return None;
        }
        let mut columns = Vec::new();
        for (name, &bytes) in self.columns.iter().zip(row.lengths()) {
            // Java: `colSize > colSizeInKB * BASE_FACTOR`, with an `int` promoted to `double`.
            #[expect(
                clippy::cast_precision_loss,
                reason = "GRD-002 is parity with a comparison Java makes in f64; a CQL value is \
                          capped at 2 GiB, far inside the range f64 represents exactly"
            )]
            if bytes as f64 > self.threshold_bytes {
                columns.push(LargeColumn {
                    name: name.clone(),
                    bytes,
                });
            }
        }
        if columns.is_empty() {
            return None;
        }
        Some(Finding {
            key: row.key().clone(),
            columns,
        })
    }

    /// Renders Java's report string for a finding (`GRD-003`).
    ///
    /// `Large columns (KB): value(1.474),fruits(2)` — the prefix, the separator and the
    /// parenthesised size are byte-identical to `Guardrail.guardrailChecks`, because operators
    /// grep their logs for it.
    ///
    /// The number is not. Java divides an `int` by an `int` and hands the truncated quotient to a
    /// `DecimalFormat("0.###")` that consequently never renders a fraction, so every column between
    /// 1000 and 1999 bytes reports `(1)`. cdm-rs renders the real quotient with the trailing zeros
    /// `0.###` would have dropped; `--compat-java` restores the truncation (`GRD-003`,
    /// `COMPAT-001`, `docs/MIGRATION_FROM_JAVA.md`).
    #[must_use]
    pub fn report(&self, finding: &Finding) -> String {
        let mut out = String::from("Large columns (KB): ");
        for (index, column) in finding.columns.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&column.name);
            out.push('(');
            out.push_str(&format_kb(column.bytes, self.compat_java));
            out.push(')');
        }
        out
    }

    /// Renders a finding as a diagnostic, for the plugin surface and the run report.
    ///
    /// Carries the table, the report string and the primary key. No value: see the module
    /// documentation on `SEC-002`.
    #[must_use]
    pub fn diagnostic(&self, finding: &Finding) -> Diagnostic {
        Diagnostic::warning(GUARDRAIL_DIAGNOSTIC_CODE, self.report(finding))
            .with_location(self.table.to_string())
            .with_rule("GRD-003")
            .with_detail(format!("primary key {}", finding.key))
    }

    /// Logs a finding (`GRD-003`, `SEC-002`).
    ///
    /// At `ERROR`, as Java does, unless the mode is [`GuardrailMode::Warn`]. The event names the
    /// table, the report string and the primary key **in hex**, and there is no field that could
    /// carry a value — a guardrail run's log is the single most likely place in cdm-rs for row data
    /// to escape, so the type this is called with has none to give.
    pub fn log(&self, finding: &Finding) {
        let report = self.report(finding);
        let key = finding.key.to_string();
        let table = self.table.to_string();
        if self.mode == GuardrailMode::Warn {
            tracing::warn!(
                table = %table,
                primary_key = %key,
                columns = finding.columns.len(),
                "Guardrails failed for row {report}"
            );
        } else {
            tracing::error!(
                table = %table,
                primary_key = %key,
                columns = finding.columns.len(),
                "Guardrails failed for row {report}"
            );
        }
    }
}

impl Plugin for ColumnSizeGuardrail {
    fn name(&self) -> &'static str {
        COLUMN_SIZE_GUARDRAIL
    }

    fn provider(&self) -> &'static str {
        PROVIDER
    }
}

impl GuardrailPlugin for ColumnSizeGuardrail {
    /// Checks a record without decoding it (`PLG-003`, `GRD-004`).
    ///
    /// This is the inline entry point: a migrate or validate job that has already read a record
    /// asks the guardrail about it here, and acts on the answer according to
    /// [`ColumnSizeGuardrail::mode`].
    fn check(&self, record: &Record) -> Result<Option<Diagnostic>, CdmError> {
        Ok(Self::check(self, &RowSizes::from_record(record))
            .map(|finding| self.diagnostic(&finding)))
    }
}

/// Formats a byte count as Java's `DecimalFormat("0.###")` would format `bytes / 1000`.
///
/// `compat_java` reproduces the integer division that makes Java's pattern moot.
fn format_kb(bytes: usize, compat_java: bool) -> String {
    let whole = bytes / 1000;
    let fraction = bytes % 1000;
    if compat_java || fraction == 0 {
        return whole.to_string();
    }
    let mut rendered = format!("{whole}.{fraction:03}");
    while rendered.ends_with('0') {
        rendered.pop();
    }
    rendered
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
    use cdm_codec::CqlTypeInfo;
    use cdm_core::{ColumnRef, RawCell, TableView};
    use proptest::prelude::*;

    use super::*;

    fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
        pairs.iter().copied().collect()
    }

    /// The table `SIT/features/05_guardrail` uses, so the parity cases below are the Java ones.
    fn origin() -> TableFacts {
        TableFacts::from_view(
            &crate::table_view(
                TableRef::new("origin", "feature_guardrail"),
                &[
                    ("key", "text"),
                    ("value", "text"),
                    ("fruits", "map<text, text>"),
                ],
            ),
            &["key"],
        )
        .unwrap()
    }

    fn key() -> PrimaryKey {
        PrimaryKey::new(vec![RawCell::from_static(b"clean")])
    }

    fn plan(kb: f64) -> ColumnSizeGuardrail {
        Guardrail {
            column_size_kb: kb,
            ..Guardrail::default()
        }
        .resolve(&origin())
        .unwrap()
    }

    // ---------------------------------------------------------------------------------------
    // GRD-001 — origin only
    // ---------------------------------------------------------------------------------------

    #[test]
    fn grd_001_the_guardrail_resolves_against_the_origin_alone() {
        let plan = plan(1.0);
        assert_eq!(plan.table().keyspace(), "origin");
        assert_eq!(plan.columns(), ["key", "value", "fruits"]);
        // The plan is the whole of the guardrail's state, and none of it can reach a target: it is
        // a table name, three column names, a float and two flags.
        assert!(plan.is_enabled());
    }

    #[test]
    fn grd_001_a_disabled_guardrail_reports_nothing_at_any_size() {
        let plan = plan(0.0);
        assert!(!plan.is_enabled());
        let row = RowSizes::new(key(), [10_000_000, 10_000_000, 10_000_000]);
        assert_eq!(plan.check(&row), None);
    }

    // ---------------------------------------------------------------------------------------
    // GRD-002 — the comparison
    // ---------------------------------------------------------------------------------------

    #[test]
    fn grd_002_the_threshold_is_kilobytes_of_a_thousand_bytes_not_1024() {
        let plan = plan(1.0);
        assert!((plan.threshold_bytes() - 1000.0).abs() < f64::EPSILON);

        // 1024 bytes is over a 1 kB threshold. Under a KiB reading it would be exactly at it.
        let row = RowSizes::new(key(), [4, 1024, 40]);
        let finding = plan.check(&row).unwrap();
        assert_eq!(finding.columns().len(), 1);
        assert_eq!(finding.columns()[0].name, "value");
    }

    #[test]
    fn grd_002_the_comparison_is_strictly_greater_than() {
        let plan = plan(1.0);
        assert_eq!(plan.check(&RowSizes::new(key(), [0, 1000, 0])), None);
        assert!(plan.check(&RowSizes::new(key(), [0, 1001, 0])).is_some());
    }

    #[test]
    fn grd_002_a_fractional_threshold_is_meaningful_unlike_java() {
        // Java parses colSizeInKB with Long.parseLong, so `0.5` becomes null and the feature is
        // silently off. cdm-rs takes it as 500 bytes (see docs/MIGRATION_FROM_JAVA.md item 12).
        let plan = plan(0.5);
        assert!((plan.threshold_bytes() - 500.0).abs() < f64::EPSILON);
        assert_eq!(plan.check(&RowSizes::new(key(), [0, 500, 0])), None);
        assert!(plan.check(&RowSizes::new(key(), [0, 501, 0])).is_some());
    }

    #[test]
    fn grd_002_a_null_column_has_length_zero_and_is_never_reported() {
        let row = Row::new(vec![
            RawCell::from_static(b"clean"),
            RawCell::NULL,
            RawCell::new(vec![0_u8; 4000]),
        ]);
        let sizes = RowSizes::from_row(key(), &row);
        assert_eq!(sizes.lengths(), [5, 0, 4000]);
        let finding = plan(1.0).check(&sizes).unwrap();
        assert_eq!(finding.columns().len(), 1);
        assert_eq!(finding.columns()[0].name, "fruits");
    }

    #[test]
    fn grd_002_every_column_is_measured_including_key_columns() {
        // Java iterates getColumnNames(false), which includes the primary key, so an oversized
        // partition key is reported like any other column.
        let plan = plan(1.0);
        let finding = plan.check(&RowSizes::new(key(), [2000, 1, 1])).unwrap();
        assert_eq!(finding.columns()[0].name, "key");
    }

    #[test]
    fn grd_002_the_whole_column_is_measured_not_its_largest_element() {
        // SIT/features/05_guardrail seeds a row whose *map key* is oversized and asserts the whole
        // `fruits` column is reported. A length is a length: there is nothing to descend into.
        let plan = plan(1.0);
        let finding = plan.check(&RowSizes::new(key(), [8, 6, 1474])).unwrap();
        assert_eq!(
            finding.columns(),
            [LargeColumn {
                name: "fruits".to_owned(),
                bytes: 1474
            }]
        );
    }

    #[test]
    fn grd_002_extra_and_missing_projection_positions_are_tolerated() {
        let plan = plan(1.0);
        // Wider than the plan: the trailing WRITETIME/TTL positions of SCH-007 are ignored.
        assert_eq!(
            plan.check(&RowSizes::new(key(), [1, 1, 1, 8, 4])),
            None,
            "virtual columns are not data the operator can shrink"
        );
        // Narrower than the plan: measured as far as it goes, and never panics.
        assert!(plan.check(&RowSizes::new(key(), [2000])).is_some());
        assert_eq!(plan.check(&RowSizes::new(key(), [])), None);
    }

    // ---------------------------------------------------------------------------------------
    // GRD-003 — the report
    // ---------------------------------------------------------------------------------------

    #[test]
    fn grd_003_the_report_matches_javas_prefix_and_separator() {
        let plan = plan(1.0);
        let finding = plan.check(&RowSizes::new(key(), [2, 1474, 2500])).unwrap();
        assert_eq!(
            plan.report(&finding),
            "Large columns (KB): value(1.474),fruits(2.5)"
        );
    }

    #[test]
    fn grd_003_compat_java_restores_the_truncated_integer_kilobytes() {
        let plan = Guardrail {
            column_size_kb: 1.0,
            ..Guardrail::default()
        }
        .with_compat_java(true)
        .resolve(&origin())
        .unwrap();
        let finding = plan.check(&RowSizes::new(key(), [2, 1474, 2500])).unwrap();
        assert_eq!(
            plan.report(&finding),
            "Large columns (KB): value(1),fruits(2)",
            "Java's integer division makes 1474 and 1999 bytes indistinguishable"
        );
    }

    #[test]
    fn grd_003_sizes_render_as_decimal_format_0_hash_hash_hash_would() {
        for (bytes, expected) in [
            (2000_usize, "2"),
            (2500, "2.5"),
            (2050, "2.05"),
            (2005, "2.005"),
            (1474, "1.474"),
            (999, "0.999"),
            (1, "0.001"),
            (0, "0"),
        ] {
            assert_eq!(format_kb(bytes, false), expected, "{bytes} bytes");
        }
        assert_eq!(format_kb(1474, true), "1");
    }

    #[test]
    fn grd_003_a_clean_row_produces_no_finding_and_a_dirty_one_names_its_columns() {
        let plan = plan(1.0);
        assert_eq!(plan.check(&RowSizes::new(key(), [8, 6, 74])), None);

        let finding = plan.check(&RowSizes::new(key(), [8, 4000, 5000])).unwrap();
        assert_eq!(finding.columns().len(), 2);
        assert_eq!(finding.largest().unwrap().name, "fruits");
        assert_eq!(finding.key(), &key());
    }

    #[test]
    fn grd_003_the_report_never_contains_a_column_value() {
        // SEC-002. The guardrail is handed lengths, so there is no value to leak — this asserts the
        // rendered artefacts as well, since they are what reaches a log.
        let plan = plan(1.0);
        let secret = "hunter2-is-the-customers-data";
        let row = Row::new(vec![
            RawCell::from_static(b"clean"),
            RawCell::new(secret.repeat(200).into_bytes()),
            RawCell::NULL,
        ]);
        let finding = plan.check(&RowSizes::from_row(key(), &row)).unwrap();
        let report = plan.report(&finding);
        let diagnostic = plan.diagnostic(&finding);
        let rendered = format!("{report} {diagnostic:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        // The primary key is present, and in hex.
        assert!(rendered.contains("0x636c65616e"), "{rendered}");
        plan.log(&finding);
    }

    #[test]
    fn grd_003_the_diagnostic_names_the_table_the_rule_and_the_key() {
        let plan = plan(1.0);
        let finding = plan.check(&RowSizes::new(key(), [8, 4000, 1])).unwrap();
        let diagnostic = plan.diagnostic(&finding);
        assert_eq!(
            diagnostic.location.as_deref(),
            Some("origin.feature_guardrail")
        );
        assert_eq!(diagnostic.rule.as_deref(), Some("GRD-003"));
        assert_eq!(diagnostic.code, GUARDRAIL_DIAGNOSTIC_CODE);
        assert!(diagnostic.title.starts_with("Large columns (KB): "));
        assert!(
            !diagnostic.is_blocking(),
            "a finding is data, not a failure"
        );
    }

    // ---------------------------------------------------------------------------------------
    // GRD-004 — the inline guardrail
    // ---------------------------------------------------------------------------------------

    #[test]
    fn grd_004_the_mode_is_loaded_and_only_block_withholds_a_row() {
        for (spelling, expected) in [
            ("check", GuardrailMode::Check),
            ("WARN", GuardrailMode::Warn),
            (" block ", GuardrailMode::Block),
        ] {
            let feature =
                Guardrail::load(&config(&[("feature.guardrail.mode", spelling)])).unwrap();
            assert_eq!(feature.mode(), expected);
        }
        assert!(!GuardrailMode::Check.blocks());
        assert!(!GuardrailMode::Warn.blocks());
        assert!(GuardrailMode::Block.blocks());
        assert_eq!(GuardrailMode::default(), GuardrailMode::Check);
        assert_eq!(GuardrailMode::Block.to_string(), "block");
    }

    #[test]
    fn grd_004_an_unrecognised_mode_is_refused_rather_than_coerced() {
        let error = Guardrail::load(&config(&[("feature.guardrail.mode", "skip")])).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert_eq!(
            error.context().config_key.as_deref(),
            Some("feature.guardrail.mode")
        );
        assert!(error.to_string().contains("check, warn, block"));
    }

    #[test]
    fn grd_004_the_mode_spellings_are_those_the_configuration_enum_serialises() {
        // `cdm-config`'s GuardrailMode is a separate type (ARCHITECTURE §3 has no
        // cdm-feature -> cdm-config edge), so the spellings are pinned on both sides.
        assert_eq!(GuardrailMode::VARIANTS, ["check", "warn", "block"]);
    }

    #[test]
    fn grd_004_the_inline_check_runs_over_a_record_through_the_plugin_trait() {
        let plan = plan(1.0);
        let record = Record::new(
            key(),
            Row::new(vec![
                RawCell::from_static(b"clean"),
                RawCell::new(vec![7_u8; 4000]),
                RawCell::NULL,
            ]),
        );
        let diagnostic = GuardrailPlugin::check(&plan, &record).unwrap().unwrap();
        assert!(diagnostic.title.contains("value(4)"));
        assert_eq!(plan.name(), "column-size");
        assert_eq!(plan.provider(), "cdm-feature");

        let clean = Record::new(key(), Row::new(vec![RawCell::from_static(b"clean")]));
        assert!(GuardrailPlugin::check(&plan, &clean).unwrap().is_none());
    }

    #[test]
    fn grd_004_a_warn_mode_finding_is_still_a_finding() {
        let feature = Guardrail::load(&config(&[
            ("feature.guardrail.column_size_kb", "1"),
            ("feature.guardrail.mode", "warn"),
        ]))
        .unwrap();
        let plan = feature.resolve(&origin()).unwrap();
        assert_eq!(plan.mode(), GuardrailMode::Warn);
        let finding = plan.check(&RowSizes::new(key(), [1, 4000, 1])).unwrap();
        plan.log(&finding);
        assert_eq!(plan.report(&finding), "Large columns (KB): value(4)");
    }

    // ---------------------------------------------------------------------------------------
    // Loading and validation
    // ---------------------------------------------------------------------------------------

    #[test]
    fn grd_002_the_threshold_is_read_from_either_spelling_and_defaults_to_disabled() {
        assert!(!Guardrail::load(&config(&[])).unwrap().is_enabled());
        assert!(
            !Guardrail::load(&config(&[("feature.guardrail.column_size_kb", "0")]))
                .unwrap()
                .is_enabled()
        );
        let legacy = Guardrail::load(&config(&[(
            "spark.cdm.feature.guardrail.colSizeInKB",
            "10",
        )]))
        .unwrap();
        assert!((legacy.column_size_kb() - 10.0).abs() < f64::EPSILON);

        let both = Guardrail::load(&config(&[
            ("feature.guardrail.column_size_kb", "2"),
            ("spark.cdm.feature.guardrail.colSizeInKB", "10"),
        ]))
        .unwrap();
        assert!((both.column_size_kb() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn grd_002_an_unparsable_threshold_is_a_config_error() {
        let error =
            Guardrail::load(&config(&[("feature.guardrail.column_size_kb", "big")])).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert_eq!(
            error.context().config_key.as_deref(),
            Some("feature.guardrail.column_size_kb")
        );
    }

    #[test]
    fn grd_002_a_negative_threshold_is_a_blocking_diagnostic_and_refuses_to_resolve() {
        let feature =
            Guardrail::load(&config(&[("feature.guardrail.column_size_kb", "-1")])).unwrap();
        let findings = feature.validate(&origin());
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_blocking());
        assert_eq!(
            findings[0].location.as_deref(),
            Some("feature.guardrail.column_size_kb")
        );
        assert!(!feature.is_enabled());
        assert_eq!(
            feature.resolve(&origin()).unwrap_err().kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn grd_001_a_columnless_origin_table_is_reported_when_the_guardrail_is_on() {
        let empty = TableFacts::new(TableRef::new("origin", "nothing"), Vec::new());
        let feature =
            Guardrail::load(&config(&[("feature.guardrail.column_size_kb", "1")])).unwrap();
        let findings = feature.validate(&empty);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].title.contains("no columns to measure"));
        // Disabled, there is nothing to complain about.
        assert!(Guardrail::default().validate(&empty).is_empty());
    }

    #[test]
    fn grd_002_a_valid_configuration_produces_no_diagnostics() {
        let feature =
            Guardrail::load(&config(&[("feature.guardrail.column_size_kb", "1")])).unwrap();
        assert!(feature.validate(&origin()).is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // Property tests over generated schemas (TST-010)
    // ---------------------------------------------------------------------------------------

    /// A table of `count` columns, spanning the type system so the generated schemas exercise
    /// collections, tuples, UDTs, frozen variants and vectors alongside the primitives.
    fn generated_table(types: &[String]) -> TableFacts {
        let columns: Vec<ColumnRef> = types
            .iter()
            .enumerate()
            .map(|(index, cql_type)| ColumnRef::new(format!("c{index}"), cql_type.clone()))
            .collect();
        TableFacts::from_view(
            &TableView::new(TableRef::new("ks", "generated"), columns),
            &["c0"],
        )
        .unwrap()
    }

    /// Every CQL type shape the codec understands, including the ones only a 5.x server has.
    fn type_universe() -> Vec<String> {
        [
            "int",
            "text",
            "blob",
            "bigint",
            "uuid",
            "timestamp",
            "decimal",
            "varint",
            "inet",
            "duration",
            "list<text>",
            "set<int>",
            "map<text, text>",
            "map<text, frozen<list<int>>>",
            "frozen<list<text>>",
            "frozen<set<frozen<tuple<int, text>>>>",
            "tuple<int, text, blob>",
            "frozen<tuple<int, frozen<map<text, int>>>>",
            "address",
            "frozen<address>",
            "list<frozen<address>>",
            "vector<float, 3>",
            "vector<float, 1536>",
        ]
        .iter()
        .map(|t| (*t).to_owned())
        .collect()
    }

    #[test]
    fn grd_002_every_supported_cql_type_is_measurable_by_length_alone() {
        // The point of the raw-length accounting: there is no type for which it is undefined. This
        // asserts the whole universe parses into the facts a plan is built from, and that a plan
        // over it measures every position.
        let types = type_universe();
        for cql_type in &types {
            assert!(
                CqlTypeInfo::parse(cql_type).is_ok(),
                "{cql_type} does not parse"
            );
        }
        let table = generated_table(&types);
        let feature =
            Guardrail::load(&config(&[("feature.guardrail.column_size_kb", "1")])).unwrap();
        let plan = feature.resolve(&table).unwrap();
        assert_eq!(plan.columns().len(), types.len());

        let lengths: Vec<usize> = (0..types.len()).map(|i| i * 500).collect();
        let finding = plan.check(&RowSizes::new(key(), lengths.clone())).unwrap();
        let expected = lengths.iter().filter(|&&len| len > 1000).count();
        assert_eq!(
            finding.columns().len(),
            expected,
            "a column's type must not change whether its length exceeds the threshold"
        );
    }

    #[test]
    fn grd_002_a_key_column_is_measured_whatever_its_type() {
        let table = generated_table(&["frozen<address>".to_owned(), "text".to_owned()]);
        assert!(table.columns()[0].is_key());
        let plan = plan_over(&table, 1.0);
        assert!(plan.check(&RowSizes::new(key(), [4000, 1])).is_some());
    }

    fn plan_over(table: &TableFacts, kb: f64) -> ColumnSizeGuardrail {
        Guardrail {
            column_size_kb: kb,
            ..Guardrail::default()
        }
        .resolve(table)
        .unwrap()
    }

    proptest! {
        /// A column is reported exactly when its length exceeds the threshold — for any schema
        /// drawn from the whole type universe, any threshold, and any set of lengths (`TST-010`).
        #[test]
        fn grd_002_a_column_is_reported_iff_its_length_exceeds_the_threshold(
            indices in prop::collection::vec(0_usize..23, 1..12),
            lengths in prop::collection::vec(0_usize..8000, 1..12),
            millibytes in 1_u32..8_000_000,
        ) {
            let universe = type_universe();
            let types: Vec<String> =
                indices.iter().map(|i| universe[*i % universe.len()].clone()).collect();
            let table = generated_table(&types);
            let kb = f64::from(millibytes) / 1_000_000.0;
            let plan = plan_over(&table, kb);
            let sizes = RowSizes::new(key(), lengths.clone());

            let threshold = plan.threshold_bytes();
            #[expect(clippy::cast_precision_loss, reason = "the same f64 comparison GRD-002 makes")]
            let expected: Vec<String> = (0..types.len())
                .zip(lengths.iter())
                .filter(|(_, len)| **len as f64 > threshold)
                .map(|(index, _)| format!("c{index}"))
                .collect();

            match plan.check(&sizes) {
                None => prop_assert!(expected.is_empty()),
                Some(finding) => {
                    let reported: Vec<String> =
                        finding.columns().iter().map(|c| c.name.clone()).collect();
                    prop_assert_eq!(reported, expected);
                }
            }
        }

        /// The rendered size always parses back to the byte count it came from, to the precision
        /// `0.###` can carry — so no report is ambiguous about which of two columns is larger.
        #[test]
        fn grd_003_a_rendered_size_round_trips_to_its_byte_count(bytes in 0_usize..100_000_000) {
            let rendered = format_kb(bytes, false);
            let parsed: f64 = rendered.parse().unwrap();
            #[expect(clippy::cast_precision_loss, reason = "well inside f64's exact range")]
            let exact = bytes as f64 / 1000.0;
            prop_assert!((parsed - exact).abs() < 1e-9, "{rendered} != {exact}");
            prop_assert!(!rendered.contains('e'), "{rendered} is in exponent form");
            prop_assert!(!rendered.ends_with('0') || rendered == "0" || !rendered.contains('.'));
        }

        /// Whatever the row, a finding carries no bytes from it — only lengths and the key.
        #[test]
        fn grd_003_a_finding_never_carries_a_column_value(
            payload in prop::collection::vec(any::<u8>(), 2000..2100),
        ) {
            let plan = plan(1.0);
            let row = Row::new(vec![
                RawCell::from_static(b"clean"),
                RawCell::new(payload.clone()),
                RawCell::NULL,
            ]);
            let finding = plan.check(&RowSizes::from_row(key(), &row)).unwrap();
            let rendered = format!("{}|{:?}", plan.report(&finding), plan.diagnostic(&finding));
            let hex = payload.iter().fold(String::new(), |mut acc, b| {
                use std::fmt::Write as _;
                let _ = write!(acc, "{b:02x}");
                acc
            });
            prop_assert!(!rendered.contains(&hex));
            prop_assert!(!rendered.contains(&String::from_utf8_lossy(&payload).to_string()));
        }
    }
}
