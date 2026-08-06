//! An in-process tracking backend (`TRK-036`).
//!
//! Two uses, and they are the same use: a test that wants to assert on what tracking recorded,
//! and a run whose tracking must not outlive the process. It keeps the same invariants the
//! Cassandra backend does — in particular `TRK-020`'s refusal to reuse a run id — because a test
//! that passes against a store with weaker rules proves nothing about the one production uses.

use std::collections::BTreeMap;

use async_trait::async_trait;
use cdm_core::{
    CdmError, ErrorKind, JobKind, Plugin, RangeRecord, RunId, RunRecord, RunStatus, TableRef,
    TrackingStore,
};
use parking_lot::Mutex;

/// One run's rows, as the tracking tables would hold them.
#[derive(Debug, Clone)]
struct StoredRun {
    info: RunRecord,
    /// Keyed by `token_min`, which is the clustering key of `cdm_run_details` (`TRK-010`), so
    /// that a second write to the same range replaces the first exactly as an `UPDATE` would.
    ranges: BTreeMap<i128, RangeRecord>,
}

/// A [`TrackingStore`] that keeps everything in memory (`TRK-036`).
#[derive(Debug, Default)]
pub struct MemoryStore {
    runs: Mutex<BTreeMap<i64, StoredRun>>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many runs have been recorded. For assertions.
    pub fn run_count(&self) -> usize {
        self.runs.lock().len()
    }

    /// Every run recorded for `table` and `job`, newest first (`TRK-034`).
    pub fn runs_for(&self, table: &TableRef, job: JobKind) -> Vec<RunRecord> {
        let mut found: Vec<RunRecord> = self
            .runs
            .lock()
            .values()
            .filter(|stored| stored.info.table == *table && stored.info.job == job)
            .map(|stored| stored.info.clone())
            .collect();
        found.sort_by_key(|run| std::cmp::Reverse(run.run_id));
        found
    }
}

impl Plugin for MemoryStore {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn provider(&self) -> &'static str {
        "cdm-track"
    }
}

#[async_trait]
impl TrackingStore for MemoryStore {
    async fn initialise(&self) -> Result<(), CdmError> {
        Ok(())
    }

    async fn create_run(&self, run: &RunRecord, ranges: &[RangeRecord]) -> Result<(), CdmError> {
        let mut runs = self.runs.lock();
        if runs.contains_key(&run.run_id.as_i64()) {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                format!(
                    "run id {} already exists for table {}",
                    run.run_id, run.table
                ),
            ));
        }
        // TRK-020's order, which matters even here: the run row exists as NOT_STARTED before any
        // range row does, so a reader that sees a range row always sees the run it belongs to.
        let mut stored = StoredRun {
            info: RunRecord {
                status: RunStatus::NotStarted,
                ..run.clone()
            },
            ranges: BTreeMap::new(),
        };
        for range in ranges {
            stored.ranges.insert(
                range.range.min(),
                RangeRecord {
                    status: RunStatus::NotStarted,
                    started_at: None,
                    info: None,
                    range: range.range,
                },
            );
        }
        stored.info.status = RunStatus::Started;
        runs.insert(run.run_id.as_i64(), stored);
        Ok(())
    }

    async fn update_run(
        &self,
        run_id: RunId,
        status: RunStatus,
        info: Option<&str>,
    ) -> Result<(), CdmError> {
        let mut runs = self.runs.lock();
        let stored = runs.get_mut(&run_id.as_i64()).ok_or_else(|| {
            CdmError::new(ErrorKind::Tracking, format!("run {run_id} does not exist"))
        })?;
        stored.info.status = status;
        if let Some(info) = info {
            stored.info.info = Some(info.to_owned());
        }
        if status == RunStatus::Ended {
            stored.info.ended_at = Some(chrono::Utc::now());
        }
        Ok(())
    }

    async fn update_range(&self, run_id: RunId, range: &RangeRecord) -> Result<(), CdmError> {
        let mut runs = self.runs.lock();
        let stored = runs.get_mut(&run_id.as_i64()).ok_or_else(|| {
            CdmError::new(ErrorKind::Tracking, format!("run {run_id} does not exist"))
        })?;
        let entry = stored
            .ranges
            .entry(range.range.min())
            .or_insert_with(|| range.clone());
        entry.status = range.status;
        entry.range = range.range;
        if range.started_at.is_some() {
            entry.started_at = range.started_at;
        }
        // A start write carries no metrics, and must not erase the ones an earlier attempt left:
        // the Cassandra backend achieves that with two different `UPDATE` statements, and this
        // one has to reproduce the same effect or the two disagree under test.
        if range.info.is_some() {
            entry.info.clone_from(&range.info);
        }
        Ok(())
    }

    async fn run(&self, run_id: RunId) -> Result<Option<RunRecord>, CdmError> {
        Ok(self
            .runs
            .lock()
            .get(&run_id.as_i64())
            .map(|stored| stored.info.clone()))
    }

    async fn ranges(&self, run_id: RunId) -> Result<Vec<RangeRecord>, CdmError> {
        Ok(self
            .runs
            .lock()
            .get(&run_id.as_i64())
            .map(|stored| stored.ranges.values().cloned().collect())
            .unwrap_or_default())
    }

    async fn latest_run(
        &self,
        table: &TableRef,
        job: JobKind,
    ) -> Result<Option<RunRecord>, CdmError> {
        Ok(self.runs_for(table, job).into_iter().next())
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

    fn table() -> TableRef {
        TableRef::new("ks", "t")
    }

    fn run_record(run_id: i64) -> RunRecord {
        RunRecord {
            run_id: RunId::from_raw(run_id),
            previous_run_id: None,
            table: table(),
            job: JobKind::Migrate,
            status: RunStatus::NotStarted,
            started_at: Some(chrono::Utc::now()),
            ended_at: None,
            info: None,
        }
    }

    fn range_record(min: i128, max: i128) -> RangeRecord {
        RangeRecord {
            range: TokenRange::new(min, max).unwrap(),
            status: RunStatus::NotStarted,
            started_at: None,
            info: None,
        }
    }

    #[tokio::test]
    async fn trk_020_a_run_id_that_already_exists_is_rejected() {
        let store = MemoryStore::new();
        store
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap();
        let err = store
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Tracking);
        assert!(err.to_string().contains("already exists"));
        assert_eq!(store.run_count(), 1);
    }

    #[tokio::test]
    async fn trk_020_creation_leaves_the_run_started_and_every_range_not_started() {
        let store = MemoryStore::new();
        store
            .create_run(&run_record(1), &[range_record(0, 9), range_record(10, 19)])
            .await
            .unwrap();
        let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Started);
        let ranges = store.ranges(RunId::from_raw(1)).await.unwrap();
        assert_eq!(ranges.len(), 2);
        assert!(ranges.iter().all(|r| r.status == RunStatus::NotStarted));
    }

    #[tokio::test]
    async fn trk_021_a_start_write_does_not_erase_an_earlier_metrics_string() {
        let store = MemoryStore::new();
        store
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap();
        let run_id = RunId::from_raw(1);
        store
            .update_range(
                run_id,
                &RangeRecord {
                    status: RunStatus::Fail,
                    info: Some("Read: 5; Write: 0".to_owned()),
                    ..range_record(0, 9)
                },
            )
            .await
            .unwrap();
        store
            .update_range(
                run_id,
                &RangeRecord {
                    status: RunStatus::Started,
                    started_at: Some(chrono::Utc::now()),
                    info: None,
                    ..range_record(0, 9)
                },
            )
            .await
            .unwrap();
        let ranges = store.ranges(run_id).await.unwrap();
        assert_eq!(ranges[0].status, RunStatus::Started);
        assert_eq!(ranges[0].info.as_deref(), Some("Read: 5; Write: 0"));
        assert!(ranges[0].started_at.is_some());
    }

    #[tokio::test]
    async fn trk_022_ending_a_run_records_the_status_metrics_and_end_time() {
        let store = MemoryStore::new();
        store
            .create_run(&run_record(1), &[range_record(0, 9)])
            .await
            .unwrap();
        store
            .update_run(RunId::from_raw(1), RunStatus::Ended, Some("Read: 9"))
            .await
            .unwrap();
        let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Ended);
        assert_eq!(run.info.as_deref(), Some("Read: 9"));
        assert!(run.ended_at.is_some());
    }

    #[tokio::test]
    async fn trk_030_the_latest_run_is_the_highest_id_for_the_table_and_job() {
        let store = MemoryStore::new();
        for id in [10_i64, 30, 20] {
            store.create_run(&run_record(id), &[]).await.unwrap();
        }
        let mut other_job = run_record(40);
        other_job.job = JobKind::Validate;
        store.create_run(&other_job, &[]).await.unwrap();

        let latest = store
            .latest_run(&table(), JobKind::Migrate)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(latest.run_id, RunId::from_raw(30), "run_type must filter");
        assert_eq!(
            store
                .latest_run(&TableRef::new("ks", "other"), JobKind::Migrate)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn trk_036_the_memory_backend_answers_the_whole_trait() {
        let store: &dyn TrackingStore = &MemoryStore::new();
        store.initialise().await.unwrap();
        assert_eq!(store.name(), "memory");
        assert_eq!(store.provider(), "cdm-track");
        assert_eq!(store.run(RunId::from_raw(1)).await.unwrap(), None);
        assert!(store.ranges(RunId::from_raw(1)).await.unwrap().is_empty());
        // Updating a run that does not exist is an error rather than a silent no-op: a tracker
        // that cannot find its own run row has lost the thread and must say so.
        assert!(store
            .update_run(RunId::from_raw(1), RunStatus::Ended, None)
            .await
            .is_err());
    }
}
