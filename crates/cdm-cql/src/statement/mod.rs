//! Statement construction and binding — the CQL a run actually executes.
//!
//! # What is here
//!
//! Everything between "we know both schemas" and "the driver has a bound statement":
//!
//! * [`ColumnMapping`] — which origin column feeds which target column, after renames, skips,
//!   constants and the explode-map pair (`SCH-003`, `SCH-004`, `SCH-006`);
//! * [`OriginProjection`] — the origin `SELECT` list, including the virtual `TTL(col)` and
//!   `WRITETIME(col)` columns and the row positions they occupy (`SCH-007`);
//! * [`OriginRangeSelect`], [`OriginSelectByPk`], [`TargetSelectByPk`] — the read statements
//!   (`FEA-060`, `FEA-061`);
//! * [`TargetUpsert`] — the write statement, an `INSERT` for an ordinary table and an
//!   `UPDATE … SET c = c + ?` for a counter table (`SCH-005`, `MIG-010`, `MIG-030`);
//! * [`Binder`] and [`BoundWrite`] — binding a row into that statement, with `UNSET` where a
//!   `NULL` would write a tombstone (`MIG-011`..`MIG-014`, `ERR-005`);
//! * [`StatementSet`] — every generated statement, in one value, for the startup log and the
//!   `GET /v1/runs/{id}/statements` endpoint (`FEA-062`).
//!
//! # Everything is resolved once
//!
//! `ARCHITECTURE.md` §5.5 requires per-row work to be *lookup*, not *decision*. So the types here
//! split the same way `cdm-feature`'s do: [`MappingOptions`] and [`StatementOptions`] carry
//! configuration, [`ColumnMapping`] and [`Binder`] carry the resolved plan, and the per-row entry
//! point ([`Binder::bind`]) does nothing but index into vectors.
//!
//! # Why the feature plans arrive as plain data
//!
//! `ARCHITECTURE.md` §3 has `cdm-feature` depending on `cdm-cql`, so this module cannot name
//! `WritetimeTtlPlan`, `ResolvedConstant` or `ExplodePlan`. The hooks of `FEA-060`..`FEA-062` are
//! therefore expressed as the *projections* of those plans — expression strings, `(column,
//! literal)` pairs, two booleans for the `USING` clause, and a pair of column names for the
//! explode map. `cdm-feature` produces exactly those already
//! (`WritetimeTtlPlan::projection`, `ConstantColumns::where_clause_terms`,
//! `UsingClause`, `ExplodePlan::key_column`), so the seam costs a struct literal at startup and
//! keeps the dependency graph acyclic.
//!
//! # Specification
//!
//! - `SCH-003`, `SCH-004`, `SCH-006` — [`ColumnMapping`]
//! - `SCH-005` — [`TargetUpsert::is_counter`]
//! - `SCH-007` — [`OriginProjection`]
//! - `FEA-060`, `FEA-061` — [`OriginRangeSelect`]
//! - `FEA-062` — [`StatementSet`]
//! - `MIG-010`, `MIG-011` — [`TargetUpsert`]
//! - `MIG-012`, `MIG-013`, `MIG-014` — [`Binder`], [`BoundValue`]
//! - `ERR-005` — [`BindFailure`]

mod bind;
mod mapping;
mod projection;
mod select;
mod upsert;

pub use bind::{
    BindFailure, BindInputs, Binder, Bound, BoundValue, BoundWrite, CounterWrite, Idempotent,
    IdempotentWrite, KeyBinding, MissingKeyPolicy, SourceRow,
};
pub use mapping::{ColumnMapping, MappingOptions, TargetSource};
pub use projection::OriginProjection;
pub use select::{OriginRangeSelect, OriginSelectByPk, TargetSelectByPk, TokenBound};
pub use upsert::{StatementOptions, TargetUpsert, UsingClause};

use std::fmt;

/// Every statement a run will execute, generated once at startup (`FEA-062`).
///
/// Held rather than re-derived because the point of `FEA-062` is that the operator can *see* the
/// CQL before a single row moves: a run that is about to write to the wrong table, or to omit
/// `USING TIMESTAMP`, is obvious in four lines of text and invisible in a configuration dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementSet {
    /// The origin token-range scan (`FEA-060`).
    pub origin_range_select: String,
    /// The origin lookup by primary key, used by validate's autocorrect and by rerun.
    pub origin_select_by_pk: String,
    /// The target lookup by primary key, used by validate and by the counter delta (`MIG-031`).
    pub target_select_by_pk: String,
    /// The target write (`MIG-010`, or `MIG-030` for a counter table).
    pub target_upsert: String,
}

impl fmt::Display for StatementSet {
    /// One statement per line, labelled — the form `FEA-062` logs at startup.
    ///
    /// Statement text is not row data, so `SEC-002` does not apply to it; a constant column's
    /// literal is the one value that appears, and it came from the operator's own configuration.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "origin range select : {}", self.origin_range_select)?;
        writeln!(f, "origin select by pk : {}", self.origin_select_by_pk)?;
        writeln!(f, "target select by pk : {}", self.target_select_by_pk)?;
        write!(f, "target upsert       : {}", self.target_upsert)
    }
}

impl StatementSet {
    /// Logs every statement once, at `INFO` (`FEA-062`).
    pub fn log(&self) {
        tracing::info!(
            origin_range_select = %self.origin_range_select,
            origin_select_by_pk = %self.origin_select_by_pk,
            target_select_by_pk = %self.target_select_by_pk,
            target_upsert = %self.target_upsert,
            "generated CQL (FEA-062)"
        );
    }
}

/// Joins CQL fragments with `,` and no space, as Java's `PropertyHelper.asString` does.
///
/// The separator is load-bearing for parity: operators grep their logs for the generated CQL, and
/// `MET-005`'s reasoning about metric strings applies here too — a cosmetic change to a string
/// people have matched against for years is not cosmetic.
fn join(parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut out = String::new();
    for part in parts {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(part.as_ref());
    }
    out
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

    fn statements() -> StatementSet {
        StatementSet {
            origin_range_select: "SELECT a FROM ks.t WHERE TOKEN(a) >= ? AND TOKEN(a) <= ?"
                .to_owned(),
            origin_select_by_pk: "SELECT a FROM ks.t WHERE a=?".to_owned(),
            target_select_by_pk: "SELECT a FROM ks.t2 WHERE a=?".to_owned(),
            target_upsert: "INSERT INTO ks.t2 (a) VALUES (?)".to_owned(),
        }
    }

    #[test]
    fn fea_062_every_statement_is_rendered_for_the_startup_log() {
        let rendered = statements().to_string();
        for line in [
            "origin range select : SELECT a FROM ks.t WHERE TOKEN(a) >= ? AND TOKEN(a) <= ?",
            "origin select by pk : SELECT a FROM ks.t WHERE a=?",
            "target select by pk : SELECT a FROM ks.t2 WHERE a=?",
            "target upsert       : INSERT INTO ks.t2 (a) VALUES (?)",
        ] {
            assert!(rendered.contains(line), "{rendered}");
        }
        assert_eq!(rendered.lines().count(), 4);
        statements().log();
    }

    #[test]
    fn mig_010_fragments_join_with_a_bare_comma_as_java_does() {
        assert_eq!(join(["a", "b", "c"]), "a,b,c");
        assert_eq!(join(Vec::<String>::new()), "");
        assert_eq!(join(["only"]), "only");
    }
}
