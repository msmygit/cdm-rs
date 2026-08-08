//! An in-process tracking backend (`TRK-036`).
//!
//! Two uses, and they are the same use: a test that wants to assert on what tracking recorded,
//! and a run whose tracking must not outlive the process. It keeps the same invariants the
//! Cassandra backend does — in particular `TRK-020`'s refusal to reuse a run id — because a test
//! that passes against a store with weaker rules proves nothing about the one production uses.

use std::collections::BTreeMap;

use async_trait::async_trait;
use cdm_core::{
    CdmError, ErrorKind, JobKind, LeaseOutcome, LeaseRecord, LeaseStore, Plugin, RangeRecord,
    RunClaim, RunId, RunRecord, RunStatus, TableRef, TokenRange, TrackingStore,
};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;

/// One run's rows, as the tracking tables would hold them.
#[derive(Debug, Clone)]
struct StoredRun {
    info: RunRecord,
    /// Keyed by `token_min`, which is the clustering key of `cdm_run_details` (`TRK-010`), so
    /// that a second write to the same range replaces the first exactly as an `UPDATE` would.
    ranges: BTreeMap<i128, RangeRecord>,
    /// The `cdm_run_leases` partition of this run, keyed by `token_min` (`DST-010`).
    leases: BTreeMap<i64, LeaseRecord>,
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
            leases: BTreeMap::new(),
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

/// The `token_min` of a range, refusing to truncate exactly as the Cassandra backend does.
///
/// The bound is a `bigint` column there (`TRK-010`), so a store that quietly accepted a wider
/// token here would let a test pass that production cannot.
fn token_min(range: TokenRange) -> Result<i64, CdmError> {
    i64::try_from(range.min()).map_err(|_| {
        CdmError::new(
            ErrorKind::Lease,
            format!(
                "token {} of range {range} does not fit in a bigint",
                range.min()
            ),
        )
    })
}

/// Lease semantics without a cluster (`DST-010`..`DST-013`).
///
/// # What this store does and does not prove
///
/// Every method takes the one mutex that guards the whole store, so the claims of two callers are
/// **totally ordered** — which is precisely the property `SERIAL` gives the Cassandra backend, and
/// it is why a contention test written against this store says something true about the one
/// production uses. What it cannot reproduce is a *failure*: a lost coordinator, a partition, a
/// timed-out transaction whose outcome is unknown. Those need a cluster and a way to kill it,
/// which is `DST-019` and `TST-042` (#52).
///
/// It is also, deliberately, not a distributed store: nothing outside this process can see it, so
/// it can never back a real `DST-001` run. Only [`CassandraStore`](super::CassandraStore) can.
#[async_trait]
impl LeaseStore for MemoryStore {
    async fn initialise_leases(&self) -> Result<(), CdmError> {
        Ok(())
    }

    async fn initialise_run(
        &self,
        run: &RunRecord,
        ranges: &[RangeRecord],
        config_hash: &str,
    ) -> Result<RunClaim, CdmError> {
        // One critical section, for the reason the Cassandra backend uses one transaction: a
        // second node must not observe the run row between its insertion and the range rows
        // `TRK-020` promises accompany it. Checking under one lock and inserting under another
        // would make two nodes racing to initialise fail with "run id already exists" instead of
        // one of them simply joining, which is the whole of `DST-002`.
        let mut runs = self.runs.lock();
        if let Some(existing) = runs.get(&run.run_id.as_i64()) {
            return Ok(RunClaim::Lost(existing.info.clone()));
        }
        let mut stored = StoredRun {
            info: RunRecord {
                // DST-003: the hash lives in `run_info` until `TRK-022` replaces it with the
                // metrics string, which is why a joining node checks it at join time.
                info: Some(config_hash.to_owned()),
                status: RunStatus::Started,
                ..run.clone()
            },
            ranges: BTreeMap::new(),
            leases: BTreeMap::new(),
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
        runs.insert(run.run_id.as_i64(), stored);
        Ok(RunClaim::Won)
    }

    async fn lease(
        &self,
        run_id: RunId,
        range: TokenRange,
    ) -> Result<Option<LeaseRecord>, CdmError> {
        let token_min = token_min(range)?;
        Ok(self
            .runs
            .lock()
            .get(&run_id.as_i64())
            .and_then(|stored| stored.leases.get(&token_min).cloned()))
    }

    /// The two conditional statements of `DST-011`, as one critical section each.
    ///
    /// `observed == None` is `INSERT ... IF NOT EXISTS`, so an existing row denies the claim even
    /// when that row has expired — the caller's next pass will see it and take the other branch.
    /// `observed == Some` is `UPDATE ... IF lease_until < now`, and writes `observed.attempt + 1`
    /// rather than the stored attempt, because that is what the transaction the Cassandra backend
    /// issues can know. Mirroring it here is what makes the attempt-undercount the trait
    /// documents reproducible in a test rather than a surprise in production.
    async fn claim_range(
        &self,
        run_id: RunId,
        range: TokenRange,
        node_id: &str,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        observed: Option<&LeaseRecord>,
    ) -> Result<LeaseOutcome, CdmError> {
        let token_min = token_min(range)?;
        let mut runs = self.runs.lock();
        let stored = runs.get_mut(&run_id.as_i64()).ok_or_else(|| {
            CdmError::new(
                ErrorKind::Lease,
                format!("run {run_id} does not exist, so its ranges cannot be leased"),
            )
        })?;
        let attempt = match (stored.leases.get(&token_min), observed) {
            // `IF NOT EXISTS` against a row that exists.
            (Some(current), None) => return Ok(LeaseOutcome::Denied(current.clone())),
            // `IF lease_until < now` against a lease that is still live.
            (Some(current), Some(_)) if current.lease_until > now => {
                return Ok(LeaseOutcome::Denied(current.clone()))
            }
            // DST-012: a reclaim counts, which is what makes DST-013's bound countable.
            (Some(_), Some(hint)) => hint.attempt.saturating_add(1),
            // `IF lease_until < now` against a row that has since been deleted; no row, no claim.
            (None, Some(hint)) => return Ok(LeaseOutcome::Denied(hint.clone())),
            (None, None) => 1,
        };
        let granted = LeaseRecord {
            token_min,
            node_id: node_id.to_owned(),
            lease_until,
            attempt,
        };
        stored.leases.insert(token_min, granted.clone());
        Ok(LeaseOutcome::Granted(granted))
    }

    async fn renew_lease(
        &self,
        run_id: RunId,
        range: TokenRange,
        node_id: &str,
        lease_until: DateTime<Utc>,
    ) -> Result<bool, CdmError> {
        let token_min = token_min(range)?;
        let mut runs = self.runs.lock();
        let Some(stored) = runs.get_mut(&run_id.as_i64()) else {
            return Ok(false);
        };
        match stored.leases.get_mut(&token_min) {
            // The condition is `IF node_id = ?`: a node whose lease was reclaimed cannot extend
            // the new holder's, and learns it lost the range from the `false` this returns.
            Some(current) if current.node_id == node_id => {
                current.lease_until = lease_until;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn release_lease(
        &self,
        run_id: RunId,
        range: TokenRange,
        node_id: &str,
    ) -> Result<bool, CdmError> {
        self.renew_lease(run_id, range, node_id, DateTime::UNIX_EPOCH)
            .await
    }

    async fn leases(&self, run_id: RunId) -> Result<Vec<LeaseRecord>, CdmError> {
        Ok(self
            .runs
            .lock()
            .get(&run_id.as_i64())
            .map(|stored| stored.leases.values().cloned().collect())
            .unwrap_or_default())
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

    #[tokio::test]
    async fn dst_002_the_election_is_lost_rather_than_erroring_when_the_run_exists() {
        let store = MemoryStore::new();
        assert_eq!(
            store
                .initialise_run(&run_record(1), &[range_record(0, 9)], "config_hash=abc")
                .await
                .unwrap(),
            RunClaim::Won
        );
        let RunClaim::Lost(existing) = store
            .initialise_run(&run_record(1), &[range_record(0, 9)], "config_hash=abc")
            .await
            .unwrap()
        else {
            panic!("a second initialisation must lose the election, not create a second run");
        };
        assert_eq!(existing.status, RunStatus::Started);
        assert_eq!(existing.info.as_deref(), Some("config_hash=abc"));
        assert_eq!(store.run_count(), 1);
    }

    #[tokio::test]
    async fn dst_011_a_claim_that_believed_the_range_was_free_is_denied_when_it_is_not() {
        // The stale-hint branch: `INSERT ... IF NOT EXISTS` against a row that exists denies the
        // claim whatever the row says, because that is what the transaction does. The coordinator
        // reads before it claims, so it reaches this only when the row appeared in between — the
        // case a store with weaker rules would silently grant.
        let store = MemoryStore::new();
        let range = TokenRange::new(0, 9).unwrap();
        let run = RunId::from_raw(1);
        store
            .initialise_run(&run_record(1), &[range_record(0, 9)], "config_hash=abc")
            .await
            .unwrap();
        let now = chrono::Utc::now();
        let until = now + chrono::Duration::seconds(60);
        assert!(matches!(
            store
                .claim_range(run, range, "node-a", now, until, None)
                .await
                .unwrap(),
            LeaseOutcome::Granted(_)
        ));
        let LeaseOutcome::Denied(holder) = store
            .claim_range(run, range, "node-b", now, until, None)
            .await
            .unwrap()
        else {
            panic!("an existing row must deny an unconditional first claim");
        };
        assert_eq!(holder.node_id, "node-a");
        assert_eq!(store.lease(run, range).await.unwrap().unwrap().attempt, 1);
    }

    #[tokio::test]
    async fn dst_011_a_reclaim_of_a_row_that_has_gone_grants_nothing() {
        let store = MemoryStore::new();
        let range = TokenRange::new(0, 9).unwrap();
        let run = RunId::from_raw(1);
        store
            .initialise_run(&run_record(1), &[range_record(0, 9)], "config_hash=abc")
            .await
            .unwrap();
        let now = chrono::Utc::now();
        let hint = LeaseRecord {
            token_min: 0,
            node_id: "node-a".to_owned(),
            lease_until: now - chrono::Duration::seconds(1),
            attempt: 1,
        };
        assert!(matches!(
            store
                .claim_range(run, range, "node-b", now, now, Some(&hint))
                .await
                .unwrap(),
            LeaseOutcome::Denied(_)
        ));
        assert!(store.lease(run, range).await.unwrap().is_none());
    }
}
