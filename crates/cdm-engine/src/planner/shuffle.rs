//! The run-reproducible double shuffle (`TOK-006`, `TOK-007`).
//!
//! Java CDM calls `Collections.shuffle` **twice** on the split list before scheduling it, so that
//! consecutive ranges — which belong to the same replica set — are not processed back to back and
//! the load spreads across the ring. Shuffling twice is mathematically no better than shuffling
//! once, but it is what Java does and the range *order* is visible in `cdm_run_details`, so the
//! second pass is kept for parity rather than quietly dropped.
//!
//! Java's shuffle is unseeded: an interrupted run cannot be reproduced, and a support engineer
//! cannot ask "which range was worker 3 on?" of a rerun. cdm-rs seeds the permutation from the
//! [`RunId`] (`TOK-007`), so replanning a run — on another node, in another process, in another
//! release — yields byte-identical range order.
//!
//! # Why a hand-rolled generator
//!
//! Reproducibility here is a *persisted* property: the order is recorded in the tracking table
//! and a resumed run must agree with it. `rand`'s `StdRng` explicitly does not guarantee value
//! stability across releases, so a dependency bump could silently change every plan. SplitMix64
//! is nine lines, has a fixed published specification, and cannot drift.

use cdm_core::RunId;

/// SplitMix64 — the finalizer of Steele et al.'s `SplittableRandom`, used here as a small,
/// specification-pinned generator (see `TOK-007`).
#[derive(Debug, Clone)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// The golden-ratio increment from the published algorithm.
    const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Seeds the generator.
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 bits.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(Self::GAMMA);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniformly distributed index in `0..bound`, by rejection so that no residue is favoured.
    ///
    /// Modulo alone would bias the low indices, which in a Fisher-Yates loop shows up as ranges
    /// near the start of the ring being scheduled early — precisely the correlation `TOK-006`
    /// exists to destroy.
    fn next_index(&mut self, bound: usize) -> usize {
        let Ok(bound) = u64::try_from(bound) else {
            return 0;
        };
        if bound <= 1 {
            return 0;
        }
        // `2^64 mod bound`, computed without naming 2^64: zero means `bound` divides the range
        // exactly and every draw is acceptable.
        let overhang = (u64::MAX % bound).wrapping_add(1) % bound;
        let limit = 0_u64.wrapping_sub(overhang);
        loop {
            let draw = self.next_u64();
            if overhang == 0 || draw < limit {
                return usize::try_from(draw % bound).unwrap_or(0);
            }
        }
    }
}

/// Derives the permutation seed from the run id.
///
/// The raw id is a timestamp with a 12-bit counter in its low bits (or, for a Java-generated id
/// being resumed, a `System.nanoTime()` value); either way the low bits vary far more than the
/// high ones, so the value is passed through the SplitMix64 finalizer once before use rather
/// than seeding the state with a near-constant.
fn seed_for(run_id: RunId) -> u64 {
    let mut seeder = SplitMix64::new(run_id.as_i64().cast_unsigned());
    seeder.next_u64()
}

/// Shuffles `items` twice, deterministically for a given [`RunId`] (`TOK-006`, `TOK-007`).
///
/// The permutation is Fisher–Yates walked from the end, which is the order
/// `java.util.Collections.shuffle` uses; only the source of randomness differs.
pub fn shuffle_for_run<T>(items: &mut [T], run_id: RunId) {
    let mut rng = SplitMix64::new(seed_for(run_id));
    // Two passes, as in `SplitPartitions.getRandomSubPartitions`. The generator state carries
    // over, so the second pass is an independent permutation, not a repeat of the first.
    for _ in 0..2 {
        shuffle_once(items, &mut rng);
    }
}

/// One Fisher–Yates pass.
fn shuffle_once<T>(items: &mut [T], rng: &mut SplitMix64) {
    let len = items.len();
    for index in (1..len).rev() {
        // `next_index(index + 1)` is at most `index`, so both indices are in bounds and `swap`
        // cannot panic.
        let target = rng.next_index(index + 1);
        items.swap(index, target);
    }
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
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    fn shuffled(run_id: i64, len: usize) -> Vec<usize> {
        let mut items: Vec<usize> = (0..len).collect();
        shuffle_for_run(&mut items, RunId::from_raw(run_id));
        items
    }

    #[test]
    fn tok_006_the_plan_order_is_a_permutation_and_not_the_identity() {
        let order = shuffled(1_234_567_890, 64);
        assert_eq!(order.len(), 64);
        assert_eq!(order.iter().copied().collect::<BTreeSet<_>>().len(), 64);
        assert_ne!(order, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn tok_007_the_same_run_id_always_produces_the_same_order() {
        assert_eq!(shuffled(42, 500), shuffled(42, 500));
        assert_ne!(shuffled(42, 500), shuffled(43, 500));
        // Java-generated ids (`System.nanoTime()`) and the unset sentinel are ordinary seeds.
        assert_eq!(shuffled(0, 32), shuffled(0, 32));
        assert_ne!(shuffled(0, 32), shuffled(-1, 32));
    }

    #[test]
    fn tok_006_shuffling_is_a_no_op_for_zero_or_one_element() {
        let mut empty: Vec<u8> = Vec::new();
        shuffle_for_run(&mut empty, RunId::from_raw(7));
        assert!(empty.is_empty());

        let mut single = vec![99];
        shuffle_for_run(&mut single, RunId::from_raw(7));
        assert_eq!(single, vec![99]);
    }

    #[test]
    fn tok_006_the_permutation_does_not_leave_elements_in_place_wholesale() {
        // A weak but useful smoke test on quality: a shuffle of 1000 elements should move the
        // overwhelming majority of them.
        let order = shuffled(20_240_101, 1000);
        let fixed_points = order.iter().enumerate().filter(|(i, v)| i == *v).count();
        assert!(fixed_points < 20, "{fixed_points} fixed points is too many");
    }

    #[test]
    fn tok_007_the_generator_is_pinned_to_its_published_specification() {
        // SplitMix64 with state 0 emits these values; they are what makes a plan reproducible
        // across releases, so they are asserted literally.
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 16_294_208_416_658_607_535);
        assert_eq!(rng.next_u64(), 7_960_286_522_194_355_700);
        assert_eq!(rng.next_u64(), 487_617_019_471_545_679);
    }

    proptest! {
        /// `TST-010`: whatever the run id and length, shuffling permutes and never loses,
        /// duplicates or invents an element.
        #[test]
        fn tst_010_tok_006_shuffling_is_always_a_permutation(
            run_id in any::<i64>(),
            len in 0_usize..300,
        ) {
            let order = shuffled(run_id, len);
            prop_assert_eq!(order.len(), len);
            let distinct: BTreeSet<usize> = order.iter().copied().collect();
            prop_assert_eq!(distinct.len(), len);
            prop_assert!(distinct.iter().all(|value| *value < len));
        }
    }
}
