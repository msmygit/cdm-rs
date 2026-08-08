//! Resume against a real node: interrupt a migration and finish it (`TST-041`, `DST-015`).
//!
//! # What only a cluster can prove
//!
//! `resume.rs` proves the *arithmetic* of a resume: for any interruption, the union of what
//! finished and what is re-planned covers every range. It works over a `MemoryStore` and a job
//! double, so it cannot say anything about the rows.
//!
//! `TST-041` asks for more than the arithmetic. It asks that the final target state after
//! `interrupt → resume` is the state a clean full run would have produced, and that no range is
//! processed twice *in a way that changes the result*. Both of those are facts about a storage
//! engine — the first about upserts carrying the origin's writetime, the second about counters,
//! which are the one thing in Cassandra that a repeated write does not leave alone.
//!
//! | Claim | Test |
//! |---|---|
//! | Interrupt, resume, and the target matches a clean run (`TST-041`) | [`tst_041_an_interrupted_migration_resumes_to_the_same_target_as_a_clean_run`] |
//! | A counter run killed mid-plan does not double-count on resume (`DST-015`, `CON-012`) | [`tst_041_a_counter_run_killed_mid_plan_does_not_double_count`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available. Run them
//! with `cargo test -p cdm-track --test resume_it -- --ignored --test-threads=1`, or via
//! `cargo xtask it`.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
// `large_futures` fires on the driver's own `SessionBuilder::build()`, reached through `connect`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures
)]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdm_codec::{CodecRegistry, Planner as CodecPlanner, PlannerOptions};
use cdm_config::model::CdmConfig;
use cdm_config::types::BatchGrouping;
use cdm_core::{CdmError, JobKind, RunId, RunStatus, Side, TableRef, TokenRange, TrackingStore};
use cdm_cql::connect::{connect, ClusterSession};
use cdm_cql::exec::{PreparedSetOptions, RunExecutor, TokenWidth};
use cdm_cql::raw::RawRow;
use cdm_cql::schema::introspect::fetch_table;
use cdm_cql::statement::{
    ColumnMapping, MappingOptions, MissingKeyPolicy, OriginProjection, OriginRangeSelect,
    OriginSelectByPk, StatementOptions, StatementSet, TargetSelectByPk, TargetUpsert,
};
use cdm_engine::jobs::migrate::{MigrateFeatures, MigrateJob, MigratePlan, MigrateSettings};
use cdm_engine::planner::{Partitioner, Planner, PlannerSettings};
use cdm_engine::scheduler::{
    RangeContext, RangeObserver, RangeProcessor, RangeVerdict, RunControl, Scheduler,
    SchedulerSettings, StopReason,
};
use cdm_testkit::{skip_without_container_runtime, ClusterFixture, Engine};
use cdm_track::store::MemoryStore;
use cdm_track::tracker::{committed_run_info, new_run_record, RunTracker, TrackerConfig};
use cdm_track::{plan_resume, RerunPolicy, ResumePlan};

/// The keyspace every case in this file uses.
const KEYSPACE: &str = "cdm_resume_it";
/// How many ranges a run plans. Small enough that the suite does not take minutes, large enough
/// that an interruption after three ranges leaves something to resume.
const NUM_PARTS: u64 = 8;

/// Cassandra 4.1 is the version the definition of done names; `CDM_IT_ENGINES` widens the matrix.
fn engine() -> Engine {
    cdm_testkit::engines_under_test()
        .expect("CDM_IT_ENGINES names an unknown engine")
        .into_iter()
        .next()
        .unwrap_or_else(|| Engine::cassandra("4.1"))
}

fn config_for(fixture: &ClusterFixture) -> CdmConfig {
    let (host, port) = fixture.contact_point().rsplit_once(':').map_or_else(
        || (fixture.contact_point().clone(), 9042),
        |(h, p)| (h.to_owned(), p.parse::<u16>().unwrap_or(9042)),
    );
    let mut config = CdmConfig::default();
    config.connect.origin.host.clone_from(&host);
    config.connect.origin.port = port;
    config.connect.target.host = host;
    config.connect.target.port = port;
    config
}

async fn sessions(fixture: &ClusterFixture) -> (ClusterSession, ClusterSession) {
    let config = config_for(fixture);
    let origin = connect(&config, Side::Origin).await.unwrap();
    let target = connect(&config, Side::Target).await.unwrap();
    (origin, target)
}

async fn ddl(session: &ClusterSession, cql: &str) {
    session
        .session()
        .query_unpaged(cql, &[])
        .await
        .unwrap_or_else(|e| panic!("{cql}: {e}"));
    session.session().await_schema_agreement().await.unwrap();
}

/// Everything `MigratePlan::resolve` needs, derived from the two live tables.
async fn plan_for(
    origin: &ClusterSession,
    target: &ClusterSession,
    origin_table: &str,
    target_table: &str,
    settings: MigrateSettings,
) -> MigratePlan {
    let origin_schema = fetch_table(
        Side::Origin,
        origin.session(),
        &TableRef::new(KEYSPACE, origin_table),
    )
    .await
    .unwrap()
    .expect("the origin table exists");
    let target_schema = fetch_table(
        Side::Target,
        target.session(),
        &TableRef::new(KEYSPACE, target_table),
    )
    .await
    .unwrap()
    .expect("the target table exists");

    let mapping =
        ColumnMapping::resolve(&origin_schema, &target_schema, &MappingOptions::default()).unwrap();
    let projection = OriginProjection::new(mapping.origin_columns(), &[]);
    let statements = StatementSet {
        origin_range_select: OriginRangeSelect::new(&origin_schema, &projection, None, false)
            .cql()
            .to_owned(),
        origin_select_by_pk: OriginSelectByPk::new(&origin_schema, &projection)
            .cql()
            .to_owned(),
        target_select_by_pk: TargetSelectByPk::new(&mapping).unwrap().cql().to_owned(),
        target_upsert: TargetUpsert::new(&mapping, StatementOptions::default())
            .unwrap()
            .cql()
            .to_owned(),
    };

    let executor = RunExecutor::prepare(
        origin,
        target,
        &statements,
        PreparedSetOptions {
            fetch_size: settings.fetch_size(),
            counter_target: target_schema.is_counter_table(),
            ..PreparedSetOptions::default()
        },
        settings.batch_size(),
        TokenWidth::Murmur3,
    )
    .await
    .unwrap();

    let codecs = CodecPlanner::new(
        CodecRegistry::with_builtins(&[], None).unwrap(),
        PlannerOptions::default(),
    );
    MigratePlan::resolve(
        executor,
        &mapping,
        &projection,
        &codecs,
        settings,
        MissingKeyPolicy::default(),
        false,
        MigrateFeatures::default(),
    )
    .unwrap()
}

/// The real migrate job, wrapped so the run can be stopped after a fixed number of ranges.
///
/// Interrupting a real migration reproducibly is the whole difficulty of this suite. A signal is
/// racy and a timer is worse; counting ranges and issuing an operator stop from inside the
/// `n`-th one is neither. `ENG-010` then does exactly what a `SIGINT` would: the range in flight
/// finishes, the ranges behind it are never claimed, and both facts reach the tracking table.
#[derive(Debug)]
struct StopAfter {
    inner: Arc<MigrateJob>,
    control: RunControl,
    after: usize,
    entered: AtomicUsize,
}

#[async_trait]
impl RangeProcessor for StopAfter {
    fn job(&self) -> JobKind {
        self.inner.job()
    }

    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
        let entered = self.entered.fetch_add(1, Ordering::SeqCst) + 1;
        let verdict = self.inner.process(ctx).await?;
        if entered == self.after {
            self.control.stop(StopReason::Operator);
        }
        Ok(verdict)
    }
}

/// One tracked run over the whole ring, stopped after `stop_after` ranges if given.
///
/// Returns the ranges that reached a successful terminal status.
async fn tracked_run(
    store: &Arc<MemoryStore>,
    run_id: RunId,
    table: TableRef,
    plan: MigratePlan,
    stop_after: Option<usize>,
) -> (BTreeSet<TokenRange>, RunStatus) {
    let token_plan =
        Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(NUM_PARTS))
            .plan(run_id, None)
            .unwrap();
    let tracker = Arc::new(
        RunTracker::start(
            Arc::clone(store) as Arc<dyn TrackingStore>,
            &new_run_record(run_id, None, table, JobKind::Migrate),
            &token_plan.token_ranges(),
            TrackerConfig::default(),
        )
        .await
        .unwrap(),
    );

    // One worker: "stop after the third range" has to name the same third range on every run, or
    // the suite is a coin toss dressed up as a test.
    let scheduler = Scheduler::new(SchedulerSettings::default().with_workers(1)).unwrap();
    let job = Arc::new(MigrateJob::new(Arc::new(plan)));
    let processor: Arc<dyn RangeProcessor> = match stop_after {
        Some(after) => Arc::new(StopAfter {
            inner: job,
            control: scheduler.control(),
            after,
            entered: AtomicUsize::new(0),
        }),
        None => job,
    };

    let report = scheduler
        .run(
            &token_plan,
            processor,
            Arc::clone(&tracker) as Arc<dyn RangeObserver>,
        )
        .await
        .unwrap();
    tracker
        .finish(report.status(), committed_run_info(report.counters()))
        .await
        .unwrap();

    let finished = report
        .outcomes()
        .iter()
        .filter(|outcome| outcome.is_success())
        .map(|outcome| outcome.range)
        .collect();
    (finished, report.status())
}

/// Runs a resume's work list, the way `cdm runs resume` does.
///
/// `TOK-011`: the scheduler is handed the outstanding ranges through
/// [`ResumePlan::token_plan`](cdm_track::ResumePlan::token_plan) rather than a fresh split of the
/// ring. That is the production path — this is not a test-only construction — and it is what makes
/// the assertions below claims about what `cdm runs resume` writes.
async fn run_resume_ranges(plan: Arc<MigratePlan>, resume: &ResumePlan, run_id: RunId) -> usize {
    let token_plan = resume.token_plan(run_id, Partitioner::Murmur3).unwrap();
    assert_eq!(token_plan.token_ranges(), resume.ranges());

    let report = Scheduler::new(SchedulerSettings::default().with_workers(1))
        .unwrap()
        .run(
            &token_plan,
            Arc::new(MigrateJob::new(plan)),
            Arc::new(cdm_engine::scheduler::NoopObserver),
        )
        .await
        .unwrap();
    assert_eq!(report.ranges_failed(), 0, "a resumed range must not fail");
    report.outcomes().len()
}

async fn read_kv(session: &ClusterSession, table: &str) -> Vec<(i32, String)> {
    let result = session
        .session()
        .query_unpaged(format!("SELECT id, data FROM {KEYSPACE}.{table}"), &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let mut rows: Vec<(i32, String)> = result
        .rows::<RawRow<'_, '_>>()
        .unwrap()
        .map(|row| {
            let row = row.unwrap();
            let id = i32::from_be_bytes(row.cell(0).unwrap().bytes.unwrap().try_into().unwrap());
            let data = String::from_utf8(row.cell(1).unwrap().bytes.unwrap().to_vec()).unwrap();
            (id, data)
        })
        .collect();
    rows.sort_unstable();
    rows
}

async fn read_counters(session: &ClusterSession, table: &str) -> Vec<(i32, i64)> {
    let result = session
        .session()
        .query_unpaged(format!("SELECT id, n FROM {KEYSPACE}.{table}"), &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let mut rows: Vec<(i32, i64)> = result
        .rows::<RawRow<'_, '_>>()
        .unwrap()
        .map(|row| {
            let row = row.unwrap();
            let id = i32::from_be_bytes(row.cell(0).unwrap().bytes.unwrap().try_into().unwrap());
            let n = i64::from_be_bytes(row.cell(1).unwrap().bytes.unwrap().try_into().unwrap());
            (id, n)
        })
        .collect();
    rows.sort_unstable();
    rows
}

/// Runs a body against a started fixture, skipping entirely without a container runtime.
macro_rules! against_a_cluster {
    ($name:ident, |$origin:ident, $target:ident| $body:block) => {
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires a container runtime; run with --ignored or via `cargo xtask it`"]
        async fn $name() {
            let _runtime = skip_without_container_runtime!();
            let engine = engine();
            let fixture = ClusterFixture::start(&engine)
                .await
                .unwrap_or_else(|e| panic!("starting {engine}: {e}"));
            let ($origin, $target) = sessions(&fixture).await;
            ddl(&$origin, &cdm_testkit::create_keyspace_statement(KEYSPACE)).await;
            $body
        }
    };
}

against_a_cluster!(
    tst_041_an_interrupted_migration_resumes_to_the_same_target_as_a_clean_run,
    |origin, target| {
        for table in ["src", "interrupted", "clean"] {
            ddl(
                &origin,
                &format!(
                    "CREATE TABLE IF NOT EXISTS {KEYSPACE}.{table} \
                     (id int PRIMARY KEY, data text)"
                ),
            )
            .await;
        }
        for id in 0..400i32 {
            origin
                .session()
                .query_unpaged(
                    format!("INSERT INTO {KEYSPACE}.src (id, data) VALUES (?, ?)"),
                    (id, format!("row-{id}")),
                )
                .await
                .unwrap();
        }

        let settings = MigrateSettings::new(1, 100, BatchGrouping::Strict, false, false, false);

        // The control: one uninterrupted run into its own target table.
        let plan = plan_for(&origin, &target, "src", "clean", settings).await;
        let store = Arc::new(MemoryStore::new());
        let (finished, status) = tracked_run(
            &store,
            RunId::from_raw(1),
            TableRef::new(KEYSPACE, "clean"),
            plan,
            None,
        )
        .await;
        assert_eq!(status, RunStatus::Ended);
        assert_eq!(finished.len(), usize::try_from(NUM_PARTS).unwrap());
        let expected = read_kv(&target, "clean").await;
        assert_eq!(expected.len(), 400, "the control run migrated everything");

        // The experiment: the same migration, stopped after three ranges.
        let plan = plan_for(&origin, &target, "src", "interrupted", settings).await;
        let store = Arc::new(MemoryStore::new());
        let (finished, status) = tracked_run(
            &store,
            RunId::from_raw(2),
            TableRef::new(KEYSPACE, "interrupted"),
            plan,
            Some(3),
        )
        .await;
        assert_eq!(status, RunStatus::Aborted, "an operator stop is an abort");
        assert_eq!(finished.len(), 3);
        let partial = read_kv(&target, "interrupted").await;
        assert!(
            partial.len() < 400,
            "the interruption has to have left work behind, or this proves nothing"
        );

        // The resume, planned from the tracking rows the interrupted run left.
        let previous = store.run(RunId::from_raw(2)).await.unwrap().unwrap();
        let records = store.ranges(RunId::from_raw(2)).await.unwrap();
        let resume = plan_resume(
            RunId::from_raw(2),
            Some(&previous),
            &records,
            RerunPolicy::idempotent(),
            1,
            RunId::from_raw(3),
        )
        .unwrap();
        assert!(!resume.is_fallback());
        assert_eq!(
            resume.ranges().len() + finished.len(),
            usize::try_from(NUM_PARTS).unwrap(),
            "the resume re-plans exactly what did not finish",
        );

        let resumed_plan =
            Arc::new(plan_for(&origin, &target, "src", "interrupted", settings).await);
        let processed = run_resume_ranges(resumed_plan, &resume, RunId::from_raw(3)).await;
        assert_eq!(processed, resume.ranges().len());

        // TST-041, in the specification's own words: the final target state equals a clean run's.
        assert_eq!(
            read_kv(&target, "interrupted").await,
            expected,
            "interrupt + resume must land where an uninterrupted run lands",
        );
    }
);

against_a_cluster!(
    tst_041_a_counter_run_killed_mid_plan_does_not_double_count,
    |origin, target| {
        ddl(
            &origin,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.hits_src \
                 (id int PRIMARY KEY, n counter)"
            ),
        )
        .await;
        ddl(
            &target,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.hits_dst \
                 (id int PRIMARY KEY, n counter)"
            ),
        )
        .await;
        for id in 0..120i32 {
            origin
                .session()
                .query_unpaged(
                    format!(
                        "UPDATE {KEYSPACE}.hits_src SET n = n + {} WHERE id = {id}",
                        i64::from(id) + 1
                    ),
                    &[],
                )
                .await
                .unwrap();
        }
        let expected = read_counters(&origin, "hits_src").await;
        assert_eq!(expected.len(), 120);

        let settings = MigrateSettings::new(50, 100, BatchGrouping::Strict, true, false, false);
        assert_eq!(
            settings.batch_size(),
            1,
            "MIG-021: a counter run never batches"
        );

        // Kill the run after three ranges. Some counters are now migrated and the rest are not.
        let plan = plan_for(&origin, &target, "hits_src", "hits_dst", settings).await;
        let store = Arc::new(MemoryStore::new());
        let (finished, status) = tracked_run(
            &store,
            RunId::from_raw(1),
            TableRef::new(KEYSPACE, "hits_dst"),
            plan,
            Some(3),
        )
        .await;
        assert_eq!(status, RunStatus::Aborted);
        assert_eq!(finished.len(), 3);

        // DST-015: the resume of a *writing* counter job re-plans only ranges that demonstrably
        // never started. Everything else is reported for reconciliation rather than replayed,
        // because a counter range that half-applied cannot be replayed without double-counting
        // and nothing afterwards could tell that it had been.
        let previous = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        let records = store.ranges(RunId::from_raw(1)).await.unwrap();
        let policy = RerunPolicy::for_job(JobKind::Migrate, true, false);
        let resume = plan_resume(
            RunId::from_raw(1),
            Some(&previous),
            &records,
            policy,
            1,
            RunId::from_raw(2),
        )
        .unwrap();
        for range in resume.ranges() {
            assert!(
                !finished.contains(range),
                "{range} completed and must not be replayed on a counter table"
            );
        }

        let resumed_plan =
            Arc::new(plan_for(&origin, &target, "hits_src", "hits_dst", settings).await);
        let processed = run_resume_ranges(resumed_plan, &resume, RunId::from_raw(2)).await;
        assert_eq!(processed, resume.ranges().len());

        // The assertion the whole case exists for. A resume that replayed a completed counter
        // range would have doubled it, and a counter carries no writetime and no version, so
        // nothing but this comparison could ever reveal it.
        assert_eq!(
            read_counters(&target, "hits_dst").await,
            expected,
            "interrupt + resume must apply each counter delta exactly once (DST-015, CON-012)",
        );
    }
);

/// The suite's own sanity check: without a container runtime everything above skips, and this is
/// the one case that must still run so `cargo test --workspace` proves the file compiles.
#[test]
fn tst_102_the_suite_declares_the_engine_it_targets() {
    assert!(!engine().to_string().is_empty());
    // A resume needs more than one range to have anything to resume.
    const { assert!(NUM_PARTS > 1) };
}
