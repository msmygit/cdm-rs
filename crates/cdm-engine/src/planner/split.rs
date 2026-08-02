//! The Java-parity ring splitter (`TOK-003`, `TOK-004`, `TOK-005`).
//!
//! # The algorithm, and where it differs from `SPEC.md` §6
//!
//! This is a transcription of Java CDM's `SplitPartitions.getSubPartitions`, which is the
//! normative source for `TOK-003`. Two details of the Java code are load-bearing and are *not* in
//! the pseudocode of `SPEC.md` §6:
//!
//! 1. **The loop is `while (curMax <= max)`, not an unconditional loop.** The pseudocode breaks
//!    only when the range is exhausted, but the common case does not set `exhausted`: when the
//!    last sub-range lands exactly on `max`, Java falls through to `curMax += 1`, and the *loop
//!    condition* is what ends the split. Without the guard the splitter emits one extra,
//!    inverted range starting past `max` — for Java CDM's own unit-test case
//!    (`min = 1, max = 100, numSplits = 10`) it would emit an eleventh range `[101, 100]`.
//! 2. **`coveragePercent` is clamped**: `if (coveragePercent < 1 || coveragePercent > 100)
//!    coveragePercent = 100`. A configured `0` therefore means *full* coverage, not none.
//!
//! Everything else matches: the `partition_size == 0 → 100_000` fallback, the overflow/past-end
//! `exhausted` detection, the `cur_max += 1` between iterations, and the coverage shrink applied
//! from each range's lower bound.
//!
//! # Overflow (`TOK-004`)
//!
//! Java uses `BigInteger`, whose `newCurMax < curMax` test is a fossil of an earlier `long`
//! implementation and can never fire. cdm-rs works in `i128`, which holds every Murmur3 and
//! RandomPartitioner token, and every addition here is checked, so the overflow is impossible
//! rather than merely detected. There is one place where `i128` genuinely can overflow and
//! `BigInteger` cannot: `cur_max += 1` when the last range ends at `i128::MAX`, which is the
//! top of the RandomPartitioner ring. That addition is checked and treated exactly as Java's
//! loop condition treats it — the split is over.

use cdm_core::{CdmError, ErrorKind, TokenRange};

/// The partition size Java falls back to when `(max - min) / num_parts` truncates to zero,
/// i.e. when the requested part count exceeds the number of tokens in the ring segment.
pub const FALLBACK_PARTITION_SIZE: i128 = 100_000;

/// The most ranges one plan may contain.
///
/// Java has no such limit and will happily try to materialise a list of `num_parts` objects.
/// `NFR-003` forbids any configuration that grows memory without bound, and a plan is held in
/// memory for the life of the run, so an absurd `perfops.num_parts` is refused up front with an
/// actionable message instead of exhausting the heap an hour into a migration.
pub const MAX_PLANNED_RANGES: u64 = 50_000_000;

/// Splits `bounds` into `num_parts` ranges using the Java algorithm exactly (`TOK-003`).
///
/// `coverage_percent` shrinks each emitted range from its lower bound (`TOK-005`): a range
/// `[a, b]` at 25% becomes `[a, a + (b - a) / 4]`. Values outside `1..=100` mean 100, matching
/// Java's clamp.
///
/// The result is in ring order, ascending. It is *not* shuffled — that is
/// [`shuffle_for_run`](super::shuffle::shuffle_for_run)'s job (`TOK-006`), kept separate so the
/// deterministic geometry can be tested without the permutation.
///
/// # Errors
///
/// Returns [`ErrorKind::Config`] if `num_parts` is zero (Java throws `ArithmeticException`), or
/// if `num_parts` exceeds [`MAX_PLANNED_RANGES`].
pub fn split_ring(
    bounds: TokenRange,
    num_parts: u64,
    coverage_percent: u8,
) -> Result<Vec<TokenRange>, CdmError> {
    if num_parts == 0 {
        return Err(CdmError::new(
            ErrorKind::Config,
            "perfops.num_parts must be at least 1; the ring cannot be split into zero parts",
        )
        .with_context(|ctx| ctx.with_config_key("perfops.num_parts")));
    }
    if num_parts > MAX_PLANNED_RANGES {
        return Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "perfops.num_parts is {num_parts}, above the {MAX_PLANNED_RANGES} range ceiling; \
                 a plan is held in memory for the whole run (NFR-003). Raise \
                 perfops.batch_size or split the table across runs instead."
            ),
        )
        .with_context(|ctx| ctx.with_config_key("perfops.num_parts")));
    }

    let coverage = effective_coverage(coverage_percent);
    let min = bounds.min();
    let max = bounds.max();
    // `TokenRange` guarantees `max >= min`, and both are inside a partitioner ring, so the span
    // of the widest legal segment (`RANDOM_FULL`) is `i128::MAX - 0`. The subtraction cannot
    // overflow; `checked_sub` says so without a comment the reader has to trust.
    let span = max.checked_sub(min).ok_or_else(|| {
        CdmError::new(
            ErrorKind::Internal,
            format!("token range {bounds} is wider than the i128 token space"),
        )
    })?;

    let mut partition_size = span / i128::from(num_parts);
    if partition_size == 0 {
        partition_size = FALLBACK_PARTITION_SIZE;
    }

    // At most one range per part, plus the fallback case, which emits `span / 100_000 + 1`
    // ranges — and the fallback only happens when `span < num_parts`.
    let mut out: Vec<TokenRange> = Vec::new();
    let mut cur_max = min;
    while cur_max <= max {
        let cur_min = cur_max;
        // Java: `newCurMax = curMin.add(partitionSize)`, then two clamps to `max`, either of
        // which exhausts the split. The overflow clamp is unreachable in `BigInteger`; here it
        // is the checked addition, and it means the same thing — the end of the ring is past
        // anything representable, so this is the last range.
        let (mut new_max, mut exhausted) = match cur_min.checked_add(partition_size) {
            Some(candidate) => (candidate, false),
            None => (max, true),
        };
        if new_max > max {
            new_max = max;
            exhausted = true;
        }
        cur_max = new_max;

        let width = cur_max - cur_min;
        let covered = scale_by_percent(width, coverage);
        out.push(TokenRange::new(cur_min, cur_min + covered)?);

        if exhausted {
            break;
        }
        // Java's `curMax = curMax.add(BigInteger.ONE)` followed by `while (curMax <= max)`. In
        // `i128` the increment can overflow, at the very top of the RandomPartitioner ring; the
        // Java loop condition would have ended the split there too.
        match cur_max.checked_add(1) {
            Some(next) => cur_max = next,
            None => break,
        }
    }

    Ok(out)
}

/// Java's coverage clamp: anything outside `1..=100` means 100 (`TOK-005`).
fn effective_coverage(configured: u8) -> u8 {
    if (1..=100).contains(&configured) {
        configured
    } else {
        100
    }
}

/// `floor(width * percent / 100)` without ever forming `width * percent`.
///
/// `width` can be as large as `i128::MAX` (the RandomPartitioner ring in one part), so the
/// multiplication Java performs in `BigInteger` would overflow. Splitting `width` into
/// `100q + r` gives `floor(width * p / 100) = q * p + floor(r * p / 100)` exactly, with `q * p`
/// bounded by `width` and `r * p` bounded by `9900`.
fn scale_by_percent(width: i128, percent: u8) -> i128 {
    let percent = i128::from(percent);
    let quotient = width / 100;
    let remainder = width % 100;
    quotient * percent + remainder * percent / 100
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// `[a, b]`, spelled the way the hand-computed expectations below read best.
    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    #[test]
    fn tok_003_matches_java_cdms_own_unit_test_case() {
        // SplitPartitionsTest.getRandomSubPartitionsTest: 10 splits of (1, 100), 100% coverage,
        // asserts 10 partitions each with `max - min == 9`.
        let parts = split_ring(range(1, 100), 10, 100).unwrap();
        assert_eq!(parts.len(), 10);
        assert!(parts.iter().copied().all(|p| p.max() - p.min() == 9));
        assert_eq!(parts[0], range(1, 10));
        assert_eq!(parts[1], range(11, 20));
        assert_eq!(parts[9], range(91, 100));
    }

    #[test]
    fn tok_003_the_loop_guard_prevents_an_eleventh_inverted_range() {
        // The case SPEC §6's pseudocode gets wrong: the tenth range ends exactly on `max`
        // without setting `exhausted`, so only `while (curMax <= max)` stops the split.
        let parts = split_ring(range(1, 100), 10, 100).unwrap();
        assert_eq!(parts.len(), 10);
        assert!(parts.iter().copied().all(|p| p.min() <= p.max()));
        assert_eq!(parts.last().copied().unwrap().max(), 100);
    }

    #[test]
    fn tok_003_matches_java_cdms_over_100_percent_coverage_case() {
        // SplitPartitionsTest.getRandomSubPartitionsTestOver100: 8 splits of (1, 44) at 200%,
        // which Java clamps to 100%.
        let parts = split_ring(range(1, 44), 8, 200).unwrap();
        assert_eq!(parts.len(), 8);
        assert_eq!(parts[0], range(1, 6));
        assert_eq!(parts[6], range(37, 42));
        assert_eq!(parts[7], range(43, 44));
    }

    #[test]
    fn tok_003_a_zero_partition_size_falls_back_to_100k() {
        // (max - min) / num_parts truncates to 0, so Java uses 100_000 and the whole span lands
        // in a single range — asking for more parts than there are tokens gives fewer, not more.
        let parts = split_ring(range(1, 100), 1000, 100).unwrap();
        assert_eq!(parts, vec![range(1, 100)]);

        // A span wider than the fallback still splits, in 100_000-token steps.
        let parts = split_ring(range(0, 250_000), 1_000_000, 100).unwrap();
        assert_eq!(
            parts,
            vec![
                range(0, 100_000),
                range(100_001, 200_001),
                range(200_002, 250_000),
            ]
        );
    }

    #[test]
    fn tok_003_num_parts_larger_than_the_span_yields_one_range() {
        assert_eq!(split_ring(range(5, 5), 1, 100).unwrap(), vec![range(5, 5)]);
        assert_eq!(split_ring(range(5, 6), 99, 100).unwrap(), vec![range(5, 6)]);
    }

    #[test]
    fn tok_003_num_parts_of_one_is_the_whole_segment() {
        assert_eq!(
            split_ring(range(-100, 100), 1, 100).unwrap(),
            vec![range(-100, 100)]
        );
    }

    #[test]
    fn tok_003_the_full_murmur3_ring_splits_into_exactly_num_parts() {
        let full = TokenRange::MURMUR3_FULL;
        for parts_requested in [1_u64, 2, 3, 5, 4096] {
            let parts = split_ring(full, parts_requested, 100).unwrap();
            assert_eq!(parts.len(), usize::try_from(parts_requested).unwrap());
            assert_eq!(parts[0].min(), i128::from(i64::MIN));
            assert_eq!(parts.last().copied().unwrap().max(), i128::from(i64::MAX));
            for pair in parts.windows(2) {
                assert_eq!(pair[0].max() + 1, pair[1].min());
            }
        }
    }

    #[test]
    fn tok_004_the_full_random_ring_in_one_part_does_not_overflow_i128() {
        // `cur_max` lands on `i128::MAX`; `cur_max + 1` is the one addition BigInteger can make
        // and `i128` cannot. Java exits through its loop condition; so does this.
        let parts = split_ring(TokenRange::RANDOM_FULL, 1, 100).unwrap();
        assert_eq!(parts, vec![TokenRange::RANDOM_FULL]);

        let parts = split_ring(TokenRange::RANDOM_FULL, 2, 100).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].min(), 0);
        assert_eq!(parts[1].max(), i128::MAX);
        assert_eq!(parts[0].max() + 1, parts[1].min());
    }

    #[test]
    fn tok_004_the_full_random_ring_at_partial_coverage_does_not_overflow_the_multiplication() {
        // Java computes `range * coveragePercent` in BigInteger. In i128 that product would
        // overflow for the whole ring, so the scaling is done without ever forming it.
        let parts = split_ring(TokenRange::RANDOM_FULL, 1, 50).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].min(), 0);
        assert_eq!(parts[0].max(), i128::MAX / 2);
    }

    #[test]
    fn tok_005_coverage_shrinks_every_range_from_its_lower_bound() {
        let parts = split_ring(range(0, 99), 10, 50).unwrap();
        assert_eq!(parts.len(), 10);
        // Each full range spans 9 tokens above its start; half of 9 truncates to 4.
        assert_eq!(parts[0], range(0, 4));
        assert_eq!(parts[1], range(10, 14));
        assert_eq!(parts[9], range(90, 94));
        // The lower bounds are exactly those of a 100% plan: sampling never moves a range.
        let full = split_ring(range(0, 99), 10, 100).unwrap();
        for (sampled, complete) in parts.iter().copied().zip(full.iter().copied()) {
            assert_eq!(sampled.min(), complete.min());
            assert!(sampled.max() <= complete.max());
        }
    }

    #[test]
    fn tok_005_coverage_outside_1_to_100_means_full_coverage_as_in_java() {
        let full = split_ring(range(1, 100), 10, 100).unwrap();
        assert_eq!(split_ring(range(1, 100), 10, 0).unwrap(), full);
        assert_eq!(split_ring(range(1, 100), 10, 200).unwrap(), full);
        assert_eq!(split_ring(range(1, 100), 10, u8::MAX).unwrap(), full);
        // 1% is honoured, and truncates to a single-token range.
        let one_percent = split_ring(range(1, 100), 10, 1).unwrap();
        assert_eq!(one_percent[0], range(1, 1));
    }

    #[test]
    fn tok_003_zero_or_absurd_num_parts_is_a_configuration_error_not_a_panic() {
        let zero = split_ring(range(0, 10), 0, 100).unwrap_err();
        assert_eq!(zero.kind(), ErrorKind::Config);
        assert_eq!(
            zero.context().config_key.as_deref(),
            Some("perfops.num_parts")
        );

        let absurd = split_ring(range(0, i128::MAX), MAX_PLANNED_RANGES + 1, 100).unwrap_err();
        assert_eq!(absurd.kind(), ErrorKind::Config);
        assert!(absurd.message().contains("ceiling"));
    }

    proptest! {
        /// `TST-010`: whatever the bounds and part count, a 100% plan is contiguous,
        /// non-overlapping and covers exactly the requested span.
        #[test]
        fn tst_010_tok_003_ranges_are_contiguous_non_overlapping_and_exact(
            min in -1_000_000_000_i128..1_000_000_000,
            width in 0_i128..1_000_000_000,
            num_parts in 1_u64..5_000,
        ) {
            let bounds = TokenRange::new(min, min + width).unwrap();
            let parts = split_ring(bounds, num_parts, 100).unwrap();

            prop_assert!(!parts.is_empty());
            prop_assert_eq!(parts[0].min(), bounds.min());
            prop_assert_eq!(parts[parts.len() - 1].max(), bounds.max());
            for pair in parts.windows(2) {
                prop_assert!(pair[0].min() <= pair[0].max());
                prop_assert_eq!(pair[0].max() + 1, pair[1].min());
                prop_assert!(!pair[0].intersects(pair[1]));
            }
            let covered: u128 = parts.iter().map(|p| p.token_count()).sum();
            prop_assert_eq!(covered, bounds.token_count());
        }

        /// `TST-010`: a sampled plan keeps the lower bounds of the full plan, never leaves the
        /// requested segment, and is deterministic — the same inputs give the same sample.
        #[test]
        fn tst_010_tok_005_sampling_is_deterministic_and_bounded_by_the_full_plan(
            min in -1_000_000_i128..1_000_000,
            width in 0_i128..10_000_000,
            num_parts in 1_u64..200,
            coverage in 1_u8..=100,
        ) {
            let bounds = TokenRange::new(min, min + width).unwrap();
            let sampled = split_ring(bounds, num_parts, coverage).unwrap();
            prop_assert_eq!(&sampled, &split_ring(bounds, num_parts, coverage).unwrap());

            let full = split_ring(bounds, num_parts, 100).unwrap();
            prop_assert_eq!(sampled.len(), full.len());
            for (sample, complete) in sampled.iter().copied().zip(full.iter().copied()) {
                prop_assert_eq!(sample.min(), complete.min());
                prop_assert!(sample.max() <= complete.max());
                prop_assert!(bounds.contains_range(sample));
            }
        }
    }
}
