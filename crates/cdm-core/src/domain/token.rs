//! Token-ring geometry: [`TokenRange`] and [`PartitionRangeId`].

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{CdmError, ErrorKind};

/// A closed interval `[min, max]` of the token ring — the unit of work, scheduling, tracking
/// and resume (`SPEC.md` §2).
///
/// # Why `i128`
///
/// `TOK-004` requires all split arithmetic to happen in `i128` for the Murmur3 partitioner so
/// that the overflow the Java implementation defends against cannot occur. Murmur3 tokens fit in
/// an `i64` and are widened here; `RandomPartitioner` tokens span `[0, 2^127 - 1]`, which also
/// fits. A partitioner whose token space exceeds `i128` (none exists today) would need a
/// different representation.
///
/// # Ordering
///
/// `Ord` is lexicographic on `(min, max)`, which makes a sorted range list ring-ordered — the
/// order tracking rows and `cdm plan` output are rendered in. It deliberately says nothing about
/// containment or overlap; use [`TokenRange::contains`] and [`TokenRange::intersects`] for that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TokenRange {
    min: i128,
    max: i128,
}

impl TokenRange {
    /// The full Murmur3 ring, `[i64::MIN, i64::MAX]` (`TOK-002`).
    // A `const` item cannot use `i128::From`, so the widening is spelled with `as`. It is
    // lossless in the direction that matters and checked by `tok_004_bounds_are_held_with_*`.
    #[allow(clippy::cast_lossless)]
    pub const MURMUR3_FULL: Self = Self {
        min: i64::MIN as i128,
        max: i64::MAX as i128,
    };

    /// The full `RandomPartitioner` ring, `[0, 2^127 - 1]` (`TOK-002`).
    pub const RANDOM_FULL: Self = Self {
        min: 0,
        max: i128::MAX,
    };

    /// Creates a range, rejecting an inverted interval.
    ///
    /// Bounds are inclusive on both ends, matching the `token_min`/`token_max` columns of
    /// `cdm_run_details` (`TRK-010`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if `min > max`.
    pub fn new(min: i128, max: i128) -> Result<Self, CdmError> {
        if min > max {
            return Err(CdmError::new(
                ErrorKind::Config,
                format!("inverted token range: min {min} is greater than max {max}"),
            ));
        }
        Ok(Self { min, max })
    }

    /// The inclusive lower bound.
    pub const fn min(self) -> i128 {
        self.min
    }

    /// The inclusive upper bound.
    pub const fn max(self) -> i128 {
        self.max
    }

    /// The number of tokens in the range. Always at least one, since bounds are inclusive.
    pub const fn token_count(self) -> u128 {
        // Cannot overflow: `max >= min` is an invariant, and the widest possible span
        // (`i128::MIN..=i128::MAX`) is `u128::MAX`, which the unsigned subtraction represents.
        self.max
            .wrapping_sub(self.min)
            .cast_unsigned()
            .saturating_add(1)
    }

    /// Whether `token` falls inside the closed interval.
    pub const fn contains(self, token: i128) -> bool {
        self.min <= token && token <= self.max
    }

    /// Whether `other` is entirely inside this range.
    pub const fn contains_range(self, other: Self) -> bool {
        self.min <= other.min && other.max <= self.max
    }

    /// Whether the two ranges share at least one token.
    pub const fn intersects(self, other: Self) -> bool {
        self.min <= other.max && other.min <= self.max
    }

    /// Splits the range into `parts` contiguous, non-overlapping sub-ranges covering exactly the
    /// same tokens, sized as evenly as the token count allows (the remainder is spread one token
    /// at a time over the leading sub-ranges).
    ///
    /// If `parts` exceeds [`TokenRange::token_count`], one sub-range per token is returned, so the
    /// result is never empty and never contains an empty range.
    ///
    /// This is the general-purpose subdivision used by `TRK-033`'s `rerun_multiplier`. It is
    /// **not** the ring planner: `TOK-003` reproduces the Java splitting algorithm, edge cases and
    /// coverage sampling included, and lives in `cdm-engine::planner`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if `parts` is zero.
    pub fn split(self, parts: u32) -> Result<Vec<Self>, CdmError> {
        if parts == 0 {
            return Err(CdmError::new(
                ErrorKind::Config,
                "cannot split a token range into zero parts",
            ));
        }
        let total = self.token_count();
        let parts = u128::from(parts).min(total);
        // `parts` is now in `1..=total`, so neither division nor the loop below can misbehave.
        let base = total / parts;
        let remainder = total % parts;

        let mut out = Vec::with_capacity(parts as usize);
        let mut cursor = self.min;
        for index in 0..parts {
            let extra = u128::from(index < remainder);
            let span = base + extra;
            // `span - 1` fits in i128 because `span <= total` and the whole span starts at
            // `cursor`, which the accumulated arithmetic keeps within `[min, max]`.
            let end = cursor.wrapping_add((span - 1).cast_signed());
            out.push(Self {
                min: cursor,
                max: end,
            });
            cursor = end.wrapping_add(1);
        }
        Ok(out)
    }
}

impl fmt::Display for TokenRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}, {}]", self.min, self.max)
    }
}

/// The position of a range within the plan of a single run.
///
/// Ranges are identified positionally rather than by their bounds: `TOK-006` shuffles the plan
/// before scheduling, and `TRK-033` may subdivide a range on rerun, so bounds alone are neither
/// stable nor unique across runs. The tracking table keys on `token_min` (`TRK-010`); this id is
/// the in-process handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PartitionRangeId(u32);

impl PartitionRangeId {
    /// Creates an id from its zero-based position in the plan.
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// The zero-based position in the plan.
    pub const fn index(&self) -> u32 {
        self.0
    }
}

impl fmt::Display for PartitionRangeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "range#{}", self.0)
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
    use super::*;

    #[test]
    fn tok_004_bounds_are_held_with_i128_precision() {
        let full = TokenRange::RANDOM_FULL;
        assert_eq!(full.min(), 0);
        assert_eq!(full.max(), i128::MAX);
        // The Murmur3 ring is the i64 range widened, not truncated.
        assert_eq!(TokenRange::MURMUR3_FULL.min(), i128::from(i64::MIN));
        assert_eq!(TokenRange::MURMUR3_FULL.max(), i128::from(i64::MAX));
    }

    #[test]
    fn tok_004_token_count_of_the_full_murmur3_ring_does_not_overflow() {
        // The count that overflows an i64 in the Java implementation.
        assert_eq!(TokenRange::MURMUR3_FULL.token_count(), 1u128 << 64);
        assert_eq!(TokenRange::new(-1, 1).unwrap().token_count(), 3);
        assert_eq!(TokenRange::new(7, 7).unwrap().token_count(), 1);
    }

    #[test]
    fn tok_004_inverted_bounds_are_rejected() {
        let err = TokenRange::new(10, 9).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.to_string().contains("inverted token range"));
    }

    #[test]
    fn tok_004_contains_is_inclusive_at_both_ends() {
        let range = TokenRange::new(-5, 5).unwrap();
        assert!(range.contains(-5));
        assert!(range.contains(0));
        assert!(range.contains(5));
        assert!(!range.contains(-6));
        assert!(!range.contains(6));
    }

    #[test]
    fn tok_004_contains_range_and_intersects_agree_on_edges() {
        let outer = TokenRange::new(0, 100).unwrap();
        let inner = TokenRange::new(10, 20).unwrap();
        let overlapping = TokenRange::new(100, 200).unwrap();
        let disjoint = TokenRange::new(101, 200).unwrap();

        assert!(outer.contains_range(inner));
        assert!(!inner.contains_range(outer));
        assert!(outer.intersects(overlapping));
        assert!(!outer.intersects(disjoint));
        assert!(outer.intersects(outer));
    }

    #[test]
    fn tok_004_split_covers_the_range_exactly_and_spreads_the_remainder() {
        let range = TokenRange::new(0, 9).unwrap();
        let parts = range.split(3).unwrap();
        assert_eq!(parts.len(), 3);
        // 10 tokens over 3 parts: 4, 3, 3.
        assert_eq!(parts[0], TokenRange::new(0, 3).unwrap());
        assert_eq!(parts[1], TokenRange::new(4, 6).unwrap());
        assert_eq!(parts[2], TokenRange::new(7, 9).unwrap());
        assert_eq!(
            parts
                .iter()
                .copied()
                .map(TokenRange::token_count)
                .sum::<u128>(),
            range.token_count()
        );
    }

    #[test]
    fn tok_004_split_of_the_full_ring_stays_within_i128() {
        let parts = TokenRange::MURMUR3_FULL.split(4).unwrap();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].min(), i128::from(i64::MIN));
        assert_eq!(parts[3].max(), i128::from(i64::MAX));
        for pair in parts.windows(2) {
            assert_eq!(pair[0].max() + 1, pair[1].min());
        }
    }

    #[test]
    fn tok_004_split_into_more_parts_than_tokens_yields_one_token_each() {
        let parts = TokenRange::new(0, 2).unwrap().split(100).unwrap();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.token_count() == 1));
    }

    #[test]
    fn tok_004_split_into_one_part_is_the_identity() {
        let range = TokenRange::new(-3, 8).unwrap();
        assert_eq!(range.split(1).unwrap(), vec![range]);
    }

    #[test]
    fn err_004_split_into_zero_parts_errors_rather_than_panicking() {
        let err = TokenRange::new(0, 10).unwrap().split(0).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
    }

    #[test]
    fn tok_004_ranges_order_lexicographically_and_render_as_a_closed_interval() {
        let a = TokenRange::new(0, 10).unwrap();
        let b = TokenRange::new(0, 11).unwrap();
        let c = TokenRange::new(1, 2).unwrap();
        assert!(a < b);
        assert!(b < c);
        assert_eq!(a.to_string(), "[0, 10]");
    }

    #[test]
    fn tok_004_range_is_copy_and_serde_round_trips() {
        let range = TokenRange::new(-1, i128::from(i64::MAX)).unwrap();
        let copied = range;
        assert_eq!(copied, range);
        let json = serde_json::to_string(&range).unwrap();
        assert_eq!(serde_json::from_str::<TokenRange>(&json).unwrap(), range);
    }

    #[test]
    fn tok_006_partition_range_id_is_positional_and_ordered() {
        let first = PartitionRangeId::new(0);
        let second = PartitionRangeId::new(1);
        assert!(first < second);
        assert_eq!(second.index(), 1);
        assert_eq!(second.to_string(), "range#1");
        assert_eq!(
            serde_json::from_str::<PartitionRangeId>("7").unwrap(),
            PartitionRangeId::new(7)
        );
    }
}
