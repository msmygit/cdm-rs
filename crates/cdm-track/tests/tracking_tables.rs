//! The tracking tables, exercised against a real Cassandra (`TRK-010`, `TRK-020`..`TRK-022`,
//! `TRK-030`..`TRK-033`, `DST-015`).
//!
//! Everything in `src/` is tested against `MemoryStore`, which proves the *logic*. It cannot
//! prove that the DDL Java wrote is DDL Cassandra accepts, that `ORDER BY run_id DESC LIMIT 1
//! ALLOW FILTERING` returns the newest run, or that `totimestamp(now())` lands in the column the
//! resume reads. Those are facts about a node, and this file asks a node.
//!
//! | Claim | Test |
//! |---|---|
//! | Java's DDL applies, and applies twice (`TRK-010`) | [`trk_010_the_java_ddl_applies_to_a_real_cluster`] |
//! | A run's rows appear in Java's schema, in Java's order (`TRK-020`..`TRK-022`) | [`trk_020_a_run_lifecycle_lands_in_the_tracking_tables`] |
//! | The newest unfinished run is the one adopted (`TRK-030`) | [`trk_030_auto_rerun_adopts_the_newest_unfinished_run`] |
//! | A resume re-plans exactly the unfinished ranges (`TRK-031`, `TRK-033`) | [`trk_031_a_resume_replans_only_the_unfinished_ranges`] |
//! | A counter table's partially-applied ranges are quarantined (`DST-015`) | [`dst_015_a_counter_resume_quarantines_partially_applied_ranges`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available, so
//! `cargo test --workspace` stays green on a laptop without Docker. Run them with
//! `cargo test -p cdm-track --test tracking_tables -- --ignored --test-threads=1`, or via
//! `cargo xtask it`.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
// `large_futures` fires on the driver's own `SessionBuilder::build()`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures
)]

use std::sync::Arc;
use std::time::Duration;

use cdm_core::{JobKind, RangeRecord, RunId, RunStatus, TableRef, TokenRange, TrackingStore};
use cdm_testkit::{skip_without_container_runtime, ClusterFixture, Engine};
use cdm_track::resume::{plan_resume, QuarantineReason, RerunPolicy};
use cdm_track::store::CassandraStore;
use cdm_track::tracker::{new_run_record, RunTracker, TrackerConfig};
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;

/// The keyspace the tracking tables are created in — the *target* keyspace (`TRK-010`).
const KEYSPACE: &str = "cdm_track_it";

/// Cassandra 4.1 is the version the definition of done names. `CDM_IT_ENGINES` overrides it for a
/// wider matrix, exactly as the other integration suites in this workspace do.
fn engine() -> Engine {
    cdm_testkit::engines_under_test()
        .expect("CDM_IT_ENGINES names an unknown engine")
        .into_iter()
        .next()
        .unwrap_or_else(|| Engine::cassandra("4.1"))
}

/// Connects to a started fixture and creates the keyspace the tracking tables go in.
async fn connect(fixture: &ClusterFixture) -> Arc<Session> {
    let session = SessionBuilder::new()
        .known_node(fixture.contact_point())
        .connection_timeout(Duration::from_secs(10))
        .build()
        .await
        .unwrap_or_else(|e| panic!("connecting to {}: {e}", fixture.contact_point()));
    session
        .query_unpaged(cdm_testkit::create_keyspace_statement(KEYSPACE), &[])
        .await
        .unwrap();
    session.await_schema_agreement().await.unwrap();
    Arc::new(session)
}

fn table(name: &str) -> TableRef {
    TableRef::new(KEYSPACE, name)
}

fn range(min: i128, max: i128) -> TokenRange {
    TokenRange::new(min, max).unwrap()
}

fn detail(range: TokenRange, status: RunStatus) -> RangeRecord {
    RangeRecord {
        range,
        status,
        started_at: None,
        info: None,
    }
}

/// Runs a body against a started fixture, skipping entirely without a container runtime.
macro_rules! against_a_cluster {
    ($name:ident, |$session:ident| $body:block) => {
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires a container runtime; run with --ignored or via `cargo xtask it`"]
        async fn $name() {
            let _runtime = skip_without_container_runtime!();
            let engine = engine();
            let fixture = ClusterFixture::start(&engine)
                .await
                .unwrap_or_else(|e| panic!("starting {engine}: {e}"));
            let $session = connect(&fixture).await;
            $body
        }
    };
}

against_a_cluster!(trk_010_the_java_ddl_applies_to_a_real_cluster, |session| {
    let store = CassandraStore::new(Arc::clone(&session), &table("ddl")).unwrap();
    store.initialise().await.unwrap();
    // `IF NOT EXISTS` throughout: a second node — or a Java run that got there first — must not
    // turn initialisation into an error (`DST-002`).
    store.initialise().await.unwrap();

    // The columns and the primary key are Java's, as read back from the node's own metadata.
    let columns = session
        .query_unpaged(
            "SELECT column_name, kind FROM system_schema.columns \
             WHERE keyspace_name = ? AND table_name = ?",
            (KEYSPACE, "cdm_run_details"),
        )
        .await
        .unwrap()
        .into_rows_result()
        .unwrap()
        .rows::<(String, String)>()
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    let partition: Vec<&str> = columns
        .iter()
        .filter(|(_, kind)| kind == "partition_key")
        .map(|(name, _)| name.as_str())
        .collect();
    let clustering: Vec<&str> = columns
        .iter()
        .filter(|(_, kind)| kind == "clustering")
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(partition.contains(&"table_name") && partition.contains(&"run_id"));
    assert_eq!(clustering, vec!["token_min"]);

    // The lease table is not created for a single-node run, so the keyspace is byte-compatible
    // with what Java would have left behind (`TRK-011`).
    let leases = session
        .query_unpaged(
            "SELECT table_name FROM system_schema.tables \
             WHERE keyspace_name = ? AND table_name = ?",
            (KEYSPACE, "cdm_run_leases"),
        )
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    assert_eq!(leases.rows_num(), 0, "cdm_run_leases must be opt-in");

    // And it does appear when distributed coordination asks for it.
    CassandraStore::new(Arc::clone(&session), &table("ddl"))
        .unwrap()
        .with_leases(true)
        .initialise()
        .await
        .unwrap();
});

against_a_cluster!(
    trk_020_a_run_lifecycle_lands_in_the_tracking_tables,
    |session| {
        let table = table("lifecycle");
        let store: Arc<dyn TrackingStore> =
            Arc::new(CassandraStore::new(Arc::clone(&session), &table).unwrap());
        let ranges = [range(0, 99), range(100, 199), range(200, 299)];
        let run = new_run_record(
            RunId::from_raw(1_001),
            None,
            table.clone(),
            JobKind::Migrate,
        );

        let tracker =
            RunTracker::start(Arc::clone(&store), &run, &ranges, TrackerConfig::default())
                .await
                .unwrap();

        // TRK-020: the run row is STARTED and every range row exists as NOT_STARTED.
        let recorded = store.run(RunId::from_raw(1_001)).await.unwrap().unwrap();
        assert_eq!(recorded.status, RunStatus::Started);
        assert_eq!(recorded.job, JobKind::Migrate);
        assert!(recorded.started_at.is_some(), "totimestamp(now()) landed");
        let rows = store.ranges(RunId::from_raw(1_001)).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.status == RunStatus::NotStarted));

        // TRK-013: the value in `run_type` is the upper-case Java spelling, which is what makes a
        // Java run and a cdm-rs run find each other.
        let (run_type,): (String,) = session
            .query_unpaged(
                format!(
                    "SELECT run_type FROM \"{KEYSPACE}\".cdm_run_info \
                     WHERE table_name = ? AND run_id = ?"
                ),
                ("lifecycle", 1_001_i64),
            )
            .await
            .unwrap()
            .into_rows_result()
            .unwrap()
            .single_row::<(String,)>()
            .unwrap();
        assert_eq!(run_type, "MIGRATE");

        // TRK-021.
        tracker.start_range(ranges[0]);
        tracker.finish_range(ranges[0], RunStatus::Pass, "Read: 10; Write: 10".to_owned());
        tracker.finish_range(ranges[1], RunStatus::Fail, "Read: 4; Error: 4".to_owned());
        // TRK-022.
        tracker
            .finish(
                RunStatus::Ended,
                "Read: 14; Write: 10; Partitions Failed: 1".to_owned(),
            )
            .await
            .unwrap();

        let rows = store.ranges(RunId::from_raw(1_001)).await.unwrap();
        let by_min: std::collections::BTreeMap<i128, &RangeRecord> =
            rows.iter().map(|r| (r.range.min(), r)).collect();
        assert_eq!(by_min[&0].status, RunStatus::Pass);
        assert_eq!(by_min[&0].info.as_deref(), Some("Read: 10; Write: 10"));
        assert!(
            by_min[&0].started_at.is_some(),
            "the STARTED write set start_time before the terminal write cleared it"
        );
        assert_eq!(by_min[&100].status, RunStatus::Fail);
        assert_eq!(by_min[&200].status, RunStatus::NotStarted);

        let ended = store.run(RunId::from_raw(1_001)).await.unwrap().unwrap();
        assert_eq!(ended.status, RunStatus::Ended);
        assert!(ended.ended_at.is_some());
        assert_eq!(
            ended.info.as_deref(),
            Some("Read: 14; Write: 10; Partitions Failed: 1")
        );

        // TRK-020: the same run id a second time is refused rather than silently resetting every
        // range row to NOT_STARTED.
        let err = store
            .create_run(&run, &[detail(ranges[0], RunStatus::NotStarted)])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }
);

against_a_cluster!(
    trk_030_auto_rerun_adopts_the_newest_unfinished_run,
    |session| {
        let table = table("adopt");
        let store = CassandraStore::new(Arc::clone(&session), &table).unwrap();
        store.initialise().await.unwrap();

        // An older run that ended cleanly, and a newer one that did not.
        for (id, status, info) in [
            (2_001_i64, RunStatus::Ended, "Partitions Failed: 0"),
            (2_002, RunStatus::Started, "Read: 1"),
        ] {
            let run = new_run_record(RunId::from_raw(id), None, table.clone(), JobKind::Migrate);
            store
                .create_run(&run, &[detail(range(0, 99), RunStatus::NotStarted)])
                .await
                .unwrap();
            store
                .update_run(RunId::from_raw(id), status, Some(info))
                .await
                .unwrap();
        }
        // A validate run with a higher id, which must not be adopted by a migrate resume.
        let other = new_run_record(
            RunId::from_raw(2_003),
            None,
            table.clone(),
            JobKind::Validate,
        );
        store.create_run(&other, &[]).await.unwrap();

        let latest = store
            .latest_run(&table, JobKind::Migrate)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            latest.run_id,
            RunId::from_raw(2_002),
            "ORDER BY run_id DESC must pick the newest run of this run_type"
        );
        assert_eq!(
            cdm_track::adopt_previous_run(Some(&latest)),
            Some(RunId::from_raw(2_002))
        );

        // A clean run is not adopted, which is the whole of TRK-030's second half.
        let clean = store.run(RunId::from_raw(2_001)).await.unwrap().unwrap();
        assert_eq!(cdm_track::adopt_previous_run(Some(&clean)), None);
    }
);

against_a_cluster!(
    trk_031_a_resume_replans_only_the_unfinished_ranges,
    |session| {
        let table = table("resume");
        let store = CassandraStore::new(Arc::clone(&session), &table).unwrap();
        store.initialise().await.unwrap();

        let previous = RunId::from_raw(3_001);
        let run = new_run_record(previous, None, table.clone(), JobKind::Migrate);
        let ranges = [
            range(0, 99),
            range(100, 199),
            range(200, 299),
            range(300, 399),
        ];
        store
            .create_run(
                &run,
                &ranges
                    .iter()
                    .map(|r| detail(*r, RunStatus::NotStarted))
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
        store
            .update_run(previous, RunStatus::Started, None)
            .await
            .unwrap();

        // One passed, one failed, one is still STARTED, one never began.
        for (range, status) in [
            (ranges[0], RunStatus::Pass),
            (ranges[1], RunStatus::Fail),
            (ranges[2], RunStatus::Started),
        ] {
            store
                .update_range(
                    previous,
                    &RangeRecord {
                        info: Some("Read: 1".to_owned()),
                        ..detail(range, status)
                    },
                )
                .await
                .unwrap();
        }

        let records = store.ranges(previous).await.unwrap();
        let plan = plan_resume(
            previous,
            store.run(previous).await.unwrap().as_ref(),
            &records,
            RerunPolicy::idempotent(),
            1,
            RunId::from_raw(3_002),
        )
        .unwrap();
        assert!(!plan.is_fallback());
        let mut replanned = plan.ranges().to_vec();
        replanned.sort_unstable();
        assert_eq!(
            replanned,
            vec![ranges[1], ranges[2], ranges[3]],
            "PASS is the only status a resume leaves alone"
        );

        // TRK-033: the same pending set, subdivided four ways at full coverage.
        let subdivided = plan_resume(
            previous,
            store.run(previous).await.unwrap().as_ref(),
            &records,
            RerunPolicy::idempotent(),
            4,
            RunId::from_raw(3_002),
        )
        .unwrap();
        assert_eq!(subdivided.ranges().len(), 12);
        let mut covered = subdivided.ranges().to_vec();
        covered.sort_unstable();
        assert_eq!(
            covered[0].min(),
            100,
            "coverage starts at the first pending range"
        );
        assert_eq!(covered[11].max(), 399, "and ends at the last");
    }
);

against_a_cluster!(
    dst_015_a_counter_resume_quarantines_partially_applied_ranges,
    |session| {
        let table = table("counters");
        let store = CassandraStore::new(Arc::clone(&session), &table).unwrap();
        store.initialise().await.unwrap();

        let previous = RunId::from_raw(4_001);
        let run = new_run_record(previous, None, table.clone(), JobKind::Migrate);
        let ranges = [range(0, 99), range(100, 199), range(200, 299)];
        store
            .create_run(
                &run,
                &ranges
                    .iter()
                    .map(|r| detail(*r, RunStatus::NotStarted))
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
        store
            .update_run(previous, RunStatus::Started, None)
            .await
            .unwrap();
        store
            .update_range(previous, &detail(ranges[0], RunStatus::Started))
            .await
            .unwrap();
        store
            .update_range(
                previous,
                &RangeRecord {
                    info: Some("Read: 3; Write: 2".to_owned()),
                    ..detail(ranges[1], RunStatus::Fail)
                },
            )
            .await
            .unwrap();

        let records = store.ranges(previous).await.unwrap();
        let plan = plan_resume(
            previous,
            store.run(previous).await.unwrap().as_ref(),
            &records,
            RerunPolicy::for_job(JobKind::Migrate, true, false),
            8,
            RunId::from_raw(4_002),
        )
        .unwrap();

        // Only the range that demonstrably never started is re-planned, subdivided eight ways.
        assert_eq!(plan.ranges().len(), 8);
        for produced in plan.ranges().iter().copied() {
            assert!(
                produced.min() >= 200,
                "{produced} came from a counter range that may have partially applied"
            );
        }
        let quarantined: Vec<TokenRange> = plan.quarantined().iter().map(|q| q.range).collect();
        assert_eq!(quarantined, vec![ranges[0], ranges[1]]);
        assert!(plan
            .quarantined()
            .iter()
            .all(|q| q.reason == QuarantineReason::CounterPartiallyApplied));
    }
);

against_a_cluster!(
    trk_037_a_status_only_write_leaves_the_recorded_metrics_alone,
    |session| {
        // The regression this exists for: `TRK-022`'s statement is `UPDATE ... SET run_info = ?`,
        // and `update_run` used to bind the `Option<&str>` straight through. `RunManager::cancel`
        // passes `None` because it has no *new* metrics — and a bound `NULL` is a tombstone, so
        // cancelling a run silently erased the only record of how far it had got. A cancelled run
        // is precisely the one an operator opens to find that out.
        let table = table("unset");
        let store = CassandraStore::new(Arc::clone(&session), &table).unwrap();
        store.initialise().await.unwrap();

        let run_id = RunId::from_raw(5_001);
        let run = new_run_record(run_id, None, table.clone(), JobKind::Migrate);
        store
            .create_run(&run, &[detail(range(0, 99), RunStatus::NotStarted)])
            .await
            .unwrap();

        // A run that got far enough to have something to say.
        let metrics = "Read: 1000; Write: 940; Error: 60";
        store
            .update_run(run_id, RunStatus::Started, Some(metrics))
            .await
            .unwrap();
        assert_eq!(
            store.run(run_id).await.unwrap().unwrap().info.as_deref(),
            Some(metrics)
        );

        // Now the status-only write that `cdm runs cancel` issues.
        store
            .update_run(run_id, RunStatus::Aborted, None)
            .await
            .unwrap();

        let after = store.run(run_id).await.unwrap().unwrap();
        assert_eq!(after.status, RunStatus::Aborted, "the status must be taken");
        assert_eq!(
            after.info.as_deref(),
            Some(metrics),
            "TRK-037: a write with no metrics must not erase the recorded ones"
        );
    }
);
