//! Per-cell conversion benchmarks (`TST-060`).
//!
//! `ConversionPlan::apply` runs once per column per row. At the row rates this tool targets
//! (`NFR-004`) it is executed billions of times in a single migration, so a regression here is
//! not a micro-optimisation question — it is the difference between a migration finishing
//! overnight and not.
//!
//! Three things are measured, because they have very different costs and the mix in a real
//! workload determines throughput:
//!
//! 1. **Passthrough** — the `MIG-040` zero-copy fast path, where origin and target types agree.
//!    This is the overwhelmingly common case and should stay close to a refcount bump.
//! 2. **Codec** — a real transformation, where the value is parsed and re-serialised.
//! 3. **Collections** — the same transformation applied per element, which is where per-cell cost
//!    stops being constant and starts scaling with data shape.

// A benchmark is test code: fixtures are known-good and a failed setup should abort loudly rather
// than be threaded through `Result`. The no-panic rule (`ERR-004`) protects production paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cdm_codec::{CodecRegistry, Codecset, ConversionPlan, CqlTypeInfo, Planner, PlannerOptions};
use cdm_core::RawCell;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// The largest registry a run can actually build.
///
/// As many codecs as possible are registered rather than only the ones under test, because
/// [`CodecRegistry::converter`] resolves by linear scan: registry size is part of what a
/// conversion costs, and benchmarking a two-entry registry would flatter the result.
///
/// `Codecset::ALL` is deliberately *not* used. It cannot be registered as a set:
/// `TIMESTAMP_STRING_FORMAT` and `TIMESTAMP_STRING_MILLIS` both claim `timestamp -> text` and
/// conflict, and the former additionally requires a configured format (`CDC-021`). Dropping the
/// format variant leaves the twelve that can coexist, which is the realistic maximum.
fn planner() -> Planner {
    let codecs: Vec<Codecset> = Codecset::ALL
        .into_iter()
        .filter(|c| !matches!(c, Codecset::TimestampStringFormat))
        .collect();
    Planner::new(
        CodecRegistry::with_builtins(&codecs, None).expect("the coexisting built-ins register"),
        PlannerOptions::default(),
    )
}

/// Plans `origin -> target`, both given as CQL type strings.
fn plan(planner: &Planner, origin: &str, target: &str) -> ConversionPlan {
    let origin = CqlTypeInfo::parse(origin).expect("origin type parses");
    let target = CqlTypeInfo::parse(target).expect("target type parses");
    planner.plan_types(&origin, &target)
}

/// One `i32`-length-prefixed element, as the native protocol frames collection members.
fn element(bytes: &[u8]) -> Vec<u8> {
    let mut out = i32::try_from(bytes.len()).unwrap().to_be_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

/// Native-protocol collection framing: a count, then length-prefixed elements.
fn collection(elements: &[Vec<u8>]) -> Vec<u8> {
    let mut out = i32::try_from(elements.len())
        .unwrap()
        .to_be_bytes()
        .to_vec();
    for e in elements {
        out.extend_from_slice(&element(e));
    }
    out
}

/// `MIG-040`: origin and target types agree, so the plan is the identity and no bytes are touched.
///
/// Benchmarked across value sizes because the fast path's whole claim is that cost is independent
/// of payload size. If this ever starts scaling with the input, the zero-copy path has been lost —
/// which is exactly the kind of regression that is invisible in a correctness test.
fn tst_060_passthrough(c: &mut Criterion) {
    let planner = planner();
    let plan = plan(&planner, "text", "text");
    assert!(plan.is_identity(), "text -> text must take the fast path");

    let mut group = c.benchmark_group("tst_060_passthrough");
    for size in [16_usize, 256, 4096] {
        let cell = RawCell::new(vec![b'x'; size]);
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &cell, |b, cell| {
            b.iter(|| plan.apply(black_box(cell)).unwrap());
        });
    }
    group.finish();
}

/// A real per-cell transformation: `int -> text` via the `INT_STRING` codec (`CDC-010`).
fn tst_060_codec_int_to_text(c: &mut Criterion) {
    let planner = planner();
    let plan = plan(&planner, "int", "text");
    assert!(
        !plan.is_identity(),
        "int -> text must resolve to a real codec, not passthrough"
    );

    let cell = RawCell::new(1_234_567_i32.to_be_bytes().to_vec());
    c.bench_function("tst_060_codec_int_to_text", |b| {
        b.iter(|| plan.apply(black_box(&cell)).unwrap());
    });
}

/// The same conversion applied per element of a `list<int> -> list<text>`.
///
/// Swept over element count so the per-element cost is visible as a slope rather than a single
/// number: a regression that adds a fixed setup cost and one that adds per-element cost are very
/// different problems, and one number cannot tell them apart.
fn tst_060_codec_collection(c: &mut Criterion) {
    let planner = planner();
    let plan = plan(&planner, "list<int>", "list<text>");

    let mut group = c.benchmark_group("tst_060_codec_collection");
    for len in [1_usize, 16, 256] {
        let elements: Vec<Vec<u8>> = (0..len)
            .map(|i| i32::try_from(i).unwrap().to_be_bytes().to_vec())
            .collect();
        let cell = RawCell::new(collection(&elements));
        group.throughput(Throughput::Elements(len as u64));
        group.bench_with_input(BenchmarkId::from_parameter(len), &cell, |b, cell| {
            b.iter(|| plan.apply(black_box(cell)).unwrap());
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    tst_060_passthrough,
    tst_060_codec_int_to_text,
    tst_060_codec_collection
);
criterion_main!(benches);
