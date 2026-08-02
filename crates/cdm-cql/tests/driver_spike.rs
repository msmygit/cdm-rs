//! Driver capability spike (PR #2, `CON-000`, `ADR-0002`).
//!
//! `docs/adr/0002-scylla-rust-driver.md` commits cdm-rs to `scylla-rust-driver` on the strength of
//! four claims. This suite exercises each of them against a real cluster, so the ADR rests on
//! evidence rather than on reading the crate docs:
//!
//! Cassandra 3.11 through 5.0 run on every pull request — that is what CDM migrates and where the
//! protocol risk lives. ScyllaDB runs nightly: the driver's own home turf, least likely to
//! regress, but a separate implementation whose token ownership and writetime behaviour diverge.
//!
//! | Claim | Test |
//! |---|---|
//! | Raw column bytes are reachable, so zero-copy passthrough is possible (`MIG-040`) | [`con_000_raw_column_bytes_are_reachable`] |
//! | `UNSET` can be bound, so nulls need not create tombstones (`MIG-012`) | [`con_000_unset_binding_does_not_create_a_tombstone`] |
//! | Every CQL type we must carry round-trips, including collections, UDTs and tuples (`CDC-001`, `CDC-002`) | [`con_000_all_cql_types_round_trip`] |
//! | Token-range scans and paging work as the engine needs them to (`FEA-060`, `ENG-003`) | [`con_000_token_range_scan_pages_and_covers_the_ring`] |
//!
//! Per `TST-102` these skip — rather than fail — when no container runtime is available, so
//! `cargo test --workspace` stays green on a laptop without Docker.
//!
//! Run explicitly with `cargo test -p cdm-cql --test driver_spike -- --ignored`.

// Tests may panic freely: a failed assertion is the reporting mechanism (see AGENTS.md).
// `large_futures` fires on `SessionBuilder::build()`, which is the driver's own future and not
// something a test can restructure; `similar_names` objects to `pager`/`paged`, which read fine.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::large_futures,
    clippy::similar_names
)]

use std::time::Duration;

use cdm_cql::raw::RawRow;
use scylla::client::session::Session;
use scylla::client::session_builder::SessionBuilder;
use scylla::value::{CqlValue, MaybeUnset, Row};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// The engines the spike runs against (`TST-002`).
///
/// Cassandra carries the risk: it is what CDM migrates, and 3.11 in particular exercises an older
/// protocol. It is therefore the per-PR matrix, covering every supported line.
///
/// ScyllaDB is the driver's home turf — `scylla-rust-driver` was chosen for its Rust maturity, and
/// Scylla is the case its maintainers test hardest, so it is the least likely to regress. It is
/// still a separate implementation whose token ownership (tablets), LWT and writetime behaviour
/// diverge, so it is covered nightly rather than dropped. See `ADR-0002`.
const CASSANDRA_VERSIONS: &[&str] = &["3.11", "4.0", "4.1", "5.0"];

/// Tags are `major.minor` so the matrix tracks the latest patch of each line.
const SCYLLA_VERSIONS: &[&str] = &["6.2"];

/// Which implementation an engine is, where behaviour genuinely differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Flavour {
    Cassandra,
    Scylla,
}

/// One container image to run the spike against.
#[derive(Clone, Copy, Debug)]
struct Engine {
    flavour: Flavour,
    tag: &'static str,
}

impl Engine {
    const fn cassandra(tag: &'static str) -> Self {
        Self {
            flavour: Flavour::Cassandra,
            tag,
        }
    }

    const fn scylla(tag: &'static str) -> Self {
        Self {
            flavour: Flavour::Scylla,
            tag,
        }
    }

    fn image(self) -> (&'static str, &'static str) {
        match self.flavour {
            Flavour::Cassandra => ("cassandra", self.tag),
            Flavour::Scylla => ("scylladb/scylla", self.tag),
        }
    }

    /// A log line that appears only once the node is accepting CQL.
    fn ready_message(self) -> &'static str {
        match self.flavour {
            Flavour::Cassandra => "Startup complete",
            Flavour::Scylla => "Starting listening for CQL clients",
        }
    }

    /// Whether this engine implements `vector<t, n>` (`CDC-004`).
    ///
    /// Cassandra introduced it in 5.0; ScyllaDB does not implement it at all.
    fn supports_vectors(self) -> bool {
        self.flavour == Flavour::Cassandra && self.tag.starts_with("5.")
    }
}

/// Engines to exercise in this process.
///
/// Four containers per test is slow on a laptop, so a bare local run covers the newest Cassandra
/// only. Override with `CDM_IT_ENGINES`:
///
/// - `cassandra` — every supported Cassandra line (the per-PR CI matrix)
/// - `scylla` — every supported Scylla line (the nightly CI job)
/// - `all` — both
/// - a comma-separated list of explicit `image:tag` pairs, e.g. `cassandra:4.0.4`, which is the
///   quickest way to run against an image already cached locally
fn engines_under_test() -> Vec<Engine> {
    fn leak(s: &str) -> &'static str {
        Box::leak(s.to_owned().into_boxed_str())
    }

    let cassandra = || CASSANDRA_VERSIONS.iter().copied().map(Engine::cassandra);
    let scylla = || SCYLLA_VERSIONS.iter().copied().map(Engine::scylla);

    match std::env::var("CDM_IT_ENGINES").ok().as_deref() {
        None => vec![Engine::cassandra("5.0")],
        Some("cassandra") => cassandra().collect(),
        Some("scylla") => scylla().collect(),
        Some("all") => cassandra().chain(scylla()).collect(),
        Some(list) => list
            .split(',')
            .filter_map(|spec| {
                let spec = spec.trim();
                let (image, tag) = spec.split_once(':')?;
                match image {
                    "cassandra" => Some(Engine::cassandra(leak(tag))),
                    "scylla" | "scylladb/scylla" => Some(Engine::scylla(leak(tag))),
                    _ => None,
                }
            })
            .collect(),
    }
}

/// Rows seeded for the token-range scan. Large enough to span several pages at the page size
/// used below, small enough to insert quickly on a cold container.
const ROWS: i32 = 500;

/// A running single-node cluster plus a connected session.
struct Fixture {
    session: Session,
    // Held solely to keep the container alive for the lifetime of the test.
    _container: ContainerAsync<GenericImage>,
}

/// Starts `engine` and connects, or returns `None` when no container runtime is available.
async fn start(engine: Engine) -> Option<Fixture> {
    let (name, tag) = engine.image();
    let image = GenericImage::new(name, tag)
        .with_wait_for(WaitFor::message_on_stdout(engine.ready_message()))
        .with_startup_timeout(Duration::from_secs(300));

    // Keep a single-node container inside a modest footprint; the defaults size for a server.
    let image = match engine.flavour {
        Flavour::Cassandra => image
            .with_env_var("HEAP_NEWSIZE", "128M")
            .with_env_var("MAX_HEAP_SIZE", "1024M"),
        Flavour::Scylla => image.with_cmd(["--smp", "1", "--skip-wait-for-gossip-to-settle", "0"]),
    };

    let container = match image.start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: cannot start {name}:{tag} ({e}). Is a container runtime running?");
            return None;
        }
    };

    let port = container.get_host_port_ipv4(9042.tcp()).await.ok()?;
    let session = SessionBuilder::new()
        .known_node(format!("127.0.0.1:{port}"))
        .connection_timeout(Duration::from_secs(30))
        .build()
        .await
        .expect("the container reported ready, so connecting must succeed");

    session
        .query_unpaged(
            "CREATE KEYSPACE IF NOT EXISTS spike WITH replication = \
             {'class': 'SimpleStrategy', 'replication_factor': 1}",
            &[],
        )
        .await
        .expect("create keyspace");
    session
        .await_schema_agreement()
        .await
        .expect("schema agreement");

    Some(Fixture {
        session,
        _container: container,
    })
}

/// Runs a test body against both engines, skipping entirely if no runtime is available.
///
/// The body is inlined rather than passed as a closure: a closure returning a future that borrows
/// its argument needs a higher-ranked bound the compiler cannot infer here.
macro_rules! spike {
    ($name:ident, |$fx:ident, $engine:ident| $body:block) => {
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "requires a container runtime; run with --ignored or via `cargo xtask it`"]
        async fn $name() {
            for $engine in engines_under_test() {
                let Some($fx) = start($engine).await else {
                    eprintln!("skipping {}: no container runtime", stringify!($name));
                    return;
                };
                $body
            }
        }
    };
}

// ---------------------------------------------------------------------------------------------
// Claim 1 — raw column bytes are reachable (MIG-040)
// ---------------------------------------------------------------------------------------------

spike!(con_000_raw_column_bytes_are_reachable, |fx, engine| {
    let s = &fx.session;
    s.query_unpaged(
        "CREATE TABLE IF NOT EXISTS spike.raw_access (k text PRIMARY KEY, t text, b blob, n int)",
        &[],
    )
    .await
    .unwrap();
    s.query_unpaged(
        "INSERT INTO spike.raw_access (k, t, b, n) VALUES ('a', 'hello', 0x00ff, 7)",
        &[],
    )
    .await
    .unwrap();
    // A row where the optional columns are genuinely null, to prove null and empty differ.
    s.query_unpaged("INSERT INTO spike.raw_access (k, t) VALUES ('b', '')", &[])
        .await
        .unwrap();

    let result = s
        .query_unpaged("SELECT k, t, b, n FROM spike.raw_access WHERE k = 'a'", &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let row: RawRow<'_, '_> = result
        .rows::<RawRow<'_, '_>>()
        .unwrap()
        .next()
        .unwrap()
        .unwrap();

    assert_eq!(row.len(), 4, "{engine:?}: projection order is preserved");
    assert_eq!(row.cell(0).unwrap().name(), "k");
    assert_eq!(row.cell(1).unwrap().bytes, Some(b"hello".as_slice()));
    assert_eq!(row.cell(2).unwrap().bytes, Some([0x00, 0xff].as_slice()));
    assert_eq!(
        row.cell(3).unwrap().bytes,
        Some(7i32.to_be_bytes().as_slice()),
        "{engine:?}: ints are big-endian on the wire, so passthrough is a byte copy"
    );
    assert_eq!(row.cell(1).unwrap().byte_len(), 5);

    // Null and empty must be distinguishable, or MIG-012 cannot be implemented correctly.
    let result = s
        .query_unpaged("SELECT t, b FROM spike.raw_access WHERE k = 'b'", &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let row: RawRow<'_, '_> = result
        .rows::<RawRow<'_, '_>>()
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(
        row.cell(0).unwrap().bytes,
        Some(b"".as_slice()),
        "{engine:?}: an empty string is Some(&[]), not None"
    );
    assert!(
        row.cell(1).unwrap().is_null(),
        "{engine:?}: an unwritten column is None"
    );
});

// ---------------------------------------------------------------------------------------------
// Claim 2 — UNSET binding avoids tombstones (MIG-012)
// ---------------------------------------------------------------------------------------------

spike!(
    con_000_unset_binding_does_not_create_a_tombstone,
    |fx, engine| {
        let s = &fx.session;
        s.query_unpaged(
            "CREATE TABLE IF NOT EXISTS spike.unset_bind (k text PRIMARY KEY, a text, b text)",
            &[],
        )
        .await
        .unwrap();

        let insert = s
            .prepare("INSERT INTO spike.unset_bind (k, a, b) VALUES (?, ?, ?)")
            .await
            .unwrap();

        s.execute_unpaged(&insert, ("k1", Some("first"), Some("second")))
            .await
            .unwrap();

        // Rewrite the row binding UNSET for `b`: the existing value must survive. Binding NULL
        // here would delete it and leave a tombstone, which is precisely what MIG-012 forbids.
        s.execute_unpaged(
            &insert,
            ("k1", MaybeUnset::Set("updated"), MaybeUnset::<&str>::Unset),
        )
        .await
        .unwrap();

        let result = s
            .query_unpaged("SELECT a, b FROM spike.unset_bind WHERE k = 'k1'", &[])
            .await
            .unwrap()
            .into_rows_result()
            .unwrap();
        let (a, b): (Option<String>, Option<String>) = result
            .rows::<(Option<String>, Option<String>)>()
            .unwrap()
            .next()
            .unwrap()
            .unwrap();

        assert_eq!(
            a.as_deref(),
            Some("updated"),
            "{engine:?}: set value applied"
        );
        assert_eq!(
            b.as_deref(),
            Some("second"),
            "{engine:?}: UNSET left the existing value untouched"
        );
    }
);

// ---------------------------------------------------------------------------------------------
// Claim 3 — every CQL type we must carry round-trips (CDC-001, CDC-002)
// ---------------------------------------------------------------------------------------------

spike!(con_000_all_cql_types_round_trip, |fx, engine| {
    let s = &fx.session;
    s.query_unpaged(
        "CREATE TYPE IF NOT EXISTS spike.addr (street text, zip int)",
        &[],
    )
    .await
    .unwrap();
    s.query_unpaged(
        "CREATE TABLE IF NOT EXISTS spike.types (
            k            text PRIMARY KEY,
            c_ascii      ascii,
            c_bigint     bigint,
            c_blob       blob,
            c_boolean    boolean,
            c_date       date,
            c_decimal    decimal,
            c_double     double,
            c_duration   duration,
            c_float      float,
            c_inet       inet,
            c_int        int,
            c_smallint   smallint,
            c_text       text,
            c_time       time,
            c_timestamp  timestamp,
            c_timeuuid   timeuuid,
            c_tinyint    tinyint,
            c_uuid       uuid,
            c_varint     varint,
            c_list       list<int>,
            c_set        set<text>,
            c_map        map<text, int>,
            c_tuple      tuple<int, text>,
            c_udt        frozen<addr>,
            c_nested     map<text, frozen<list<frozen<addr>>>>
        )",
        &[],
    )
    .await
    .unwrap();
    s.await_schema_agreement().await.unwrap();

    s.query_unpaged(
        "INSERT INTO spike.types (
            k, c_ascii, c_bigint, c_blob, c_boolean, c_date, c_decimal, c_double, c_duration,
            c_float, c_inet, c_int, c_smallint, c_text, c_time, c_timestamp, c_timeuuid,
            c_tinyint, c_uuid, c_varint, c_list, c_set, c_map, c_tuple, c_udt, c_nested
         ) VALUES (
            'row', 'ascii', 9223372036854775807, 0xdeadbeef, true, '2024-02-29',
            3.14159265358979323846, 1.7976931348623157E308, 89h4m48s, 3.4028235E38,
            '2001:db8::1', 2147483647, 32767, 'unicode ✓ 日本語', '13:30:54.234',
            '2024-02-29T13:30:54.234Z', 8e14e360-0000-11ee-0000-000000000000, 127,
            123e4567-e89b-12d3-a456-426614174000, 123456789012345678901234567890,
            [1, 2, 3], {'a', 'b'}, {'x': 1, 'y': 2}, (7, 'seven'),
            {street: 'Main', zip: 12345},
            {'k1': [{street: 'A', zip: 1}, {street: 'B', zip: 2}]}
         )",
        &[],
    )
    .await
    .unwrap();

    let result = s
        .query_unpaged("SELECT * FROM spike.types WHERE k = 'row'", &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let row: RawRow<'_, '_> = result
        .rows::<RawRow<'_, '_>>()
        .unwrap()
        .next()
        .unwrap()
        .unwrap();

    // Every column must be readable as raw bytes and as a typed value. If either fails for a
    // type, that type cannot be migrated, which is a blocking finding for ADR-0002.
    assert_eq!(row.len(), 26, "{engine:?}: all columns returned");
    for cell in row.cells() {
        assert!(
            !cell.is_null(),
            "{engine:?}: column {} came back null; the literal above did not round-trip",
            cell.name()
        );
    }

    let typed = s
        .query_unpaged("SELECT * FROM spike.types WHERE k = 'row'", &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let row: Row = typed.rows::<Row>().unwrap().next().unwrap().unwrap();
    assert!(
        row.columns.iter().all(Option::is_some),
        "{engine:?}: every column deserializes into a CqlValue"
    );
});

// ---------------------------------------------------------------------------------------------
// Claim 3b — vector<float, N> (CDC-004)
// ---------------------------------------------------------------------------------------------

/// `SIT/smoke/06_vector` migrates a `vector<float, 3>` column, so cdm-rs must handle the type.
/// Only Cassandra 5.0 implements it; earlier lines are skipped rather than failed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a container runtime; run with --ignored or via `cargo xtask it`"]
async fn cdc_004_vector_type_round_trips() {
    let Some(engine) = engines_under_test()
        .into_iter()
        .find(|e: &Engine| e.supports_vectors())
    else {
        eprintln!("skipping cdc_004_vector_type_round_trips: no engine under test has vectors");
        return;
    };
    let Some(fx) = start(engine).await else {
        eprintln!("skipping cdc_004_vector_type_round_trips: no container runtime");
        return;
    };
    let s = &fx.session;

    s.query_unpaged(
        "CREATE TABLE IF NOT EXISTS spike.vectors (k text PRIMARY KEY, v vector<float, 3>)",
        &[],
    )
    .await
    .unwrap();
    s.query_unpaged(
        "INSERT INTO spike.vectors (k, v) VALUES ('v1', [1.5, 2.5, 3.5])",
        &[],
    )
    .await
    .unwrap();

    let result = s
        .query_unpaged("SELECT v FROM spike.vectors WHERE k = 'v1'", &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let row: RawRow<'_, '_> = result
        .rows::<RawRow<'_, '_>>()
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let cell = row.cell(0).unwrap();

    assert!(!cell.is_null(), "vector column read back");
    assert_eq!(
        cell.byte_len(),
        12,
        "three f32 elements are a contiguous 12-byte array, so passthrough is a byte copy"
    );

    let typed = s
        .query_unpaged("SELECT v FROM spike.vectors WHERE k = 'v1'", &[])
        .await
        .unwrap()
        .into_rows_result()
        .unwrap();
    let (value,): (Option<CqlValue>,) = typed
        .rows::<(Option<CqlValue>,)>()
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    match value {
        Some(CqlValue::Vector(elements)) => {
            assert_eq!(elements.len(), 3, "dimensions preserved");
            assert_eq!(elements[0], CqlValue::Float(1.5));
        }
        other => panic!("expected CqlValue::Vector, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Claim 4 — token-range scans and paging (FEA-060, ENG-003, TOK-001)
// ---------------------------------------------------------------------------------------------

spike!(
    con_000_token_range_scan_pages_and_covers_the_ring,
    |fx, engine| {
        use futures::StreamExt;

        let s = &fx.session;
        s.query_unpaged(
            "CREATE TABLE IF NOT EXISTS spike.ring (k int PRIMARY KEY, v text)",
            &[],
        )
        .await
        .unwrap();

        let insert = s
            .prepare("INSERT INTO spike.ring (k, v) VALUES (?, ?)")
            .await
            .unwrap();
        for k in 0..ROWS {
            s.execute_unpaged(&insert, (k, format!("value-{k}")))
                .await
                .unwrap();
        }

        // The partitioner must be discoverable, or the planner cannot choose token bounds
        // (TOK-001).
        assert!(
            s.get_cluster_state()
                .keyspaces_iter()
                .any(|(name, _)| name == "spike"),
            "{engine:?}: cluster metadata exposes the keyspace, so the planner can read the \
             partitioner and schema (TOK-001, SCH-001)"
        );

        // A full-ring scan, split in two, must return every row exactly once. This is the shape
        // of the origin select in FEA-060.
        let scan = s
            .prepare(
                "SELECT k FROM spike.ring WHERE token(k) >= ? AND token(k) <= ? \
                 BYPASS CACHE",
            )
            .await;
        // `BYPASS CACHE` is Scylla-only; fall back to the portable form.
        let scan = match scan {
            Ok(p) => p,
            Err(_) => s
                .prepare("SELECT k FROM spike.ring WHERE token(k) >= ? AND token(k) <= ?")
                .await
                .unwrap(),
        };

        let midpoint = 0i64;
        let mut seen = std::collections::BTreeSet::new();
        for (lo, hi) in [(i64::MIN, midpoint), (midpoint + 1, i64::MAX)] {
            let mut pager = s
                .execute_iter(scan.clone(), (lo, hi))
                .await
                .unwrap()
                .rows_stream::<(i32,)>()
                .unwrap();
            while let Some(row) = pager.next().await {
                let (k,) = row.unwrap();
                assert!(seen.insert(k), "{engine:?}: {k} returned by two ranges");
            }
        }
        assert_eq!(
            seen.len(),
            ROWS as usize,
            "{engine:?}: two adjacent token ranges cover the ring exactly once"
        );

        // Paging must be driven by page size, not by materialising the whole result (ENG-003).
        let mut paged = scan.clone();
        paged.set_page_size(37);
        let mut pager = s
            .execute_iter(paged, (i64::MIN, i64::MAX))
            .await
            .unwrap()
            .rows_stream::<(i32,)>()
            .unwrap();
        let mut counted = 0usize;
        while let Some(row) = pager.next().await {
            row.unwrap();
            counted += 1;
        }
        assert_eq!(
            counted, ROWS as usize,
            "{engine:?}: a small page size still streams every row"
        );
    }
);
