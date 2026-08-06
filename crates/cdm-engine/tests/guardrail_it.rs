//! The guardrail job against a real cluster (`GRD-001`..`GRD-003`).
//!
//! The unit tests in `cdm_engine::jobs::guardrail` prove what the job does with a given set of
//! column *lengths*. Only a node can prove the claim underneath that one — that a length read off
//! the response frame is the size Java computes by decoding a cell and re-encoding it — and it has
//! to hold for every CQL type, not for the two in a hand-written fixture.
//!
//! | Claim | Test |
//! |---|---|
//! | Java's own SIT fixture produces Java's own counts against a real node (`GRD-002`, `GRD-003`) | [`grd_003_the_java_sit_fixture_reproduces_its_counts_against_a_real_node`] |
//! | A row just over the threshold is `LARGE` and one just under it is `VALID` (`GRD-002`) | [`grd_002_a_row_over_the_threshold_is_large_and_one_under_it_is_valid`] |
//! | Every CQL type the engine supports is measured, and measured as its frame length (`GRD-002`) | [`grd_002_every_supported_cql_type_is_measured_as_its_frame_length`] |
//! | A `vector<float, 3>` is measured like anything else, where the engine has vectors (`CDC-004`) | [`grd_002_every_supported_cql_type_is_measured_as_its_frame_length`] |
//! | The run exits `1` when it found something and `0` when it did not (`CLI-004`) | [`grd_003_a_run_that_found_oversized_columns_reports_a_finding_not_a_failure`] |
//! | Nothing is written: the origin is byte-identical afterwards (`GRD-001`) | [`grd_001_a_guardrail_run_leaves_the_origin_untouched`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available.
//!
//! Run with `cargo xtask it`, or
//! `cargo test -p cdm-engine --test guardrail_it -- --ignored --test-threads=1`.
//!
//! # `vector<>` is gated on a capability, never on a version string
//!
//! `SchemaGen::all_types` takes the fixture's [`Capabilities`] and includes `vector<float, 3>` only
//! where the engine has it (Cassandra 5.0 and later). The tests below never mention a version.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
// `large_futures` fires on `SessionBuilder::build()`, which is the driver's own future.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures
)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdm_core::{CdmError, ErrorKind, JobKind, PrimaryKey, RawCell, RunId, RunStatus, TokenRange};
use cdm_cql::raw::RawRow;
use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
use cdm_cql::statement::{OriginProjection, OriginRangeSelect};
use cdm_engine::jobs::guardrail::{run_status, GuardrailJob, OriginRows, RowSizeStream};
use cdm_engine::planner::{Partitioner, Planner, PlannerSettings, TokenPlan};
use cdm_engine::scheduler::{NoopObserver, RunReport, Scheduler, SchedulerSettings};
use cdm_feature::{table_view, ColumnSizeGuardrail, Guardrail, RowSizes, TableFacts};
use cdm_metrics::{CounterKind, CounterView};
use cdm_testkit::{
    apply_schema, seed_rows, skip_without_container_runtime, ClusterFixture, DataGen,
    DataGenOptions, Engine, SchemaGen, Seed, TableSpec,
};
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;

/// The keyspace every case in this file uses.
const KEYSPACE: &str = "cdm_guardrail";

/// The engines this process exercises, from `CDM_IT_ENGINES`.
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
                let $session = Arc::new(connect(&$fx).await);
                ddl(&$session, &cdm_testkit::create_keyspace_statement(KEYSPACE)).await;
                $body
            }
        }
    };
}

async fn connect(fixture: &ClusterFixture) -> Session {
    SessionBuilder::new()
        .known_node(fixture.contact_point())
        .connection_timeout(Duration::from_secs(10))
        .build()
        .await
        .unwrap_or_else(|e| panic!("connecting to {}: {e}", fixture.contact_point()))
}

async fn ddl(session: &Session, cql: &str) {
    session
        .query_unpaged(cql, &[])
        .await
        .unwrap_or_else(|e| panic!("{cql}: {e}"));
    session.await_schema_agreement().await.unwrap();
}

// =================================================================================================
// The driver-backed origin reader
// =================================================================================================

/// The origin range scan of `FEA-060`, reduced to lengths as it is read (`GRD-001`, `SEC-002`).
///
/// This is the whole of what a guardrail run can do to a cluster. Note what it does *not* hold: no
/// upsert, no target session, no sink. It also never keeps a column value — the primary-key cells
/// are copied because they identify the row, and every other cell contributes a `usize`.
///
/// The scan is unpaged here because a case seeds a handful of rows and `ENG-003`'s paging is the
/// scheduler's contract, proven in `cdm_engine::scheduler`; a production reader pages, which is why
/// [`OriginRows::scan`] is handed the page size.
struct CqlOrigin {
    session: Arc<Session>,
    cql: String,
    key_indices: Vec<usize>,
}

#[async_trait]
impl OriginRows for CqlOrigin {
    async fn scan(
        &self,
        range: TokenRange,
        _fetch_size: u32,
    ) -> Result<Box<dyn RowSizeStream>, CdmError> {
        let min = i64::try_from(range.min())
            .map_err(|e| CdmError::new(ErrorKind::Read, format!("token out of range: {e}")))?;
        let max = i64::try_from(range.max())
            .map_err(|e| CdmError::new(ErrorKind::Read, format!("token out of range: {e}")))?;
        let result = self
            .session
            .query_unpaged(self.cql.as_str(), (min, max))
            .await
            .map_err(|e| CdmError::new(ErrorKind::Read, format!("{e}")))?;
        let rows = result
            .into_rows_result()
            .map_err(|e| CdmError::new(ErrorKind::Read, format!("{e}")))?;

        let mut out = VecDeque::new();
        for row in rows
            .rows::<RawRow<'_, '_>>()
            .map_err(|e| CdmError::new(ErrorKind::Read, format!("{e}")))?
        {
            let row = row.map_err(|e| CdmError::new(ErrorKind::Read, format!("{e}")))?;
            let key = PrimaryKey::new(
                self.key_indices
                    .iter()
                    .map(|index| {
                        row.cell(*index)
                            .and_then(|cell| cell.bytes)
                            .map_or(RawCell::NULL, |bytes| RawCell::new(bytes.to_vec()))
                    })
                    .collect(),
            );
            out.push_back(RowSizes::new(
                key,
                row.cells().iter().map(cdm_cql::raw::RawCell::byte_len),
            ));
        }
        Ok(Box::new(BufferedRows(out)))
    }
}

struct BufferedRows(VecDeque<RowSizes>);

#[async_trait]
impl RowSizeStream for BufferedRows {
    async fn next_row(&mut self) -> Result<Option<RowSizes>, CdmError> {
        Ok(self.0.pop_front())
    }
}

// =================================================================================================
// Shared plumbing
// =================================================================================================

/// The `cdm-cql` schema for a testkit [`TableSpec`], in the spec's own column order.
fn schema_of(spec: &TableSpec) -> TableSchema {
    let mut partition = -1_i32;
    let mut clustering = -1_i32;
    let columns = spec
        .columns()
        .iter()
        .map(|column| {
            let (kind, position) = match column.kind() {
                cdm_testkit::ColumnKind::Partition => {
                    partition += 1;
                    (ColumnKind::PartitionKey, partition)
                }
                cdm_testkit::ColumnKind::Clustering => {
                    clustering += 1;
                    (ColumnKind::Clustering, clustering)
                }
                cdm_testkit::ColumnKind::Static => (ColumnKind::Static, -1),
                cdm_testkit::ColumnKind::Regular => (ColumnKind::Regular, -1),
            };
            ColumnMeta {
                name: column.name().to_owned(),
                cql_type: column.cql_type().to_string(),
                kind,
                position,
                clustering_order: if kind == ColumnKind::Clustering {
                    ClusteringOrder::Asc
                } else {
                    ClusteringOrder::None
                },
            }
        })
        .collect();
    TableSchema {
        keyspace: spec.keyspace().to_owned(),
        table: spec.table().to_owned(),
        columns,
        is_materialized_view: false,
    }
}

/// The guardrail facts for a spec, in the same column order the projection selects.
fn facts_of(spec: &TableSpec) -> TableFacts {
    let types: Vec<(String, String)> = spec
        .columns()
        .iter()
        .map(|column| (column.name().to_owned(), column.cql_type().to_string()))
        .collect();
    let pairs: Vec<(&str, &str)> = types
        .iter()
        .map(|(name, cql_type)| (name.as_str(), cql_type.as_str()))
        .collect();
    let key: Vec<&str> = spec
        .columns()
        .iter()
        .filter(|column| column.kind().is_key())
        .map(cdm_testkit::ColumnSpec::name)
        .collect();
    TableFacts::from_view(
        &table_view(
            cdm_core::TableRef::new(spec.keyspace(), spec.table()),
            &pairs,
        ),
        &key,
    )
    .unwrap()
}

fn guardrail_for(spec: &TableSpec, kb: f64) -> ColumnSizeGuardrail {
    let config: cdm_core::EffectiveConfig = [("feature.guardrail.column_size_kb", kb.to_string())]
        .into_iter()
        .collect();
    Guardrail::load(&config)
        .unwrap()
        .resolve(&facts_of(spec))
        .unwrap()
}

/// The origin reader for a spec, over the real `FEA-060` range scan.
fn origin_for(session: &Arc<Session>, spec: &TableSpec) -> Arc<dyn OriginRows> {
    let schema = schema_of(spec);
    let projection = OriginProjection::new(&schema.columns, &[]);
    let select = OriginRangeSelect::new(&schema, &projection, None, false);
    let key_indices = spec
        .columns()
        .iter()
        .enumerate()
        .filter(|(_, column)| column.kind().is_key())
        .map(|(index, _)| index)
        .collect();
    Arc::new(CqlOrigin {
        session: Arc::clone(session),
        cql: select.cql().to_owned(),
        key_indices,
    })
}

fn plan() -> TokenPlan {
    Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(4))
        .plan(RunId::from_raw(1_712_345_678_901_234), None)
        .unwrap()
}

async fn run_guardrail(session: &Arc<Session>, spec: &TableSpec, kb: f64) -> RunReport {
    let job = GuardrailJob::new(origin_for(session, spec), guardrail_for(spec, kb)).unwrap();
    Scheduler::new(
        SchedulerSettings::default()
            .with_workers(2)
            .with_ratelimits(0, 0)
            .with_node_id("guardrail-it"),
    )
    .unwrap()
    .run(&plan(), Arc::new(job), Arc::new(NoopObserver))
    .await
    .unwrap()
}

fn total(report: &RunReport, kind: CounterKind) -> u64 {
    report.counters().count_of(kind, CounterView::Committed)
}

/// Every row's column lengths, read independently of the job, so a case can state what the job
/// *should* have found rather than trusting it to agree with itself.
async fn measured_lengths(session: &Session, spec: &TableSpec) -> Vec<Vec<usize>> {
    let names: Vec<&str> = spec
        .columns()
        .iter()
        .map(cdm_testkit::ColumnSpec::name)
        .collect();
    let cql = format!("SELECT {} FROM {}", names.join(","), spec.qualified_name());
    let result = session.query_unpaged(cql.as_str(), &[]).await.unwrap();
    let rows = result.into_rows_result().unwrap();
    rows.rows::<RawRow<'_, '_>>()
        .unwrap()
        .map(|row| {
            row.unwrap()
                .cells()
                .iter()
                .map(cdm_cql::raw::RawCell::byte_len)
                .collect()
        })
        .collect()
}

// =================================================================================================
// GRD-002, GRD-003 — Java's own fixture, against a real node
// =================================================================================================

/// The four rows of `SIT/features/05_guardrail`, verbatim: one clean, one with an oversized value,
/// one with an oversized map key and one with an oversized map value.
const LOREM: &str = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. Etiam sed commodo \
                     enim, eu ullamcorper nunc. Curabitur et risus id ligula commodo convallis. \
                     In hac habitasse platea dictumst. Phasellus blandit, felis id elementum \
                     facilisis, felis est dictum lectus, a rhoncus massa odio vel dolor. Donec \
                     interdum sodales erat, quis facilisis est porttitor sagittis. Aliquam non \
                     neque cursus, sodales quam vitae, malesuada risus. Phasellus nec porttitor \
                     lacus. Aliquam erat volutpat. Mauris velit massa, luctus ut nunc quis, \
                     mollis malesuada purus. Vestibulum non feugiat magna. Nullam mattis \
                     vestibulum velit in iaculis. Phasellus aliquet sit amet urna nec volutpat. \
                     Phasellus mollis metus ac enim lacinia vehicula. Vestibulum ante ipsum \
                     primis in faucibus orci luctus et ultrices posuere cubilia curae; Mauris eu \
                     sapien neque. Nulla eu dolor tellus. Quisque id augue ex. Vivamus nec \
                     hendrerit mi, id malesuada orci. Aenean in lectus porta, placerat sem nec, \
                     tristique massa. Morbi tristique pulvinar massa eget fermentum. Donec \
                     elementum quam a augue vulputate convallis non sit amet velit. Nunc cras.";

fn sit_table() -> TableSpec {
    TableSpec::builder(KEYSPACE, "feature_guardrail")
        .partition_key("key", cdm_codec::CqlTypeInfo::Text)
        .column("value", cdm_codec::CqlTypeInfo::Text)
        .column(
            "fruits",
            cdm_codec::CqlTypeInfo::Map {
                key: Box::new(cdm_codec::CqlTypeInfo::Text),
                value: Box::new(cdm_codec::CqlTypeInfo::Text),
                frozen: false,
            },
        )
        .build()
        .unwrap()
}

async fn seed_sit_rows(session: &Session) {
    let fruits = "{'apples': 'delicious', 'oranges': 'sweet', 'bananas': 'squishy', \
                  'grapes': 'sour'}";
    for (key, value, map) in [
        ("clean", "valueA".to_owned(), fruits.to_owned()),
        ("badValue", LOREM.to_owned(), fruits.to_owned()),
        (
            "badMapKey",
            "valueA".to_owned(),
            format!("{{'{LOREM}': 'delicious', 'oranges': 'sweet'}}"),
        ),
        (
            "badMapValue",
            "valueA".to_owned(),
            format!("{{'apples': '{LOREM}', 'oranges': 'sweet'}}"),
        ),
    ] {
        ddl(
            session,
            &format!(
                "INSERT INTO {KEYSPACE}.feature_guardrail (key, value, fruits) \
                 VALUES ('{key}', '{value}', {map})"
            ),
        )
        .await;
    }
}

against_every_engine!(
    grd_003_the_java_sit_fixture_reproduces_its_counts_against_a_real_node,
    |session, fx, engine| {
        let _ = &fx;
        let spec = sit_table();
        ddl(&session, &spec.create_table_statement()).await;
        seed_sit_rows(&session).await;

        // `spark.cdm.feature.guardrail.colSizeInKB 1`, as SIT/features/05_guardrail sets it.
        let report = run_guardrail(&session, &spec, 1.0).await;

        // cdm.guardrailCheck.assert, byte for byte: READ 4, VALID 1, SKIPPED 0, LARGE 3.
        assert_eq!(total(&report, CounterKind::Read), 4, "{engine}");
        assert_eq!(total(&report, CounterKind::Valid), 1, "{engine}");
        assert_eq!(total(&report, CounterKind::Skipped), 0, "{engine}");
        assert_eq!(total(&report, CounterKind::Large), 3, "{engine}");
        assert_eq!(total(&report, CounterKind::PartitionsFailed), 0, "{engine}");
        assert_eq!(report.job(), JobKind::Guardrail);
    }
);

against_every_engine!(
    grd_003_a_run_that_found_oversized_columns_reports_a_finding_not_a_failure,
    |session, fx, engine| {
        let _ = &fx;
        let spec = sit_table();
        ddl(&session, &spec.create_table_statement()).await;
        seed_sit_rows(&session).await;

        // CLI-004: found something → DIFF → exit 1. Nothing failed; the run worked.
        let found = run_guardrail(&session, &spec, 1.0).await;
        assert_eq!(found.status(), RunStatus::Ended, "{engine}");
        assert_eq!(found.ranges_failed(), 0, "{engine}");
        assert_eq!(run_status(&found), RunStatus::Diff, "{engine}");

        // A threshold nothing reaches → ENDED → exit 0.
        let clean = run_guardrail(&session, &spec, 1_000.0).await;
        assert_eq!(total(&clean, CounterKind::Large), 0, "{engine}");
        assert_eq!(run_status(&clean), RunStatus::Ended, "{engine}");
    }
);

against_every_engine!(
    grd_002_a_row_over_the_threshold_is_large_and_one_under_it_is_valid,
    |session, fx, engine| {
        let _ = &fx;
        let spec = TableSpec::builder(KEYSPACE, "threshold_edge")
            .partition_key("key", cdm_codec::CqlTypeInfo::Text)
            .column("value", cdm_codec::CqlTypeInfo::Text)
            .build()
            .unwrap();
        ddl(&session, &spec.create_table_statement()).await;

        // ASCII, so one character is one serialised byte: 1000 is exactly at a 1 kB threshold and
        // 1001 is one byte over it. GRD-002's comparison is strictly greater than.
        for (key, len) in [("under", 1000_usize), ("over", 1001)] {
            ddl(
                &session,
                &format!(
                    "INSERT INTO {KEYSPACE}.threshold_edge (key, value) VALUES ('{key}', '{}')",
                    "x".repeat(len)
                ),
            )
            .await;
        }

        let lengths = measured_lengths(&session, &spec).await;
        let mut value_lengths: Vec<usize> = lengths.iter().map(|row| row[1]).collect();
        value_lengths.sort_unstable();
        assert_eq!(
            value_lengths,
            [1000, 1001],
            "{engine}: the node stored the sizes the case assumes"
        );

        let report = run_guardrail(&session, &spec, 1.0).await;
        assert_eq!(total(&report, CounterKind::Read), 2, "{engine}");
        assert_eq!(total(&report, CounterKind::Valid), 1, "{engine}");
        assert_eq!(total(&report, CounterKind::Large), 1, "{engine}");
    }
);

// =================================================================================================
// GRD-002 — every CQL type the engine has
// =================================================================================================

against_every_engine!(
    grd_002_every_supported_cql_type_is_measured_as_its_frame_length,
    |session, fx, engine| {
        // Collections, tuples, UDTs, frozen variants and — where the engine has them — vectors,
        // all in one table, gated by `Capabilities` and never by a version string.
        let capabilities = fx.capabilities();
        let spec = SchemaGen::all_types(KEYSPACE, "all_types", capabilities).unwrap();
        assert_eq!(
            spec.column("c_vector_float_3").is_some(),
            capabilities.vectors,
            "{engine}: vector<> must be present exactly when the engine supports it (CDC-004)"
        );

        let driver = DriverSession {
            session: Arc::clone(&session),
        };
        apply_schema(&driver, &spec).await.unwrap();

        let seed = Seed::from_env_or_entropy();
        let _guard = seed.report_on_panic();
        let mut generator = DataGen::with_options(
            seed,
            DataGenOptions::default()
                .with_null_probability(0.1)
                .with_collection_len(1, 6)
                .with_max_text_len(400),
        );
        let rows = generator.rows(&spec, 12).unwrap();
        seed_rows(&driver, &spec, &rows).await.unwrap();

        // What the guardrail *should* find, computed from the frame lengths independently of it.
        let threshold_bytes = 100.0_f64;
        let lengths = measured_lengths(&session, &spec).await;
        assert_eq!(
            lengths.len(),
            rows.len(),
            "{engine}: every seeded row read back"
        );
        #[expect(
            clippy::cast_precision_loss,
            reason = "lengths are far inside f64's exact range"
        )]
        let expected_large = lengths
            .iter()
            .filter(|row| row.iter().any(|len| *len as f64 > threshold_bytes))
            .count() as u64;

        let report = run_guardrail(&session, &spec, threshold_bytes / 1000.0).await;
        assert_eq!(
            total(&report, CounterKind::Read),
            rows.len() as u64,
            "{engine}"
        );
        assert_eq!(
            total(&report, CounterKind::Large),
            expected_large,
            "{engine}: the job's per-type accounting must equal the frame lengths, seed {seed}"
        );
        assert_eq!(
            total(&report, CounterKind::Read),
            total(&report, CounterKind::Large) + total(&report, CounterKind::Valid),
            "{engine}: every row counted exactly once"
        );

        // And the check itself, applied column by column to the same lengths, agrees with the
        // frame — which is the whole claim about size accounting across the type system.
        let guardrail = guardrail_for(&spec, threshold_bytes / 1000.0);
        for (index, row) in lengths.iter().enumerate() {
            let sizes = RowSizes::new(PrimaryKey::default(), row.clone());
            #[expect(clippy::cast_precision_loss, reason = "as above")]
            let over: Vec<&str> = spec
                .columns()
                .iter()
                .zip(row.iter())
                .filter(|(_, len)| **len as f64 > threshold_bytes)
                .map(|(column, _)| column.name())
                .collect();
            let reported: Vec<String> = guardrail
                .check(&sizes)
                .map(|finding| {
                    finding
                        .columns()
                        .iter()
                        .map(|column| column.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(reported, over, "{engine}: row {index}, seed {seed}");
        }
    }
);

// =================================================================================================
// GRD-001 — nothing is written
// =================================================================================================

against_every_engine!(
    grd_001_a_guardrail_run_leaves_the_origin_untouched,
    |session, fx, engine| {
        let _ = &fx;
        let spec = sit_table();
        ddl(&session, &spec.create_table_statement()).await;
        seed_sit_rows(&session).await;

        let before = measured_lengths(&session, &spec).await;
        let report = run_guardrail(&session, &spec, 1.0).await;
        assert_eq!(total(&report, CounterKind::Large), 3, "{engine}");
        let after = measured_lengths(&session, &spec).await;

        assert_eq!(
            before, after,
            "{engine}: a guardrail run must not change a single byte of the origin (GRD-001)"
        );
        // And nothing was created anywhere else in the keyspace: the job has no statement that
        // could have.
        let tables: Vec<String> = session
            .query_unpaged(
                "SELECT table_name FROM system_schema.tables WHERE keyspace_name = ?",
                (KEYSPACE,),
            )
            .await
            .unwrap()
            .into_rows_result()
            .unwrap()
            .rows::<(String,)>()
            .unwrap()
            .map(|row| row.unwrap().0)
            .collect();
        assert!(
            tables.iter().all(|table| table != "cdm_run_info"),
            "{engine}: a guardrail run wrote a tracking table it was never given: {tables:?}"
        );
    }
);

// =================================================================================================
// The testkit session seam
// =================================================================================================

/// A `scylla::Session` behind the testkit's session seam, as `cdm-cql`'s own suite defines it.
#[derive(Debug)]
struct DriverSession {
    session: Arc<Session>,
}

#[async_trait]
impl cdm_testkit::TestSession for DriverSession {
    async fn execute(&self, cql: &str) -> Result<Vec<cdm_testkit::TestRow>, CdmError> {
        self.session
            .query_unpaged(cql, &[])
            .await
            .map_err(|e| CdmError::new(ErrorKind::Read, format!("{e}")))?;
        Ok(Vec::new())
    }

    async fn await_schema_agreement(&self) -> Result<(), CdmError> {
        self.session
            .await_schema_agreement()
            .await
            .map_err(|e| CdmError::new(ErrorKind::SchemaMismatch, format!("{e}")))?;
        Ok(())
    }
}
