//! The SQLite backend, driven through the tracker and the resume it exists to serve (`TRK-036`).
//!
//! The unit tests beside `store::sqlite` prove each statement does what it says. What they cannot
//! prove is the claim `TRK-036` actually makes: that a backend is *swappable* — that a run tracked
//! into a local file behaves, from the tracker's and the resume's point of view, exactly like one
//! tracked into the target keyspace. That is a claim about the seam, so it is tested here, from
//! outside the crate, using only the public API a caller would use.
//!
//! No container runtime is needed and none is used: this is a file in a temporary directory, so
//! the suite runs everywhere `cargo test --workspace` does, Windows included.
//!
//! | Claim | Test |
//! |---|---|
//! | A run outlives the process that wrote it, and resumes | [`trk_036_an_interrupted_run_resumes_from_the_file_after_the_writer_is_gone`] |
//! | The file backend records what the in-memory one records | [`trk_036_the_sqlite_backend_records_what_the_memory_backend_records`] |
//! | `cdm runs` works over a file-backed store | [`trk_036_run_management_works_over_a_file_backed_store`] |

// A failed assertion *is* the reporting mechanism in a test; the no-panic rule (ERR-004) exists
// to protect production paths, not test bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::Arc;

use cdm_core::{JobKind, RangeRecord, RunId, RunStatus, TableRef, TokenRange, TrackingStore};
use cdm_track::manage::RunManager;
use cdm_track::store::{MemoryStore, SqliteStore};
use cdm_track::tracker::{new_run_record, RunTracker, TrackerConfig};
use cdm_track::{adopt_previous_run, plan_resume, RerunPolicy};

/// The table every run in this file tracks against.
fn table() -> TableRef {
    TableRef::new("target_ks", "customers")
}

/// The four ranges a run in this file plans.
fn ranges() -> Vec<TokenRange> {
    vec![
        TokenRange::new(0, 24).unwrap(),
        TokenRange::new(25, 49).unwrap(),
        TokenRange::new(50, 74).unwrap(),
        TokenRange::new(75, 99).unwrap(),
    ]
}

/// An initialised store over a fresh file.
async fn open(path: &Path) -> Arc<SqliteStore> {
    let store = Arc::new(SqliteStore::open(path, &table()).unwrap());
    store.initialise().await.unwrap();
    store
}

/// Runs half a plan and then stops, the way an interrupted migration does (`ENG-010`).
///
/// The first two ranges complete; the third is claimed and never finishes, which is the case a
/// resume has to get right — `TRK-031` counts a `STARTED` range as pending, because the worker
/// that claimed it is gone and nothing else recorded what it managed.
async fn run_half_a_plan(store: Arc<dyn TrackingStore>, run_id: RunId) {
    let run = new_run_record(run_id, None, table(), JobKind::Migrate);
    let tracker = RunTracker::start(store, &run, &ranges(), TrackerConfig::default())
        .await
        .unwrap();
    for range in ranges().into_iter().take(2) {
        tracker.start_range(range);
        tracker.finish_range(range, RunStatus::Pass, "Read: 10; Write: 10".to_owned());
    }
    tracker.start_range(ranges()[2]);
    tracker
        .finish(
            RunStatus::Interrupted,
            "Read: 20; Write: 20; Partitions Failed: 0".to_owned(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn trk_036_an_interrupted_run_resumes_from_the_file_after_the_writer_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cdm-tracking.db");

    // The first process: it tracks a run into the file and is then interrupted.
    {
        let store = open(&path).await;
        run_half_a_plan(store, RunId::from_raw(1)).await;
    }

    // The second process: a different `SqliteStore` over the same file, which is all a resume has
    // to go on. `MemoryStore` cannot get this far, and a target that forbids DDL cannot either —
    // between them that is the whole case for this backend.
    let store = open(&path).await;
    let latest = store
        .latest_run(&table(), JobKind::Migrate)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(latest.run_id, RunId::from_raw(1));
    assert_eq!(latest.status, RunStatus::Interrupted);
    // TRK-030: an interrupted run is not an ended one, so `auto_rerun` adopts it.
    assert_eq!(adopt_previous_run(Some(&latest)), Some(RunId::from_raw(1)));

    let records = store.ranges(RunId::from_raw(1)).await.unwrap();
    let plan = plan_resume(
        RunId::from_raw(1),
        Some(&latest),
        &records,
        RerunPolicy::idempotent(),
        1,
        RunId::from_raw(2),
    )
    .unwrap();

    assert!(
        !plan.is_fallback(),
        "the previous run was found and started"
    );
    // TRK-031: the two `PASS` ranges are done; the claimed-but-unfinished one and the untouched
    // one are not.
    let mut planned = plan.ranges().to_vec();
    planned.sort_by_key(|range| TokenRange::min(*range));
    assert_eq!(planned, vec![ranges()[2], ranges()[3]]);
}

#[tokio::test]
async fn trk_036_the_sqlite_backend_records_what_the_memory_backend_records() {
    // `TRK-036`'s substance: the tracker does not know which store it is writing to, and the two
    // must not diverge on anything a resume reads. Timestamps are excluded — they are wall clocks
    // and will differ — but statuses, metrics strings and range identities must match exactly.
    let dir = tempfile::tempdir().unwrap();
    let sqlite = open(&dir.path().join("cdm-tracking.db")).await;
    let memory = Arc::new(MemoryStore::new());

    run_half_a_plan(sqlite.clone(), RunId::from_raw(7)).await;
    run_half_a_plan(memory.clone(), RunId::from_raw(7)).await;

    let from_sqlite = sqlite.run(RunId::from_raw(7)).await.unwrap().unwrap();
    let from_memory = memory.run(RunId::from_raw(7)).await.unwrap().unwrap();
    assert_eq!(from_sqlite.status, from_memory.status);
    assert_eq!(from_sqlite.info, from_memory.info);
    assert_eq!(from_sqlite.job, from_memory.job);
    assert_eq!(from_sqlite.table, from_memory.table);
    assert_eq!(from_sqlite.previous_run_id, from_memory.previous_run_id);

    let strip = |records: Vec<RangeRecord>| -> Vec<(TokenRange, RunStatus, Option<String>)> {
        let mut records: Vec<_> = records
            .into_iter()
            .map(|record| (record.range, record.status, record.info))
            .collect();
        records.sort_by_key(|(range, _, _)| TokenRange::min(*range));
        records
    };
    assert_eq!(
        strip(sqlite.ranges(RunId::from_raw(7)).await.unwrap()),
        strip(memory.ranges(RunId::from_raw(7)).await.unwrap())
    );
}

#[tokio::test]
async fn trk_036_run_management_works_over_a_file_backed_store() {
    // `cdm runs` (TRK-034) is written against `TrackingStore + RunCatalog`, so it has to work over
    // this backend too — an operator whose tracking is a local file still needs to ask what ran.
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("cdm-tracking.db")).await;
    run_half_a_plan(store.clone(), RunId::from_raw(1)).await;
    run_half_a_plan(store.clone(), RunId::from_raw(2)).await;

    let manager = RunManager::new(store.clone(), table());
    let listed = manager.list(None).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].run_id, 2, "newest first");
    assert_eq!(listed[0].run_type, "MIGRATE");
    assert!(listed[0].resumable, "an interrupted run can be resumed");

    let detail = manager
        .show(RunId::from_raw(1), RerunPolicy::idempotent())
        .await
        .unwrap();
    assert_eq!(detail.pending_ranges, 2);

    // Cancelling records ABORTED without losing the metrics the run reported.
    manager.cancel(RunId::from_raw(1)).await.unwrap();
    let cancelled = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
    assert_eq!(cancelled.status, RunStatus::Aborted);
    assert_eq!(
        cancelled.info.as_deref(),
        Some("Read: 20; Write: 20; Partitions Failed: 0")
    );
}
