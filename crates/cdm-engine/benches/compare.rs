//! Validate's inner loop and the cost of watching a run (`TST-060`, `NFR-004`).
//!
//! [`ComparisonPlan::compare`] runs once per origin row and, inside that, once per target column.
//! It is therefore the only thing standing between the driver's throughput and the run's: every
//! nanosecond added here is multiplied by rows × columns, and on a wide table that multiplier is
//! large enough that a regression which looks like noise in a unit test shows up as hours on a
//! real migration. The sweep over column count is what makes such a regression legible — a change
//! that costs a constant per row and one that costs a constant per column are indistinguishable at
//! a single width and very different at a hundred billion cells.
//!
//! The second half of the file measures something nobody had measured: what per-request
//! instrumentation (`MET-010`) and the adaptive rate controller (`ENG-006`) cost on the paths that
//! carry them. Both are `Option`s that a run without a dashboard, or without
//! `perfops.adaptive_ratelimit`, leaves as `None`, so the same call can be benchmarked wired and
//! unwired and the delta attributed to the feature rather than to the surrounding machinery.

// A benchmark is test code: fixtures are known-good and a failed setup should abort loudly rather
// than be threaded through `Result`. The no-panic rule (`ERR-004`) protects production paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use cdm_codec::{CodecRegistry, Planner, PlannerOptions};
use cdm_core::{PrimaryKey, RawCell, Record, RequestObserver, Row};
use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
use cdm_cql::statement::{ColumnMapping, MappingOptions};
use cdm_engine::jobs::validate::ComparisonPlan;
use cdm_engine::scheduler::{LoadSignal, RuntimeLimits, SchedulerSettings};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// The widths swept. Four is a narrow lookup table, sixteen a typical one, sixty-four the wide
/// denormalised tables where validation stops being free.
const WIDTHS: [usize; 3] = [4, 16, 64];

/// `id int PRIMARY KEY` followed by `columns - 1` `text` columns, on both sides.
///
/// Identical schemas on purpose: that is the overwhelmingly common migration, and it resolves to
/// the identity conversion, so what is measured is the comparator rather than a codec that
/// `cdm-codec`'s own benchmarks already cover.
fn schema(columns: usize) -> TableSchema {
    let mut meta = vec![ColumnMeta {
        name: "id".to_owned(),
        cql_type: "int".to_owned(),
        kind: ColumnKind::PartitionKey,
        position: 0,
        clustering_order: ClusteringOrder::None,
    }];
    for index in 1..columns {
        meta.push(ColumnMeta {
            name: format!("c{index}"),
            cql_type: "text".to_owned(),
            kind: ColumnKind::Regular,
            position: -1,
            clustering_order: ClusteringOrder::None,
        });
    }
    TableSchema {
        keyspace: "ks".to_owned(),
        table: "t".to_owned(),
        columns: meta,
        is_materialized_view: false,
    }
}

/// The plan a run of this width would resolve at startup.
fn plan(columns: usize) -> ComparisonPlan {
    let table = schema(columns);
    let mapping = ColumnMapping::resolve(&table, &table, &MappingOptions::default()).unwrap();
    let planner = Planner::new(
        CodecRegistry::with_builtins(&[], None).unwrap(),
        PlannerOptions::default(),
    );
    ComparisonPlan::resolve(&mapping, &planner, None, false).unwrap()
}

/// One row of `columns` cells, with every text cell holding `filler`.
///
/// The cells are short: comparison is byte equality, so long values would measure `memcmp` on the
/// value rather than the per-column work that the loop is made of.
fn row(columns: usize, filler: &str) -> Row {
    let mut cells = vec![RawCell::new(7_i32.to_be_bytes().to_vec())];
    for _ in 1..columns {
        cells.push(RawCell::new(filler.as_bytes().to_vec()));
    }
    Row::new(cells)
}

/// The origin record for [`row`].
fn record(columns: usize, filler: &str) -> Record {
    Record::new(
        PrimaryKey::new(vec![RawCell::new(7_i32.to_be_bytes().to_vec())]),
        row(columns, filler),
    )
}

/// Every column agrees — the case a healthy validation run is made of.
///
/// This must be the fastest path in the file, because it is the one taken by every row of a
/// migration that worked. It is also the path with no early exit: agreement can only be
/// established by looking at every column, so this is the true cost of a validated row.
fn tst_060_compare_all_columns_match(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_compare_all_columns_match");
    for width in WIDTHS {
        let plan = plan(width);
        let record = record(width, "value");
        let target = row(width, "value");
        group.throughput(Throughput::Elements(width as u64));
        group.bench_with_input(BenchmarkId::from_parameter(width), &width, |b, _| {
            b.iter(|| plan.compare(black_box(&record), black_box(Some(&target))));
        });
    }
    group.finish();
}

/// A single differing column, the last one.
///
/// Placed last so the full scan still happens, isolating what a discrepancy adds on top of it: a
/// `Vec` allocation, a cloned column name and two cloned `RawCell`s. The delta against the
/// matching case is what a run over badly-migrated data pays per row, and it is the number to
/// watch if the discrepancy report ever starts carrying more than it does today.
fn tst_060_compare_mismatch_in_last_column(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_compare_mismatch_in_last_column");
    for width in WIDTHS {
        let plan = plan(width);
        let record = record(width, "value");
        let mut cells: Vec<RawCell> = (0..width)
            .map(|index| {
                if index == 0 {
                    RawCell::new(7_i32.to_be_bytes().to_vec())
                } else {
                    RawCell::new(b"value".to_vec())
                }
            })
            .collect();
        // The last column is where the difference is, so nothing about the scan is skipped.
        if let Some(last) = cells.last_mut() {
            *last = RawCell::new(b"other".to_vec());
        }
        let target = Row::new(cells);
        group.throughput(Throughput::Elements(width as u64));
        group.bench_with_input(BenchmarkId::from_parameter(width), &width, |b, _| {
            b.iter(|| plan.compare(black_box(&record), black_box(Some(&target))));
        });
    }
    group.finish();
}

/// The target has no row at all (`VAL-002`'s missing case).
///
/// Swept over the same widths although it cannot depend on them: the point of the sweep here is to
/// assert that flatness. If this ever starts tracking width, something has begun touching the
/// columns before establishing that there is nothing to compare them against.
fn tst_060_compare_missing_target_row(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_compare_missing_target_row");
    for width in WIDTHS {
        let plan = plan(width);
        let record = record(width, "value");
        group.throughput(Throughput::Elements(width as u64));
        group.bench_with_input(BenchmarkId::from_parameter(width), &width, |b, _| {
            b.iter(|| plan.compare(black_box(&record), black_box(None)));
        });
    }
    group.finish();
}

/// `validate.keys_only`, which answers "is the row there?" and nothing else (`VAL-015`).
///
/// The floor the other cases are measured against: the comparator's unavoidable cost once the
/// target lookup has happened. The gap between this and the matching case is exactly what
/// `keys_only` buys an operator doing a pre-flight run.
fn tst_060_compare_keys_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_compare_keys_only");
    for width in WIDTHS {
        let plan = plan(width).with_keys_only(true);
        let record = record(width, "value");
        let target = row(width, "value");
        group.throughput(Throughput::Elements(width as u64));
        group.bench_with_input(BenchmarkId::from_parameter(width), &width, |b, _| {
            b.iter(|| plan.compare(black_box(&record), black_box(Some(&target))));
        });
    }
    group.finish();
}

/// What `MET-010`'s per-request instrumentation costs on an unthrottled acquisition.
///
/// Every row read and every row written passes through [`RuntimeLimits::acquire_read_rows`], and
/// on an unlimited limiter — the default, and what most runs are configured with — that call does
/// no sleeping at all, so whatever the observer costs is the whole of the difference. The `None`
/// arm is the run nobody is watching; the `Some` arm is the same call with a real
/// `cdm_metrics::Instruments` behind the `dyn RequestObserver`, not a double, because a double
/// would measure the dispatch and none of the histogram update it exists to perform.
///
/// The delta is the price of a live dashboard, per row, per side.
fn tst_060_ratelimit_wait_observer_overhead(c: &mut Criterion) {
    /// Acquisitions per iteration. Entering the runtime costs about a microsecond, which is three
    /// orders of magnitude more than the thing being measured; batching amortises it away so the
    /// difference between the two arms is not buried in `block_on`.
    const BATCH: u64 = 1_024;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    // Unlimited, so `acquire` returns immediately and never yields — no sleep is being timed here,
    // only the null check and, in the second arm, the histogram update behind it.
    let settings = SchedulerSettings::default().with_ratelimits(0, 0);
    let unwatched = RuntimeLimits::new(&settings).unwrap();
    let instruments = Arc::new(cdm_metrics::Instruments::new(std::time::Instant::now()));
    let watched = RuntimeLimits::new(&settings)
        .unwrap()
        .observing(Some(Arc::clone(&instruments) as Arc<dyn RequestObserver>));

    let mut group = c.benchmark_group("tst_060_ratelimit_wait_observer_overhead");
    group.throughput(Throughput::Elements(BATCH));
    group.bench_function("none", |b| {
        b.iter(|| {
            runtime.block_on(async {
                for _ in 0..BATCH {
                    unwatched.acquire_read_rows(black_box(1)).await;
                }
            });
        });
    });
    group.bench_function("instruments", |b| {
        b.iter(|| {
            runtime.block_on(async {
                for _ in 0..BATCH {
                    watched.acquire_read_rows(black_box(1)).await;
                }
            });
        });
    });
    group.finish();
}

/// What `ENG-006`'s adaptive controller costs on the write path that feeds it.
///
/// Every target write reports its outcome through [`RuntimeLimits::record_target_signal`], whether
/// or not `perfops.adaptive_ratelimit` is set. Without it the call touches no state; with it the
/// call reads the clock and updates the control window, and that clock read is the part worth
/// knowing about, since it happens once per write rather than once per control window.
fn tst_060_adaptive_signal_overhead(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let _guard = runtime.enter();
    let off = RuntimeLimits::new(&SchedulerSettings::default().with_ratelimits(0, 10_000)).unwrap();
    let on = RuntimeLimits::new(
        &SchedulerSettings::default()
            .with_ratelimits(0, 10_000)
            .with_adaptive_ratelimit(true, 10),
    )
    .unwrap();

    let mut group = c.benchmark_group("tst_060_adaptive_signal_overhead");
    group.throughput(Throughput::Elements(1));
    group.bench_function("disabled", |b| {
        b.iter(|| off.record_target_signal(black_box(LoadSignal::Ok)));
    });
    group.bench_function("enabled", |b| {
        b.iter(|| on.record_target_signal(black_box(LoadSignal::Ok)));
    });
    group.finish();
}

criterion_group!(
    benches,
    tst_060_compare_all_columns_match,
    tst_060_compare_mismatch_in_last_column,
    tst_060_compare_missing_target_row,
    tst_060_compare_keys_only,
    tst_060_ratelimit_wait_observer_overhead,
    tst_060_adaptive_signal_overhead,
);
criterion_main!(benches);
