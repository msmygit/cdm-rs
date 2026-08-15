//! The differential corpus loads into a real cluster and reads back (`TST-020`).
//!
//! Every other test of the corpus asserts on strings, and a string that looks like CQL is not CQL.
//! Only a cluster can say whether `set<tuple<int, text>>` is a type it accepts, whether `NaN` is a
//! float literal, whether an empty string is legal in a clustering position, and whether a counter
//! delta of `i64::MIN` can be written at all. Three of those four turned out to be yes and the
//! fourth turned out to be no — which is a gap the corpus now documents, and which no amount of
//! reading the CQL grammar had established.
//!
//! `cqlsh` inside the container, rather than a driver: `cdm-testkit` may not depend on `scylla`
//! (`ARCHITECTURE.md` §3), and
//! [`ClusterFixture::exec_cql`](cdm_testkit::ClusterFixture::exec_cql) is the seam that already
//! exists for exactly this.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cdm_testkit::differential::{Corpus, CorpusScale};
use cdm_testkit::{skip_without_container_runtime, ClusterFixture, Engine, Seed};

/// Loads `corpus` into a fresh keyspace on `engine` and returns the fixture, so a caller can read
/// it back.
async fn load(engine: &Engine, corpus: &Corpus) -> ClusterFixture {
    let fixture = ClusterFixture::start(engine)
        .await
        .unwrap_or_else(|e| panic!("cannot start {engine}: {e}"));

    // A failed statement is reported by `exec_cql` as an error carrying the statement, which is
    // the whole value of this test: an unloadable corpus says *which* literal a cluster rejected.
    fixture
        .exec_cql(&format!("DROP KEYSPACE IF EXISTS {};", corpus.keyspace()))
        .await
        .expect("the keyspace can be dropped");
    fixture
        .exec_cql(&corpus.load_script())
        .await
        .unwrap_or_else(|e| panic!("the corpus does not load: {e}"));
    fixture
}

/// The single number `cqlsh` printed for a `SELECT count(*)`.
fn count(output: &str) -> u64 {
    output
        .lines()
        .find_map(|line| line.trim().parse::<u64>().ok())
        .unwrap_or_else(|| panic!("no count in cqlsh output:\n{output}"))
}

#[tokio::test]
#[ignore = "needs a container runtime; run with `cargo xtask it`"]
async fn tst_020_the_full_corpus_loads_into_a_real_cluster_and_reads_back() {
    skip_without_container_runtime!();
    let seed = Seed::new(20);
    let _report_on_failure = seed.report_on_panic();

    let engine = Engine::cassandra("5.0");
    let corpus = Corpus::full(seed).expect("the corpus builds");
    let fixture = load(&engine, &corpus).await;

    // Every row landed. A collision on the primary key would silently collapse two rows into one,
    // and the counter block the harness compares is exactly this number.
    let rows = fixture
        .exec_cql(&format!("SELECT count(*) FROM {};", corpus.table()))
        .await
        .expect("the table can be counted");
    assert_eq!(count(&rows), corpus.row_count());

    let counters = corpus.counter_table().expect("a counter table");
    let rows = fixture
        .exec_cql(&format!(
            "SELECT count(*) FROM {};",
            counters.qualified_name()
        ))
        .await
        .expect("the counter table can be counted");
    assert_eq!(count(&rows), counters.row_count());

    // The two ends of the counter range survived as values, not as saturated nonsense.
    let counters = fixture
        .exec_cql(&format!("SELECT * FROM {};", counters.qualified_name()))
        .await
        .expect("the counters can be read");
    assert!(counters.contains("9223372036854775807"), "{counters}");
    assert!(counters.contains("-9223372036854775807"), "{counters}");
}

#[tokio::test]
#[ignore = "needs a container runtime; run with `cargo xtask it`"]
async fn mig_012_an_empty_frozen_collection_reads_back_and_an_empty_unfrozen_one_reads_as_null() {
    skip_without_container_runtime!();
    let seed = Seed::new(20);
    let _report_on_failure = seed.report_on_panic();

    let corpus = Corpus::smoke(seed).expect("the corpus builds");
    let fixture = load(&Engine::cassandra("5.0"), &corpus).await;

    // The empty row is the one whose `c_frozen_list_int` is `[]`. Reading it back is what proves
    // the claim the corpus documentation makes and that `MIG-012` rests on: an empty *non-frozen*
    // collection is stored as no cells and is therefore indistinguishable from NULL, while an
    // empty *frozen* one is a value. A migration that binds NULL where it meant an empty
    // collection writes a tombstone onto every row, and only a cluster can show the difference.
    let read_back = fixture
        .exec_cql(&format!(
            "SELECT c_list_int, c_frozen_list_int, c_map_text_int, c_frozen_map_text_int, \
             c_text, c_blob FROM {} WHERE c_frozen_list_int = [] ALLOW FILTERING;",
            corpus.table()
        ))
        .await
        .expect("the empty row can be read");

    assert!(
        read_back.contains("[]"),
        "the frozen empty list: {read_back}"
    );
    assert!(
        read_back.contains("{}"),
        "the frozen empty map: {read_back}"
    );
    assert!(
        read_back.contains("null"),
        "the unfrozen empty collections should read as null: {read_back}"
    );
    // An empty blob is a value and is not a null, on the same row.
    assert!(read_back.contains("0x"), "the empty blob: {read_back}");
}

#[tokio::test]
#[ignore = "needs a container runtime; run with `cargo xtask it`"]
async fn tst_020_writetime_is_selectable_for_exactly_the_columns_the_corpus_says_it_is() {
    skip_without_container_runtime!();
    let seed = Seed::new(20);
    let _report_on_failure = seed.report_on_panic();

    let corpus = Corpus::smoke(seed).expect("the corpus builds");
    let fixture = load(&Engine::cassandra("5.0"), &corpus).await;
    let table = corpus.tables().first().expect("the table under test");

    // The comparison engine builds its snapshot `SELECT` from exactly this, so if the eligibility
    // rule is wrong the comparator does not report a difference — it fails to run at all.
    //
    // Four columns at a time, not all of them at once. Measured on `cassandra:5.0.9`: eight
    // `WRITETIME`/`TTL` expressions in one `SELECT ... LIMIT 1` return in ten seconds and sixteen
    // do not return within ten minutes. That is a property of this `cqlsh` and not of anything
    // under test — the comparator speaks the native protocol — but a test that hangs is a test
    // nobody runs, so the batch size stays under the measured cliff.
    let eligible: Vec<&str> = table
        .value_columns()
        .iter()
        .filter(|column| column.timestamp_eligible())
        .map(|column| column.name())
        .collect();
    assert!(!eligible.is_empty());
    for batch in eligible.chunks(4) {
        let projection: Vec<String> = batch
            .iter()
            .map(|name| format!("WRITETIME({name}), TTL({name})"))
            .collect();
        fixture
            .exec_cql(&format!(
                "SELECT {} FROM {} LIMIT 1;",
                projection.join(", "),
                corpus.table()
            ))
            .await
            .unwrap_or_else(|e| panic!("WRITETIME is not selectable for {batch:?}: {e}"));
    }

    // The other direction, which is the half that would otherwise be an assumption. Each excluded
    // category is excluded for a *different* measured reason, and only one of the three is the
    // outright rejection the restriction is usually described as.
    for column in table.key_columns() {
        let outcome = fixture
            .exec_cql(&format!(
                "SELECT WRITETIME({}) FROM {} LIMIT 1;",
                column.name(),
                corpus.table()
            ))
            .await;
        let error = outcome.expect_err("a key column's writetime must be rejected");
        assert!(
            error.to_string().contains("PRIMARY KEY"),
            "unexpected rejection for {}: {error}",
            column.name()
        );
    }

    // A non-frozen collection is *accepted* and answers with a `list<bigint>` — one timestamp per
    // cell — which is a different type from every other column's answer and a length that depends
    // on the value. That, not a rejection, is why the corpus declines to compare it.
    let multi_cell = fixture
        .exec_cql(&format!(
            "SELECT WRITETIME(c_map_text_int) FROM {} WHERE pk = 'p0000' AND ck = 1 \
             ALLOW FILTERING;",
            corpus.table()
        ))
        .await
        .expect("a multi-cell writetime is selectable on this image");
    assert!(
        multi_cell.contains('['),
        "expected a list of per-cell writetimes: {multi_cell}"
    );

    // A counter is accepted too, and answers with the coordinator's clock: a value that differs
    // between two independently-executed runs by construction, because a counter update cannot
    // carry `USING TIMESTAMP`.
    let counters = corpus.counter_table().expect("a counter table");
    assert!(counters
        .value_columns()
        .iter()
        .all(|column| !column.timestamp_eligible()));
    fixture
        .exec_cql(&format!(
            "SELECT WRITETIME(c_hits), TTL(c_hits) FROM {} LIMIT 1;",
            counters.qualified_name()
        ))
        .await
        .expect("a counter writetime is selectable, which is why it needs excluding by policy");
}

#[tokio::test]
#[ignore = "needs a container runtime; run with `cargo xtask it`"]
async fn cdc_004_the_vector_column_loads_where_the_engine_has_vectors() {
    skip_without_container_runtime!();
    let seed = Seed::new(20);
    let _report_on_failure = seed.report_on_panic();

    let engine = Engine::cassandra("5.0");
    // What this node can actually do, rather than a hand-written capability set: `Capabilities::of`
    // is where the version arithmetic lives, and an image that gains or loses vectors changes this
    // test's coverage without changing this test.
    let corpus = Corpus::with_capabilities(seed, CorpusScale::Smoke, engine.capabilities())
        .expect("the corpus builds");
    assert!(
        engine.capabilities().vectors,
        "this test is pointless against an engine with no vectors"
    );
    let fixture = load(&engine, &corpus).await;

    // `Infinity` and `NaN` inside a `vector<float, 3>` are the reason this is a container test:
    // the column is a fixed-width array of floats, and a serialiser that rejected a non-finite
    // element would fail here and nowhere else.
    let read_back = fixture
        .exec_cql(&format!(
            "SELECT c_vector_float_3 FROM {} LIMIT 20;",
            corpus.table()
        ))
        .await
        .expect("the vector column can be read");
    assert!(read_back.contains("Infinity"), "{read_back}");
    assert!(read_back.contains("NaN"), "{read_back}");
}
