//! Connection, capability-probe and schema-introspection integration tests
//! (`CON-001`, `CON-002`, `CON-009`, `CON-013`, `SCH-001`, `SCH-002`, `SCH-010`, `CFG-020`).
//!
//! Everything here needs a real cluster: `system_schema` shapes, identifier folding, the local
//! datacenter and the capability rules are all claims about a server, and a unit test can only
//! assert what cdm-rs believes about them.
//!
//! Per `TST-102` these **skip** rather than fail when no container runtime is available, so
//! `cargo test --workspace` stays green on a laptop without Docker.
//!
//! ```text
//! CDM_IT_ENGINES=cassandra:5.0 cargo test -p cdm-cql --test connect_it -- --ignored --test-threads=1
//! ```
//!
//! # Why one container, one fixed port
//!
//! Like `driver_spike.rs` — whose setup this follows — the node is published on host port 9042
//! and told to advertise `127.0.0.1`. A containerised Cassandra otherwise advertises its Docker
//! bridge address in `system.local`, the control connection succeeds on the mapped port, and
//! every pooled connection is then refused because the host cannot route to the bridge network.
//!
//! A fixed port means this binary and `driver_spike` must not run at the same time. Both are
//! `#[ignore]`d for that reason, and both are meant to be run with `--test-threads=1`.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures
)]

use std::time::Duration;

use cdm_config::{CdmConfig, ValidationOptions};
use cdm_core::{Side, TableRef};
use cdm_cql::connect::{self, ConnectionMode};
use cdm_cql::schema::introspect;
use cdm_cql::schema::table::{ClusteringOrder, ColumnKind};
use cdm_cql::schema::SchemaSnapshot;
use scylla::client::session::Session;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// The CQL port, published on the host unchanged so the address the node advertises is reachable.
const CQL_PORT: u16 = 9042;

/// The keyspace every test in this file works in.
const KEYSPACE: &str = "cdm_it";

/// The image tag to run against. `CDM_IT_ENGINES` accepts the same `image:tag` list as the spike;
/// only Cassandra is meaningful here, since `system_schema` is what is under test.
fn cassandra_tag() -> String {
    let requested = std::env::var("CDM_IT_ENGINES").unwrap_or_default();
    requested
        .split(',')
        .filter_map(|spec| spec.trim().strip_prefix("cassandra:"))
        .map(str::to_owned)
        .next()
        .unwrap_or_else(|| "5.0".to_owned())
}

/// A running node plus a connected session.
struct Fixture {
    session: Session,
    tag: String,
    _container: ContainerAsync<GenericImage>,
}

/// Starts Cassandra, or returns `None` when no container runtime is available (`TST-102`).
async fn start() -> Option<Fixture> {
    let tag = cassandra_tag();

    // Materialized views are disabled in `cassandra.yaml` from 4.0 onwards, and the image exposes
    // no environment variable for the setting, so the flag is flipped in the file before the real
    // entrypoint runs. `SCH-010` is about rejecting a view as a target, which needs a view to
    // exist in the first place.
    let enable_views = "sed -i \
         's/^materialized_views_enabled: false/materialized_views_enabled: true/; \
          s/^enable_materialized_views: false/enable_materialized_views: true/' \
         \"${CASSANDRA_CONF:-/etc/cassandra}/cassandra.yaml\"; \
         exec docker-entrypoint.sh cassandra -f";

    let image = GenericImage::new("cassandra", &tag)
        .with_wait_for(WaitFor::message_on_stdout("Startup complete"))
        .with_startup_timeout(Duration::from_secs(300))
        .with_mapped_port(CQL_PORT, CQL_PORT.tcp())
        .with_env_var("CASSANDRA_BROADCAST_RPC_ADDRESS", "127.0.0.1")
        .with_cmd(["bash", "-c", enable_views]);

    let container = match image.start().await {
        Ok(container) => container,
        // `TST-102` says skip when there is no container runtime — not when there is one and the
        // port is taken. That second case means another test binary (`driver_spike` binds the
        // same port) or a leftover container holds 9042, and skipping it would report a green
        // suite that never ran, which is worse than no suite at all.
        Err(e) if e.to_string().contains("port is already allocated") => panic!(
            "host port {CQL_PORT} is already in use, so this suite cannot run: {e}\n\
             Both this binary and driver_spike bind it: run them one at a time, and check for a \
             leftover container with `docker ps`."
        ),
        Err(e) => {
            eprintln!(
                "skipping: cannot start cassandra:{tag} ({e}). Is a container runtime running?"
            );
            return None;
        }
    };

    // "Startup complete" is logged before the native transport binds on 4.1 and 5.0, so poll.
    let deadline = std::time::Instant::now() + Duration::from_secs(240);
    let session = loop {
        let attempt = scylla::client::session_builder::SessionBuilder::new()
            .known_node(format!("127.0.0.1:{CQL_PORT}"))
            .connection_timeout(Duration::from_secs(10))
            .build()
            .await;
        match attempt {
            Ok(session) => match session
                .query_unpaged("SELECT now() FROM system.local", &[])
                .await
            {
                Ok(_) => break session,
                Err(e) if std::time::Instant::now() < deadline => {
                    eprintln!("waiting for CQL: {e}");
                }
                Err(e) => panic!("cassandra:{tag} never became queryable: {e}"),
            },
            Err(e) if std::time::Instant::now() < deadline => eprintln!("waiting for CQL: {e}"),
            Err(e) => panic!("cassandra:{tag} never accepted a connection: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    };

    session
        .query_unpaged(
            format!(
                "CREATE KEYSPACE IF NOT EXISTS {KEYSPACE} WITH replication = \
                 {{'class': 'SimpleStrategy', 'replication_factor': 1}}"
            ),
            &[],
        )
        .await
        .expect("create keyspace");
    session.await_schema_agreement().await.expect("agreement");

    Some(Fixture {
        session,
        tag,
        _container: container,
    })
}

/// A configuration pointed at the container, for both sides (`CON-001`).
fn config() -> CdmConfig {
    let mut config = CdmConfig::default();
    for side in [&mut config.connect.origin, &mut config.connect.target] {
        "127.0.0.1".clone_into(&mut side.host);
        side.port = CQL_PORT;
    }
    config
}

macro_rules! integration {
    ($name:ident, |$fx:ident| $body:block) => {
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires a container runtime; run with --ignored --test-threads=1"]
        async fn $name() {
            let Some($fx) = start().await else {
                eprintln!("skipping {}: no container runtime", stringify!($name));
                return;
            };
            $body
        }
    };
}

// -------------------------------------------------------------------------------------------
// Connectivity
// -------------------------------------------------------------------------------------------

integration!(con_001_origin_and_target_connect_independently, |fx| {
    let _ = &fx;
    let mut config = config();
    config.perfops.consistency.read = cdm_config::types::ConsistencyLevel::One;
    config.perfops.consistency.write = cdm_config::types::ConsistencyLevel::LocalQuorum;

    let origin = connect::connect(&config, Side::Origin).await.unwrap();
    let target = connect::connect(&config, Side::Target).await.unwrap();

    assert_eq!(origin.side(), Side::Origin);
    assert_eq!(target.side(), Side::Target);
    assert_eq!(origin.mode(), &ConnectionMode::Plain);
    assert_eq!(origin.contact_points(), ["127.0.0.1:9042"]);

    // Both sessions work, and neither depends on the other.
    for session in [origin.session(), target.session()] {
        session
            .query_unpaged("SELECT release_version FROM system.local", &[])
            .await
            .unwrap();
    }
    drop(origin);
    target
        .session()
        .query_unpaged("SELECT release_version FROM system.local", &[])
        .await
        .expect("dropping one side must not disturb the other");
});

integration!(con_009_the_local_datacenter_is_auto_detected, |fx| {
    let session = connect::connect(&config(), Side::Origin).await.unwrap();
    assert_eq!(
        session.local_datacenter(),
        "datacenter1",
        "the single-node default datacenter must be detected from system.local"
    );

    let mut configured = config();
    configured.connect.origin.local_datacenter = Some("datacenter1".to_owned());
    let pinned = connect::connect(&configured, Side::Origin).await.unwrap();
    assert_eq!(pinned.local_datacenter(), "datacenter1");
    let _ = &fx;
});

integration!(con_013_the_capability_probe_describes_the_cluster, |fx| {
    let session = connect::connect(&config(), Side::Origin).await.unwrap();
    let capabilities = session.capabilities();

    assert_eq!(capabilities.flavour, cdm_cql::connect::Flavour::Cassandra);
    assert!(
        capabilities.release_version.starts_with(&fx.tag),
        "probed {} but the container is {}",
        capabilities.release_version,
        fx.tag
    );
    assert!(capabilities.partitioner.ends_with("Murmur3Partitioner"));
    assert_eq!(capabilities.datacenter, "datacenter1");
    assert!(!capabilities.cql_version.is_empty());
    assert!(capabilities.native_protocol_version.is_none());

    // The version-derived rules of CON-013, checked against what the server actually accepts.
    let vector_accepted = fx
        .session
        .query_unpaged(
            format!("CREATE TABLE {KEYSPACE}.probe_vec (k int PRIMARY KEY, v vector<float, 3>)"),
            &[],
        )
        .await
        .is_ok();
    assert_eq!(
        capabilities.supports_vectors, vector_accepted,
        "the probe's vector claim must match the server's behaviour"
    );
});

// -------------------------------------------------------------------------------------------
// Schema introspection
// -------------------------------------------------------------------------------------------

integration!(sch_001_table_metadata_is_read_from_system_schema, |fx| {
    fx.session
        .query_unpaged(
            format!(
                "CREATE TABLE {KEYSPACE}.orders (
                     account uuid, region text, created timestamp, seq int,
                     total decimal, tags set<text>, notes frozen<list<text>>,
                     label text static,
                     PRIMARY KEY ((account, region), created, seq)
                 ) WITH CLUSTERING ORDER BY (created DESC, seq ASC)"
            ),
            &[],
        )
        .await
        .unwrap();
    fx.session.await_schema_agreement().await.unwrap();

    let table = introspect::fetch_table(
        Side::Origin,
        &fx.session,
        &TableRef::new(KEYSPACE, "orders"),
    )
    .await
    .unwrap()
    .expect("the table exists");

    let partition: Vec<&str> = table
        .partition_key()
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(partition, vec!["account", "region"], "partition key order");

    let clustering: Vec<(&str, ClusteringOrder)> = table
        .clustering_columns()
        .iter()
        .map(|c| (c.name.as_str(), c.clustering_order))
        .collect();
    assert_eq!(
        clustering,
        vec![
            ("created", ClusteringOrder::Desc),
            ("seq", ClusteringOrder::Asc)
        ],
        "clustering order and direction"
    );

    assert_eq!(table.column("total").unwrap().cql_type, "decimal");
    assert_eq!(table.column("tags").unwrap().cql_type, "set<text>");
    assert!(table.column("tags").unwrap().is_collection());
    assert!(table.column("notes").unwrap().is_frozen());
    assert_eq!(table.column("label").unwrap().kind, ColumnKind::Static);
    assert!(!table.is_counter_table());
    assert!(!table.is_materialized_view);

    // A missing table is reported as absent rather than as an error.
    assert!(introspect::fetch_table(
        Side::Origin,
        &fx.session,
        &TableRef::new(KEYSPACE, "no_such_table")
    )
    .await
    .unwrap()
    .is_none());
    assert!(
        introspect::keyspace_exists(Side::Origin, &fx.session, KEYSPACE)
            .await
            .unwrap()
    );
    assert!(
        !introspect::keyspace_exists(Side::Origin, &fx.session, "no_such_keyspace")
            .await
            .unwrap()
    );
});

integration!(sch_001_a_counter_table_is_detected, |fx| {
    fx.session
        .query_unpaged(
            format!("CREATE TABLE {KEYSPACE}.hits (k text PRIMARY KEY, n counter)"),
            &[],
        )
        .await
        .unwrap();
    let table =
        introspect::fetch_table(Side::Origin, &fx.session, &TableRef::new(KEYSPACE, "hits"))
            .await
            .unwrap()
            .unwrap();
    assert!(table.is_counter_table());
});

integration!(
    sch_002_reserved_and_mixed_case_identifiers_round_trip,
    |fx| {
        // SIT `05_reserved_keyword`: a reserved word and a mixed-case name, both quoted.
        fx.session
            .query_unpaged(
                format!(
                    "CREATE TABLE {KEYSPACE}.\"Reserved_Words\" (
                     \"token\" text PRIMARY KEY, \"Data\" text, data text, \"we\"\"ird\" text)"
                ),
                &[],
            )
            .await
            .unwrap();
        fx.session.await_schema_agreement().await.unwrap();

        let table = introspect::fetch_table(
            Side::Origin,
            &fx.session,
            &TableRef::new(KEYSPACE, "Reserved_Words"),
        )
        .await
        .unwrap()
        .expect("a quoted, mixed-case table name must be found by its internal form");

        assert_eq!(
            table.quoted_name(),
            format!("{KEYSPACE}.\"Reserved_Words\"")
        );
        assert_eq!(table.column("token").unwrap().quoted_name(), "\"token\"");
        assert_eq!(table.column("Data").unwrap().quoted_name(), "\"Data\"");
        assert_eq!(table.column("data").unwrap().quoted_name(), "data");
        assert_eq!(
            table.column("we\"ird").unwrap().quoted_name(),
            "\"we\"\"ird\""
        );

        // The quoting is not merely self-consistent: the server accepts what it produces, and the
        // two case-differing columns really are different columns.
        let columns = table
            .columns
            .iter()
            .map(cdm_cql::schema::ColumnMeta::quoted_name)
            .collect::<Vec<_>>()
            .join(", ");
        fx.session
            .query_unpaged(
                format!(
                    "INSERT INTO {} (\"token\", \"Data\", data, \"we\"\"ird\") \
                 VALUES ('k', 'upper', 'lower', 'odd')",
                    table.quoted_name()
                ),
                &[],
            )
            .await
            .unwrap();
        let row = fx
            .session
            .query_unpaged(
                format!(
                    "SELECT {columns} FROM {} WHERE \"token\" = 'k'",
                    table.quoted_name()
                ),
                &[],
            )
            .await
            .unwrap()
            .into_rows_result()
            .unwrap()
            .first_row::<(String, String, String, String)>()
            .unwrap();
        assert_eq!(row.0, "k");
        assert_eq!(row.1, "upper");
        assert_eq!(row.2, "lower");
        assert_eq!(row.3, "odd");
    }
);

integration!(sch_010_a_materialized_view_is_rejected_as_a_target, |fx| {
    fx.session
        .query_unpaged(
            format!("CREATE TABLE {KEYSPACE}.mv_base (k int, c int, v text, PRIMARY KEY (k, c))"),
            &[],
        )
        .await
        .unwrap();
    let created = fx
        .session
        .query_unpaged(
            format!(
                "CREATE MATERIALIZED VIEW {KEYSPACE}.mv_view AS SELECT * FROM {KEYSPACE}.mv_base \
                 WHERE k IS NOT NULL AND c IS NOT NULL PRIMARY KEY (c, k)"
            ),
            &[],
        )
        .await;
    if created.is_err() {
        eprintln!(
            "skipping the live half of sch_010: this server refuses to create materialized views \
             ({created:?}); the rejection itself is covered by the unit tests"
        );
        return;
    }
    fx.session.await_schema_agreement().await.unwrap();

    let view = introspect::fetch_table(
        Side::Target,
        &fx.session,
        &TableRef::new(KEYSPACE, "mv_view"),
    )
    .await
    .unwrap()
    .expect("a view's columns live in system_schema.columns like a table's");
    assert!(view.is_materialized_view);

    let err = introspect::fetch_target_table(&fx.session, &TableRef::new(KEYSPACE, "mv_view"))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), cdm_core::ErrorKind::SchemaMismatch);
    assert!(err.to_string().contains("materialized view"), "{err}");

    // The base table is fine.
    assert!(
        introspect::fetch_target_table(&fx.session, &TableRef::new(KEYSPACE, "mv_base"))
            .await
            .unwrap()
            .is_some()
    );
});

// -------------------------------------------------------------------------------------------
// Tier-3 validation, which could not run until this crate existed
// -------------------------------------------------------------------------------------------

integration!(
    cfg_020_tier_three_validation_runs_against_the_live_schema,
    |fx| {
        fx.session
            .query_unpaged(
                format!("CREATE TABLE {KEYSPACE}.t3 (k text PRIMARY KEY, v text, n int)"),
                &[],
            )
            .await
            .unwrap();
        fx.session.await_schema_agreement().await.unwrap();

        let session = connect::connect(&config(), Side::Origin).await.unwrap();
        let snapshot = SchemaSnapshot::fetch(
            Side::Origin,
            session.session(),
            &[
                TableRef::new(KEYSPACE, "t3"),
                TableRef::new(KEYSPACE, "absent"),
            ],
        )
        .await
        .unwrap();

        let mut config = config();
        config.schema.origin.keyspace_table = Some(format!("{KEYSPACE}.t3"));
        config.schema.target.keyspace_table = Some(format!("{KEYSPACE}.t3"));
        let report = snapshot.validate(&config, ValidationOptions::default());
        assert!(
            report.tiers_run.contains(&cdm_config::Tier::SchemaBound),
            "tier 3 must have run"
        );
        assert!(report.is_valid(), "{:?}", report.diagnostics);

        // A column that does not exist is a Tier-3 error naming it.
        config.schema.origin.column.skip = vec!["no_such_column".to_owned()];
        let report = snapshot.validate(&config, ValidationOptions::default());
        assert!(!report.is_valid());

        // And an absent table is reported rather than crashing.
        config.schema.origin.column.skip.clear();
        config.schema.target.keyspace_table = Some(format!("{KEYSPACE}.absent"));
        let report = snapshot.validate(&config, ValidationOptions::default());
        assert!(!report.is_valid());
    }
);
