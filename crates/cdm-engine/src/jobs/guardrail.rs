//! The guardrail job (`GRD-001`..`GRD-004`).
//!
//! Reads the origin, measures every column of every row, and reports the ones that are bigger than
//! the operator said they should be. It writes nothing, anywhere.
//!
//! ```text
//!   Scheduler ── RangeContext ──► GuardrailJob::process
//!                                     │
//!                                     ├─ OriginRows::scan(range)   ── the only capability it has
//!                                     │
//!                                     └─ per row: READ++
//!                                                 ColumnSizeGuardrail::check(&RowSizes)
//!                                                    Some(finding) → LARGE++, log
//!                                                    None          → VALID++
//! ```
//!
//! # Read-only, structurally (`GRD-001`)
//!
//! [`GuardrailJob`] holds two things: an [`OriginRows`] and a
//! [`ColumnSizeGuardrail`]. [`OriginRows`] has exactly one method and it returns rows;
//! `ColumnSizeGuardrail` holds a table name, a list of column names and a threshold. Neither has a
//! session, a statement, a sink or a target of any kind. There is no line of code to review for
//! "does this write?" — there is no reachable type that could.
//!
//! That is also why the job takes its own reader trait rather than the general row source a
//! migrate job would use: a trait that *could* write would make `GRD-001` a matter of discipline,
//! and `GRD-001` is the kind of requirement that has to hold on the worst day of a migration.
//!
//! # No value is ever in scope (`SEC-002`)
//!
//! [`OriginRows`] yields [`RowSizes`] — lengths and a primary key. A guardrail run's whole output
//! is a report about specific rows, which makes it the likeliest place in cdm-rs for customer data
//! to reach a log; the job answers that by never receiving any. See `cdm_feature::guardrail` for
//! the full argument.
//!
//! # What the counters mean (`MET-002`)
//!
//! Guardrail registers `READ, VALID, SKIPPED, LARGE, PARTITIONS_PASSED, PARTITIONS_FAILED` and
//! nothing else — in particular no `ERROR`, which is why a failed guardrail range costs the run no
//! row accounting at all (`ENG-008`). `READ` is incremented for every row, then exactly one of
//! `LARGE` or `VALID`, so `READ == LARGE + VALID` holds for every completed range, exactly as it
//! does in Java's `GuardrailCheckJobSession`.

use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{CdmError, ErrorKind, JobKind, Record, RunStatus, TokenRange};
use cdm_feature::{ColumnSizeGuardrail, RowSizes};
use cdm_metrics::{CounterKind, CounterView, JobCounters};

use crate::scheduler::{RangeContext, RangeProcessor, RangeVerdict, RunReport};

mod origin;

pub use origin::CqlOriginRows;

/// A source of origin rows for one token range — the only capability a guardrail run has
/// (`GRD-001`).
///
/// Deliberately narrow. It cannot write, cannot reach a target, and cannot hand back a column
/// value: an implementation reads the origin range scan of `FEA-060` and reduces each row to its
/// column lengths and primary key as it goes, which is both the cheapest thing it can do and the
/// only thing the job needs.
///
/// Implementations page the scan themselves, honouring `fetch_size` (`ENG-003`); the job consumes
/// one row at a time and never materialises a range.
#[async_trait]
pub trait OriginRows: Send + Sync {
    /// Opens a paged scan over `range`, reading at most `fetch_size` rows per page.
    ///
    /// # Errors
    ///
    /// Any read failure. It fails the range and only the range (`ENG-008`); the scheduler counts
    /// the partition failed and moves on.
    async fn scan(
        &self,
        range: TokenRange,
        fetch_size: u32,
    ) -> Result<Box<dyn RowSizeStream>, CdmError>;
}

/// The rows of one range, one at a time.
#[async_trait]
pub trait RowSizeStream: Send {
    /// The next row's sizes, or `None` at the end of the range.
    ///
    /// # Errors
    ///
    /// Any read failure, including one that only shows up when the next page is fetched — which is
    /// most of them, since the first page usually succeeds.
    async fn next_row(&mut self) -> Result<Option<RowSizes>, CdmError>;
}

/// The standalone guardrail job (`GRD-001`..`GRD-003`).
///
/// One per run, shared by every worker: cloning is not required because the scheduler takes an
/// `Arc<dyn RangeProcessor>`.
pub struct GuardrailJob {
    origin: Arc<dyn OriginRows>,
    guardrail: ColumnSizeGuardrail,
}

impl std::fmt::Debug for GuardrailJob {
    /// Names the guardrail, not the reader: a reader is somebody else's type and has nothing
    /// useful to say, where the threshold and the columns are what an operator wants confirmed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardrailJob")
            .field("guardrail", &self.guardrail)
            .finish_non_exhaustive()
    }
}

impl GuardrailJob {
    /// Builds the job from an origin reader and a resolved guardrail.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] when the guardrail is not enabled — a threshold of `0`, or
    /// none at all. Java logs `GuardrailCheckJobSession is disabled - is it configured correctly?`
    /// and then runs the job anyway, where `guardrailChecks` returns `null` for every row and the
    /// whole table is reported clean. A run that says "no oversized columns" because it was never
    /// looking is the worst possible answer to the question being asked, so cdm-rs refuses to
    /// start instead. See `docs/MIGRATION_FROM_JAVA.md`.
    pub fn new(
        origin: Arc<dyn OriginRows>,
        guardrail: ColumnSizeGuardrail,
    ) -> Result<Self, CdmError> {
        if !guardrail.is_enabled() {
            return Err(CdmError::new(
                ErrorKind::Config,
                "the guardrail job needs `feature.guardrail.column_size_kb` to be greater than \
                 zero; at zero there is no column it could report",
            )
            .with_context(|c| c.with_config_key("feature.guardrail.column_size_kb")));
        }
        Ok(Self { origin, guardrail })
    }

    /// The resolved guardrail this job applies.
    #[must_use]
    pub const fn guardrail(&self) -> &ColumnSizeGuardrail {
        &self.guardrail
    }
}

#[async_trait]
impl RangeProcessor for GuardrailJob {
    fn job(&self) -> JobKind {
        JobKind::Guardrail
    }

    /// Measures every row of one range (`GRD-002`, `GRD-003`).
    ///
    /// Mirrors `GuardrailCheckJobSession.processPartitionRange` term for term: acquire an origin
    /// rate-limit permit, increment `READ`, check the row, then increment `LARGE` and log the
    /// finding or increment `VALID`. `PARTITIONS_PASSED` and `PARTITIONS_FAILED` are the
    /// scheduler's (`ENG-002`, `ENG-008`), not this method's — which is the single copy of the
    /// accounting that `ENG-008` exists to preserve.
    ///
    /// # Errors
    ///
    /// Any read failure, which fails this range alone.
    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
        let counters = ctx.counters();
        let read = counters.counter(CounterKind::Read)?;
        let valid = counters.counter(CounterKind::Valid)?;
        let large = counters.counter(CounterKind::Large)?;

        // ENG-007: one in-flight origin request per range for the whole scan, held until the range
        // ends. A paged reader has at most one request outstanding at a time, so this is the true
        // bound rather than a per-row approximation of it.
        let _slot = ctx.read_slot().await?;
        let mut rows = self.origin.scan(ctx.range(), ctx.fetch_size()).await?;

        while let Some(row) = rows.next_row().await? {
            // ENG-010: wind down promptly when in-flight work has been abandoned. Checked after
            // the read rather than before it so that a row already fetched is still accounted for.
            if ctx.is_cancelled() {
                break;
            }
            // ENG-004, ENG-005: pace the origin exactly where Java's `rateLimiterOrigin.acquire(1)`
            // sits — after the row arrives and before it is counted.
            ctx.acquire_read_rows(1).await;
            counters.increment(read);

            if let Some(finding) = self.guardrail.check(&row) {
                counters.increment(large);
                self.guardrail.log(&finding);
            } else {
                counters.increment(valid);
            }
        }

        // A range that found oversized columns did not fail and did not differ: it did exactly what
        // it was asked to do. The run-level finding is carried by the `LARGE` counter and surfaces
        // as an exit code through [`run_status`], which keeps the per-range tracking status
        // byte-compatible with Java's (`TRK-012`, `COMPAT-003`).
        Ok(RangeVerdict::Pass)
    }
}

/// The terminal status a finished guardrail run reports (`CLI-004`, `GRD-003`).
///
/// [`RunStatus::Diff`] when any row was `LARGE`, which `Exit::for_run_status` maps to exit code
/// `1`: the command worked, and it found something. That is a different thing from a defect
/// (code `5`) and from an interruption (code `4`), and a pipeline that guards a migration behind a
/// guardrail run needs to tell them apart — a guardrail that exits `0` on a table full of
/// three-megabyte blobs has told the operator nothing.
///
/// A run stopped early keeps the status the scheduler gave it: `INTERRUPTED` and `ABORTED` say
/// something more urgent than "found large columns", and an incomplete run's findings are a floor,
/// not an answer.
///
/// Read at the **committed** level (`MET-004`): every range flushes and merges on completion, so
/// the committed totals are the run's real numbers, where the interim level is whatever the last
/// range had not yet folded in.
#[must_use]
pub fn run_status(report: &RunReport) -> RunStatus {
    if report.status() != RunStatus::Ended {
        return report.status();
    }
    if report
        .counters()
        .count_of(CounterKind::Large, CounterView::Committed)
        > 0
    {
        RunStatus::Diff
    } else {
        RunStatus::Ended
    }
}

/// The guardrail running *inside* a migrate or validate run (`GRD-004`).
///
/// The standalone job of `GRD-001` answers "which rows are too big?" before a migration. This
/// answers "…and what should I do about this one?" during it, which is a different question: the
/// row is already in hand, already decoded, and about to be written.
///
/// # Why a blocked row counts `SKIPPED`, not `LARGE`
///
/// `GRD-004` says a blocked row is counted in `LARGE`. `MET-002` says migrate registers
/// `READ, WRITE, SKIPPED, ERROR, UNFLUSHED, PARTITIONS_PASSED, PARTITIONS_FAILED` and validate its
/// own list — neither includes `LARGE` — and `MET-003` makes using an unregistered counter a
/// startup error rather than a runtime one. The two cannot both hold, and the tie-breaker is that
/// `MET-002` and `MET-005` are parity requirements whose observable form is the final metrics
/// block: adding a `Large Record Count` line to migrate's block would break every assertion file
/// written against Java's output, including the ones in Java's own SIT suite.
///
/// So a blocked row is `SKIPPED` — which is exactly what `MIG-002` already means by it, "rejected
/// before the write, not an error" — and the finding itself is logged and returned as a
/// [`Diagnostic`](cdm_core::Diagnostic) so nothing about *why* is lost. `docs/SPEC.md` records the
/// correction.
#[derive(Debug, Clone)]
pub struct InlineGuardrail {
    guardrail: ColumnSizeGuardrail,
}

impl InlineGuardrail {
    /// Wraps a resolved guardrail for use inside another job.
    ///
    /// A disabled guardrail is accepted here, unlike in [`GuardrailJob::new`]: a migrate run with
    /// no guardrail configured is an entirely ordinary migrate run, and [`InlineGuardrail::inspect`]
    /// then costs one predictable branch per row.
    #[must_use]
    pub const fn new(guardrail: ColumnSizeGuardrail) -> Self {
        Self { guardrail }
    }

    /// Whether the guardrail will do anything at all.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.guardrail.is_enabled()
    }

    /// Checks one record and says whether it must be withheld from the target (`GRD-004`).
    ///
    /// Returns `true` when the row violates the guardrail **and** the mode is
    /// `block` — the only combination that changes what a run writes. In every other case the
    /// finding is reported and the row proceeds, which is what makes `check` and `warn` safe to
    /// switch on mid-migration.
    ///
    /// The caller increments `SKIPPED` for a `true`, exactly as it does for a filtered row
    /// (`MIG-002`); this type does not touch counters, because which counter a job may use is the
    /// job's business (`MET-002`) and a guardrail that reached for one would be the second copy of
    /// accounting `ENG-008` exists to prevent.
    #[must_use]
    pub fn inspect(&self, record: &Record) -> bool {
        let Some(finding) = self.guardrail.check(&RowSizes::from_record(record)) else {
            return false;
        };
        self.guardrail.log(&finding);
        self.guardrail.mode().blocks()
    }

    /// The resolved guardrail.
    #[must_use]
    pub const fn guardrail(&self) -> &ColumnSizeGuardrail {
        &self.guardrail
    }
}

/// Increments `SKIPPED` for a row the inline guardrail blocked (`GRD-004`, `MIG-002`).
///
/// A free function rather than a method on [`InlineGuardrail`] so that the counter a job uses is
/// still chosen by the job: a validate run and a migrate run both register `SKIPPED`, but nothing
/// here assumes which of them is calling.
///
/// # Errors
///
/// Returns [`ErrorKind::Internal`] if the calling job does not register `SKIPPED` (`MET-003`),
/// which no built-in job can be.
pub fn record_blocked_row(counters: &JobCounters) -> Result<(), CdmError> {
    let skipped = counters.counter(CounterKind::Skipped)?;
    counters.increment(skipped);
    Ok(())
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
mod tests;
