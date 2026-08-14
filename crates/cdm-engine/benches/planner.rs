//! Ring-planning benchmarks (`TST-060`).
//!
//! Planning happens once, before any row is read, so nothing here is on the per-row path. It is
//! measured for two reasons.
//!
//! The first is `NFR-002`: cold start to first row read must stay under two seconds, and the plan
//! is built inside that window. A run configured with a large `perfops.num_parts` materialises
//! every range up front — `MAX_PLANNED_RANGES` permits fifty million — so this is one of the few
//! things that can plausibly spend the whole budget.
//!
//! The second is that [`split_ring`] is a transcription of Java CDM's
//! `SplitPartitions.getSubPartitions` (`TOK-003`), edge cases and overflow behaviour included. It
//! is the code most likely to be rewritten by someone who thinks they see a simplification, and
//! the geometry it produces is load-bearing. A benchmark will not catch a correctness regression —
//! the unit tests do that — but a large movement here is a signal that the transcription has been
//! restructured, which is worth knowing.

// A benchmark is test code: fixtures are known-good and a failed setup should abort loudly rather
// than be threaded through `Result`. The no-panic rule (`ERR-004`) protects production paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cdm_core::{RunId, TokenRange};
use cdm_engine::planner::{shuffle_for_run, split_ring, FALLBACK_PARTITION_SIZE};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// Splitting the full Murmur3 ring at full coverage — what almost every run does.
///
/// Reported per-element so the figure reads as cost per planned range. That is the number that
/// matters: the interesting regression is not "planning got slower" but "planning stopped being
/// linear in the part count", which a single total would hide.
fn tst_060_split_ring_murmur3(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_split_ring_murmur3");
    for parts in [64_u64, 1_024, 65_536] {
        group.throughput(Throughput::Elements(parts));
        group.bench_with_input(BenchmarkId::from_parameter(parts), &parts, |b, &parts| {
            b.iter(|| split_ring(TokenRange::MURMUR3_FULL, black_box(parts), 100).unwrap());
        });
    }
    group.finish();
}

/// The same split with `TOK-005` coverage sampling applied.
///
/// Each emitted range is shrunk from its lower bound, which is extra arithmetic per range but
/// emits the same number of them. Benchmarked against the 100% case above so the cost of the
/// coverage feature is separable from the cost of splitting at all — a run that samples is doing
/// strictly less *scanning*, and it would be perverse if it paid for that with slower planning.
fn tst_060_split_ring_coverage(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_split_ring_coverage");
    for coverage in [1_u8, 25, 100] {
        group.bench_with_input(
            BenchmarkId::from_parameter(coverage),
            &coverage,
            |b, &coverage| {
                b.iter(|| {
                    split_ring(TokenRange::MURMUR3_FULL, 8_192, black_box(coverage)).unwrap()
                });
            },
        );
    }
    group.finish();
}

/// The two sides of the `partition_size == 0` boundary, at an identical `num_parts`.
///
/// `partition_size` is `span / num_parts` in integer arithmetic, and Java substitutes
/// [`FALLBACK_PARTITION_SIZE`] when it truncates to zero. The consequence is a discontinuity that
/// the configuration does not hint at:
///
/// - `span >= num_parts` (`dense`): `partition_size` is at least 1, so the plan is large.
/// - `span < num_parts` (`fallback`): `partition_size` becomes 100_000, far wider than the whole
///   span, so the split emits a *single* range and stops.
///
/// Same `perfops.num_parts`, a narrower range, and the plan collapses from half a million entries
/// to one. Both arms are benchmarked together because the ratio is the number worth knowing: it is
/// what a user who points a large `num_parts` at a narrow range — precisely what `TRK-033`'s rerun
/// path does — is unknowingly choosing between.
///
/// # `num_parts` is a request, not a guarantee
///
/// The `dense` arm emits 500_001 ranges for a `num_parts` of 1_000_000, and that is correct Java
/// behaviour rather than an off-by-one. Each range spans `[cur_min, cur_min + partition_size]`,
/// and `cur_max += 1` between iterations, so the stride is `partition_size + 1` while the range
/// covers `partition_size + 1` tokens. When `partition_size` is large relative to the ring — every
/// realistic configuration, including 65_536 parts over the full Murmur3 ring — the `+1` is lost in
/// the rounding and the count matches `num_parts` closely. When `partition_size` is small it does
/// not: at `partition_size == 1` the emitted count is half what was asked for.
fn tst_060_split_ring_partition_floor(c: &mut Criterion) {
    const PARTS: u64 = 1_000_000;

    // `span == num_parts`, so `partition_size` is exactly 1 and the fallback does not fire. Each
    // range then covers two tokens, which is where the 500_001 above comes from.
    let dense = TokenRange::new(0, i128::from(PARTS)).expect("a well-ordered range");
    // `span << num_parts`, so `partition_size` truncates to 0 and the fallback takes over.
    let narrow = TokenRange::new(0, 1_000).expect("a well-ordered range");

    // Asserted rather than assumed. Which side of the floor an input lands on is decided by
    // integer truncation several lines inside `split_ring`, and getting it wrong yields a
    // benchmark that runs perfectly while measuring the other branch entirely.
    assert!(
        narrow.max() < FALLBACK_PARTITION_SIZE,
        "the narrow span must be inside one fallback partition for the fallback to be visible"
    );
    assert_eq!(
        split_ring(dense, PARTS, 100).unwrap().len(),
        500_001,
        "the dense arm must stay dense: partition_size 1, stride 2, so half of PARTS plus one"
    );
    assert_eq!(
        split_ring(narrow, PARTS, 100).unwrap().len(),
        1,
        "the fallback arm must collapse to a single range"
    );

    let mut group = c.benchmark_group("tst_060_split_ring_partition_floor");
    group.bench_function("dense", |b| {
        b.iter(|| split_ring(dense, black_box(PARTS), 100).unwrap());
    });
    group.bench_function("fallback", |b| {
        b.iter(|| split_ring(narrow, black_box(PARTS), 100).unwrap());
    });
    group.finish();
}

/// The `TOK-006` shuffle that follows the split.
///
/// Two Fisher-Yates passes over the whole plan, so it is linear in range count and runs
/// immediately after `split_ring` in the same cold-start window. Measured separately because the
/// two are independent — the split is deterministic geometry, the shuffle is a permutation seeded
/// by the run id — and a regression in either would otherwise be attributed to the wrong one.
fn tst_060_shuffle_for_run(c: &mut Criterion) {
    let run_id = RunId::from_parts(1_700_000_000_000_000, 1).expect("a representable run id");
    let mut group = c.benchmark_group("tst_060_shuffle_for_run");
    for parts in [64_u64, 1_024, 65_536] {
        let plan = split_ring(TokenRange::MURMUR3_FULL, parts, 100).expect("the ring splits");
        group.throughput(Throughput::Elements(parts));
        group.bench_with_input(BenchmarkId::from_parameter(parts), &plan, |b, plan| {
            // Cloned inside the loop because the shuffle is in place; the clone is a flat memcpy
            // of `Copy` ranges and is the same cost in every arm, so it biases the absolute
            // figure but not the slope this benchmark exists to show.
            b.iter(|| {
                let mut items = plan.clone();
                shuffle_for_run(black_box(&mut items), run_id);
                items
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    tst_060_split_ring_murmur3,
    tst_060_split_ring_coverage,
    tst_060_split_ring_partition_floor,
    tst_060_shuffle_for_run
);
criterion_main!(benches);
