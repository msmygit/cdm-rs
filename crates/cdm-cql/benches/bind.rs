//! Statement-bind benchmarks (`TST-060`).
//!
//! Everything measured here runs once per row, per column, for the whole of a run: a migration of a
//! hundred billion rows executes [`Binder::bind`] a hundred billion times, and
//! [`TargetKeyPlan::key_of`] as often again on the validate side. `ARCHITECTURE.md` §5.5 is the
//! design rule these numbers defend — per-row work must be *lookup*, not *decision*, with every
//! conversion plan, bind slot and key slot resolved once at startup. A regression here does not look
//! like a failure; it looks like a run that takes eleven hours instead of five, which is the
//! difference `NFR-004` is stated in.
//!
//! Three things are swept over column count, because a wide table is where per-row cost actually
//! bites: a 128-column table pays every one of these costs 128 times per row, and a mistake that
//! turns a resolved lookup back into a per-column decision shows up as a change in the *slope*
//! rather than in any single number.
//!
//! What is **not** here: nothing in this file touches a session. Both hot paths were built to be
//! driven from plain data — [`Binder::bind`] takes a `SourceRow` and returns bytes, and the driver
//! only appears at `SerializeRow` time — so the interesting work is reachable on a bare runner. The
//! one cost deliberately excluded is the driver's own frame serialisation, which needs a
//! `RowSerializationContext` from a prepared statement and therefore a cluster.
//!
//! # Specification
//!
//! - `TST-060` — the benchmarks themselves
//! - `NFR-004` — the throughput target they defend
//! - `MIG-012` — the `UNSET`-vs-`NULL` decision, benchmarked at all three of its entry points
//! - `MIG-013` — the null-key substitution, in the key and in the row
//! - `MIG-040` — the zero-copy passthrough the bind numbers are only credible with

// A benchmark is test code: fixtures are known-good and a failed setup should abort loudly rather
// than be threaded through `Result`. The no-panic rule (`ERR-004`) protects production paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

use cdm_codec::{CodecRegistry, Planner, PlannerOptions};
use cdm_core::{RawCell, Row};
use cdm_cql::rows::{ExplodedKeyParts, TargetKeyPlan};
use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
use cdm_cql::statement::{
    BindInputs, Binder, ColumnMapping, MappingOptions, MissingKeyPolicy, OriginProjection,
    OriginRangeSelect, StatementOptions, TargetSelectByPk, TargetUpsert,
};

/// Table widths to sweep. 8 is a small table, 32 a typical one, 128 the wide table where per-row
/// per-column cost dominates everything else a migration does.
const WIDTHS: [usize; 3] = [8, 32, 128];

/// Primary-key component counts to sweep. Cassandra composite keys of eight components are
/// unusual but entirely legal, and the key is rebuilt for every row on both sides of a validate.
const KEY_COMPONENTS: [usize; 3] = [1, 3, 8];

/// The types a real wide table is mostly made of, cycled through so the bind sweep exercises the
/// fixed-width, variable-width and collection branches in the proportion a table has them.
const REGULAR_TYPES: [&str; 4] = ["text", "int", "map<text, text>", "set<text>"];

/// The width the `MIG-012` decision is measured at: wide enough that the per-column branch is what
/// the number is made of, rather than the fixed cost of the surrounding call.
const UNSET_WIDTH: usize = 32;

criterion_group!(
    benches,
    tst_060_key_extraction,
    tst_060_key_substitution,
    tst_060_bind_row,
    tst_060_bind_unset_decision,
    tst_060_statement_construction
);
criterion_main!(benches);

/// Deriving a record's target primary key, once per row (`SCH-006`, `VAL-001`).
///
/// Swept over the number of bound key components rather than over table width, because that is what
/// the derivation actually loops over — a plan is a vector of slots, and the row is indexed, not
/// searched. A slope steeper than linear in the component count means a name lookup crept back in.
fn tst_060_key_extraction(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060/key_extraction");
    for components in KEY_COMPONENTS {
        let schema = table("keyed", components, components + 4);
        let plan = key_plan(&schema);
        let row = populated_row(&schema);
        group.throughput(Throughput::Elements(count(components)));
        group.bench_with_input(
            BenchmarkId::from_parameter(components),
            &components,
            |b, _| {
                b.iter(|| black_box(plan.key_of(black_box(&row), ExplodedKeyParts::NONE)));
            },
        );
    }
    group.finish();
}

/// The `MIG-013` null-key substitution written back into the origin row.
///
/// Two cases per width, and the pair is the point. `substituted` returns `None` when there is
/// nothing to replace — the overwhelmingly common case, which every scanned row pays and which must
/// allocate nothing — and clones the row's cells when there is. The `clean` number is the one that
/// belongs on the hot path; `substituted` is the price of correctness on the rows that need it, and
/// it grows with the row's width rather than with the key's, because the whole cell vector is
/// copied.
fn tst_060_key_substitution(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060/key_substitution");
    for width in WIDTHS {
        // Two text key components, so the second one can be null and take `MIG-013`'s empty-string
        // substitute: text is the one type that substitutes with no configuration at all.
        let schema = table("substituted", 2, width);
        let plan = key_plan(&schema);
        let clean = populated_row(&schema);
        let null_key = row_with_null_key(&schema);
        group.throughput(Throughput::Elements(count(width)));
        group.bench_with_input(BenchmarkId::new("clean", width), &width, |b, _| {
            b.iter(|| black_box(plan.substituted(black_box(&clean))));
        });
        group.bench_with_input(BenchmarkId::new("substituted", width), &width, |b, _| {
            b.iter(|| black_box(plan.substituted(black_box(&null_key))));
        });
    }
    group.finish();
}

/// Binding one fully-populated row into the target upsert (`MIG-011`, `MIG-040`).
///
/// The origin and target schemas are identical here, which is the common migration and the case
/// `MIG-040` is written for: every conversion plan is the identity, so each value should be a
/// borrow of the frame slice and the per-column cost should be a slot lookup and a branch. The
/// throughput figure is in columns per second for exactly that reason — it is the number to compare
/// against when a conversion is added, since a converting plan decodes and re-encodes and is a
/// different order of magnitude.
fn tst_060_bind_row(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060/bind_row");
    for width in WIDTHS {
        let schema = table("wide", 1, width);
        let binder = binder_for(&schema);
        let row = populated_row(&schema);
        group.throughput(Throughput::Elements(count(width)));
        group.bench_with_input(BenchmarkId::from_parameter(width), &width, |b, _| {
            b.iter(|| {
                let source: &Row = black_box(&row);
                black_box(binder.bind(&source, BindInputs::default()).unwrap())
            });
        });
    }
    group.finish();
}

/// The `MIG-012` `UNSET`-vs-`NULL` decision, at each of the three ways a column reaches it.
///
/// This is the single most correctness-critical branch in the write path: binding `NULL` writes a
/// tombstone, and a table with thirty nullable columns migrated that way gains thirty tombstones per
/// row, degrading the target's reads until compaction catches up — weeks, at petabyte scale. It also
/// runs per column per row, so it is on the hot path twice over.
///
/// The three inputs are the same width and the same binder, and differ only in what the cells hold:
///
/// * `value` — every collection non-empty, so every column binds bytes;
/// * `null` — every cell CQL `NULL`, which short-circuits to `UNSET` without looking at bytes;
/// * `empty_collection` — every cell a well-formed collection of zero elements, which `MIG-012`
///   also requires to be `UNSET` and which costs a four-byte length inspection to recognise.
///
/// A collection-only table is used so the third case applies to every column rather than to a
/// quarter of them; the comparison between the three is what the benchmark is for.
fn tst_060_bind_unset_decision(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060/bind_unset_decision");
    let schema = collection_table(UNSET_WIDTH);
    let binder = binder_for(&schema);
    let rows = [
        ("value", populated_row(&schema)),
        ("null", row_with_null_regulars(&schema)),
        ("empty_collection", row_with_empty_collections(&schema)),
    ];
    group.throughput(Throughput::Elements(count(UNSET_WIDTH)));
    for (name, row) in &rows {
        group.bench_with_input(BenchmarkId::from_parameter(name), row, |b, row| {
            b.iter(|| {
                let source: &Row = black_box(row);
                black_box(binder.bind(&source, BindInputs::default()).unwrap())
            });
        });
    }
    group.finish();
}

/// Generating the run's CQL and resolving the binding plan, once at startup (`FEA-062`, `MIG-011`).
///
/// Not a hot path — it happens once per run — but it is the work that *makes* the hot path a
/// lookup, so it is measured to keep the trade visible: every microsecond here buys the per-row
/// numbers above. It is also the sweep most likely to go quadratic unnoticed, since the mapping
/// resolves each target column against every origin column by name.
///
/// `cql` covers the four statements `FEA-062` logs at startup; `binder` covers turning the same
/// mapping into per-column conversion plans and the target key plan.
fn tst_060_statement_construction(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060/statement_construction");
    let planner = planner();
    for width in WIDTHS {
        let schema = table("wide", 1, width);
        group.throughput(Throughput::Elements(count(width)));
        group.bench_with_input(BenchmarkId::new("cql", width), &width, |b, _| {
            b.iter(|| {
                let schema = black_box(&schema);
                let mapping = ColumnMapping::resolve(schema, schema, &MappingOptions::default())
                    .expect("the identity mapping resolves");
                let projection = OriginProjection::new(mapping.origin_columns(), &[]);
                black_box((
                    OriginRangeSelect::new(schema, &projection, None, false),
                    TargetSelectByPk::new(&mapping).expect("the target key is derivable"),
                    TargetUpsert::new(&mapping, StatementOptions::default())
                        .expect("the upsert builds"),
                ))
            });
        });

        let mapping = ColumnMapping::resolve(&schema, &schema, &MappingOptions::default()).unwrap();
        let select = TargetSelectByPk::new(&mapping).unwrap();
        group.bench_with_input(BenchmarkId::new("binder", width), &width, |b, _| {
            b.iter(|| {
                let mapping = black_box(&mapping);
                let statement = TargetUpsert::new(mapping, StatementOptions::default())
                    .expect("the upsert builds");
                black_box((
                    Binder::new(
                        mapping,
                        statement,
                        &planner,
                        MissingKeyPolicy::default(),
                        false,
                    )
                    .expect("every declared type parses"),
                    TargetKeyPlan::resolve(mapping, &select, MissingKeyPolicy::default())
                        .expect("every key component is derivable"),
                ))
            });
        });
    }
    group.finish();
}

/// A planner over the built-in codecs and nothing else, which is what a run with no
/// `--codecs` configured resolves against.
fn planner() -> Planner {
    Planner::new(
        CodecRegistry::with_builtins(&[], None).expect("the built-in registry builds"),
        PlannerOptions::default(),
    )
}

/// One column, with the clustering direction implied by its kind.
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

/// A table of `width` columns whose first `key_components` are the primary key.
///
/// The first key column is the partition key and the rest are clustering columns, all `text` — the
/// one type `MIG-013` substitutes with no configuration, which is what lets the substitution
/// benchmark exist without a `transform.missing_key_ts_replace` fixture. The remaining columns cycle
/// through [`REGULAR_TYPES`].
fn table(name: &str, key_components: usize, width: usize) -> TableSchema {
    let mut columns = Vec::with_capacity(width);
    for index in 0..key_components.min(width) {
        let (kind, position) = if index == 0 {
            (ColumnKind::PartitionKey, 0)
        } else {
            (ColumnKind::Clustering, position_of(index - 1))
        };
        columns.push(column(&format!("k{index}"), "text", kind, position));
    }
    for index in key_components..width {
        let cql_type = REGULAR_TYPES
            .get(index % REGULAR_TYPES.len())
            .copied()
            .unwrap_or("text");
        columns.push(column(
            &format!("c{index}"),
            cql_type,
            ColumnKind::Regular,
            -1,
        ));
    }
    schema_of(name, columns)
}

/// A table whose every non-key column is a collection, so the empty-collection arm of `MIG-012`
/// applies to all of them.
fn collection_table(width: usize) -> TableSchema {
    let mut columns = vec![column("k0", "text", ColumnKind::PartitionKey, 0)];
    for index in 1..width {
        let cql_type = if index % 2 == 0 {
            "map<text, text>"
        } else {
            "set<text>"
        };
        columns.push(column(
            &format!("c{index}"),
            cql_type,
            ColumnKind::Regular,
            -1,
        ));
    }
    schema_of("collections", columns)
}

fn schema_of(name: &str, columns: Vec<ColumnMeta>) -> TableSchema {
    TableSchema {
        keyspace: "bench".to_owned(),
        table: name.to_owned(),
        columns,
        is_materialized_view: false,
    }
}

fn position_of(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(0)
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// The key plan a validate run derives its lookup key with, for an identity migration of `schema`.
fn key_plan(schema: &TableSchema) -> TargetKeyPlan {
    let mapping =
        ColumnMapping::resolve(schema, schema, &MappingOptions::default()).expect("it resolves");
    let select = TargetSelectByPk::new(&mapping).expect("the target key is derivable");
    TargetKeyPlan::resolve(&mapping, &select, MissingKeyPolicy::default())
        .expect("every key component comes from an origin column")
}

/// The binder for an identity migration of `schema`: every conversion plan is the identity, which
/// is the `MIG-040` passthrough the bind numbers are measured on.
fn binder_for(schema: &TableSchema) -> Binder {
    let mapping =
        ColumnMapping::resolve(schema, schema, &MappingOptions::default()).expect("it resolves");
    let statement =
        TargetUpsert::new(&mapping, StatementOptions::default()).expect("the upsert builds");
    Binder::new(
        &mapping,
        statement,
        &planner(),
        MissingKeyPolicy::default(),
        false,
    )
    .expect("every declared type parses")
}

/// A row with a value in every cell, in the schema's column order — which, with no skips
/// configured, is the origin projection order the binder indexes into.
fn populated_row(schema: &TableSchema) -> Row {
    Row::new(
        schema
            .columns
            .iter()
            .map(|c| cell_for(&c.cql_type))
            .collect(),
    )
}

/// The same row with every non-key cell CQL `NULL`: the `MIG-012` short circuit.
fn row_with_null_regulars(schema: &TableSchema) -> Row {
    Row::new(
        schema
            .columns
            .iter()
            .map(|c| {
                if c.kind.is_key() {
                    cell_for(&c.cql_type)
                } else {
                    RawCell::NULL
                }
            })
            .collect(),
    )
}

/// The same row with every non-key cell an empty collection: the arm `MIG-012` has to recognise
/// from the serialised bytes rather than from the absence of them.
fn row_with_empty_collections(schema: &TableSchema) -> Row {
    Row::new(
        schema
            .columns
            .iter()
            .map(|c| {
                if c.kind.is_key() {
                    cell_for(&c.cql_type)
                } else {
                    empty_collection()
                }
            })
            .collect(),
    )
}

/// The same row with its last key cell null, which is the row `MIG-013` substitutes into.
fn row_with_null_key(schema: &TableSchema) -> Row {
    let last_key = schema
        .columns
        .iter()
        .rposition(|c| c.kind.is_key())
        .unwrap_or(0);
    Row::new(
        schema
            .columns
            .iter()
            .enumerate()
            .map(|(index, c)| {
                if index == last_key {
                    RawCell::NULL
                } else {
                    cell_for(&c.cql_type)
                }
            })
            .collect(),
    )
}

/// A representative serialised value for a declared type.
///
/// Wire bytes, not typed values: that is what a row holds on this side of the driver, and binding an
/// identity-planned column never decodes them.
fn cell_for(cql_type: &str) -> RawCell {
    match cql_type {
        "int" => RawCell::new(7_i32.to_be_bytes().to_vec()),
        // `[count][len]k[len]v` — the protocol's collection framing, which is what `MIG-012`'s
        // emptiness test and `MIG-014`'s null-value strip both walk.
        "map<text, text>" => collection(1, &[b"colour", b"green"]),
        "set<text>" => collection(1, &[b"green"]),
        _ => RawCell::new(b"a value of a representative length".to_vec()),
    }
}

/// A serialised collection of `count` elements over the given length-prefixed parts.
fn collection(count: i32, parts: &[&[u8]]) -> RawCell {
    let mut bytes = count.to_be_bytes().to_vec();
    for part in parts {
        bytes.extend_from_slice(&i32::try_from(part.len()).unwrap_or(0).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    RawCell::new(bytes)
}

/// A well-formed collection with no elements — not an empty buffer, which would be a different
/// case altogether.
fn empty_collection() -> RawCell {
    RawCell::new(0_i32.to_be_bytes().to_vec())
}
