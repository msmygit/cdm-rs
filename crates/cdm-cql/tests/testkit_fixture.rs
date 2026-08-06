//! The `cdm-testkit` harness, exercised against real clusters (`TST-100`, `TST-101`, `TST-102`).
//!
//! `cdm-testkit` may not depend on the driver — only this crate may (`ARCHITECTURE.md` §3) — so
//! the harness produces CQL *statements* and leaves executing them to a
//! [`TestSession`](cdm_testkit::TestSession). This suite lives here because it is the first place
//! in the workspace where both halves exist at once: it implements `TestSession` over a
//! `scylla::Session` and proves that what the harness generates, a real node accepts.
//!
//! It is therefore also the acceptance test for the fixture itself:
//!
//! | Claim | Test |
//! |---|---|
//! | A fixture starts a node and hands back an address a driver can connect to (`TST-100`) | [`tst_100_the_fixture_hands_back_a_contact_point_a_driver_can_use`] |
//! | The generated all-types schema and data apply cleanly, gated by capability (`TST-100`, `CDC-004`) | [`tst_100_the_generated_schema_and_data_apply_to_a_real_cluster`] |
//! | The same seed reproduces the same rows, against a cluster and not just in memory (`TST-101`) | [`tst_101_the_same_seed_writes_the_same_rows_to_a_real_cluster`] |
//! | Origin and target run side by side on distinct ports (`TST-100`) | [`tst_100_an_origin_and_a_target_run_side_by_side`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available, so
//! `cargo test --workspace` stays green on a laptop without Docker.
//!
//! Run explicitly with `cargo xtask it`, or
//! `cargo test -p cdm-cql --test testkit_fixture -- --ignored --test-threads=1`.
//!
//! # One at a time
//!
//! The fixture publishes the CQL port on the host *unchanged*, because a containerised node
//! advertises its own port and address and a driver honours both. Two fixtures therefore cannot
//! share a port, and this suite must not run in parallel with itself: hence `--test-threads=1`,
//! which is what `cargo xtask it` and `.github/workflows/integration.yml` both pass.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
// `large_futures` fires on `SessionBuilder::build()`, which is the driver's own future.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures
)]

use std::time::Duration;

use async_trait::async_trait;
use cdm_core::{CdmError, ErrorKind};
use cdm_testkit::{
    apply_schema, seed_rows, skip_without_container_runtime, Capabilities, ClusterFixture, DataGen,
    Engine, OriginTarget, SchemaGen, Seed, TableSpec, TestRow, TestSession,
};
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;

/// The keyspace every case in this file uses.
const KEYSPACE: &str = "cdm_testkit";

/// A `scylla::Session` behind the testkit's session seam.
///
/// This is the whole of the "seam awaiting `cdm-cql`" that `TST-100` leaves open: once
/// `cdm-cql`'s own `SessionHandle` lands (PR #7), this impl moves into the crate proper and
/// nothing in `cdm-testkit` changes.
#[derive(Debug)]
struct DriverSession {
    session: Session,
}

#[async_trait]
impl TestSession for DriverSession {
    async fn execute(&self, cql: &str) -> Result<Vec<TestRow>, CdmError> {
        let result = self
            .session
            .query_unpaged(cql, &[])
            .await
            .map_err(|e| CdmError::new(ErrorKind::Read, format!("{e}")))?;

        // A DDL or write statement returns no rows at all, which is not an error.
        let Ok(rows) = result.into_rows_result() else {
            return Ok(Vec::new());
        };
        let names: Vec<String> = rows
            .column_specs()
            .iter()
            .map(|spec| spec.name().to_owned())
            .collect();

        let mut out = Vec::new();
        for row in rows
            .rows::<cdm_cql::raw::RawRow<'_, '_>>()
            .map_err(|e| CdmError::new(ErrorKind::Read, format!("{e}")))?
        {
            let row = row.map_err(|e| CdmError::new(ErrorKind::Read, format!("{e}")))?;
            let columns = row
                .cells()
                .iter()
                .enumerate()
                .map(|(index, cell)| {
                    let name = names
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| cell.name().to_owned());
                    (name, cell.bytes.map(<[u8]>::to_vec))
                })
                .collect();
            out.push(TestRow::new(columns));
        }
        Ok(out)
    }

    async fn await_schema_agreement(&self) -> Result<(), CdmError> {
        self.session
            .await_schema_agreement()
            .await
            .map_err(|e| CdmError::new(ErrorKind::SchemaMismatch, format!("{e}")))?;
        Ok(())
    }
}

impl DriverSession {
    /// Connects to a started fixture.
    async fn connect(fixture: &ClusterFixture) -> Self {
        let session = SessionBuilder::new()
            .known_node(fixture.contact_point())
            .connection_timeout(Duration::from_secs(10))
            .build()
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "the fixture reported {} ready but the driver could not connect: {e}",
                    fixture.contact_point()
                )
            });
        Self { session }
    }

    /// Counts the rows of a table.
    async fn count(&self, table: &TableSpec) -> i64 {
        let (count,): (i64,) = self
            .session
            .query_unpaged(
                format!("SELECT COUNT(*) FROM {}", table.qualified_name()),
                &[],
            )
            .await
            .unwrap()
            .into_rows_result()
            .unwrap()
            .single_row::<(i64,)>()
            .unwrap();
        count
    }
}

/// The engines this process exercises, from `CDM_IT_ENGINES`.
fn engines() -> Vec<Engine> {
    cdm_testkit::engines_under_test().expect("CDM_IT_ENGINES names an unknown engine")
}

/// Runs a body against every engine under test, skipping entirely without a container runtime.
///
/// The body is inlined rather than passed as a closure: a closure returning a future that borrows
/// its argument needs a higher-ranked bound the compiler cannot infer here.
macro_rules! against_every_engine {
    ($name:ident, |$fx:ident, $engine:ident| $body:block) => {
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires a container runtime; run with --ignored or via `cargo xtask it`"]
        async fn $name() {
            let _runtime = skip_without_container_runtime!();
            for $engine in engines() {
                let $fx = ClusterFixture::start(&$engine)
                    .await
                    .unwrap_or_else(|e| panic!("starting {}: {e}", $engine));
                $body
            }
        }
    };
}

// ---------------------------------------------------------------------------------------------
// TST-100 — the fixture itself
// ---------------------------------------------------------------------------------------------

against_every_engine!(
    tst_100_the_fixture_hands_back_a_contact_point_a_driver_can_use,
    |fx, engine| {
        // The fixture's readiness probe speaks the native protocol rather than merely opening a
        // socket, so "ready" must mean the driver connects on the first attempt — no retry loop
        // in the test. If this is ever flaky, the probe is wrong, not the test.
        let session = DriverSession::connect(&fx).await;

        assert_eq!(
            fx.host_port(),
            fx.native_port(),
            "{engine}: the host port must equal the node's own, or the address it advertises is \
             unreachable"
        );
        assert!(fx.contact_point().starts_with("127.0.0.1:"));

        // The node is genuinely serving, not merely listening.
        session
            .execute("SELECT now() FROM system.local")
            .await
            .unwrap();

        // And the capability query agrees with what the node actually implements (CDC-004).
        session
            .execute(&cdm_testkit::create_keyspace_statement(KEYSPACE))
            .await
            .unwrap();
        let vector_ddl = format!(
            "CREATE TABLE IF NOT EXISTS {KEYSPACE}.probe_vector (k text PRIMARY KEY, v vector<float, 3>)"
        );
        let accepted = session.execute(&vector_ddl).await.is_ok();
        assert_eq!(
            accepted,
            fx.supports_vectors(),
            "{engine}: supports_vectors() disagrees with the node; a test gating on it would \
             either skip needlessly or fail on DDL"
        );
    }
);

against_every_engine!(
    tst_100_the_generated_schema_and_data_apply_to_a_real_cluster,
    |fx, engine| {
        let seed = Seed::from_env_or_entropy();
        let _report_on_failure = seed.report_on_panic();
        let session = DriverSession::connect(&fx).await;

        // Capabilities come from the fixture, so the same test body covers 3.11 (no vectors) and
        // 5.0 (vectors) without a version check anywhere in it (TST-100).
        let capabilities = fx.capabilities();
        let table = SchemaGen::all_types(KEYSPACE, "all_types", capabilities).unwrap();
        apply_schema(&session, &table).await.unwrap();

        let rows = DataGen::new(seed).rows(&table, 25).unwrap();
        assert_eq!(seed_rows(&session, &table, &rows).await.unwrap(), 25);
        assert_eq!(
            session.count(&table).await,
            25,
            "{engine}: every row landed"
        );

        // Every generated value must be readable back, which is what proves the literals were not
        // merely syntactically accepted.
        let first = &rows[0];
        let read_back = session
            .execute(&format!(
                "SELECT * FROM {} WHERE {}",
                table.qualified_name(),
                first.primary_key_predicate(&table).unwrap()
            ))
            .await
            .unwrap();
        assert_eq!(
            read_back.len(),
            1,
            "{engine}: the row is addressable by its key"
        );
        for column in table.columns() {
            assert!(
                read_back[0].bytes(column.name()).is_some(),
                "{engine}: column {} came back null, so its generated literal did not round-trip",
                column.name()
            );
        }

        // A counter table is its own shape: UPDATE, not INSERT (MIG-030).
        let counters = SchemaGen::counters(KEYSPACE, "counters").unwrap();
        apply_schema(&session, &counters).await.unwrap();
        let counter_rows = DataGen::new(seed).rows(&counters, 5).unwrap();
        seed_rows(&session, &counters, &counter_rows).await.unwrap();
        assert_eq!(
            session.count(&counters).await,
            5,
            "{engine}: counters applied"
        );
    }
);

// ---------------------------------------------------------------------------------------------
// TST-101 — determinism, against a cluster rather than in memory
// ---------------------------------------------------------------------------------------------

against_every_engine!(
    tst_101_the_same_seed_writes_the_same_rows_to_a_real_cluster,
    |fx, engine| {
        let seed = Seed::new(20_260_806);
        let _report_on_failure = seed.report_on_panic();
        let session = DriverSession::connect(&fx).await;

        // The portable capability set, so the two tables are identical on every engine and the
        // comparison below is about the data and not about the schema.
        let first =
            SchemaGen::all_types(KEYSPACE, "determinism_a", Capabilities::portable()).unwrap();
        let second =
            SchemaGen::all_types(KEYSPACE, "determinism_b", Capabilities::portable()).unwrap();
        apply_schema(&session, &first).await.unwrap();
        apply_schema(&session, &second).await.unwrap();

        let rows_a = DataGen::new(seed).rows(&first, 10).unwrap();
        let rows_b = DataGen::new(seed).rows(&second, 10).unwrap();
        seed_rows(&session, &first, &rows_a).await.unwrap();
        seed_rows(&session, &second, &rows_b).await.unwrap();

        // Same seed, two tables: the stored bytes must agree column for column. This is stronger
        // than comparing the generated literals, because it also proves the literals mean the
        // same thing to the *node* — a generator that emitted an ambiguous timestamp would pass
        // an in-memory comparison and fail here.
        for row in &rows_a {
            let key = row.primary_key_predicate(&first).unwrap();
            let a = session
                .execute(&format!(
                    "SELECT * FROM {} WHERE {key}",
                    first.qualified_name()
                ))
                .await
                .unwrap();
            let b = session
                .execute(&format!(
                    "SELECT * FROM {} WHERE {key}",
                    second.qualified_name()
                ))
                .await
                .unwrap();
            assert_eq!(a.len(), 1, "{engine}: {key}");
            assert_eq!(
                a[0],
                b[0],
                "{engine}: the same seed produced different stored bytes ({})",
                seed.banner()
            );
        }
    }
);

// ---------------------------------------------------------------------------------------------
// TST-100 — the origin/target pair
// ---------------------------------------------------------------------------------------------

/// Two clusters at once, which is the shape every migration test needs.
///
/// Separate from the matrix above because two containers per engine is four containers on the
/// per-PR matrix; the newest Cassandra is enough to prove the pair works, and a version-skewed
/// pair is what [`OriginTarget::start_pair`] exists for.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a container runtime; run with --ignored or via `cargo xtask it`"]
async fn tst_100_an_origin_and_a_target_run_side_by_side() {
    let _runtime = skip_without_container_runtime!();
    let engine = engines()
        .into_iter()
        .next_back()
        .expect("CDM_IT_ENGINES selected no engines");

    let pair = OriginTarget::start(&engine)
        .await
        .unwrap_or_else(|e| panic!("starting an origin/target pair of {engine}: {e}"));

    assert_ne!(
        pair.origin().host_port(),
        pair.target().host_port(),
        "the two sides must publish different host ports"
    );

    let origin = DriverSession::connect(pair.origin()).await;
    let target = DriverSession::connect(pair.target()).await;

    // The two sides are genuinely distinct clusters: a table on one is not on the other.
    let table = SchemaGen::simple(KEYSPACE, "kv").unwrap();
    apply_schema(&origin, &table).await.unwrap();

    let rows = DataGen::new(Seed::new(1)).rows(&table, 3).unwrap();
    seed_rows(&origin, &table, &rows).await.unwrap();
    assert_eq!(origin.count(&table).await, 3);

    assert!(
        target
            .execute(&format!("SELECT * FROM {}", table.qualified_name()))
            .await
            .is_err(),
        "the target must not see the origin's schema; if it does, both fixtures are the same node"
    );

    // And the target is a working cluster in its own right.
    apply_schema(&target, &table).await.unwrap();
    assert_eq!(target.count(&table).await, 0);
    assert_eq!(
        pair.common_capabilities().vectors,
        engine.supports_vectors(),
        "a homogeneous pair supports what its engine supports"
    );
}
