//! Per-row feature-pipeline benchmarks (`TST-060`, `NFR-004`).
//!
//! Every feature in this crate runs once per migrated row, between the origin read and the target
//! write, so its cost is multiplied by the row count of the whole migration — a run of a billion
//! rows turns a wasted microsecond per row into a wasted quarter of an hour. That is also why
//! `ARCHITECTURE.md` §5.5 splits each feature into a configuration type and a `…Plan`: the plan is
//! resolved once at startup and the per-row path is supposed to be nothing but a positional lookup
//! and an already-chosen conversion. These benchmarks are what keeps that claim honest — they
//! measure the `…Plan` side, so a regression here is a regression in the migration's throughput
//! rather than in its startup time.
//!
//! The two numbers worth watching are the *baseline*, where nothing is configured and the pipeline
//! should be indistinguishable from not having one, and *extract JSON*, which is the only feature
//! that parses and is therefore expected to dominate any pipeline it is enabled in.

// A benchmark is test code: fixtures are known-good and a failed setup should abort loudly rather
// than be threaded through `Result`. The no-panic rule (`ERR-004`) protects production paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;
use std::sync::Arc;

use cdm_codec::{CodecRegistry, Codecset, Planner, PlannerOptions};
use cdm_core::{
    BindingBuilder, EffectiveConfig, FeaturePlugin, FilterPlugin, PrimaryKey, RawCell, Record, Row,
    TableRef,
};
use cdm_feature::{
    table_view, ColumnValueFilter, ConstantColumns, ExplodeMap, ExtractJson, FeatureSchema,
    FilterChain, TableFacts,
};

/// The configuration shape every feature loads from, built from canonical keys.
fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
    pairs.iter().copied().collect()
}

/// A record with no primary key and the given origin cells.
///
/// The key is empty because none of the benchmarked paths reads it except the error path of
/// `extract_record`, which these benchmarks never take.
fn record(cells: Vec<RawCell>) -> Record {
    Record::new(PrimaryKey::default(), Row::new(cells))
}

/// The planner the explode plan converts entries with.
///
/// `IntString` is the codec set a real run most often enables; the conversions these benchmarks
/// then plan are identity, which is the common case and the one whose overhead is worth knowing.
fn planner() -> Planner {
    Planner::new(
        CodecRegistry::with_builtins(&[Codecset::IntString], None).unwrap(),
        PlannerOptions::default(),
    )
}

/// The cost of a pipeline with nothing configured, which is the default for every migration.
///
/// This is the number that decides whether features are pay-for-what-you-use. Almost every run
/// enables none of them, so if the empty chain or a disabled feature costs anything measurable it
/// is a tax on users who never asked for the functionality — and unlike the other benchmarks here
/// there is no work being done to justify it.
fn tst_060_no_features_baseline(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_no_features_baseline");

    let chain = FilterChain::new();
    let row = record(vec![
        RawCell::new(1_i32.to_be_bytes().to_vec()),
        RawCell::NULL,
    ]);
    group.bench_function("empty_filter_chain", |b| {
        b.iter(|| chain.accepts(black_box(&row)).unwrap());
    });

    let off = ConstantColumns::load(&EffectiveConfig::new()).unwrap();
    group.bench_function("disabled_constant_columns", |b| {
        b.iter(|| {
            let mut binding = BindingBuilder::new();
            off.extend_target_binding(black_box(&mut binding));
            binding
        });
    });

    group.finish();
}

/// The cost of injecting constant columns into a row's target binding (`FEA-010`).
///
/// Two halves are measured separately because only one of them is on the hot path. `resolve` parses
/// each literal against the target column's type and belongs at startup; `extend_target_binding`
/// runs per row and should be nothing but string clones. Their ratio is the evidence that the
/// startup/hot-path split is actually where the documentation says it is — if `resolve` were ever
/// pulled into the row loop, this pair of numbers is what would show the damage.
fn tst_060_constant_columns(c: &mut Criterion) {
    let target = TableFacts::from_view(
        &table_view(
            TableRef::new("ks", "dst"),
            &[("id", "int"), ("tenant", "text"), ("n", "int")],
        ),
        &["id", "tenant"],
    )
    .unwrap();
    let feature = ConstantColumns::load(&config(&[
        ("feature.constant_columns.names", "tenant,n"),
        ("feature.constant_columns.values", "'acme',1234"),
    ]))
    .unwrap();

    let mut group = c.benchmark_group("tst_060_constant_columns");
    group.throughput(Throughput::Elements(2));
    group.bench_function("extend_target_binding", |b| {
        b.iter(|| {
            let mut binding = BindingBuilder::new();
            feature.extend_target_binding(black_box(&mut binding));
            binding
        });
    });
    group.bench_function("resolve", |b| {
        b.iter(|| feature.resolve(black_box(&target)).unwrap());
    });
    group.finish();
}

/// A JSON document with `fields` filler properties, plus the two properties the paths address.
///
/// `city` sits at the top level and `/address/city` nested beneath it, so the same bytes can be read
/// through both a field name and a JSON Pointer — which is what isolates path resolution from the
/// parse that precedes it.
fn json_document(fields: usize) -> String {
    let mut address = serde_json::Map::new();
    address.insert(
        "city".to_owned(),
        serde_json::Value::String("Springfield".to_owned()),
    );
    let mut document = serde_json::Map::new();
    document.insert(
        "city".to_owned(),
        serde_json::Value::String("Springfield".to_owned()),
    );
    document.insert("address".to_owned(), serde_json::Value::Object(address));
    for index in 0..fields {
        document.insert(
            format!("filler_{index}"),
            serde_json::Value::String(format!("value-{index}")),
        );
    }
    serde_json::Value::Object(document).to_string()
}

/// The cost of promoting one JSON property to a column (`FEA-030`, `FEA-035`).
///
/// This is the only feature that parses, and it parses the *whole* document to read one property of
/// it — `serde_json` has no way to stop early — so the cost is linear in the document's size no
/// matter how shallow the mapping is. The sweep over filler-field counts is there to make that
/// linearity visible: a run whose origin column holds large documents pays for every byte of them
/// on every row, which is the argument for `exclusive` (`FEA-033`) narrowing the read.
///
/// Throughput is reported in bytes rather than rows because the per-row figure is meaningless
/// without the document size that produced it.
fn tst_060_extract_json(c: &mut Criterion) {
    let schema = FeatureSchema::new(
        TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "src"),
                &[("id", "int"), ("doc", "text")],
            ),
            &["id"],
        )
        .unwrap(),
        TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "dst"),
                &[("id", "int"), ("city", "text")],
            ),
            &["id"],
        )
        .unwrap(),
    );
    let field_plan = ExtractJson::load(&config(&[
        ("feature.extract_json.origin_column", "doc"),
        ("feature.extract_json.property_mapping", "city"),
    ]))
    .resolve(&schema)
    .unwrap();
    let pointer_plan = ExtractJson::load(&config(&[
        ("feature.extract_json.origin_column", "doc"),
        (
            "feature.extract_json.property_mapping",
            "/address/city:city",
        ),
    ]))
    .resolve(&schema)
    .unwrap();

    let mut group = c.benchmark_group("tst_060_extract_json");
    for fields in [0_usize, 16, 256] {
        let cell = RawCell::new(json_document(fields).into_bytes());
        group.throughput(Throughput::Bytes(u64::try_from(cell.len()).unwrap()));
        group.bench_with_input(BenchmarkId::new("field", fields), &cell, |b, cell| {
            b.iter(|| field_plan.extract(black_box(cell)).unwrap());
        });
        group.bench_with_input(BenchmarkId::new("pointer", fields), &cell, |b, cell| {
            b.iter(|| pointer_plan.extract(black_box(cell)).unwrap());
        });
    }
    group.finish();
}

/// The cost of deciding whether a row is this run's business (`FEA-052`, `FEA-054`).
///
/// The filter chain runs before any other per-row work, on *every* row including the ones it
/// rejects, so it is the one feature whose cost is paid even when it saves work. Three shapes are
/// measured: an accepted row, which pays for the whole chain; a rejected row, which should exit at
/// the first filter that says no; and a four-filter chain, whose gap from the one-filter case is the
/// per-filter dynamic-dispatch overhead — the price of `FilterChain` holding `Arc<dyn FilterPlugin>`
/// rather than a closed enum.
fn tst_060_filter_chain(c: &mut Criterion) {
    let origin = TableFacts::from_view(
        &table_view(
            TableRef::new("ks", "src"),
            &[("id", "int"), ("status", "text"), ("v", "text")],
        ),
        &["id"],
    )
    .unwrap();
    let filter = || {
        Arc::new(ColumnValueFilter::load(
            &config(&[
                ("filter.column.name", "status"),
                ("filter.column.value", "deleted"),
            ]),
            &origin,
        )) as Arc<dyn FilterPlugin>
    };

    let one = FilterChain::new().with(filter());
    let four = FilterChain::new()
        .with(filter())
        .with(filter())
        .with(filter())
        .with(filter());
    let cells = |status: &str| {
        vec![
            RawCell::new(7_i32.to_be_bytes().to_vec()),
            RawCell::new(status.as_bytes().to_vec()),
            RawCell::new(b"payload".to_vec()),
        ]
    };
    let accepted = record(cells("active"));
    let rejected = record(cells("deleted"));

    let mut group = c.benchmark_group("tst_060_filter_chain");
    group.bench_function("accept", |b| {
        b.iter(|| one.accepts(black_box(&accepted)).unwrap());
    });
    group.bench_function("reject", |b| {
        b.iter(|| one.accepts(black_box(&rejected)).unwrap());
    });
    group.bench_function("accept_chain_of_four", |b| {
        b.iter(|| four.accepts(black_box(&accepted)).unwrap());
    });
    group.finish();
}

/// A serialised `map<text, int>` in the protocol v4+ framing `wire::map_entries` decodes.
fn map_cell(entries: usize) -> RawCell {
    let mut out = i32::try_from(entries).unwrap().to_be_bytes().to_vec();
    for index in 0..entries {
        let key = format!("key-{index:04}");
        out.extend_from_slice(&i32::try_from(key.len()).unwrap().to_be_bytes());
        out.extend_from_slice(key.as_bytes());
        out.extend_from_slice(&4_i32.to_be_bytes());
        out.extend_from_slice(&i32::try_from(index).unwrap().to_be_bytes());
    }
    RawCell::new(out)
}

/// The cost of turning one origin row into one target row per map entry (`FEA-020`, `FEA-021`).
///
/// Explode is the only feature whose per-row cost is not bounded by the schema: it is bounded by the
/// *data*, because a map with a thousand entries decodes a thousand length-prefixed pairs and
/// allocates a `RawCell` for each half of each one. Sweeping the entry count is therefore the only
/// way to state its cost at all, and the per-element throughput figure is what a capacity plan needs
/// — a table whose maps average 256 entries is doing 256 times the work per row that the row count
/// suggests.
fn tst_060_explode_map(c: &mut Criterion) {
    let schema = FeatureSchema::new(
        TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "src"),
                &[("id", "int"), ("m", "map<text, int>")],
            ),
            &["id"],
        )
        .unwrap(),
        TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "dst"),
                &[("id", "int"), ("k", "text"), ("v", "int")],
            ),
            &["id", "k"],
        )
        .unwrap(),
    );
    let plan = ExplodeMap::load(&config(&[
        ("feature.explode_map.origin_column", "m"),
        ("feature.explode_map.target_key_column", "k"),
        ("feature.explode_map.target_value_column", "v"),
    ]))
    .resolve(&schema, &planner())
    .unwrap();

    let mut group = c.benchmark_group("tst_060_explode_map");
    for entries in [1_usize, 16, 256] {
        let cell = map_cell(entries);
        group.throughput(Throughput::Elements(u64::try_from(entries).unwrap()));
        group.bench_with_input(BenchmarkId::from_parameter(entries), &cell, |b, cell| {
            b.iter(|| plan.explode(black_box(cell)).unwrap());
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    tst_060_no_features_baseline,
    tst_060_constant_columns,
    tst_060_extract_json,
    tst_060_filter_chain,
    tst_060_explode_map
);
criterion_main!(benches);
