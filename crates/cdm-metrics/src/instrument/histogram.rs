//! Bucketed distributions: latency percentiles and batch sizes (`MET-010`).
//!
//! `MET-010` asks for `p50/p90/p99/p999` per side and per operation, plus a batch-size
//! distribution and a rate-limiter wait-time distribution. Percentiles cannot be aggregated from
//! averages and must not be computed by keeping every sample, so this module keeps a fixed
//! log-linear bucket array — the same shape HdrHistogram uses, with the same trade: a bounded,
//! lock-free, allocation-free recorder in exchange for a known, bounded relative error.
//!
//! # The error is bounded and stated
//!
//! Values are bucketed by exponent with [`SUB_BUCKETS`] linear sub-buckets inside each power of
//! two, so a bucket is at most `1/SUB_BUCKETS` of its own magnitude wide. A reported percentile is
//! the **upper bound** of the bucket containing the sample, which means it never understates the
//! latency and overstates it by at most 6.25%. That is the right direction to be wrong in for a
//! latency percentile, and 6.25% is far below the run-to-run variance of a Cassandra p99.
//!
//! The whole array is 976 `AtomicU64`s — under 8 KiB — regardless of how many samples it sees, so
//! `NFR-003`'s memory bound is unaffected by run length.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Bits of precision kept below the leading one bit: four, giving sixteen sub-buckets.
const SIGNIFICANT_BITS: u32 = 4;

/// Linear sub-buckets within each power of two.
pub const SUB_BUCKETS: u64 = 1 << SIGNIFICANT_BITS;

/// [`SUB_BUCKETS`] as an index width.
const SUB_BUCKETS_INDEX: usize = 1 << SIGNIFICANT_BITS;

/// How many buckets the array holds: `(64 - 4 + 1) * 16`, enough for every `u64` value including
/// [`u64::MAX`].
///
/// Written out rather than computed, because the computation needs casts between `u32`, `u64` and
/// `usize` that say nothing and lint loudly. `met_010_buckets_are_monotonic_and_small_values_are_exact`
/// checks the arithmetic.
pub const BUCKETS: usize = 976;

/// The percentiles `MET-010` names, as fractions.
pub const PERCENTILES: [f64; 4] = [0.5, 0.9, 0.99, 0.999];

/// The percentiles `MET-010` names, spelled as Prometheus and OTLP quantile labels.
pub const PERCENTILE_LABELS: [&str; 4] = ["0.5", "0.9", "0.99", "0.999"];

/// A lock-free distribution of `u64` values (`MET-010`).
///
/// Records nanoseconds for a latency ([`Histogram::record_duration`]) or a plain count for a batch
/// size ([`Histogram::record`]); the unit is the caller's business and is carried by the metric
/// name, not by the histogram.
///
/// ```
/// use std::time::Duration;
/// use cdm_metrics::Histogram;
///
/// let latency = Histogram::new();
/// for millis in 1..=100 {
///     latency.record_duration(Duration::from_millis(millis));
/// }
///
/// let snapshot = latency.snapshot();
/// assert_eq!(snapshot.count, 100);
/// // The reported percentile never understates, and overstates by at most one bucket width.
/// assert!(snapshot.percentile(0.5) >= 50_000_000);
/// assert!(snapshot.percentile(0.5) <= 54_000_000);
/// ```
#[derive(Debug)]
pub struct Histogram {
    buckets: Box<[AtomicU64; BUCKETS]>,
    count: AtomicU64,
    sum: AtomicU64,
    max: AtomicU64,
}

impl Histogram {
    /// An empty histogram.
    ///
    /// The bucket array is boxed: 976 atomics is more than belongs on a stack, and a histogram is
    /// built once per side and operation at startup, never per range and never per row.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buckets: Box::new(std::array::from_fn(|_| AtomicU64::new(0))),
            count: AtomicU64::new(0),
            sum: AtomicU64::new(0),
            max: AtomicU64::new(0),
        }
    }

    /// Records one value.
    pub fn record(&self, value: u64) {
        self.bucket(bucket_index(value)).fetch_add(1, RELAXED);
        self.count.fetch_add(1, RELAXED);
        self.sum.fetch_add(value, RELAXED);
        self.max.fetch_max(value, RELAXED);
    }

    /// Records one duration, in nanoseconds.
    ///
    /// A duration longer than 584 years saturates, which is the only way `as_nanos` can exceed a
    /// `u64` and is not a latency anybody is waiting on.
    pub fn record_duration(&self, value: Duration) {
        self.record(u64::try_from(value.as_nanos()).unwrap_or(u64::MAX));
    }

    /// How many values have been recorded.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count.load(RELAXED)
    }

    /// A point-in-time reading.
    ///
    /// Recording continues while the snapshot is taken, so `count`, `sum` and the buckets can
    /// disagree by the handful of samples recorded during the read. Percentiles are computed from
    /// the buckets alone and are therefore self-consistent; only the mean can be a sample or two
    /// out, which no percentile consumer will notice.
    #[must_use]
    pub fn snapshot(&self) -> HistogramSnapshot {
        let buckets: Vec<u64> = self.buckets.iter().map(|slot| slot.load(RELAXED)).collect();
        let total: u64 = buckets.iter().sum();
        HistogramSnapshot {
            count: total,
            sum: self.sum.load(RELAXED),
            max: self.max.load(RELAXED),
            percentiles: PERCENTILES.map(|p| percentile_of(&buckets, total, p)),
        }
    }

    /// The bucket at `index`.
    //
    // SAFETY-INVARIANT: `bucket_index` returns a value in `0..BUCKETS` for every `u64` — proved
    // exhaustively over the exponent range by `met_010_every_value_lands_in_a_bucket_in_range` —
    // and the array has exactly `BUCKETS` elements. `get()` here would put an `Option` on the hot
    // path for a case that cannot arise.
    #[allow(clippy::indexing_slicing)]
    fn bucket(&self, index: usize) -> &AtomicU64 {
        &self.buckets[index]
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Relaxed throughout: buckets are independent of one another and of any other memory, and the
/// only consistency a reader needs is provided by the snapshot being explicitly approximate.
const RELAXED: Ordering = Ordering::Relaxed;

/// The bucket a value belongs to.
///
/// Values below [`SUB_BUCKETS`] are counted exactly, one bucket each. Above that, the leading one
/// bit selects a power of two and the next [`SIGNIFICANT_BITS`] bits select a sub-bucket within it.
#[must_use]
pub fn bucket_index(value: u64) -> usize {
    if value < SUB_BUCKETS {
        // Every small value is exact, so a sub-millisecond latency in nanoseconds — or a batch
        // size, which is single-digit — is not smeared across a bucket.
        return narrow(value);
    }
    let exponent = u64::BITS - 1 - value.leading_zeros();
    let shift = exponent - SIGNIFICANT_BITS;
    let sub = narrow((value >> shift) - SUB_BUCKETS);
    // `shift` is at most 59, so the widening cannot fail.
    (usize::try_from(shift).unwrap_or(0) + 1) * SUB_BUCKETS_INDEX + sub
}

/// Narrows a value already known to be below [`SUB_BUCKETS`], i.e. below sixteen.
///
/// `try_from` cannot fail here; reporting zero rather than panicking on the impossible branch is
/// what keeps this module free of a panicking path (`ERR-004`).
fn narrow(value: u64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

/// The largest value that falls in `index` — the value a percentile in that bucket reports.
#[must_use]
pub fn bucket_upper_bound(index: usize) -> u64 {
    let index = u64::try_from(index).unwrap_or(u64::MAX);
    if index < SUB_BUCKETS {
        return index;
    }
    let shift = index / SUB_BUCKETS - 1;
    let sub = index % SUB_BUCKETS;
    // The bucket covers `[(SUB_BUCKETS + sub) << shift, ... + (1 << shift) - 1]`. The very last
    // bucket ends at `u64::MAX`, and computing that as a sum overflows by exactly one, so the
    // addition saturates rather than wrapping to zero.
    ((SUB_BUCKETS + sub) << shift).saturating_add((1 << shift) - 1)
}

/// The value at `fraction` of the distribution, or zero when nothing has been recorded.
fn percentile_of(buckets: &[u64], total: u64, fraction: f64) -> u64 {
    if total == 0 {
        return 0;
    }
    // `ceil` so that p50 of a single sample is that sample, not the empty bucket before it. The
    // product is between 1 and `total`, so the narrowing back to `u64` cannot lose anything that
    // a percentile of a bucketed distribution could notice.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    let rank = ((total as f64) * fraction).ceil().max(1.0) as u64;
    let mut seen = 0_u64;
    for (index, count) in buckets.iter().enumerate() {
        seen += count;
        if seen >= rank {
            return bucket_upper_bound(index);
        }
    }
    // Unreachable while `total` is the sum of `buckets`; reported as the largest bucket rather
    // than by panicking (`ERR-004`).
    bucket_upper_bound(BUCKETS - 1)
}

/// A distribution at one instant (`MET-010`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HistogramSnapshot {
    /// How many values were recorded.
    pub count: u64,
    /// Their sum, in the histogram's unit. Prometheus and OTLP both want this alongside the
    /// percentiles so that a mean can be computed across scrapes.
    pub sum: u64,
    /// The largest value recorded, exactly — not bucketed.
    pub max: u64,
    /// The percentiles of [`PERCENTILES`], in that order.
    pub percentiles: [u64; PERCENTILES.len()],
}

impl HistogramSnapshot {
    /// The recorded value at `fraction`, which must be one of [`PERCENTILES`].
    ///
    /// An unknown fraction reports zero rather than failing: a percentile is a diagnostic, and no
    /// caller should have to handle an error to print one.
    #[must_use]
    pub fn percentile(&self, fraction: f64) -> u64 {
        PERCENTILES
            .iter()
            .position(|candidate| (candidate - fraction).abs() < f64::EPSILON)
            .and_then(|index| self.percentiles.get(index).copied())
            .unwrap_or(0)
    }

    /// The percentiles paired with their Prometheus and OTLP quantile labels.
    #[must_use]
    pub fn labelled(&self) -> Vec<(&'static str, u64)> {
        PERCENTILE_LABELS
            .into_iter()
            .zip(self.percentiles)
            .collect()
    }

    /// Whether anything has been recorded. An empty distribution is not exported at all, so an
    /// operation a job never performs contributes no series.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The sum interpreted as nanoseconds, for the histograms that hold durations.
    #[must_use]
    pub const fn sum_duration(&self) -> Duration {
        Duration::from_nanos(self.sum)
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
    use std::sync::Arc;
    use std::thread;

    use super::*;

    #[test]
    fn met_010_every_value_lands_in_a_bucket_in_range() {
        // The invariant the `indexing_slicing` allow in `Histogram::bucket` rests on: exhaustive
        // over the small values and over every exponent, plus the extremes.
        let mut values: Vec<u64> = (0..1_000).collect();
        for exponent in 0..64 {
            let base = 1_u64 << exponent;
            values.extend([base, base + 1, base.saturating_mul(3) / 2]);
        }
        values.extend([u64::MAX, u64::MAX - 1]);

        for value in values {
            let index = bucket_index(value);
            assert!(index < BUCKETS, "{value} indexed {index}");
            assert!(
                bucket_upper_bound(index) >= value,
                "{value} in bucket {index} whose bound is {}",
                bucket_upper_bound(index)
            );
        }
    }

    #[test]
    fn met_010_buckets_are_monotonic_and_small_values_are_exact() {
        for value in 0..SUB_BUCKETS {
            assert_eq!(bucket_index(value), usize::try_from(value).unwrap());
            assert_eq!(bucket_upper_bound(usize::try_from(value).unwrap()), value);
        }
        for value in 0..10_000_u64 {
            assert!(bucket_index(value) <= bucket_index(value + 1));
        }
        assert_eq!(
            BUCKETS,
            (usize::try_from(u64::BITS - SIGNIFICANT_BITS).unwrap() + 1) * SUB_BUCKETS_INDEX
        );
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn met_010_the_relative_error_is_bounded_by_one_sub_bucket() {
        // The claim the module documentation makes: a reported value never understates, and
        // overstates by at most 1/SUB_BUCKETS of the value.
        for exponent in SIGNIFICANT_BITS..64 {
            let value = (1_u64 << exponent) + 12_345 % (1_u64 << exponent);
            let bound = bucket_upper_bound(bucket_index(value));
            assert!(bound >= value, "{value} reported as {bound}");
            let error = (bound - value) as f64 / value as f64;
            assert!(error <= 1.0 / SUB_BUCKETS as f64, "{value}: {error}");
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn met_010_percentiles_are_reported_for_a_known_distribution() {
        let histogram = Histogram::new();
        for value in 1..=1_000_u64 {
            histogram.record(value);
        }
        let snapshot = histogram.snapshot();

        assert_eq!(snapshot.count, 1_000);
        assert_eq!(snapshot.sum, 500_500);
        assert_eq!(snapshot.max, 1_000);
        for (fraction, expected) in [(0.5, 500_u64), (0.9, 900), (0.99, 990), (0.999, 999)] {
            let reported = snapshot.percentile(fraction);
            assert!(reported >= expected, "p{fraction}: {reported} < {expected}");
            let error = (reported - expected) as f64 / expected as f64;
            assert!(error <= 1.0 / SUB_BUCKETS as f64, "p{fraction}: {error}");
        }
        assert_eq!(snapshot.labelled().len(), 4);
        assert_eq!(snapshot.labelled()[0].0, "0.5");
        assert_eq!(snapshot.percentile(0.75), 0, "an unknown fraction is zero");
    }

    #[test]
    fn met_010_an_empty_histogram_reports_zeroes_and_is_not_exported() {
        let snapshot = Histogram::default().snapshot();
        assert!(snapshot.is_empty());
        assert_eq!(snapshot.count, 0);
        assert_eq!(snapshot.percentiles, [0; 4]);
        assert_eq!(snapshot.max, 0);
    }

    #[test]
    fn met_010_a_single_sample_is_its_own_median() {
        let histogram = Histogram::new();
        histogram.record_duration(Duration::from_millis(7));
        let snapshot = histogram.snapshot();
        assert_eq!(snapshot.count, 1);
        assert_eq!(snapshot.sum_duration(), Duration::from_millis(7));
        assert!(snapshot.percentile(0.5) >= 7_000_000);
        assert!(snapshot.percentile(0.999) >= 7_000_000);
    }

    #[test]
    fn met_010_a_saturating_duration_does_not_overflow_the_recorder() {
        let histogram = Histogram::new();
        histogram.record_duration(Duration::from_secs(u64::MAX / 2));
        assert_eq!(histogram.count(), 1);
        assert!(histogram.snapshot().max > 0);
    }

    #[test]
    fn met_010_concurrent_recording_loses_no_sample() {
        const THREADS: u64 = 8;
        const PER_THREAD: u64 = 5_000;

        let histogram = Arc::new(Histogram::new());
        let handles: Vec<_> = (0..THREADS)
            .map(|thread_index| {
                let histogram = Arc::clone(&histogram);
                thread::spawn(move || {
                    for value in 0..PER_THREAD {
                        histogram.record(value + thread_index);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(histogram.count(), THREADS * PER_THREAD);
        assert_eq!(histogram.snapshot().count, THREADS * PER_THREAD);
    }
}
