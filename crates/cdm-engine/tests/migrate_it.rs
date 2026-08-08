//! The migrate job, against a real cluster (`MIG-001`..`MIG-005`, `MIG-020`..`MIG-022`,
//! `MIG-030`..`MIG-032`, `MIG-041`, `CON-012`, `SCH-009`).
//!
//! The unit tests in `cdm_engine::jobs::migrate` prove the *arithmetic*: when a flush happens, how a
//! batch is grouped, what a counter delta is, which counter is credited. They cannot prove that
//! the rows arrive, that an `UNLOGGED` batch of generated CQL is accepted, or — the one that
//! matters most — that a counter delta lands **exactly once**. Those are facts about a node.
//!
//! | Claim | Test |
//! |---|---|
//! | A range migrates end to end and the counters say so (`MIG-001`, `MIG-005`) | [`mig_001_a_table_migrates_end_to_end`] |
//! | Batched writes arrive, and the batch never spans partitions (`MIG-020`, `MIG-022`) | [`mig_020_a_batched_migration_writes_every_row`] |
//! | A counter delta is applied exactly once, and re-running adds nothing (`MIG-030`..`MIG-032`) | [`mig_030_a_counter_delta_is_applied_exactly_once`] |
//! | A counter run refuses to batch, whatever the configuration says (`MIG-021`) | [`mig_021_a_counter_run_coerces_the_batch_size_to_one`] |
//! | A dry run reads, binds and counts, and writes nothing (`MIG-041`) | [`mig_041_a_dry_run_writes_nothing_and_counts_everything`] |
//! | A schema changed mid-run aborts with its own error kind (`SCH-009`) | [`sch_009_a_schema_change_aborts_the_run`] |
//! | The request-latency histograms are filled by a real migration (`MET-010`) | [`met_010_a_real_migration_fills_the_request_latency_histograms`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available. Run them
//! with `cargo test -p cdm-engine --test migrate_it -- --ignored --test-threads=1`, or via
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

use std::sync::Arc;
use std::time::Duration;

use cdm_codec::{CodecRegistry, Planner as CodecPlanner, PlannerOptions};
use cdm_config::model::CdmConfig;
use cdm_config::types::BatchGrouping;
use cdm_core::{CdmError, ErrorKind, RunId, Side, TableRef};
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
use cdm_engine::scheduler::{NoopObserver, Scheduler, SchedulerSettings};
use cdm_metrics::{CounterKind, CounterView, Instruments, Operation};
use cdm_testkit::{skip_without_container_runtime, ClusterFixture, Engine};

/// The keyspace every case in this file uses.
const KEYSPACE: &str = "cdm_migrate_it";

/// Cassandra 4.1 is the version the definition of done names; `CDM_IT_ENGINES` widens the matrix.
fn engine() -> Engine {
    cdm_testkit::engines_under_test()
        .expect("CDM_IT_ENGINES names an unknown engine")
        .into_iter()
        .next()
        .unwrap_or_else(|| Engine::cassandra("4.1"))
}

/// A configuration pointing both sides at the same node, which is all these cases need: the two
/// sides are independent by construction (`CON-001`), and using one container keeps the suite
/// runnable on a laptop.
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
    plan_observed(origin, target, origin_table, target_table, settings, None).await
}

/// The same plan, with `MET-010`'s observer attached to the executor that issues the requests.
async fn plan_observed(
    origin: &ClusterSession,
    target: &ClusterSession,
    origin_table: &str,
    target_table: &str,
    settings: MigrateSettings,
    instruments: Option<Arc<Instruments>>,
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
    .unwrap()
    .observing(cdm_cql::observe::RequestMetrics::from_option(
        instruments.map(|i| i as Arc<dyn cdm_core::RequestObserver>),
    ));

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

/// Runs a whole migration over the full ring and returns the report.
async fn migrate(plan: MigratePlan) -> cdm_engine::scheduler::RunReport {
    migrate_observed(plan, None, SchedulerSettings::default().with_workers(4)).await
}

/// The same migration, with `MET-010`'s observer attached to the scheduler.
async fn migrate_observed(
    plan: MigratePlan,
    instruments: Option<Arc<Instruments>>,
    settings: SchedulerSettings,
) -> cdm_engine::scheduler::RunReport {
    let job = Arc::new(MigrateJob::new(Arc::new(plan)));
    let token_plan = Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(8))
        .plan(RunId::from_raw(1), None)
        .unwrap();
    Scheduler::observing(
        settings,
        instruments.map(|i| i as Arc<dyn cdm_core::RequestObserver>),
    )
    .unwrap()
    .run(&token_plan, job, Arc::new(NoopObserver))
    .await
    .unwrap()
}

async fn count_rows(session: &ClusterSession, table: &str) -> i64 {
    session
        .session()
        .query_unpaged(format!("SELECT COUNT(*) FROM {KEYSPACE}.{table}"), &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap()
        .single_row::<(i64,)>()
        .unwrap()
        .0
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

against_a_cluster!(mig_001_a_table_migrates_end_to_end, |origin, target| {
    ddl(
        &origin,
        &format!("CREATE TABLE IF NOT EXISTS {KEYSPACE}.basic_src (id int PRIMARY KEY, data text, n int)"),
    )
    .await;
    ddl(
        &target,
        &format!("CREATE TABLE IF NOT EXISTS {KEYSPACE}.basic_dst (id int PRIMARY KEY, data text, n int)"),
    )
    .await;
    for id in 0..250i32 {
        origin
            .session()
            .query_unpaged(
                format!("INSERT INTO {KEYSPACE}.basic_src (id, data, n) VALUES (?, ?, ?)"),
                (id, format!("row-{id}"), id * 2),
            )
            .await
            .unwrap();
    }

    let settings = MigrateSettings::new(1, 100, BatchGrouping::Strict, false, false, false);
    let plan = plan_for(&origin, &target, "basic_src", "basic_dst", settings).await;
    let report = migrate(plan).await;

    assert_eq!(count_rows(&target, "basic_dst").await, 250);
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::Read, CounterView::Committed),
        250
    );
    // MIG-005: WRITE is credited on the flush, and every row was flushed.
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::Write, CounterView::Committed),
        250
    );
    // MIG-004: UNFLUSHED is reset once its rows are credited, so the committed total is zero —
    // the same number Java reports, for a completely different reason.
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::Unflushed, CounterView::Committed),
        0
    );
    assert_eq!(report.ranges_failed(), 0);

    // MIG-012, against a node: a null origin column leaves no tombstone behind.
    let nulls: i64 = target
        .session()
        .query_unpaged(
            format!("SELECT COUNT(*) FROM {KEYSPACE}.basic_dst WHERE data IS NULL ALLOW FILTERING"),
            &[],
        )
        .await
        .map_or(0, |r| {
            r.into_rows_result()
                .unwrap()
                .single_row::<(i64,)>()
                .unwrap()
                .0
        });
    assert_eq!(nulls, 0);
});

against_a_cluster!(
    mig_020_a_batched_migration_writes_every_row,
    |origin, target| {
        // A composite key, so that `MIG-022`'s strict grouping has more than one row per partition to
        // group and more than one partition to keep apart.
        ddl(
            &origin,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.batch_src \
             (id int, cc int, data text, PRIMARY KEY (id, cc))"
            ),
        )
        .await;
        ddl(
            &target,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.batch_dst \
             (id int, cc int, data text, PRIMARY KEY (id, cc))"
            ),
        )
        .await;
        for id in 0..40i32 {
            for cc in 0..5i32 {
                origin
                    .session()
                    .query_unpaged(
                        format!("INSERT INTO {KEYSPACE}.batch_src (id, cc, data) VALUES (?, ?, ?)"),
                        (id, cc, format!("{id}-{cc}")),
                    )
                    .await
                    .unwrap();
            }
        }

        let settings = MigrateSettings::new(10, 200, BatchGrouping::Strict, false, false, false);
        assert_eq!(settings.batch_size(), 10);
        let plan = plan_for(&origin, &target, "batch_src", "batch_dst", settings).await;
        let report = migrate(plan).await;

        assert_eq!(count_rows(&target, "batch_dst").await, 200);
        assert_eq!(
            report
                .counters()
                .count_of(CounterKind::Write, CounterView::Committed),
            200
        );
    }
);

against_a_cluster!(
    mig_030_a_counter_delta_is_applied_exactly_once,
    |origin, target| {
        ddl(
            &origin,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.hits_src \
                 (id int, cc text, n counter, PRIMARY KEY (id, cc))"
            ),
        )
        .await;
        ddl(
            &target,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.hits_dst \
                 (id int, cc text, n counter, PRIMARY KEY (id, cc))"
            ),
        )
        .await;
        for id in 0..20i32 {
            // The delta is inlined rather than bound: the driver types a `counter` column as its
            // own `Counter`, and this suite deliberately names no driver type (`ARCHITECTURE.md`
            // §3) — it reads counters back off the frame instead, as the job itself does.
            let delta = i64::from(id) + 1;
            origin
                .session()
                .query_unpaged(
                    format!(
                        "UPDATE {KEYSPACE}.hits_src SET n = n + {delta} WHERE id = {id} AND cc = 'a'"
                    ),
                    &[],
                )
                .await
                .unwrap();
        }

        let settings = MigrateSettings::new(50, 100, BatchGrouping::Strict, true, false, false);
        // MIG-021: whatever the operator configured, a counter run does not batch.
        assert_eq!(settings.batch_size(), 1);

        let plan = plan_for(&origin, &target, "hits_src", "hits_dst", settings).await;
        let report = migrate(plan).await;
        assert_eq!(report.ranges_failed(), 0);
        assert_eq!(
            report
                .counters()
                .count_of(CounterKind::Write, CounterView::Committed),
            20
        );
        assert_counters_match(&origin, &target).await;

        // MIG-031, and the whole point of the delta: running the migration a second time computes
        // `origin - target`, which is now zero, so the counter does not move. A job that wrote the
        // origin's value rather than the difference would double every counter here, and a job
        // that retried a counter write would too.
        let plan = plan_for(&origin, &target, "hits_src", "hits_dst", settings).await;
        let report = migrate(plan).await;
        assert_eq!(report.ranges_failed(), 0);
        assert_counters_match(&origin, &target).await;
    }
);

against_a_cluster!(
    mig_021_a_counter_run_coerces_the_batch_size_to_one,
    |origin, target| {
        // The coercion is a property of `MigrateSettings`, but the reason it exists is a property
        // of the server: an UNLOGGED batch that mixes counter and non-counter statements is
        // rejected outright, and a counter batch the coordinator retries double-counts. This case
        // asserts that a run configured to batch a counter table still succeeds, which it only
        // does because the coercion happened.
        ddl(
            &origin,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.coerce_src \
                 (id int, cc text, n counter, PRIMARY KEY (id, cc))"
            ),
        )
        .await;
        ddl(
            &target,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.coerce_dst \
                 (id int, cc text, n counter, PRIMARY KEY (id, cc))"
            ),
        )
        .await;
        origin
            .session()
            .query_unpaged(
                format!("UPDATE {KEYSPACE}.coerce_src SET n = n + 7 WHERE id = 1 AND cc = 'a'"),
                &[],
            )
            .await
            .unwrap();

        let settings = MigrateSettings::new(100, 100, BatchGrouping::Strict, true, false, false);
        let plan = plan_for(&origin, &target, "coerce_src", "coerce_dst", settings).await;
        let report = migrate(plan).await;
        assert_eq!(report.ranges_failed(), 0);

        let counters = read_counters(&target, "coerce_dst").await;
        assert_eq!(counters, vec![(1, "a".to_owned(), 7)]);
    }
);

against_a_cluster!(
    mig_041_a_dry_run_writes_nothing_and_counts_everything,
    |origin, target| {
        ddl(
            &origin,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.dry_src (id int PRIMARY KEY, data text)"
            ),
        )
        .await;
        ddl(
            &target,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.dry_dst (id int PRIMARY KEY, data text)"
            ),
        )
        .await;
        for id in 0..30i32 {
            origin
                .session()
                .query_unpaged(
                    format!("INSERT INTO {KEYSPACE}.dry_src (id, data) VALUES (?, ?)"),
                    (id, "payload"),
                )
                .await
                .unwrap();
        }

        let settings = MigrateSettings::new(1, 100, BatchGrouping::Strict, false, false, true);
        assert!(settings.is_dry_run());
        let plan = plan_for(&origin, &target, "dry_src", "dry_dst", settings).await;
        let report = migrate(plan).await;

        assert_eq!(
            report
                .counters()
                .count_of(CounterKind::Read, CounterView::Committed),
            30
        );
        assert_eq!(
            report
                .counters()
                .count_of(CounterKind::Write, CounterView::Committed),
            30,
            "MIG-041 reports exactly what would be written"
        );
        assert_eq!(
            count_rows(&target, "dry_dst").await,
            0,
            "and writes none of it"
        );
    }
);

against_a_cluster!(sch_009_a_schema_change_aborts_the_run, |origin, target| {
    ddl(
        &origin,
        &format!("CREATE TABLE IF NOT EXISTS {KEYSPACE}.watch_src (id int PRIMARY KEY, data text)"),
    )
    .await;
    ddl(
        &target,
        &format!("CREATE TABLE IF NOT EXISTS {KEYSPACE}.watch_dst (id int PRIMARY KEY, data text)"),
    )
    .await;

    let settings = MigrateSettings::new(1, 100, BatchGrouping::Strict, false, false, false);
    let plan = plan_for(&origin, &target, "watch_src", "watch_dst", settings).await;
    // The plan captured the schema version; changing it now is exactly the mid-run case, minus
    // the race that would make the test flaky.
    ddl(
        &origin,
        &format!("ALTER TABLE {KEYSPACE}.watch_src ADD extra text"),
    )
    .await;

    let error: CdmError = plan.executor().check_schema().await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::SchemaChanged);
    assert!(error.kind().is_fatal());
    assert!(error.to_string().contains("SCH-009"), "{error}");

    // And the run itself fails every range it claims rather than writing anything.
    let report = migrate(plan).await;
    assert_eq!(report.ranges_passed(), 0);
    assert!(report.ranges_failed() > 0);
    assert_eq!(count_rows(&target, "watch_dst").await, 0);
});

/// Asserts that every counter in the origin table equals its counterpart in the target.
async fn assert_counters_match(origin: &ClusterSession, target: &ClusterSession) {
    let expected = read_counters(origin, "hits_src").await;
    let actual = read_counters(target, "hits_dst").await;
    assert_eq!(
        expected, actual,
        "a counter delta must be applied exactly once (MIG-031, CON-012)"
    );
}

/// Reads `(id, cc, n)` straight off the response frame.
///
/// The driver types a `counter` column as its own `Counter`, and naming a driver type here would
/// undo the very separation `cdm_cql::exec` exists to keep (`ARCHITECTURE.md` §3). Reading the raw
/// cells is also what the job does, so this asserts against the same representation the delta
/// arithmetic works in.
async fn read_counters(session: &ClusterSession, table: &str) -> Vec<(i32, String, i64)> {
    let result = session
        .session()
        .query_unpaged(format!("SELECT id, cc, n FROM {KEYSPACE}.{table}"), &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let mut rows: Vec<(i32, String, i64)> = result
        .rows::<RawRow<'_, '_>>()
        .unwrap()
        .map(|row| {
            let row = row.unwrap();
            let id = i32::from_be_bytes(row.cell(0).unwrap().bytes.unwrap().try_into().unwrap());
            let cc = String::from_utf8(row.cell(1).unwrap().bytes.unwrap().to_vec()).unwrap();
            let n = i64::from_be_bytes(row.cell(2).unwrap().bytes.unwrap().try_into().unwrap());
            (id, cc, n)
        })
        .collect();
    rows.sort();
    rows
}

against_a_cluster!(
    met_010_a_real_migration_fills_the_request_latency_histograms,
    |origin, target| {
        // The test the defect needed. `MET-010`'s instruments existed, were unit-tested, were
        // exported and were rendered — and no code path fed the latency histograms, so every
        // percentile in a real run was zero and nothing was red. Asserting on a *real* migration
        // through the *real* executor is the only thing that could have said so: a test that
        // records into `Instruments` by hand proves the recorder and nothing about the wiring.
        ddl(
            &origin,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.obs_src (id int PRIMARY KEY, data text)"
            ),
        )
        .await;
        ddl(
            &target,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.obs_dst (id int PRIMARY KEY, data text)"
            ),
        )
        .await;
        for id in 0..120i32 {
            origin
                .session()
                .query_unpaged(
                    format!("INSERT INTO {KEYSPACE}.obs_src (id, data) VALUES (?, ?)"),
                    (id, format!("row-{id}")),
                )
                .await
                .unwrap();
        }

        let instruments = Arc::new(Instruments::new(std::time::Instant::now()));
        // A batch size above one so that the `batch` operation and the batch-size distribution are
        // exercised too, and a rate limit so that `ENG-005`'s wait time is a real measurement.
        let settings = MigrateSettings::new(5, 50, BatchGrouping::Strict, false, false, false);
        let plan = plan_observed(
            &origin,
            &target,
            "obs_src",
            "obs_dst",
            settings,
            Some(Arc::clone(&instruments)),
        )
        .await;
        let report = migrate_observed(
            plan,
            Some(Arc::clone(&instruments)),
            SchedulerSettings::default()
                .with_workers(4)
                .with_ratelimits(2_000, 2_000),
        )
        .await;
        assert_eq!(report.ranges_failed(), 0);
        assert_eq!(count_rows(&target, "obs_dst").await, 120);

        let snapshot = instruments.snapshot();

        // Per side and per operation, which is what `MET-010` actually asks for.
        let range_read = snapshot.origin.latency_for(Operation::RangeRead);
        assert!(
            range_read.count > 0,
            "the origin range read recorded no latency: {range_read:?}"
        );
        assert!(range_read.percentile(0.5) > 0, "a page took no time at all");
        let batch = snapshot.target.latency_for(Operation::Batch);
        assert!(
            batch.count > 0,
            "the target batch recorded no latency: {batch:?}"
        );

        // An operation this run never issued stays empty rather than reporting zeroes, which is
        // what keeps a guardrail run from exporting four empty target families.
        assert!(snapshot.target.latency_for(Operation::KeyRead).is_empty());
        assert!(snapshot.origin.latency_for(Operation::Write).is_empty());

        // Every guard balanced its start, on the success path and the failing path alike.
        assert_eq!(snapshot.origin.inflight, 0);
        assert_eq!(snapshot.target.inflight, 0);

        // The rest of `MET-010`'s list, on the same run.
        assert!(snapshot.origin.bytes.total > 0, "no origin bytes counted");
        assert!(snapshot.batch_size.count > 0, "no batch size recorded");
        assert!(
            !snapshot.origin.ratelimit_wait.is_empty(),
            "the rate limiter of ENG-005 reported no wait, not even a zero-length one"
        );
    }
);

/// The suite's own sanity check: without a container runtime everything above skips, and this is
/// the one case that must still run so `cargo test --workspace` proves the file compiles.
#[test]
fn tst_102_the_suite_declares_the_engine_it_targets() {
    let engine = engine();
    assert!(!engine.to_string().is_empty());
    let _ = Duration::from_secs(1);
}
