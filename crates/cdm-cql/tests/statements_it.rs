//! Statement construction and binding, against a real cluster.
//!
//! The unit tests in `cdm_cql::statement` prove the *shape* of the CQL and the *content* of a
//! binding. Only a node can prove the two claims that actually matter:
//!
//! | Claim | Test |
//! |---|---|
//! | A generated statement is accepted and prepares (`SCH-003`, `SCH-007`, `FEA-060`, `MIG-010`) | [`fea_060_every_generated_statement_prepares_on_a_real_node`] |
//! | An `UNSET` bind leaves an existing target value alone; a `NULL` bind would delete it (`MIG-012`) | [`mig_012_an_unset_bind_does_not_erase_the_target_value`] |
//! | An empty collection is `UNSET` too, and does not erase a populated target collection (`MIG-012`) | [`mig_012_an_empty_collection_is_unset_against_a_real_node`] |
//! | A row read as raw frame bytes binds without a copy and round-trips byte-identically (`MIG-040`) | [`mig_040_a_passthrough_row_round_trips_without_being_decoded`] |
//! | The virtual `TTL`/`WRITETIME` columns are selectable and land where the plan says (`SCH-007`) | [`sch_007_virtual_projection_columns_read_back_from_a_node`] |
//! | A counter target writes through the `UPDATE` form (`SCH-005`) | [`sch_005_a_counter_target_writes_through_the_update_form`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available.
//!
//! Run with `cargo xtask it`, or
//! `cargo test -p cdm-cql --test statements_it -- --ignored --test-threads=1`.

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

use cdm_codec::{CodecRegistry, Planner, PlannerOptions};
use cdm_cql::raw::RawRow;
use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
use cdm_cql::statement::{
    BindInputs, Binder, Bound, BoundValue, ColumnMapping, MappingOptions, MissingKeyPolicy,
    OriginProjection, OriginRangeSelect, StatementOptions, StatementSet, TargetSelectByPk,
    TargetUpsert, UsingClause,
};
use cdm_testkit::{skip_without_container_runtime, ClusterFixture, Engine};
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use scylla::statement::prepared::PreparedStatement;

/// The keyspace every case in this file uses.
const KEYSPACE: &str = "cdm_statements";

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
                let $session = connect(&$fx).await;
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

/// A column of the shape `cdm-cql`'s introspection produces.
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

/// `id int PRIMARY KEY, data text, tags set<text>` — the shape most cases here use.
fn simple(table: &str) -> TableSchema {
    schema(
        table,
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("data", "text", ColumnKind::Regular, -1),
            column("tags", "set<text>", ColumnKind::Regular, -1),
        ],
    )
}

fn planner() -> Planner {
    Planner::new(
        CodecRegistry::with_builtins(&[], None).unwrap(),
        PlannerOptions::default(),
    )
}

/// Builds every statement for a mapping, so a case asserts against the same values a run would.
fn statement_set(
    origin: &TableSchema,
    mapping: &ColumnMapping,
    projection: &OriginProjection,
    using: UsingClause,
) -> (StatementSet, TargetUpsert) {
    let upsert = TargetUpsert::new(mapping, StatementOptions { using }).unwrap();
    let set = StatementSet {
        origin_range_select: OriginRangeSelect::new(origin, projection, None, false)
            .cql()
            .to_owned(),
        origin_select_by_pk: cdm_cql::statement::OriginSelectByPk::new(origin, projection)
            .cql()
            .to_owned(),
        target_select_by_pk: TargetSelectByPk::new(mapping).unwrap().cql().to_owned(),
        target_upsert: upsert.cql().to_owned(),
    };
    (set, upsert)
}

async fn prepare(session: &Session, cql: &str) -> PreparedStatement {
    session
        .prepare(cql)
        .await
        .unwrap_or_else(|e| panic!("the node rejected the generated CQL `{cql}`: {e}"))
}

/// Reads one row of the origin as raw frame bytes, which is the shape `MIG-040` operates on.
async fn scan(session: &Session, select: &str) -> Vec<Vec<Option<Vec<u8>>>> {
    let result = session.query_unpaged(select, &[]).await.unwrap();
    let rows = result.into_rows_result().unwrap();
    rows.rows::<RawRow<'_, '_>>()
        .unwrap()
        .map(|row| {
            row.unwrap()
                .cells()
                .iter()
                .map(|cell| cell.bytes.map(<[u8]>::to_vec))
                .collect()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The statements prepare
// ---------------------------------------------------------------------------------------------

against_every_engine!(
    fea_060_every_generated_statement_prepares_on_a_real_node,
    |session, fx, engine| {
        let _ = &fx;
        let origin = simple("prep_src");
        let target = schema(
            "prep_dst",
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("payload", "text", ColumnKind::Regular, -1),
                column("tags", "set<text>", ColumnKind::Regular, -1),
                column("tenant", "text", ColumnKind::Regular, -1),
            ],
        );
        ddl(
            &session,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.prep_src \
                 (id int PRIMARY KEY, data text, tags set<text>)"
            ),
        )
        .await;
        ddl(
            &session,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.prep_dst \
                 (id int PRIMARY KEY, payload text, tags set<text>, tenant text)"
            ),
        )
        .await;

        let options = MappingOptions {
            rename: vec!["data:payload".to_owned()],
            constants: vec![("tenant".to_owned(), "'acme'".to_owned())],
            ..MappingOptions::default()
        };
        let mapping = ColumnMapping::resolve(&origin, &target, &options).unwrap();
        let projection = OriginProjection::new(
            mapping.origin_columns(),
            &["WRITETIME(data)".to_owned(), "TTL(data)".to_owned()],
        );
        let using = UsingClause {
            ttl: true,
            timestamp: true,
        };
        let (statements, _) = statement_set(&origin, &mapping, &projection, using);
        statements.log();

        for cql in [
            statements.origin_range_select.as_str(),
            statements.origin_select_by_pk.as_str(),
            statements.target_select_by_pk.as_str(),
            statements.target_upsert.as_str(),
        ] {
            let prepared = prepare(&session, cql).await;
            assert!(
                !prepared.get_statement().is_empty(),
                "{engine}: `{cql}` prepared to nothing"
            );
        }
    }
);

// ---------------------------------------------------------------------------------------------
// MIG-012 — UNSET, not NULL
// ---------------------------------------------------------------------------------------------

against_every_engine!(
    mig_012_an_unset_bind_does_not_erase_the_target_value,
    |session, fx, engine| {
        let _ = &fx;
        let table = simple("unset_scalar");
        ddl(
            &session,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.unset_scalar \
                 (id int PRIMARY KEY, data text, tags set<text>)"
            ),
        )
        .await;

        // The target already holds a value the migration must not destroy. If binding produced a
        // NULL, this row's `data` would become a tombstone and read back as null.
        ddl(
            &session,
            &format!("INSERT INTO {KEYSPACE}.unset_scalar (id, data) VALUES (1, 'pre-existing')"),
        )
        .await;

        let mapping = ColumnMapping::resolve(&table, &table, &MappingOptions::default()).unwrap();
        let projection = OriginProjection::new(mapping.origin_columns(), &[]);
        let upsert = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let prepared = prepare(&session, upsert.cql()).await;
        let binder = Binder::new(
            &mapping,
            upsert,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        // An origin row whose non-key columns are all null.
        let origin_row = cdm_core::Row::new(vec![
            cdm_core::RawCell::new(1i32.to_be_bytes().to_vec()),
            cdm_core::RawCell::NULL,
            cdm_core::RawCell::NULL,
        ]);
        let bound = binder.bind(&&origin_row, BindInputs::default()).unwrap();
        assert!(matches!(bound, Bound::Idempotent(_)));
        assert!(
            bound.values().values()[1].is_unset(),
            "{engine}: a null origin column must bind UNSET"
        );
        session
            .execute_unpaged(&prepared, bound.values())
            .await
            .unwrap();

        let (data,): (Option<String>,) = session
            .query_unpaged(
                format!("SELECT data FROM {KEYSPACE}.unset_scalar WHERE id = 1"),
                &[],
            )
            .await
            .unwrap()
            .into_rows_result()
            .unwrap()
            .single_row::<(Option<String>,)>()
            .unwrap();
        assert_eq!(
            data.as_deref(),
            Some("pre-existing"),
            "{engine}: the UNSET bind wrote a tombstone — MIG-012 is broken"
        );

        let _ = projection;
    }
);

against_every_engine!(
    mig_012_an_empty_collection_is_unset_against_a_real_node,
    |session, fx, engine| {
        let _ = &fx;
        let table = simple("unset_collection");
        ddl(
            &session,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.unset_collection \
                 (id int PRIMARY KEY, data text, tags set<text>)"
            ),
        )
        .await;
        ddl(
            &session,
            &format!(
                "INSERT INTO {KEYSPACE}.unset_collection (id, data, tags) \
                 VALUES (7, 'keep', {{'alpha','beta'}})"
            ),
        )
        .await;

        // Cassandra stores an empty collection as null, so the origin row's `tags` reads back as a
        // null cell — and an empty *serialised* collection is the other half of MIG-012. Both must
        // bind UNSET; this case covers the serialised-empty half directly.
        let mapping = ColumnMapping::resolve(&table, &table, &MappingOptions::default()).unwrap();
        let upsert = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let prepared = prepare(&session, upsert.cql()).await;
        let binder = Binder::new(
            &mapping,
            upsert,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let empty_set = 0i32.to_be_bytes().to_vec();
        let origin_row = cdm_core::Row::new(vec![
            cdm_core::RawCell::new(7i32.to_be_bytes().to_vec()),
            cdm_core::RawCell::new(b"keep".to_vec()),
            cdm_core::RawCell::new(empty_set),
        ]);
        let bound = binder.bind(&&origin_row, BindInputs::default()).unwrap();
        assert!(bound.values().values()[2].is_unset());
        session
            .execute_unpaged(&prepared, bound.values())
            .await
            .unwrap();

        let (tags,): (Option<Vec<String>>,) = session
            .query_unpaged(
                format!("SELECT tags FROM {KEYSPACE}.unset_collection WHERE id = 7"),
                &[],
            )
            .await
            .unwrap()
            .into_rows_result()
            .unwrap()
            .single_row::<(Option<Vec<String>>,)>()
            .unwrap();
        assert_eq!(
            tags.map(|mut t| {
                t.sort();
                t
            }),
            Some(vec!["alpha".to_owned(), "beta".to_owned()]),
            "{engine}: an empty collection erased the target's collection — MIG-012 is broken"
        );
    }
);

// ---------------------------------------------------------------------------------------------
// MIG-040 — the passthrough fast path survives binding
// ---------------------------------------------------------------------------------------------

against_every_engine!(
    mig_040_a_passthrough_row_round_trips_without_being_decoded,
    |session, fx, engine| {
        let _ = &fx;
        let origin = simple("pass_src");
        let target = simple("pass_dst");
        for name in ["pass_src", "pass_dst"] {
            ddl(
                &session,
                &format!(
                    "CREATE TABLE IF NOT EXISTS {KEYSPACE}.{name} \
                     (id int PRIMARY KEY, data text, tags set<text>)"
                ),
            )
            .await;
        }
        ddl(
            &session,
            &format!(
                "INSERT INTO {KEYSPACE}.pass_src (id, data, tags) \
                 VALUES (3, 'the quick brown fox', {{'x','y'}})"
            ),
        )
        .await;

        let mapping = ColumnMapping::resolve(&origin, &target, &MappingOptions::default()).unwrap();
        let projection = OriginProjection::new(mapping.origin_columns(), &[]);
        let select = OriginRangeSelect::new(&origin, &projection, None, false);
        let upsert = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        let prepared = prepare(&session, upsert.cql()).await;
        let binder = Binder::new(
            &mapping,
            upsert,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let read = prepare(&session, select.cql()).await;
        let result = session
            .execute_unpaged(&read, (i64::MIN, i64::MAX))
            .await
            .unwrap();
        let rows = result.into_rows_result().unwrap();
        let mut seen = 0usize;
        for row in rows.rows::<RawRow<'_, '_>>().unwrap() {
            let row = row.unwrap();
            let source = row.cell(1).unwrap().bytes.unwrap().as_ptr();

            let bound = binder.bind(&row, BindInputs::default()).unwrap();
            match &bound.values().values()[1] {
                BoundValue::Value(std::borrow::Cow::Borrowed(bytes)) => assert_eq!(
                    bytes.as_ptr(),
                    source,
                    "{engine}: the bind copied the frame slice; MIG-040's fast path was lost"
                ),
                other => panic!("{engine}: passthrough was lost; got {other:?}"),
            }
            session
                .execute_unpaged(&prepared, bound.values())
                .await
                .unwrap();
            seen += 1;
        }
        assert_eq!(seen, 1, "{engine}: the range scan found the seeded row");

        // And the bytes that never got decoded still mean the same thing on the other side.
        let origin_bytes = scan(
            &session,
            &format!("SELECT id,data,tags FROM {KEYSPACE}.pass_src"),
        )
        .await;
        let target_bytes = scan(
            &session,
            &format!("SELECT id,data,tags FROM {KEYSPACE}.pass_dst"),
        )
        .await;
        assert_eq!(
            origin_bytes, target_bytes,
            "{engine}: a passthrough migration must be byte-identical (TST-030)"
        );
    }
);

// ---------------------------------------------------------------------------------------------
// SCH-007 — the virtual projection columns
// ---------------------------------------------------------------------------------------------

against_every_engine!(
    sch_007_virtual_projection_columns_read_back_from_a_node,
    |session, fx, engine| {
        let _ = &fx;
        let origin = simple("virt_src");
        ddl(
            &session,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.virt_src \
                 (id int PRIMARY KEY, data text, tags set<text>)"
            ),
        )
        .await;
        ddl(
            &session,
            &format!(
                "INSERT INTO {KEYSPACE}.virt_src (id, data) VALUES (11, 'v') \
                 USING TTL 3600 AND TIMESTAMP 1700000000000000"
            ),
        )
        .await;

        let mapping = ColumnMapping::resolve(&origin, &origin, &MappingOptions::default()).unwrap();
        let projection = OriginProjection::new(
            mapping.origin_columns(),
            &["TTL(data)".to_owned(), "WRITETIME(data)".to_owned()],
        );
        assert_eq!(projection.virtual_index(0), Some(3));
        assert_eq!(projection.virtual_index(1), Some(4));

        let select = OriginRangeSelect::new(&origin, &projection, None, false);
        let read = prepare(&session, select.cql()).await;
        let result = session
            .execute_unpaged(&read, (i64::MIN, i64::MAX))
            .await
            .unwrap();
        let rows = result.into_rows_result().unwrap();
        let mut seen = 0usize;
        for row in rows.rows::<RawRow<'_, '_>>().unwrap() {
            let row = row.unwrap();
            assert_eq!(
                row.len(),
                projection.width(),
                "{engine}: the row is as wide as the projection says"
            );
            let ttl = row.cell(projection.virtual_index(0).unwrap()).unwrap();
            let writetime = row.cell(projection.virtual_index(1).unwrap()).unwrap();
            let ttl = i32::from_be_bytes(ttl.bytes.unwrap().try_into().unwrap());
            let writetime = i64::from_be_bytes(writetime.bytes.unwrap().try_into().unwrap());
            assert!(
                (1..=3600).contains(&ttl),
                "{engine}: TTL(data) landed at the wrong index, or is not an int: {ttl}"
            );
            assert_eq!(writetime, 1_700_000_000_000_000);
            seen += 1;
        }
        assert_eq!(seen, 1);
    }
);

// ---------------------------------------------------------------------------------------------
// SCH-005 — the counter write path
// ---------------------------------------------------------------------------------------------

against_every_engine!(
    sch_005_a_counter_target_writes_through_the_update_form,
    |session, fx, engine| {
        let _ = &fx;
        let table = schema(
            "counter_dst",
            vec![
                column("id", "int", ColumnKind::PartitionKey, 0),
                column("n", "counter", ColumnKind::Regular, -1),
            ],
        );
        ddl(
            &session,
            &format!(
                "CREATE TABLE IF NOT EXISTS {KEYSPACE}.counter_dst (id int PRIMARY KEY, n counter)"
            ),
        )
        .await;

        let mapping = ColumnMapping::resolve(&table, &table, &MappingOptions::default()).unwrap();
        let upsert = TargetUpsert::new(&mapping, StatementOptions::default()).unwrap();
        assert!(
            upsert.is_counter(),
            "{engine}: SCH-005 must detect the counter"
        );
        assert_eq!(
            upsert.cql(),
            format!("UPDATE {KEYSPACE}.counter_dst SET n=n+? WHERE id=?")
        );

        let prepared = prepare(&session, upsert.cql()).await;
        let binder = Binder::new(
            &mapping,
            upsert,
            &planner(),
            MissingKeyPolicy::default(),
            false,
        )
        .unwrap();

        let origin_row = cdm_core::Row::new(vec![
            cdm_core::RawCell::new(5i32.to_be_bytes().to_vec()),
            cdm_core::RawCell::new(9i64.to_be_bytes().to_vec()),
        ]);
        let bound = binder.bind(&&origin_row, BindInputs::default()).unwrap();
        match &bound {
            Bound::Counter(_) => {}
            Bound::Idempotent(_) => panic!("{engine}: a counter target produced a retryable write"),
        }
        session
            .execute_unpaged(&prepared, bound.values())
            .await
            .unwrap();

        // Read the counter back as raw bytes: the driver maps `counter` to its own newtype, and
        // the point here is the wire value the UPDATE produced, not the mapping.
        let read_back = scan(
            &session,
            &format!("SELECT n FROM {KEYSPACE}.counter_dst WHERE id = 5"),
        )
        .await;
        let n = i64::from_be_bytes(read_back[0][0].as_deref().unwrap().try_into().unwrap());
        assert_eq!(
            n, 9,
            "{engine}: the counter delta was applied exactly once — a retry would make it 18"
        );
    }
);
