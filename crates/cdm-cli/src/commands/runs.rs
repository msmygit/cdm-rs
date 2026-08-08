//! `cdm runs list|show|cancel` — first-class run management (`TRK-034`).
//!
//! Java records runs in `cdm_run_info` and `cdm_run_details` and then offers no way to read them:
//! answering "which runs exist, and what happened to them?" means writing CQL by hand against a
//! schema you have to go and look up. These commands are that answer.
//!
//! The operations themselves live in `cdm-track::manage`, not here. That is deliberate and is what
//! `TST-050` is about: `GET /v1/runs` will render the same [`RunSummary`]
//! this command renders, so the terminal and the API cannot drift into disagreeing about what a
//! run's status was.
//!
//! # Nothing here can print a row
//!
//! A run summary carries ids, statuses, timestamps, token bounds and Java's counter string. It
//! carries no column name from the migrated table, no primary key and no value — a property of the
//! types in `cdm-track`, not of the rendering here (`SEC-002`).

use std::io::Write;
use std::sync::Arc;

use cdm_core::{CdmError, ErrorKind, RunId};
use cdm_track::manage::{RunDetail, RunManager, RunSummary};
use cdm_track::resume::RerunPolicy;
use cdm_track::CassandraStore;
use serde::Serialize;

use crate::cli::ConfigArgs;
use crate::loader::load;
use crate::output::Report;

/// What `cdm runs list` prints (`TRK-034`).
#[derive(Debug, Serialize)]
pub struct ListReport {
    /// The table the runs belong to. Tracking is per target table (`TRK-010`).
    pub table: String,
    /// Every recorded run, newest first.
    pub runs: Vec<RunSummary>,
}

impl Report for ListReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        if self.runs.is_empty() {
            return writeln!(
                out,
                "No runs recorded for {}. Run tracking is off unless `track_run.enabled` is set.",
                self.table
            );
        }

        writeln!(out, "Runs recorded for {}:\n", self.table)?;
        writeln!(
            out,
            "  {:>12}  {:<9}  {:<14}  {:<20}  RESUMABLE",
            "RUN ID", "TYPE", "STATUS", "STARTED"
        )?;
        for run in &self.runs {
            writeln!(
                out,
                "  {:>12}  {:<9}  {:<14}  {:<20}  {}",
                run.run_id,
                run.run_type,
                format!("{:?}", run.status).to_uppercase(),
                run.started_at.as_deref().unwrap_or("—"),
                if run.resumable { "yes" } else { "no" }
            )?;
        }
        writeln!(
            out,
            "\n{} run(s). `cdm runs show <id>` for the range breakdown.",
            self.runs.len()
        )
    }
}

/// What `cdm runs show` prints (`TRK-034`).
#[derive(Debug, Serialize)]
pub struct ShowReport {
    /// The table the run belongs to.
    pub table: String,
    /// The run and its range breakdown.
    pub detail: RunDetail,
}

impl Report for ShowReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        let run = &self.detail.run;
        writeln!(out, "run {} on {}", run.run_id, self.table)?;
        writeln!(out, "  type:      {}", run.run_type)?;
        writeln!(
            out,
            "  status:    {}",
            format!("{:?}", run.status).to_uppercase()
        )?;
        writeln!(
            out,
            "  started:   {}",
            run.started_at.as_deref().unwrap_or("—")
        )?;
        writeln!(
            out,
            "  ended:     {}",
            run.ended_at.as_deref().unwrap_or("—")
        )?;
        if let Some(previous) = run.previous_run_id {
            writeln!(out, "  resumed:   run {previous}")?;
        }
        if let Some(metrics) = &run.metrics {
            // Java's aggregate counter string, verbatim: tooling greps it (`MET-005`).
            writeln!(out, "  metrics:   {metrics}")?;
        }

        writeln!(out, "\n  ranges by status:")?;
        for (status, count) in &self.detail.ranges_by_status {
            writeln!(out, "    {status:<14} {count}")?;
        }

        writeln!(
            out,
            "\n  {} range(s) a resume would re-plan.",
            self.detail.pending_ranges
        )?;
        if !self.detail.pending_sample.is_empty() {
            writeln!(out, "  first pending token ranges:")?;
            for (start, end) in &self.detail.pending_sample {
                writeln!(out, "    ({start}, {end}]")?;
            }
            if self.detail.pending_ranges > self.detail.pending_sample.len() {
                writeln!(
                    out,
                    "    … and {} more",
                    self.detail.pending_ranges - self.detail.pending_sample.len()
                )?;
            }
        }
        Ok(())
    }

    fn has_findings(&self) -> bool {
        // An unfinished run is a finding: it is the reason somebody typed this command, and a
        // pipeline that gates on it must be able to see it in the exit code (`CLI-004`).
        self.detail.pending_ranges > 0
    }
}

/// What `cdm runs cancel` prints (`TRK-034`).
#[derive(Debug, Serialize)]
pub struct CancelReport {
    /// The run that was marked `ABORTED`.
    pub run_id: i64,
    /// The table it belongs to.
    pub table: String,
}

impl Report for CancelReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(
            out,
            "Run {} on {} is marked ABORTED.",
            self.run_id, self.table
        )?;
        // Saying this plainly matters: an operator who reads "cancelled" and walks away from a
        // still-running process has been misled by the command they trusted.
        writeln!(
            out,
            "\nThis records the status; it does not stop a process that is still running. \
             A node executing this run stops at its next range boundary if it is watching the \
             run row, and otherwise must be stopped where it runs. The run stays resumable \
             (TRK-030)."
        )
    }
}

/// What `cdm runs resume` prints (`TRK-038`, `TRK-039`).
#[derive(Debug, Serialize)]
pub struct ResumeReport {
    /// The run that was resumed.
    pub previous_run_id: i64,
    /// The new run's id, absent when nothing was outstanding and no run was started.
    pub run_id: Option<i64>,
    /// The job that was rebuilt, from the previous run's `run_type` (`TRK-013`).
    pub job: &'static str,
    /// How many recorded ranges the resume considered.
    pub ranges_considered: usize,
    /// How many of them it re-planned and ran.
    pub ranges_replanned: usize,
    /// The ranges it refused to replay (`DST-015`).
    pub quarantined: Vec<crate::harness::QuarantineEntry>,
    /// The run's own summary, absent when nothing ran.
    pub run: Option<crate::harness::RunSummary>,
    /// The `MET-033` document, for `--summary-out`. Not part of the rendered report: it is a
    /// second, much longer view of the same run, and `--output json` renders one document.
    #[serde(skip)]
    record: Option<cdm_metrics::RunSummary>,
}

impl ResumeReport {
    fn from_outcome(outcome: crate::harness::ResumeOutcome) -> Self {
        let (run, record) = match outcome.outcome {
            Some(outcome) => (Some(outcome.summary), Some(outcome.record)),
            None => (None, None),
        };
        Self {
            previous_run_id: outcome.previous_run_id,
            run_id: outcome.run_id,
            job: cdm_track::run_type(outcome.job),
            ranges_considered: outcome.ranges_considered,
            ranges_replanned: outcome.ranges_replanned,
            quarantined: outcome.quarantined,
            run,
            record,
        }
    }

    /// The `MET-033` document for `--summary-out`, if a run happened.
    pub const fn record(&self) -> Option<&cdm_metrics::RunSummary> {
        self.record.as_ref()
    }

    /// The process exit code (`CLI-004`).
    ///
    /// The resumed run's own, when there was one: a resume that is itself interrupted must exit
    /// `4`, because that is the code a supervisor may retry unchanged — and retrying a resume is
    /// exactly the right thing to do. Otherwise `Completed` if ranges were withheld (`TRK-039`),
    /// and `Success` only when there is genuinely nothing left.
    pub fn exit(&self) -> crate::exit::Exit {
        match &self.run {
            Some(run) => run.exit(),
            None if self.has_findings() => crate::exit::Exit::Completed,
            None => crate::exit::Exit::Success,
        }
    }
}

impl Report for ResumeReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        match self.run_id {
            Some(run_id) => writeln!(
                out,
                "Resumed run {} as run {run_id} ({}): {} of {} recorded range(s) re-planned.\n",
                self.previous_run_id, self.job, self.ranges_replanned, self.ranges_considered
            )?,
            None => writeln!(
                out,
                "Run {} has no outstanding ranges; nothing was re-run.\n",
                self.previous_run_id
            )?,
        }
        if let Some(run) = &self.run {
            run.render_human(out)?;
        }
        if !self.quarantined.is_empty() {
            // Loudly, and never as a footnote: these ranges are unfinished, they were *not* run,
            // and no later resume will pick them up either. A human has to reconcile them.
            writeln!(
                out,
                "\nWARNING: {} range(s) were left unfinished by run {} and could not be safely \
                 re-run (DST-015):",
                self.quarantined.len(),
                self.previous_run_id
            )?;
            for entry in &self.quarantined {
                writeln!(
                    out,
                    "    {} [{}] — {}",
                    entry.range,
                    entry.status.as_deref().unwrap_or("unrecognised"),
                    entry.reason
                )?;
            }
            writeln!(
                out,
                "  They remain recorded against run {}; `cdm runs show {}` lists them.",
                self.previous_run_id, self.previous_run_id
            )?;
        }
        Ok(())
    }

    fn has_findings(&self) -> bool {
        // TRK-039: a resume that withheld ranges did not recover the run, and must not exit 0. So
        // must a resumed run that failed ranges of its own — `RunSummary` decides that part.
        !self.quarantined.is_empty() || self.run.as_ref().is_some_and(Report::has_findings)
    }
}

/// Re-runs the ranges a previous run did not finish (`TRK-038`).
///
/// # Errors
///
/// As [`show`], plus everything a job can fail with before it starts.
pub fn resume(
    args: &crate::cli::JobArgs,
    options: crate::harness::ResumeOptions,
    presentation: crate::tui::Presentation,
) -> Result<ResumeReport, CdmError> {
    crate::harness::resume(args, options, presentation).map(ResumeReport::from_outcome)
}

/// Lists the runs recorded for the configured target table (`TRK-034`).
///
/// # Errors
///
/// [`ErrorKind::Config`] for a configuration that cannot be assembled or names no table,
/// [`ErrorKind::Connect`] for an unreachable target, and [`ErrorKind::Tracking`] when the tracking
/// tables cannot be read.
pub fn list(args: &ConfigArgs) -> Result<ListReport, CdmError> {
    with_manager(args, |manager, table| async move {
        Ok(ListReport {
            table,
            runs: manager.list(None).await?,
        })
    })
}

/// Shows one run in detail (`TRK-034`).
///
/// # Errors
///
/// As [`list`], plus [`ErrorKind::Tracking`] when no such run is recorded.
pub fn show(args: &ConfigArgs, run_id: i64) -> Result<ShowReport, CdmError> {
    with_manager(args, move |manager, table| async move {
        // The idempotent policy is the reporting one: it re-plans Java's four statuses, which is
        // what "pending" means to an operator asking what is left. A counter table's stricter
        // policy is a property of the *resume*, and applying it here would under-report what is
        // outstanding on a table that is not one.
        let detail = manager
            .show(RunId::from_raw(run_id), RerunPolicy::idempotent())
            .await?;
        Ok(ShowReport { table, detail })
    })
}

/// Marks a run `ABORTED` (`TRK-034`).
///
/// # Errors
///
/// As [`show`].
pub fn cancel(args: &ConfigArgs, run_id: i64) -> Result<CancelReport, CdmError> {
    with_manager(args, move |manager, table| async move {
        manager.cancel(RunId::from_raw(run_id)).await?;
        Ok(CancelReport { run_id, table })
    })
}

/// Loads the configuration, opens the target, and hands a [`RunManager`] to `body`.
///
/// Only the target is opened. Run tracking lives in the target keyspace (`TRK-010`), so connecting
/// to the origin as well would make `cdm runs list` fail for a perfectly readable history whenever
/// the origin happens to be down — which, on the day somebody is asking what their last migration
/// did, is not unlikely.
fn with_manager<F, Fut, R>(args: &ConfigArgs, body: F) -> Result<R, CdmError>
where
    F: FnOnce(RunManager<CassandraStore>, String) -> Fut,
    Fut: std::future::Future<Output = Result<R, CdmError>>,
{
    let outcome = load(args)?;
    let Some(config) = outcome.config else {
        return Err(CdmError::new(
            ErrorKind::Config,
            "the configuration could not be assembled; run `cdm config validate` to see why",
        ));
    };
    let config = cdm_config::EffectiveConfig::resolve(config);

    // `CFG-023`: the target defaults to the origin when unset, so the run history is keyed by
    // whichever of the two the run actually wrote to.
    let table = config
        .target_table()
        .or_else(|| config.origin_table())
        .cloned()
        .ok_or_else(|| {
            CdmError::new(
                ErrorKind::Config,
                "run tracking is keyed by the target table, and none is configured; set \
                 `schema.origin.keyspace_table` (or `schema.target.keyspace_table`)",
            )
            .with_context(|c| c.with_config_key("schema.target.keyspace_table"))
        })?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!("cannot start the async runtime: {error}"),
            )
        })?;

    runtime.block_on(async {
        let session = cdm_cql::connect::connect(config.config(), cdm_core::Side::Target).await?;
        let store = CassandraStore::for_target(&session, &table)?;
        let label = table.to_string();
        body(RunManager::new(Arc::new(store), table), label).await
    })
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
    use crate::exit::Exit;
    use crate::harness::QuarantineEntry;

    use super::*;

    fn quarantined(count: usize) -> Vec<QuarantineEntry> {
        (0..count)
            .map(|i| QuarantineEntry {
                range: format!("({}, {}]", i * 10, i * 10 + 9),
                status: Some("STARTED".to_owned()),
                reason: "counter range may have partially applied; manual reconciliation required",
            })
            .collect()
    }

    fn report(quarantined: Vec<QuarantineEntry>) -> ResumeReport {
        ResumeReport {
            previous_run_id: 10,
            run_id: Some(11),
            job: "MIGRATE",
            ranges_considered: 8,
            ranges_replanned: 3,
            quarantined,
            run: None,
            record: None,
        }
    }

    #[test]
    fn trk_039_a_resume_that_withheld_ranges_does_not_report_success() {
        // The whole point of `TRK-039`. These ranges are unfinished, were not run, and no later
        // resume will pick them up; a zero exit code would make that indistinguishable from a
        // clean recovery to whatever pipeline gates on it.
        let withheld = report(quarantined(2));
        assert!(withheld.has_findings());
        assert_eq!(withheld.exit(), Exit::Completed);

        let clean = report(Vec::new());
        assert!(!clean.has_findings());
        assert_eq!(clean.exit(), Exit::Success);
    }

    #[test]
    fn trk_039_every_withheld_range_is_named_with_its_status_and_reason() {
        let mut out = Vec::new();
        report(quarantined(2)).render_human(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        assert!(text.contains("WARNING"), "{text}");
        assert!(text.contains("(0, 9]"), "{text}");
        assert!(text.contains("(10, 19]"), "{text}");
        assert!(text.contains("STARTED"), "{text}");
        assert!(text.contains("reconciliation"), "{text}");
        // And where they still live, so the operator has somewhere to go.
        assert!(text.contains("cdm runs show 10"), "{text}");
    }

    #[test]
    fn trk_038_a_resume_with_nothing_outstanding_says_so_and_exits_zero() {
        let report = ResumeReport {
            run_id: None,
            ranges_replanned: 0,
            ..report(Vec::new())
        };
        let mut out = Vec::new();
        report.render_human(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("no outstanding ranges"), "{text}");
        assert_eq!(report.exit(), Exit::Success);
    }

    #[test]
    fn sec_002_a_resume_report_serialises_bounds_counts_and_nothing_else() {
        let json = serde_json::to_string(&report(quarantined(1))).unwrap();
        assert!(json.contains("\"ranges_replanned\":3"), "{json}");
        assert!(json.contains("previous_run_id"), "{json}");
        for forbidden in ["password", "username", "secret"] {
            assert!(!json.to_lowercase().contains(forbidden), "{json}");
        }
    }
}
