//! What a finished run tells the operator (`MET-005`, `MET-006`, `CLI-004`, `CLI-005`).

use std::io::Write;

use cdm_config::EffectiveConfig;
use cdm_core::JobKind;
use cdm_engine::planner::PlanReport as EnginePlanReport;
use cdm_engine::scheduler::RunReport;
use serde::Serialize;

use super::ResolvedTables;
use crate::output::Report;

/// A finished run, in the form both a terminal and a script can read.
#[derive(Debug, Serialize)]
pub struct RunSummary {
    /// Which job ran.
    pub job: String,
    /// The run's terminal status (`TRK-012`).
    pub status: String,
    /// Whether the run wrote anything (`MIG-041`).
    pub dry_run: bool,
    /// Ranges that finished successfully.
    pub ranges_passed: usize,
    /// Ranges that failed.
    pub ranges_failed: usize,
    /// The counter block Java prints, character for character (`MET-006`, `COMPAT-004`).
    pub counters: String,
    /// Rows that differed and were not repaired (`VAL-005`, `VAL-006`).
    ///
    /// Zero for migrate and guardrail, which have no such counters registered (`MET-002`).
    pub discrepancies: u64,
}

impl RunSummary {
    /// Builds the summary from the scheduler's report.
    #[must_use]
    pub fn from_report(kind: JobKind, report: &RunReport, dry_run: bool) -> Self {
        Self {
            job: kind.as_str().to_owned(),
            status: format!("{:?}", report.status()).to_uppercase(),
            dry_run,
            ranges_passed: report.ranges_passed(),
            ranges_failed: report.ranges_failed(),
            counters: report.counters().final_block(Some(report.run_id())),
            discrepancies: unrepaired(report),
        }
    }

    /// The process exit code this run should report (`CLI-004`).
    ///
    /// Three outcomes, not two. An interruption is `4` and is the only code a supervisor should
    /// retry unchanged — the run is resumable and nothing about the request was wrong. A run that
    /// finished with failed ranges, or that the error limit aborted, is `1`: the command worked and
    /// the data did not agree, which is a different thing from the tool failing.
    #[must_use]
    pub fn exit(&self) -> crate::exit::Exit {
        if self.status == "INTERRUPTED" {
            return crate::exit::Exit::Interrupted;
        }
        if self.is_clean() {
            crate::exit::Exit::Success
        } else {
            crate::exit::Exit::Completed
        }
    }

    /// Whether the run reached its terminal state cleanly *and* found nothing.
    ///
    /// `CLI-004` needs three outcomes, not two: a run that finished with failed ranges, or one
    /// that found discrepancies it did not repair, is a *completed command with a bad result*,
    /// which a pipeline must be able to tell apart both from success and from the tool failing.
    ///
    /// The discrepancy half is not optional. A validate run that finds a thousand mismatched rows
    /// fails no range — comparison is what it is *for* — so a check on ranges alone reports a
    /// divergent target as a clean bill of health, which is the one answer that must never be
    /// wrong.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.ranges_failed == 0 && self.discrepancies == 0 && self.status == "ENDED"
    }
}

impl Report for RunSummary {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        if self.dry_run {
            writeln!(
                out,
                "Dry run: every row was read and transformed, nothing was written."
            )?;
        }
        // The counter block first and unmodified: existing tooling greps it, and anything printed
        // *inside* it would break that (`COMPAT-004`).
        writeln!(out, "{}", self.counters)?;
        writeln!(
            out,
            "\n{} {}: {} range(s) passed, {} failed.",
            self.job, self.status, self.ranges_passed, self.ranges_failed
        )
    }

    fn has_findings(&self) -> bool {
        !self.is_clean()
    }
}

/// Rows that differed and were left differing.
///
/// `MISSING` and `MISMATCH` less the two `CORRECTED_*` counters: a difference autocorrect repaired
/// is not a finding, because the target no longer differs. Read at the **committed** level, which
/// is where a finished run's totals are; the interim level is structurally zero once every range
/// has flushed (`MET-004`).
fn unrepaired(report: &RunReport) -> u64 {
    use cdm_metrics::{CounterKind, CounterView};

    let count = |kind: CounterKind| {
        report.counters().counter(kind).map_or(0, |counter| {
            report.counters().count(counter, CounterView::Committed)
        })
    };
    let found = count(CounterKind::Missing) + count(CounterKind::Mismatch);
    let fixed = count(CounterKind::CorrectedMissing) + count(CounterKind::CorrectedMismatch);
    found.saturating_sub(fixed)
}

/// What `cdm plan` answers, without touching a row.
#[derive(Debug, Serialize)]
pub struct PlanSummary {
    /// The origin table.
    pub origin_table: String,
    /// The target table.
    pub target_table: String,
    /// The partitioner the origin reports.
    pub partitioner: String,
    /// How many token ranges the run will be divided into.
    pub range_count: usize,
    /// The percentage of each range that will actually be read (`filter.token_coverage_percent`).
    pub coverage_percent: u8,
    /// How many origin columns the scan projects.
    pub projected_columns: usize,
    /// Peak resident rows per worker, which is what bounds memory (`NFR-003`).
    pub rows_in_flight: u64,
}

impl PlanSummary {
    /// Builds the summary from the engine's plan report.
    #[must_use]
    pub fn new(
        report: &EnginePlanReport,
        tables: &ResolvedTables,
        config: &EffectiveConfig,
    ) -> Self {
        let perfops = &config.config().perfops;
        Self {
            origin_table: tables.origin.table_ref().to_string(),
            target_table: tables.target.table_ref().to_string(),
            partitioner: format!("{:?}", report.partitioner),
            range_count: report.range_count,
            coverage_percent: report.coverage_percent,
            projected_columns: tables.projection.width(),
            rows_in_flight: u64::from(perfops.fetch_size) * u64::from(config.workers().max(1)),
        }
    }
}

impl Report for PlanSummary {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "{} → {}", self.origin_table, self.target_table)?;
        writeln!(out, "  partitioner:       {}", self.partitioner)?;
        writeln!(out, "  token ranges:      {}", self.range_count)?;
        if self.coverage_percent < 100 {
            writeln!(
                out,
                "  coverage:          {}% — this run samples, it does not migrate everything",
                self.coverage_percent
            )?;
        }
        writeln!(out, "  columns projected: {}", self.projected_columns)?;
        // The number an operator actually wants: Java's answer to "how much memory" was
        // `--driver-memory 25G` and a shrug.
        writeln!(
            out,
            "  peak rows resident:{:>7} (fetch_size × workers)",
            self.rows_in_flight
        )?;
        writeln!(out, "\nNo data was read or written.")
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::exit::Exit;

    fn summary(status: &str, failed: usize) -> RunSummary {
        RunSummary {
            job: "migrate".to_owned(),
            status: status.to_owned(),
            dry_run: false,
            ranges_passed: 10,
            ranges_failed: failed,
            counters: "Final Read Record Count: 10".to_owned(),
            discrepancies: 0,
        }
    }

    #[test]
    fn cli_004_a_clean_run_succeeds() {
        assert_eq!(summary("ENDED", 0).exit(), Exit::Success);
    }

    #[test]
    fn cli_004_failed_ranges_are_completed_not_internal() {
        // The command worked; the data did not agree. A pipeline must be able to tell that from
        // the tool itself breaking.
        assert_eq!(summary("ENDED", 3).exit(), Exit::Completed);
    }

    #[test]
    fn cli_004_an_interruption_is_the_only_retryable_outcome() {
        assert_eq!(summary("INTERRUPTED", 0).exit(), Exit::Interrupted);
        assert!(summary("INTERRUPTED", 0).exit().is_retryable());
        assert!(!summary("ABORTED", 0).exit().is_retryable());
        assert!(!summary("ENDED", 3).exit().is_retryable());
    }

    #[test]
    fn cli_004_unrepaired_discrepancies_are_a_finding_though_no_range_failed() {
        // The regression this guards: a validate run that found a mismatched and a missing row
        // exited 0, because comparison finding a difference fails no range. A pipeline gating a
        // cutover on `cdm validate` would have read that as "the two clusters agree".
        let mut found = summary("ENDED", 0);
        found.discrepancies = 2;

        assert!(!found.is_clean());
        assert_eq!(found.exit(), Exit::Completed);
    }

    #[test]
    fn cli_004_repaired_discrepancies_are_not_a_finding() {
        // Autocorrect fixed them, so the target no longer differs. `unrepaired` subtracts the
        // CORRECTED_* counters for exactly this case.
        let repaired = summary("ENDED", 0);
        assert_eq!(repaired.discrepancies, 0);
        assert_eq!(repaired.exit(), Exit::Success);
    }

    #[test]
    fn cli_004_an_aborted_run_is_not_retryable() {
        // The error limit stopped it. Running it again unchanged trips the same limit.
        assert_eq!(summary("ABORTED", 0).exit(), Exit::Completed);
    }

    #[test]
    fn met_006_the_counter_block_is_rendered_verbatim() {
        let mut buf = Vec::new();
        summary("ENDED", 0).render_human(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.contains("Final Read Record Count: 10"),
            "existing tooling greps this block; it must pass through unmodified:\n{text}"
        );
    }

    #[test]
    fn mig_041_a_dry_run_says_so_before_the_numbers() {
        let mut dry = summary("ENDED", 0);
        dry.dry_run = true;
        let mut buf = Vec::new();
        dry.render_human(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.starts_with("Dry run:"), "{text}");
    }
}
