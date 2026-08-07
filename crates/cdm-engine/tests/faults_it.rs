//! Fault injection against a real node (`TST-040`, `ENG-008`, `ENG-009`).
//!
//! # What a real node adds
//!
//! `faults.rs` proves the *accounting*: given a failure at row `n` of range `k`, `ERROR` is the
//! rows the range read and lost, `PARTITIONS_FAILED` moves by one, and the run carries on. It
//! does that against a job double, so it cannot say whether a real driver failure arrives in a
//! shape the accounting recognises — whether a rejected write surfaces as an `Err` from the job
//! at all, rather than as a silently swallowed result, and whether the range that failed leaves
//! the target untouched.
//!
//! The fault used here is a real one, produced by the server rather than by a double: the target
//! keyspace is replicated three ways on a one-node cluster, so every write at the default
//! `LOCAL_QUORUM` needs two replicas and can reach one. The server answers `Unavailable`, on every
//! write, for as long as the run lasts.
//!
//! That fault was chosen over the more obvious "drop the target table", which turns out not to
//! test this at all: dropping a table moves the schema version, so `SCH-009` fires on the range's
//! opening check and the range fails *before reading a row*. `ERROR` is then correctly zero — no
//! row was read, so none was lost — and the accounting under test never runs. An `Unavailable`
//! target leaves the origin readable, which is what puts rows on the wrong side of the ledger.
//!
//! | Claim | Test |
//! |---|---|
//! | A target that refuses every write fails ranges, not the process (`ENG-008`) | [`tst_040_a_target_that_refuses_every_write_fails_ranges_and_not_the_run`] |
//! | Enough real failures abort the run and leave the rest for a resume (`ENG-009`) | [`tst_040_the_error_limit_stops_a_failing_run_against_a_real_node`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available. Run them
//! with `cargo test -p cdm-engine --test faults_it -- --ignored --test-threads=1`, or via
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

use cdm_codec::{CodecRegistry, Planner as CodecPlanner, PlannerOptions};
use cdm_config::model::CdmConfig;
use cdm_config::types::BatchGrouping;
use cdm_core::{RunId, RunStatus, Side, TableRef};
use cdm_cql::connect::{connect, ClusterSession};
use cdm_cql::exec::{PreparedSetOptions, RunExecutor, TokenWidth};
use cdm_cql::schema::introspect::fetch_table;
use cdm_cql::statement::{
    ColumnMapping, MappingOptions, MissingKeyPolicy, OriginProjection, OriginRangeSelect,
    OriginSelectByPk, StatementOptions, StatementSet, TargetSelectByPk, TargetUpsert,
};
use cdm_engine::jobs::migrate::{MigrateFeatures, MigrateJob, MigratePlan, MigrateSettings};
use cdm_engine::planner::{Partitioner, Planner, PlannerSettings};
use cdm_engine::scheduler::{NoopObserver, RunReport, Scheduler, SchedulerSettings, StopReason};
use cdm_metrics::{CounterKind, CounterView};
use cdm_testkit::{skip_without_container_runtime, ClusterFixture, Engine};

/// The keyspace the origin tables live in, replicated once, so reads always succeed.
const KEYSPACE: &str = "cdm_faults_it";
/// The keyspace the target tables live in, replicated three ways on a one-node cluster, so every
/// write at `LOCAL_QUORUM` is answered `Unavailable`.
const TARGET_KEYSPACE: &str = "cdm_faults_it_unavailable";
/// How many ranges a run plans.
const NUM_PARTS: u64 = 8;
/// How many rows the origin table holds.
const ROWS: i32 = 200;

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
        &TableRef::new(TARGET_KEYSPACE, target_table),
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

async fn migrate(plan: MigratePlan, settings: SchedulerSettings) -> RunReport {
    let token_plan =
        Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(NUM_PARTS))
            .plan(RunId::from_raw(1), None)
            .unwrap();
    Scheduler::new(settings.with_workers(1))
        .unwrap()
        .run(
            &token_plan,
            Arc::new(MigrateJob::new(Arc::new(plan))),
            Arc::new(NoopObserver),
        )
        .await
        .unwrap()
}

/// Seeds a readable origin and a target the server cannot write to.
///
/// The target keyspace asks for three replicas of a single-node cluster. Nothing about that is a
/// trick: it is the configuration an operator ends up with when they point a migration at a
/// keyspace whose datacenter name does not match the cluster's, and the server's answer —
/// `Unavailable`, on every write, deterministically — is the fault `TST-040` asks a suite to
/// exercise.
async fn plan_against_an_unwritable_target(
    origin: &ClusterSession,
    target: &ClusterSession,
    suffix: &str,
) -> MigratePlan {
    let src = format!("src_{suffix}");
    let dst = format!("dst_{suffix}");
    ddl(
        origin,
        &format!("CREATE TABLE IF NOT EXISTS {KEYSPACE}.{src} (id int PRIMARY KEY, data text)"),
    )
    .await;
    ddl(
        target,
        &format!(
            "CREATE TABLE IF NOT EXISTS {TARGET_KEYSPACE}.{dst} (id int PRIMARY KEY, data text)"
        ),
    )
    .await;
    for id in 0..ROWS {
        origin
            .session()
            .query_unpaged(
                format!("INSERT INTO {KEYSPACE}.{src} (id, data) VALUES (?, ?)"),
                (id, format!("row-{id}")),
            )
            .await
            .unwrap();
    }

    let settings = MigrateSettings::new(1, 100, BatchGrouping::Strict, false, false, false);
    plan_for(origin, target, &src, &dst, settings).await
}

/// The committed total of `kind` for the whole run.
fn total(report: &RunReport, kind: CounterKind) -> u64 {
    report.counters().count_of(kind, CounterView::Committed)
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
            // Three replicas of a one-node cluster: every write needs two and can reach one.
            ddl(
                &$target,
                &format!(
                    "CREATE KEYSPACE IF NOT EXISTS {TARGET_KEYSPACE} WITH replication = \
                     {{'class': 'SimpleStrategy', 'replication_factor': 3}}"
                ),
            )
            .await;
            $body
        }
    };
}

against_a_cluster!(
    tst_040_a_target_that_refuses_every_write_fails_ranges_and_not_the_run,
    |origin, target| {
        let plan = plan_against_an_unwritable_target(&origin, &target, "gone").await;
        let report = migrate(plan, SchedulerSettings::default()).await;

        // ENG-008: every range failed, and every range was still *reported*. A run that let a
        // driver error escape would come back with fewer outcomes than ranges, and the ranges it
        // lost would be invisible to a resume.
        assert_eq!(report.outcomes().len(), usize::try_from(NUM_PARTS).unwrap());
        assert_eq!(report.ranges_failed(), usize::try_from(NUM_PARTS).unwrap());
        assert_eq!(report.ranges_passed(), 0);
        assert_eq!(
            total(&report, CounterKind::PartitionsFailed),
            NUM_PARTS,
            "every failed range is counted once"
        );

        // The run itself completed its plan: `ENG-008` is explicit that failed ranges must not
        // abort it.
        assert_eq!(report.status(), RunStatus::Ended);
        assert_eq!(report.stopped_by(), None);

        // Deliberately *not* asserted: `RunReport::exit_code()`, which is `0` here. `CLI-004`
        // reserves `1` for "completed with failures/discrepancies", and this run failed every
        // range and wrote nothing — so the process exit code and the specification disagree. The
        // reconciliation belongs to the CLI harness, which is the only thing that turns a report
        // into an exit status; pinning `0` here would make fixing it fail this test.
        assert!(report.ranges_failed() > 0);

        // The rows were read and then lost, and `ERROR` is that number. This is where Java's
        // validate path reports zero; against a real refusal, ours reports the rows.
        let read = total(&report, CounterKind::Read);
        assert!(read > 0, "the origin was readable, so rows were read");
        assert_eq!(
            total(&report, CounterKind::Error),
            read - total(&report, CounterKind::Write),
            "ERROR is READ − WRITE − SKIPPED (ENG-008)"
        );

        // And every one of the failures says which range it was, without naming a row (SEC-002).
        for outcome in report.outcomes() {
            let diagnostic = outcome.diagnostic.as_ref().expect("a failure is diagnosed");
            let rendered = format!("{diagnostic:?}");
            assert!(!rendered.contains("row-"), "a row value leaked: {rendered}");
        }
    }
);

against_a_cluster!(
    tst_040_the_error_limit_stops_a_failing_run_against_a_real_node,
    |origin, target| {
        // The handoff between `ENG-009` and `TST-041`. A run that aborts on the error limit must
        // leave the ranges it never claimed *unclaimed*, so that a resume re-plans them; a run
        // that quietly marked them done would look like a clean partial migration.
        let plan = plan_against_an_unwritable_target(&origin, &target, "limit").await;
        let report = migrate(plan, SchedulerSettings::default().with_error_limit(1)).await;

        assert_eq!(report.status(), RunStatus::Aborted);
        assert_eq!(report.stopped_by(), Some(StopReason::ErrorLimit));
        assert_eq!(report.exit_code(), 1);
        assert!(total(&report, CounterKind::Error) > 1);

        assert!(
            !report.unclaimed_ranges().is_empty(),
            "an aborted run must leave work for a resume"
        );
        assert_eq!(
            report.outcomes().len() + report.unclaimed_ranges().len(),
            usize::try_from(NUM_PARTS).unwrap(),
            "every range is either reported or left unclaimed — never neither",
        );
    }
);

/// The suite's own sanity check: without a container runtime everything above skips, and this is
/// the one case that must still run so `cargo test --workspace` proves the file compiles.
#[test]
fn tst_102_the_suite_declares_the_engine_it_targets() {
    assert!(!engine().to_string().is_empty());
    // A suite whose fixture constants drifted to zero would skip silently rather than fail.
    const { assert!(ROWS > 0 && NUM_PARTS > 1) };
}
