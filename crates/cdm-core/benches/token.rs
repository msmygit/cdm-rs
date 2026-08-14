//! Token-range subdivision benchmarks (`TST-060`).
//!
//! Unlike the codec and compare benchmarks, nothing here is on the per-row path. Subdivision runs
//! once, at startup, and the reason to measure it is `NFR-002`: cold start to first row read must
//! stay under two seconds. A run with a large `rerun_multiplier` over a finely-split ring can ask
//! for hundreds of thousands of sub-ranges before a single row has been read, so this is one of
//! the few things that can plausibly eat that budget.
//!
//! Note this is [`TokenRange::split`], the general-purpose subdivision behind `TRK-033`'s
//! `rerun_multiplier` — *not* the `TOK-003` ring planner, which reproduces the Java splitting
//! algorithm and lives in `cdm-engine::planner`.

// A benchmark is test code: fixtures are known-good and a failed setup should abort loudly rather
// than be threaded through `Result`. The no-panic rule (`ERR-004`) protects production paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cdm_core::TokenRange;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// Subdividing the full Murmur3 ring, swept over part count.
///
/// Reported per-element so the result reads as cost per produced sub-range. That is the number
/// that matters: the interesting regression is not "splitting got slower" but "splitting stopped
/// being linear", which a single total would hide.
fn tst_060_split_murmur3_ring(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_split_murmur3_ring");
    for parts in [64_u32, 1_024, 65_536] {
        group.throughput(Throughput::Elements(u64::from(parts)));
        group.bench_with_input(BenchmarkId::from_parameter(parts), &parts, |b, &parts| {
            b.iter(|| TokenRange::MURMUR3_FULL.split(black_box(parts)).unwrap());
        });
    }
    group.finish();
}

/// The same subdivision over the `RandomPartitioner` ring, whose span is `2^127 - 1`.
///
/// Worth measuring separately from Murmur3 because the arithmetic runs at the top of `i128`'s
/// range, where the widening and the `wrapping_*` guards in `token_count`/`split` are actually
/// exercised rather than operating on values that fit comfortably in 64 bits.
fn tst_060_split_random_ring(c: &mut Criterion) {
    let mut group = c.benchmark_group("tst_060_split_random_ring");
    for parts in [64_u32, 1_024, 65_536] {
        group.throughput(Throughput::Elements(u64::from(parts)));
        group.bench_with_input(BenchmarkId::from_parameter(parts), &parts, |b, &parts| {
            b.iter(|| TokenRange::RANDOM_FULL.split(black_box(parts)).unwrap());
        });
    }
    group.finish();
}

/// Re-splitting an already-narrow range, as `TRK-033` does when a failed range is retried.
///
/// A rerun subdivides one previously-planned range rather than the whole ring, so the span is tiny
/// relative to the part count. This is the path where `parts` can exceed the token count and the
/// one-sub-range-per-token clamp kicks in.
fn tst_060_split_narrow_range(c: &mut Criterion) {
    let range = TokenRange::new(0, 4_095).expect("a well-ordered range");
    let mut group = c.benchmark_group("tst_060_split_narrow_range");
    for parts in [64_u32, 4_096, 65_536] {
        group.throughput(Throughput::Elements(u64::from(parts)));
        group.bench_with_input(BenchmarkId::from_parameter(parts), &parts, |b, &parts| {
            b.iter(|| range.split(black_box(parts)).unwrap());
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    tst_060_split_murmur3_ring,
    tst_060_split_random_ring,
    tst_060_split_narrow_range
);
criterion_main!(benches);
