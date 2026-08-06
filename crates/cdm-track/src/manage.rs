//! First-class run management: what `cdm runs` and `GET /v1/runs` are built on (`TRK-034`).
//!
//! Java offers no way to ask "which runs exist, and what happened to them?" — the tracking tables
//! are there, but reading them means writing CQL by hand and knowing the schema. The operations
//! here are the answer, and they live in this crate rather than in the CLI so that the terminal
//! and the REST API cannot drift apart: both render the same [`RunSummary`] and [`RunDetail`].
//!
//! # Nothing here can leak a row
//!
//! A summary carries run ids, statuses, timestamps, token bounds and counter strings. It carries
//! no column names from the migrated table, no primary keys and no values (`SEC-002`), and no
//! part of the configuration (`SEC-001`). That is a property of the types: there is no field a
//! caller could put a row in.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{
    CdmError, ErrorKind, JobKind, RangeRecord, RunId, RunRecord, RunStatus, TableRef, TrackingStore,
};
use serde::Serialize;

use crate::compat::run_type;
use crate::resume::{plan_resume, RerunPolicy, ResumePlan};

/// A backend that can enumerate runs, not merely find the latest one (`TRK-034`).
///
/// Separate from [`TrackingStore`] because listing is a management concern, not a run-time one: a
/// third-party store can be perfectly useful for recording a run without being able to page
/// through the history, and requiring it of everyone would make the trait harder to implement for
/// no benefit to the migration itself.
#[async_trait]
pub trait RunCatalog: Send + Sync {
    /// Every run recorded for `table`, newest first, optionally narrowed to one job.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`].
    async fn runs(
        &self,
        table: &TableRef,
        job: Option<JobKind>,
    ) -> Result<Vec<RunRecord>, CdmError>;
}

#[async_trait]
impl RunCatalog for crate::store::MemoryStore {
    async fn runs(
        &self,
        table: &TableRef,
        job: Option<JobKind>,
    ) -> Result<Vec<RunRecord>, CdmError> {
        let mut runs: Vec<RunRecord> = match job {
            Some(job) => self.runs_for(table, job),
            None => JobKind::ALL
                .into_iter()
                .flat_map(|job| self.runs_for(table, job))
                .collect(),
        };
        runs.sort_by_key(|run| std::cmp::Reverse(run.run_id));
        Ok(runs)
    }
}

#[async_trait]
impl RunCatalog for crate::store::CassandraStore {
    async fn runs(
        &self,
        _table: &TableRef,
        job: Option<JobKind>,
    ) -> Result<Vec<RunRecord>, CdmError> {
        let mut runs = self.all_runs().await?;
        if let Some(job) = job {
            runs.retain(|run| run.job == job);
        }
        Ok(runs)
    }
}

/// One line of `cdm runs list` (`TRK-034`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunSummary {
    /// The run id, as stored.
    pub run_id: i64,
    /// The run this one resumed, if any.
    pub previous_run_id: Option<i64>,
    /// `MIGRATE`, `VALIDATE` or `GUARDRAIL` — the value in the `run_type` column (`TRK-013`).
    pub run_type: &'static str,
    /// The run's status.
    pub status: RunStatus,
    /// When it started, RFC 3339 UTC (`NFR-007`).
    pub started_at: Option<String>,
    /// When it ended.
    pub ended_at: Option<String>,
    /// The aggregate committed metrics string (`MET-005`).
    pub metrics: Option<String>,
    /// Whether `auto_rerun` would adopt this run (`TRK-030`).
    pub resumable: bool,
}

impl RunSummary {
    /// Summarises a run row.
    pub fn from_record(run: &RunRecord) -> Self {
        Self {
            run_id: run.run_id.as_i64(),
            previous_run_id: run.previous_run_id.as_ref().map(RunId::as_i64),
            run_type: run_type(run.job),
            status: run.status,
            started_at: run.started_at.map(|t| t.to_rfc3339()),
            ended_at: run.ended_at.map(|t| t.to_rfc3339()),
            metrics: run.info.clone(),
            resumable: crate::resume::is_resumable(run),
        }
    }
}

/// What `cdm runs show` prints (`TRK-034`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunDetail {
    /// The run itself.
    pub run: RunSummary,
    /// How many ranges are in each status, in `RunStatus::ALL` order.
    pub ranges_by_status: BTreeMap<String, usize>,
    /// How many ranges a resume would re-plan (`TRK-031`).
    pub pending_ranges: usize,
    /// The token bounds of the pending ranges, capped so that a five-thousand-range run does not
    /// print five thousand lines. Bounds only — never a key or a value (`SEC-002`).
    pub pending_sample: Vec<(i64, i64)>,
}

/// How many pending ranges [`RunDetail`] lists before it stops.
///
/// A default plan has five thousand ranges. Printing all of them is not a report, it is a dump,
/// and the number that matters — [`RunDetail::pending_ranges`] — is exact regardless.
pub const PENDING_SAMPLE_LIMIT: usize = 20;

/// Run management over any store (`TRK-034`).
#[derive(Debug)]
pub struct RunManager<C> {
    store: Arc<C>,
    table: TableRef,
}

impl<C> RunManager<C>
where
    C: TrackingStore + RunCatalog,
{
    /// Manages the runs recorded for `table`.
    pub fn new(store: Arc<C>, table: TableRef) -> Self {
        Self { store, table }
    }

    /// `cdm runs list` / `GET /v1/runs` (`TRK-034`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`].
    pub async fn list(&self, job: Option<JobKind>) -> Result<Vec<RunSummary>, CdmError> {
        Ok(self
            .store
            .runs(&self.table, job)
            .await?
            .iter()
            .map(RunSummary::from_record)
            .collect())
    }

    /// `cdm runs show` / `GET /v1/runs/{id}` (`TRK-034`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] when the run does not exist — a missing run is an operator
    /// error worth reporting, unlike in [`plan_resume`], where it is a reason to plan afresh.
    pub async fn show(&self, run_id: RunId, policy: RerunPolicy) -> Result<RunDetail, CdmError> {
        let run = self.store.run(run_id).await?.ok_or_else(|| {
            CdmError::new(
                ErrorKind::Tracking,
                format!("no run {run_id} is recorded for {}", self.table),
            )
        })?;
        let ranges = self.store.ranges(run_id).await?;
        Ok(summarise(&run, &ranges, policy))
    }

    /// `cdm runs resume` (`TRK-030`..`TRK-033`).
    ///
    /// `previous` is the run to resume, or `None` to let `auto_rerun` choose the most recent
    /// unfinished one. `run_id` is the *new* run's id, which seeds the shuffle.
    ///
    /// The returned plan may be a fallback; see [`ResumePlan::is_fallback`]. A caller that treats
    /// an empty plan as "nothing to do" without checking that flag skips the whole table.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`].
    pub async fn resume(
        &self,
        previous: Option<RunId>,
        job: JobKind,
        policy: RerunPolicy,
        rerun_multiplier: u32,
        run_id: RunId,
    ) -> Result<Option<ResumePlan>, CdmError> {
        let previous_run_id = match previous {
            Some(id) if !id.is_unset() => Some(id),
            _ => crate::resume::adopt_previous_run(
                self.store.latest_run(&self.table, job).await?.as_ref(),
            ),
        };
        // No previous run at all is not a fallback: there is nothing to fall back *from*, and the
        // caller simply plans a fresh run with `prev_run_id = 0`, as Java does.
        let Some(previous_run_id) = previous_run_id else {
            return Ok(None);
        };
        let previous = self.store.run(previous_run_id).await?;
        let ranges = match previous {
            Some(_) => self.store.ranges(previous_run_id).await?,
            None => Vec::new(),
        };
        plan_resume(
            previous_run_id,
            previous.as_ref(),
            &ranges,
            policy,
            rerun_multiplier,
            run_id,
        )
        .map(Some)
    }

    /// `cdm runs cancel` (`TRK-034`).
    ///
    /// Records `ABORTED` on the run row. It does not stop a process — nothing here can reach
    /// another node's scheduler — but `TRK-030` adopts anything that is not `ENDED`, so a
    /// cancelled run is resumable, and `DST-002`'s joiners see it is over.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`].
    pub async fn cancel(&self, run_id: RunId) -> Result<(), CdmError> {
        if self.store.run(run_id).await?.is_none() {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                format!("no run {run_id} is recorded for {}", self.table),
            ));
        }
        self.store
            .update_run(run_id, RunStatus::Aborted, None)
            .await
    }
}

/// A token as the `bigint` column holds it, or `None` for one that does not fit.
///
/// A report is not the place to fail: a RandomPartitioner bound wider than 64 bits is refused
/// when it is *written* (`store::cassandra::token_bound`), so by the time it is being displayed
/// there is nothing left to do but omit it from the sample. The exact count is unaffected.
fn token_column(token: i128) -> Option<i64> {
    i64::try_from(token).ok()
}

/// Builds a [`RunDetail`] from a run and its ranges.
fn summarise(run: &RunRecord, ranges: &[RangeRecord], policy: RerunPolicy) -> RunDetail {
    let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
    let mut pending = Vec::new();
    for record in ranges {
        *by_status
            .entry(record.status.as_str().to_owned())
            .or_default() += 1;
        if policy.rerunnable_statuses().contains(&record.status) {
            pending.push(record.range);
        }
    }
    pending.sort_unstable();
    let sample = pending
        .iter()
        .take(PENDING_SAMPLE_LIMIT)
        .copied()
        .filter_map(|range| Some((token_column(range.min())?, token_column(range.max())?)))
        .collect();
    RunDetail {
        run: RunSummary::from_record(run),
        ranges_by_status: by_status,
        pending_ranges: pending.len(),
        pending_sample: sample,
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
    use cdm_core::TokenRange;

    use super::*;
    use crate::store::MemoryStore;

    fn table() -> TableRef {
        TableRef::new("ks", "t")
    }

    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    fn run_record(id: i64, job: JobKind) -> RunRecord {
        crate::tracker::new_run_record(RunId::from_raw(id), None, table(), job)
    }

    fn range_record(min: i128, max: i128, status: RunStatus) -> RangeRecord {
        RangeRecord {
            range: range(min, max),
            status,
            started_at: None,
            info: None,
        }
    }

    async fn seeded() -> Arc<MemoryStore> {
        let store = Arc::new(MemoryStore::new());
        store
            .create_run(
                &run_record(10, JobKind::Migrate),
                &[
                    range_record(0, 9, RunStatus::NotStarted),
                    range_record(10, 19, RunStatus::NotStarted),
                    range_record(20, 29, RunStatus::NotStarted),
                ],
            )
            .await
            .unwrap();
        store
            .update_range(
                RunId::from_raw(10),
                &RangeRecord {
                    status: RunStatus::Pass,
                    info: Some("Read: 5".to_owned()),
                    ..range_record(0, 9, RunStatus::Pass)
                },
            )
            .await
            .unwrap();
        store
            .update_range(
                RunId::from_raw(10),
                &RangeRecord {
                    status: RunStatus::Fail,
                    info: Some("Read: 1; Error: 4".to_owned()),
                    ..range_record(10, 19, RunStatus::Fail)
                },
            )
            .await
            .unwrap();
        store
            .update_run(
                RunId::from_raw(10),
                RunStatus::Ended,
                Some("Read: 6; Partitions Failed: 1"),
            )
            .await
            .unwrap();
        store
            .create_run(&run_record(20, JobKind::Validate), &[])
            .await
            .unwrap();
        store
    }

    #[tokio::test]
    async fn trk_034_list_reports_every_run_newest_first_and_can_filter_by_job() {
        let manager = RunManager::new(seeded().await, table());
        let all = manager.list(None).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].run_id, 20, "newest first");
        assert_eq!(all[0].run_type, "VALIDATE");

        let migrations = manager.list(Some(JobKind::Migrate)).await.unwrap();
        assert_eq!(migrations.len(), 1);
        assert_eq!(migrations[0].run_id, 10);
        assert_eq!(migrations[0].run_type, "MIGRATE");
        assert!(
            migrations[0].resumable,
            "an ENDED run with failed partitions is still resumable (TRK-030)"
        );
    }

    #[tokio::test]
    async fn trk_034_show_counts_ranges_by_status_and_names_the_pending_ones() {
        let manager = RunManager::new(seeded().await, table());
        let detail = manager
            .show(RunId::from_raw(10), RerunPolicy::idempotent())
            .await
            .unwrap();
        assert_eq!(detail.run.run_id, 10);
        assert_eq!(detail.ranges_by_status.get("PASS"), Some(&1));
        assert_eq!(detail.ranges_by_status.get("FAIL"), Some(&1));
        assert_eq!(detail.ranges_by_status.get("NOT_STARTED"), Some(&1));
        assert_eq!(detail.pending_ranges, 2);
        assert_eq!(detail.pending_sample, vec![(10, 19), (20, 29)]);
    }

    #[tokio::test]
    async fn trk_034_show_and_cancel_reject_a_run_that_does_not_exist() {
        let manager = RunManager::new(seeded().await, table());
        assert!(manager
            .show(RunId::from_raw(999), RerunPolicy::idempotent())
            .await
            .is_err());
        assert!(manager.cancel(RunId::from_raw(999)).await.is_err());
    }

    #[tokio::test]
    async fn trk_034_cancel_marks_the_run_aborted_and_leaves_it_resumable() {
        let store = seeded().await;
        let manager = RunManager::new(Arc::clone(&store), table());
        manager.cancel(RunId::from_raw(20)).await.unwrap();
        let run = store.run(RunId::from_raw(20)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Aborted);
        assert!(crate::resume::is_resumable(&run));
    }

    #[tokio::test]
    async fn trk_034_resume_adopts_the_most_recent_unfinished_run_when_none_is_named() {
        let manager = RunManager::new(seeded().await, table());
        let plan = manager
            .resume(
                None,
                JobKind::Migrate,
                RerunPolicy::idempotent(),
                1,
                RunId::from_raw(11),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(plan.previous_run_id(), Some(RunId::from_raw(10)));
        let mut ranges = plan.ranges().to_vec();
        ranges.sort_unstable();
        assert_eq!(ranges, vec![range(10, 19), range(20, 29)]);
    }

    #[tokio::test]
    async fn trk_032_resume_of_an_unknown_previous_run_reports_a_fallback() {
        let manager = RunManager::new(seeded().await, table());
        let plan = manager
            .resume(
                Some(RunId::from_raw(777)),
                JobKind::Migrate,
                RerunPolicy::idempotent(),
                1,
                RunId::from_raw(11),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(plan.is_fallback(), "the caller must plan the whole ring");
    }

    #[tokio::test]
    async fn trk_030_resume_returns_none_when_there_is_nothing_to_adopt() {
        let store = Arc::new(MemoryStore::new());
        store
            .create_run(&run_record(1, JobKind::Migrate), &[])
            .await
            .unwrap();
        store
            .update_run(
                RunId::from_raw(1),
                RunStatus::Ended,
                Some("Partitions Failed: 0"),
            )
            .await
            .unwrap();
        let manager = RunManager::new(store, table());
        assert!(manager
            .resume(
                None,
                JobKind::Migrate,
                RerunPolicy::idempotent(),
                1,
                RunId::from_raw(2)
            )
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn sec_002_a_run_report_serialises_bounds_and_counters_only() {
        let manager = RunManager::new(seeded().await, table());
        let detail = manager
            .show(RunId::from_raw(10), RerunPolicy::idempotent())
            .await
            .unwrap();
        let json = serde_json::to_string(&detail).unwrap();
        assert!(json.contains("\"pending_ranges\":2"));
        assert!(json.contains("Partitions Failed: 1"));
        // The type has no field a row, a key or a credential could occupy.
        for forbidden in ["password", "username", "secret", "cassandra"] {
            assert!(!json.to_lowercase().contains(forbidden), "{json}");
        }
    }

    #[tokio::test]
    async fn trk_034_the_pending_sample_is_capped() {
        let store = Arc::new(MemoryStore::new());
        let ranges: Vec<RangeRecord> = (0..100)
            .map(|i| range_record(i * 10, i * 10 + 9, RunStatus::NotStarted))
            .collect();
        store
            .create_run(&run_record(1, JobKind::Migrate), &ranges)
            .await
            .unwrap();
        let detail = RunManager::new(store, table())
            .show(RunId::from_raw(1), RerunPolicy::idempotent())
            .await
            .unwrap();
        assert_eq!(detail.pending_ranges, 100);
        assert_eq!(detail.pending_sample.len(), PENDING_SAMPLE_LIMIT);
    }
}
