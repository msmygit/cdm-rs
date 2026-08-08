//! The validate job end to end, against a real node.
//!
//! The unit tests in `cdm_engine::jobs::validate` prove the *decisions* — what counts as a
//! mismatch, what the detail says, which counter moves. Only a node can prove the claims that
//! actually decide whether a validate run is trustworthy:
//!
//! | Claim | Test |
//! |---|---|
//! | A seeded difference is found, and only the seeded ones (`VAL-002`, `VAL-006`, `VAL-008`) | [`val_002_val_006_a_seeded_difference_is_found_against_a_real_node`] |
//! | Autocorrect repairs both kinds, and a re-run then passes (`VAL-003`, `VAL-007`, `VAL-016`) | [`val_003_val_007_autocorrect_repairs_what_validation_found`] |
//! | A counter row is not re-inserted without the opt-in (`VAL-004`) | [`val_004_a_counter_row_is_not_reinserted_against_a_real_node`] |
//! | The difference log is a real file at the configured path (`VAL-012`) | [`val_012_the_difference_log_is_written_to_its_own_file`] |
//! | A `vector<float, N>` column compares (`CDC-004`) — only where the engine has them | [`val_005_a_vector_column_compares_where_the_engine_has_vectors`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available.
//!
//! Run with `cargo xtask it`, or
//! `cargo test -p cdm-engine --test validate_it -- --ignored --test-threads=1`.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures
)]

use std::sync::Arc;

use cdm_codec::{CodecRegistry, Planner, PlannerOptions};
use cdm_config::model::{Autocorrect, CdmConfig};
use cdm_config::types::ReportFormat;
use cdm_core::{JobKind, RunId, Side};
use cdm_cql::connect::{connect, ClusterSession};
use cdm_cql::rows::{CqlRowSink, CqlRowSource, TokenKind};
use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
use cdm_cql::statement::{
    Binder, ColumnMapping, MappingOptions, MissingKeyPolicy, OriginProjection, OriginRangeSelect,
    StatementOptions, TargetSelectByPk, TargetUpsert,
};
use cdm_engine::jobs::validate::{
    ComparisonPlan, DiffLog, DiscrepancyRecord, DiscrepancyReport, ValidateJob, ValidateSettings,
    REDACTED_PREFIX,
};
use cdm_engine::planner::{Partitioner, Planner as TokenPlanner, PlannerSettings};
use cdm_engine::scheduler::{NoopObserver, Scheduler, SchedulerSettings};
use cdm_metrics::{CounterKind, CounterView};
use cdm_testkit::{skip_without_container_runtime, ClusterFixture, Engine};

/// The keyspace every case in this file uses.
const KEYSPACE: &str = "cdm_validate";

fn engines() -> Vec<Engine> {
    cdm_testkit::engines_under_test().expect("CDM_IT_ENGINES names an unknown engine")
}

/// Runs a body against every engine under test, skipping entirely without a container runtime.
macro_rules! against_every_engine {
    ($name:ident, |$session:ident, $fx:ident, $engine:ident| $body:block) => {
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires a container runtime; run with --ignored or via `cargo xtask it`"]
        async fn $name() {
            let _runtime = skip_without_container_runtime!();
            for $engine in engines() {
                let $fx = ClusterFixture::start(&$engine)
                    .await
                    .unwrap_or_else(|e| panic!("starting {}: {e}", $engine));
                let $session = session(&$fx).await;
                ddl(&$session, &cdm_testkit::create_keyspace_statement(KEYSPACE)).await;
                $body
            }
        }
    };
}

/// A cdm-rs session pointed at the fixture, built through the same path a real run uses.
async fn session(fixture: &ClusterFixture) -> ClusterSession {
    let mut config = CdmConfig::default();
    "127.0.0.1".clone_into(&mut config.connect.origin.host);
    config.connect.origin.port = fixture.host_port();
    connect(&config, Side::Origin)
        .await
        .unwrap_or_else(|e| panic!("connecting to {}: {e}", fixture.contact_point()))
}

async fn ddl(session: &ClusterSession, cql: &str) {
    session
        .session()
        .query_unpaged(cql, ())
        .await
        .unwrap_or_else(|e| panic!("{cql}: {e}"));
    session.session().await_schema_agreement().await.unwrap();
}

fn column(name: &str, cql_type: &str, kind: ColumnKind, position: i32) -> ColumnMeta {
    ColumnMeta {
        name: name.to_owned(),
        cql_type: cql_type.to_owned(),
        kind,
        position,
        clustering_order: if kind == ColumnKind::Clustering {
            ClusteringOrder::Asc
        } else {
            ClusteringOrder::None
        },
    }
}

fn schema(table: &str, columns: Vec<ColumnMeta>) -> TableSchema {
    TableSchema {
        keyspace: KEYSPACE.to_owned(),
        table: table.to_owned(),
        columns,
        is_materialized_view: false,
    }
}

/// Everything one validate run needs, assembled the way a real run assembles it.
struct Harness {
    job: Arc<ValidateJob>,
    diff: Arc<DiffLog>,
    report: Arc<DiscrepancyReport>,
}

async fn build(
    session: &ClusterSession,
    origin: &TableSchema,
    target: &TableSchema,
    settings: ValidateSettings,
    diff: Arc<DiffLog>,
) -> Harness {
    build_reporting(
        session,
        origin,
        target,
        settings,
        diff,
        Arc::new(DiscrepancyReport::disabled()),
        false,
    )
    .await
}

/// A read-only harness whose comparison plan compares existence only (`VAL-015`).
async fn build_keys_only(
    session: &ClusterSession,
    origin: &TableSchema,
    target: &TableSchema,
) -> Harness {
    build_reporting(
        session,
        origin,
        target,
        ValidateSettings::read_only(),
        Arc::new(DiffLog::in_memory()),
        Arc::new(DiscrepancyReport::disabled()),
        true,
    )
    .await
}

async fn build_reporting(
    session: &ClusterSession,
    origin: &TableSchema,
    target: &TableSchema,
    settings: ValidateSettings,
    diff: Arc<DiffLog>,
    report: Arc<DiscrepancyReport>,
    keys_only: bool,
) -> Harness {
    let mapping = ColumnMapping::resolve(origin, target, &MappingOptions::default()).unwrap();
    let projection = OriginProjection::new(mapping.origin_columns(), &[]);
    let range_select = OriginRangeSelect::new(origin, &projection, None, false);
    let target_select = TargetSelectByPk::new(&mapping).unwrap();
    let upsert = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
    let planner = Planner::new(
        CodecRegistry::with_builtins(&[], None).unwrap(),
        PlannerOptions::default(),
    );
    let binder = Binder::new(
        &mapping,
        upsert,
        &planner,
        MissingKeyPolicy::default(),
        false,
    )
    .unwrap();

    let source = CqlRowSource::prepare(
        Arc::clone(session.session()),
        &range_select,
        &mapping,
        &target_select,
        TokenKind::Murmur3,
        MissingKeyPolicy::default(),
    )
    .await
    .unwrap();
    let sink = CqlRowSink::prepare(
        Arc::clone(session.session()),
        &target_select,
        binder,
        &mapping,
    )
    .await
    .unwrap();
    let plan = ComparisonPlan::resolve(&mapping, &planner, None, false)
        .unwrap()
        .with_keys_only(keys_only);

    Harness {
        job: Arc::new(
            ValidateJob::new(
                Arc::new(source),
                Arc::new(sink),
                Arc::new(plan),
                settings,
                Arc::clone(&diff),
            )
            .with_report(Arc::clone(&report)),
        ),
        diff,
        report,
    }
}

/// The run's committed counters, after validating the whole ring.
async fn validate(harness: &Harness) -> cdm_engine::RunReport {
    let plan = TokenPlanner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(8))
        .plan(RunId::from_raw(1), None)
        .unwrap();
    let scheduler = Scheduler::new(SchedulerSettings::default().with_workers(2)).unwrap();
    scheduler
        .run(
            &plan,
            Arc::clone(&harness.job) as Arc<_>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap()
}

fn count(report: &cdm_engine::RunReport, kind: CounterKind) -> u64 {
    report.counters().count_of(kind, CounterView::Committed)
}

/// `id int PRIMARY KEY, data text` on both sides, with five origin rows and a target that agrees
/// about three of them, differs about one, and has never heard of the fifth.
async fn seed_simple(session: &ClusterSession, suffix: &str) -> (TableSchema, TableSchema) {
    let columns = || {
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("data", "text", ColumnKind::Regular, -1),
        ]
    };
    let (src, dst) = (format!("src_{suffix}"), format!("dst_{suffix}"));
    for name in [&src, &dst] {
        ddl(
            session,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.{name} (id int PRIMARY KEY, data text)"
            ),
        )
        .await;
    }
    for id in 1..=5 {
        ddl(
            session,
            &format!("INSERT INTO {KEYSPACE}.{src} (id, data) VALUES ({id}, 'row-{id}')"),
        )
        .await;
    }
    for id in 1..=3 {
        ddl(
            session,
            &format!("INSERT INTO {KEYSPACE}.{dst} (id, data) VALUES ({id}, 'row-{id}')"),
        )
        .await;
    }
    // Row 4 disagrees; row 5 is absent entirely.
    ddl(
        session,
        &format!("INSERT INTO {KEYSPACE}.{dst} (id, data) VALUES (4, 'tampered')"),
    )
    .await;

    (schema(&src, columns()), schema(&dst, columns()))
}

against_every_engine!(
    val_002_val_006_a_seeded_difference_is_found_against_a_real_node,
    |session, fx, engine| {
        let _ = (&fx, &engine);
        let (src, dst) = seed_simple(&session, "find").await;
        let harness = build(
            &session,
            &src,
            &dst,
            ValidateSettings::read_only(),
            Arc::new(DiffLog::in_memory()),
        )
        .await;

        let report = validate(&harness).await;
        assert_eq!(report.job(), JobKind::Validate);
        assert_eq!(count(&report, CounterKind::Read), 5);
        assert_eq!(count(&report, CounterKind::Valid), 3);
        assert_eq!(count(&report, CounterKind::Mismatch), 1);
        assert_eq!(count(&report, CounterKind::Missing), 1);
        assert_eq!(count(&report, CounterKind::Error), 0);
        assert_eq!(report.ranges_failed(), 0);

        // VAL-016: the ranges that found something are DIFF, and nothing was corrected.
        let diffs = report
            .outcomes()
            .iter()
            .filter(|outcome| outcome.status == cdm_core::RunStatus::Diff)
            .count();
        assert_eq!(
            diffs, 2,
            "one range for the mismatch, one for the missing row"
        );

        // VAL-017: the findings name keys and columns, never values.
        let lines = harness.diff.captured().join("\n");
        assert!(
            lines.contains("Missing target row found for key:"),
            "{lines}"
        );
        assert!(lines.contains("Target column:data"), "{lines}");
        assert!(!lines.contains("tampered"), "a row value leaked: {lines}");
        assert!(!lines.contains("row-4"), "a row value leaked: {lines}");
    }
);

against_every_engine!(
    val_003_val_007_autocorrect_repairs_what_validation_found,
    |session, fx, engine| {
        let _ = (&fx, &engine);
        let (src, dst) = seed_simple(&session, "fix").await;
        let harness = build(
            &session,
            &src,
            &dst,
            ValidateSettings {
                autocorrect: Autocorrect {
                    missing: true,
                    mismatch: true,
                    missing_counter: false,
                },
                target_is_counter: false,
            },
            Arc::new(DiffLog::in_memory()),
        )
        .await;

        let report = validate(&harness).await;
        assert_eq!(count(&report, CounterKind::Missing), 1);
        assert_eq!(count(&report, CounterKind::CorrectedMissing), 1);
        assert_eq!(count(&report, CounterKind::Mismatch), 1);
        assert_eq!(count(&report, CounterKind::CorrectedMismatch), 1);
        // VAL-016: every discrepancy was corrected, so the ranges that found one are
        // DIFF_CORRECTED and `TRK-031` will not re-plan them.
        assert_eq!(
            report
                .outcomes()
                .iter()
                .filter(|o| o.status == cdm_core::RunStatus::DiffCorrected)
                .count(),
            2
        );

        // The repair is real: a second, read-only run finds nothing at all.
        let after = build(
            &session,
            &src,
            &dst,
            ValidateSettings::read_only(),
            Arc::new(DiffLog::in_memory()),
        )
        .await;
        let report = validate(&after).await;
        assert_eq!(count(&report, CounterKind::Valid), 5);
        assert_eq!(count(&report, CounterKind::Mismatch), 0);
        assert_eq!(count(&report, CounterKind::Missing), 0);
        assert!(after.diff.captured().is_empty());
        assert!(report
            .outcomes()
            .iter()
            .all(|o| o.status == cdm_core::RunStatus::Pass));
    }
);

against_every_engine!(
    val_004_a_counter_row_is_not_reinserted_against_a_real_node,
    |session, fx, engine| {
        let _ = (&fx, &engine);
        let columns = || {
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("hits", "counter", ColumnKind::Regular, -1),
            ]
        };
        for name in ["src_ctr", "dst_ctr"] {
            ddl(
                &session,
                &format!(
                    "CREATE TABLE IF NOT EXISTS {KEYSPACE}.{name} (id int PRIMARY KEY, \
                     hits counter)"
                ),
            )
            .await;
        }
        ddl(
            &session,
            &format!("UPDATE {KEYSPACE}.src_ctr SET hits = hits + 7 WHERE id = 1"),
        )
        .await;

        let (src, dst) = (schema("src_ctr", columns()), schema("dst_ctr", columns()));
        let refused = build(
            &session,
            &src,
            &dst,
            ValidateSettings {
                autocorrect: Autocorrect {
                    missing: true,
                    mismatch: false,
                    missing_counter: false,
                },
                target_is_counter: true,
            },
            Arc::new(DiffLog::in_memory()),
        )
        .await;
        let report = validate(&refused).await;
        assert_eq!(count(&report, CounterKind::Missing), 1);
        assert_eq!(
            count(&report, CounterKind::CorrectedMissing),
            0,
            "a counter row must not be re-inserted without autocorrect.missing_counter"
        );

        // With the opt-in, the row is written and the counter converges on the origin's value.
        let allowed = build(
            &session,
            &src,
            &dst,
            ValidateSettings {
                autocorrect: Autocorrect {
                    missing: true,
                    mismatch: false,
                    missing_counter: true,
                },
                target_is_counter: true,
            },
            Arc::new(DiffLog::in_memory()),
        )
        .await;
        let report = validate(&allowed).await;
        assert_eq!(count(&report, CounterKind::CorrectedMissing), 1);

        // The counter converged on the origin's value rather than being added to twice: a
        // read-only re-validation now finds the two sides in agreement (`MIG-031`).
        let after = build(
            &session,
            &src,
            &dst,
            ValidateSettings {
                autocorrect: Autocorrect::default(),
                target_is_counter: true,
            },
            Arc::new(DiffLog::in_memory()),
        )
        .await;
        let report = validate(&after).await;
        assert_eq!(count(&report, CounterKind::Valid), 1);
        assert_eq!(count(&report, CounterKind::Mismatch), 0);
        assert_eq!(count(&report, CounterKind::Missing), 0);
    }
);

against_every_engine!(
    val_012_the_difference_log_is_written_to_its_own_file,
    |session, fx, engine| {
        let _ = (&fx, &engine);
        let (src, dst) = seed_simple(&session, "log").await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cdm_logs").join("cdm_diff.log");
        let harness = build(
            &session,
            &src,
            &dst,
            ValidateSettings::read_only(),
            Arc::new(DiffLog::open(&path).unwrap()),
        )
        .await;

        validate(&harness).await;
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written.lines().count(), 2, "{written}");
        assert!(written.contains("Missing target row found for key:"));
        assert!(written.contains("Mismatch row found for key:"));
        assert!(
            !written.contains("tampered"),
            "a row value leaked: {written}"
        );
    }
);

against_every_engine!(
    val_005_a_vector_column_compares_where_the_engine_has_vectors,
    |session, fx, engine| {
        // CDC-004: `vector<T, N>` exists only from Cassandra 5.0. The gate is the fixture's
        // capability, never a version string — a new image, or ScyllaDB, changes the answer
        // without changing this test.
        if !fx.capabilities().vectors {
            eprintln!("skipping the vector case on {engine}: no vector<> support");
            continue;
        }
        let columns = || {
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("embedding", "vector<float, 3>", ColumnKind::Regular, -1),
            ]
        };
        for name in ["src_vec", "dst_vec"] {
            ddl(
                &session,
                &format!(
                    "CREATE TABLE IF NOT EXISTS {KEYSPACE}.{name} (id int PRIMARY KEY, \
                     embedding vector<float, 3>)"
                ),
            )
            .await;
        }
        for (table, vector) in [
            ("src_vec", "[1.0, 2.0, 3.0]"),
            ("dst_vec", "[1.0, 2.0, 3.0]"),
        ] {
            ddl(
                &session,
                &format!("INSERT INTO {KEYSPACE}.{table} (id, embedding) VALUES (1, {vector})"),
            )
            .await;
        }
        ddl(
            &session,
            &format!("INSERT INTO {KEYSPACE}.src_vec (id, embedding) VALUES (2, [9.0, 9.0, 9.0])"),
        )
        .await;
        ddl(
            &session,
            &format!("INSERT INTO {KEYSPACE}.dst_vec (id, embedding) VALUES (2, [9.0, 9.0, 8.0])"),
        )
        .await;

        let (src, dst) = (schema("src_vec", columns()), schema("dst_vec", columns()));
        let harness = build(
            &session,
            &src,
            &dst,
            ValidateSettings::read_only(),
            Arc::new(DiffLog::in_memory()),
        )
        .await;
        let report = validate(&harness).await;
        assert_eq!(count(&report, CounterKind::Valid), 1);
        assert_eq!(count(&report, CounterKind::Mismatch), 1);
        let lines = harness.diff.captured().join("\n");
        assert!(lines.contains("Target column:embedding"), "{lines}");
    }
);

against_every_engine!(
    val_013_met_033_a_seeded_difference_reaches_the_report_and_the_summary,
    |session, fx, engine| {
        // The end-to-end claim of this PR: a real difference on a real node becomes a record in a
        // real file, with no row value in it, and the run summary points at that file.
        let _ = (&fx, &engine);
        let (src, dst) = seed_simple(&session, "report").await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cdm_logs").join("discrepancies.ndjson");
        let harness = build_reporting(
            &session,
            &src,
            &dst,
            ValidateSettings::read_only(),
            Arc::new(DiffLog::in_memory()),
            Arc::new(
                DiscrepancyReport::open(RunId::from_raw(7), ReportFormat::Ndjson, &path, true)
                    .unwrap(),
            ),
            false,
        )
        .await;

        let run = validate(&harness).await;
        harness.report.finish().unwrap();

        // One record per discrepancy: the tampered row and the absent one.
        let written = std::fs::read_to_string(&path).unwrap();
        let records: Vec<DiscrepancyRecord> = written
            .lines()
            .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}")))
            .collect();
        assert_eq!(records.len(), 2, "{written}");
        assert_eq!(harness.report.records(), 2);

        let mismatch = records
            .iter()
            .find(|record| record.kind == cdm_metrics::DiscrepancyKind::Mismatch)
            .unwrap_or_else(|| panic!("{written}"));
        assert_eq!(mismatch.columns.len(), 1);
        assert_eq!(mismatch.columns[0].column, "data");
        assert!(mismatch.values_redacted);
        assert!(mismatch.columns[0].origin.starts_with(REDACTED_PREFIX));
        assert!(records
            .iter()
            .any(|record| record.kind == cdm_metrics::DiscrepancyKind::Missing));

        // SEC-002: the default carries no row value, and this is the file that would travel.
        for leak in ["tampered", "row-4", "row-5"] {
            assert!(!written.contains(leak), "a row value leaked: {written}");
        }

        // MET-033: the summary the CLI writes, with the report attached to it.
        let summary = run
            .summary(chrono::Utc::now())
            .with_config_hash("0123456789abcdef")
            .with_discrepancy_report(harness.report.reference().unwrap());
        let discrepancies = summary.discrepancies.clone().unwrap();
        assert_eq!(discrepancies.missing, 1);
        assert_eq!(discrepancies.mismatch, 1);
        assert_eq!(discrepancies.outstanding, 2);
        let reference = discrepancies.report.unwrap();
        assert_eq!(reference.path, path);
        assert_eq!(reference.format, "ndjson");
        assert_eq!(reference.records, 2);
        assert!(reference.values_redacted);

        let summary_path = dir.path().join("summary.json");
        summary.write_to(&summary_path).unwrap();
        let text = std::fs::read_to_string(&summary_path).unwrap();
        assert!(text.contains("\"READ\": 5"), "{text}");
        assert!(text.contains("cdm.run-summary/v1"), "{text}");
        for leak in ["tampered", "row-4", "password", "cassandra"] {
            assert!(!text.contains(leak), "the summary leaked {leak}: {text}");
        }
    }
);

against_every_engine!(
    val_015_a_keys_only_run_finds_the_missing_row_and_not_the_tampered_one,
    |session, fx, engine| {
        let _ = (&fx, &engine);
        let (src, dst) = seed_simple(&session, "keysonly").await;
        let mapping_only = build_keys_only(&session, &src, &dst).await;

        let run = validate(&mapping_only).await;
        assert_eq!(count(&run, CounterKind::Read), 5);
        assert_eq!(count(&run, CounterKind::Missing), 1);
        assert_eq!(
            count(&run, CounterKind::Mismatch),
            0,
            "existence is all a keys-only run compares"
        );
        assert_eq!(
            count(&run, CounterKind::Valid),
            4,
            "including the row whose value was tampered with"
        );
    }
);
