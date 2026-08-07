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
