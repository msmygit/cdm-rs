//! Progress, ranges by state, and the estimated time to completion (`MET-010`, `MET-011`).
//!
//! # Why the obvious formula is wrong
//!
//! `MET-011` says progress is `ranges_completed / ranges_total`, refined by `system.size_estimates`
//! row estimates. The first half of that sentence is a trap, and this module deliberately does not
//! implement it as written.
//!
//! Ranges are not equal. Java's splitter (`TOK-003`) divides the *token space* into `num_parts`
//! equal slices, and the last slice absorbs whatever is left over; the `ring_aware` strategy
//! (`TOK-008`) gives each ring segment a share proportional to its width, so a cluster with an
//! uneven ring produces uneven ranges by design; `TRK-033`'s rerun multiplier subdivides some
//! ranges and not others; and a resume (`TRK-031`) re-plans an arbitrary subset. Counting ranges
//! therefore measures how many *pieces of paper* have been dealt with, not how much work is left,
//! and on a real plan the two differ by an order of magnitude.
//!
//! So progress here is **weighted**. Every range carries a weight, and progress is
//! `completed_weight / total_weight`. Two weightings are available:
//!
//! | Weighting | Weight of a range | Assumes |
//! |---|---|---|
//! | [`ProgressTracker::by_token_span`] | its token count | rows are spread evenly around the ring |
//! | [`ProgressTracker::by_estimates`] | the estimated partitions it holds | `system.size_estimates` is roughly current |
//!
//! Both are estimates, and neither is good. The honest accounting of their error is in
//! [`Progress::eta`].
//!
//! # A range is atomic
//!
//! A range in flight contributes nothing to progress until it completes, because that is the only
//! thing the run actually knows: `ENG-002` makes the range the unit of atomicity, and a range that
//! is 90% read has produced no durable outcome. With the default 5000 ranges this granularity is
//! invisible; with `perfops.num_parts = 4` it is a fifth of the bar, and the tracker says so
//! through [`Progress::ranges_in_flight`].

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use cdm_core::{RunStatus, TokenRange};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// A row estimate for a slice of the ring, as `system.size_estimates` reports it.
///
/// `cdm-metrics` cannot depend on `cdm-engine` (`ARCHITECTURE.md` §3), so the planner's
/// `SizeEstimate` arrives here as this minimal shape. Note that `system.size_estimates` counts
/// **partitions**, not rows — `SPEC.md` §15.2 and `TOK-009` both say "rows", which is wrong for
/// any table with clustering columns. The distinction does not affect a *relative* weighting,
/// which is all this module uses it for, and the field is named for what the column holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeEstimate {
    /// The slice of the ring the estimate covers.
    pub range: TokenRange,
    /// Estimated partitions in it.
    pub partitions: u64,
}

impl RangeEstimate {
    /// An estimate for one slice of the ring.
    #[must_use]
    pub const fn new(range: TokenRange, partitions: u64) -> Self {
        Self { range, partitions }
    }
}

/// The fraction of the total weight that must complete before an ETA is offered.
///
/// Two percent. Below that the sample is one or two ranges, the ETA swings by hundreds of percent
/// between updates, and an operator who sees "4 minutes remaining" become "3 hours remaining"
/// stops believing any of it. Reporting nothing is better than reporting noise.
pub const ETA_MIN_FRACTION: f64 = 0.02;

/// The number of completed ranges that also unlocks an ETA, for plans too small for
/// [`ETA_MIN_FRACTION`] to be reached early.
pub const ETA_MIN_RANGES: u64 = 16;

/// Tracks which ranges are pending, in flight and done, and what that implies (`MET-011`).
///
/// One per run. Every method takes `&self`; the state is behind a mutex taken once per range
/// transition, which happens twice per range rather than once per row.
///
/// ```
/// use std::time::{Duration, Instant};
/// use cdm_core::{RunStatus, TokenRange};
/// use cdm_metrics::ProgressTracker;
///
/// let start = Instant::now();
/// let ranges = TokenRange::MURMUR3_FULL.split(4)?;
/// let progress = ProgressTracker::by_token_span(&ranges, start);
///
/// progress.range_started(ranges[0]);
/// progress.range_completed(ranges[0], RunStatus::Pass);
///
/// let now = start + Duration::from_secs(60);
/// let snapshot = progress.snapshot_at(now);
/// assert_eq!(snapshot.ranges_completed, 1);
/// assert!((snapshot.weight_fraction - 0.25).abs() < 1e-9);
/// // Three quarters of the ring took a minute to leave, so about three minutes remain.
/// assert_eq!(snapshot.eta, Some(Duration::from_secs(180)));
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug)]
pub struct ProgressTracker {
    started_at: Instant,
    total_weight: u128,
    ranges_total: u64,
    state: Mutex<TrackerState>,
}

/// The mutable half of a tracker.
#[derive(Debug)]
struct TrackerState {
    /// Every planned range and its weight.
    weights: BTreeMap<TokenRange, u128>,
    /// Ranges claimed by a worker and not yet finished (`ENG-002`).
    in_flight: BTreeSet<TokenRange>,
    /// Terminal status counts, keyed by [`RunStatus`].
    by_status: BTreeMap<RunStatus, u64>,
    completed_weight: u128,
    ranges_completed: u64,
}

impl ProgressTracker {
    /// Weights every range by its token count (`MET-011`).
    ///
    /// The weighting available without touching the cluster, and the one `cdm plan` and a
    /// `filter.token.*`-bounded run get. It is exactly right for a synthetic uniform data set and
    /// approximately right for a real one; see [`Progress::eta`] for how wrong it can be.
    #[must_use]
    pub fn by_token_span(ranges: &[TokenRange], now: Instant) -> Self {
        Self::with_weights(
            ranges.iter().map(|range| (*range, range.token_count())),
            now,
        )
    }

    /// Weights every range by the partitions `system.size_estimates` puts in it (`MET-011`).
    ///
    /// An estimate is prorated across the ranges it overlaps, in the same way the adaptive planner
    /// prorates it (`TOK-010`). A range no estimate covers — a keyspace the estimates are stale
    /// for, or a ring the table has only just been written to — falls back to its token count
    /// scaled to the mean weight of the ranges that *are* covered, so that an uncovered range is
    /// neither invisible nor dominant.
    #[must_use]
    pub fn by_estimates(ranges: &[TokenRange], estimates: &[RangeEstimate], now: Instant) -> Self {
        let weighted: Vec<(TokenRange, u128)> = ranges
            .iter()
            .map(|range| (*range, estimated_partitions_in(*range, estimates)))
            .collect();

        let covered: Vec<&(TokenRange, u128)> =
            weighted.iter().filter(|(_, weight)| *weight > 0).collect();
        if covered.is_empty() {
            // No estimate touches the plan at all: this is the token-span weighting.
            return Self::by_token_span(ranges, now);
        }

        let covered_partitions: u128 = covered.iter().map(|(_, weight)| *weight).sum();
        let covered_tokens: u128 = covered
            .iter()
            .map(|(range, _)| range.token_count())
            .sum::<u128>()
            .max(1);

        Self::with_weights(
            weighted.into_iter().map(|(range, partitions)| {
                let weight = if partitions > 0 {
                    partitions
                } else {
                    // Partitions per token, applied to the uncovered range.
                    (range.token_count().saturating_mul(covered_partitions) / covered_tokens).max(1)
                };
                (range, weight)
            }),
            now,
        )
    }

    /// Builds a tracker from explicit weights.
    ///
    /// A weight of zero is raised to one: a range with no weight could never contribute to
    /// progress, and a plan of nothing but such ranges would divide by zero.
    #[must_use]
    pub fn with_weights(
        ranges: impl IntoIterator<Item = (TokenRange, u128)>,
        now: Instant,
    ) -> Self {
        let weights: BTreeMap<TokenRange, u128> = ranges
            .into_iter()
            .map(|(range, weight)| (range, weight.max(1)))
            .collect();
        let total_weight = weights.values().copied().sum::<u128>().max(1);
        let ranges_total = u64::try_from(weights.len()).unwrap_or(u64::MAX);
        Self {
            started_at: now,
            total_weight,
            ranges_total,
            state: Mutex::new(TrackerState {
                weights,
                in_flight: BTreeSet::new(),
                by_status: BTreeMap::new(),
                completed_weight: 0,
                ranges_completed: 0,
            }),
        }
    }

    /// How many ranges the plan holds.
    #[must_use]
    pub const fn ranges_total(&self) -> u64 {
        self.ranges_total
    }

    /// The sum of every range's weight.
    #[must_use]
    pub const fn total_weight(&self) -> u128 {
        self.total_weight
    }

    /// Records that a worker has claimed a range (`ENG-002`).
    ///
    /// A range the tracker does not know — one a plugin or a resume added after planning — is
    /// ignored rather than rejected: progress is a diagnostic, and it must not be able to fail a
    /// run.
    pub fn range_started(&self, range: TokenRange) {
        let mut state = self.state.lock();
        if state.weights.contains_key(&range) {
            state.in_flight.insert(range);
        }
    }

    /// Records that a range reached a terminal status (`ENG-002`, `MET-011`).
    ///
    /// Every terminal status counts as progress, failure included: `ENG-008` is explicit that a
    /// failed range does not stop the run, and a run whose ETA ignored its failures would never
    /// reach 100%. A range abandoned by shutdown (`ENG-010`) is reported as
    /// [`RunStatus::Started`], which is not terminal, and is put back to pending.
    pub fn range_completed(&self, range: TokenRange, status: RunStatus) {
        let mut state = self.state.lock();
        state.in_flight.remove(&range);
        if status == RunStatus::Started || status == RunStatus::NotStarted {
            return; // abandoned mid-flight; it is pending again
        }
        // Removing the weight is what makes this idempotent: a range counted twice — a duplicate
        // observer notification, or a range reclaimed after a lease expiry (`DST-012`) — must not
        // push progress past 100%.
        let Some(weight) = state.weights.remove(&range) else {
            return;
        };
        state.completed_weight = state.completed_weight.saturating_add(weight);
        state.ranges_completed += 1;
        *state.by_status.entry(status).or_insert(0) += 1;
    }

    /// The current progress, reading the clock.
    #[must_use]
    pub fn snapshot(&self) -> Progress {
        self.snapshot_at(Instant::now())
    }

    /// The current progress at an explicit instant (`MET-011`).
    #[must_use]
    pub fn snapshot_at(&self, now: Instant) -> Progress {
        let state = self.state.lock();
        let elapsed = now.saturating_duration_since(self.started_at);
        let weight_fraction = ratio(state.completed_weight, self.total_weight);
        let ranges_fraction = ratio(
            u128::from(state.ranges_completed),
            u128::from(self.ranges_total.max(1)),
        );
        let in_flight = u64::try_from(state.in_flight.len()).unwrap_or(u64::MAX);

        Progress {
            ranges_total: self.ranges_total,
            ranges_completed: state.ranges_completed,
            ranges_in_flight: in_flight,
            ranges_pending: self
                .ranges_total
                .saturating_sub(state.ranges_completed)
                .saturating_sub(in_flight),
            ranges_by_status: state
                .by_status
                .iter()
                .map(|(status, count)| (status.as_str().to_owned(), *count))
                .collect(),
            weight_fraction,
            ranges_fraction,
            elapsed,
            eta: estimate_remaining(weight_fraction, elapsed, state.ranges_completed),
        }
    }
}

/// `completed / total` as a fraction clamped to `[0, 1]`.
///
/// The clamp matters: a duplicate completion or a reclaimed lease could otherwise show 103%, and a
/// progress bar that overshoots is a bug report.
#[allow(clippy::cast_precision_loss)]
fn ratio(completed: u128, total: u128) -> f64 {
    if total == 0 {
        return 0.0;
    }
    ((completed as f64) / (total as f64)).clamp(0.0, 1.0)
}

/// The ETA: elapsed time scaled by the weight still outstanding (`MET-011`).
///
/// `None` until enough of the run has completed for the extrapolation to mean anything — see
/// [`ETA_MIN_FRACTION`] and [`ETA_MIN_RANGES`].
fn estimate_remaining(fraction: f64, elapsed: Duration, ranges_completed: u64) -> Option<Duration> {
    if fraction <= 0.0 || elapsed.is_zero() {
        return None;
    }
    if fraction < ETA_MIN_FRACTION && ranges_completed < ETA_MIN_RANGES {
        return None;
    }
    if fraction >= 1.0 {
        return Some(Duration::ZERO);
    }
    let remaining = elapsed.as_secs_f64() * (1.0 - fraction) / fraction;
    // A run that has barely started can produce an absurd extrapolation; `try_from_secs_f64`
    // rejects a value that does not fit a `Duration` rather than saturating silently.
    Duration::try_from_secs_f64(remaining).ok()
}

/// Estimated partitions inside `range`, prorated across the estimates that overlap it.
///
/// The same proration the adaptive planner performs (`TOK-010`), reproduced here because the
/// dependency graph does not allow calling it.
fn estimated_partitions_in(range: TokenRange, estimates: &[RangeEstimate]) -> u128 {
    let mut total: u128 = 0;
    for estimate in estimates {
        if !estimate.range.intersects(range) {
            continue;
        }
        let estimate_tokens = estimate.range.token_count();
        if estimate_tokens == 0 {
            continue;
        }
        let overlap_min = range.min().max(estimate.range.min());
        let overlap_max = range.max().min(estimate.range.max());
        let Ok(overlap) = TokenRange::new(overlap_min, overlap_max) else {
            continue;
        };
        total = total.saturating_add(
            u128::from(estimate.partitions).saturating_mul(overlap.token_count()) / estimate_tokens,
        );
    }
    total
}

/// How far along a run is (`MET-011`).
///
/// Serialisable: this is what `GET /v1/runs/{id}` reports as progress (`API-003`), what the
/// terminal UI of `MET-031` draws, and what the exporters of `MET-020` and `MET-021` publish.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// Ranges in the plan.
    pub ranges_total: u64,
    /// Ranges that reached a terminal status, failures included (`ENG-008`).
    pub ranges_completed: u64,
    /// Ranges claimed by a worker and still running.
    pub ranges_in_flight: u64,
    /// Ranges no worker has claimed yet.
    pub ranges_pending: u64,
    /// Completed ranges by terminal status, keyed by the Java spelling of `TRK-012`
    /// (`PASS`, `FAIL`, `DIFF`, `DIFF_CORRECTED`) — `MET-010`'s "ranges in each state".
    pub ranges_by_status: BTreeMap<String, u64>,
    /// Completed weight over total weight: the number a progress bar should draw.
    pub weight_fraction: f64,
    /// Completed ranges over total ranges: `MET-011`'s literal formula, kept because it is what
    /// the specification asks for and because the gap between the two is itself diagnostic.
    pub ranges_fraction: f64,
    /// Wall-clock time since the run started.
    pub elapsed: Duration,
    /// The estimated time to completion (`MET-010`, `MET-011`).
    ///
    /// # How wrong this is
    ///
    /// The estimate is `elapsed × (1 − fraction) / fraction`: it assumes the rest of the run will
    /// proceed at the average speed of the part already done. Four things break that assumption,
    /// in roughly descending order of how badly:
    ///
    /// 1. **Weight is not work.** A token-span weighting assumes rows are spread evenly around the
    ///    ring, which a hot partition, a low-cardinality partition key or a recently-added node
    ///    makes false. A size-estimate weighting assumes `system.size_estimates` is current, and
    ///    it is refreshed on compaction, so on a freshly-loaded table it can be wildly out.
    /// 2. **Throughput is not constant.** The rate limiter (`ENG-004`) makes it *nearly* constant
    ///    while it is the binding constraint, which is the case that estimates best; a target that
    ///    starts to overload, or an adaptive limiter (`ENG-006`) backing off, does not.
    /// 3. **The tail is not the mean.** Straggler ranges dominate the last few percent, so the
    ///    estimate is optimistic near the end. `TOK-010`'s adaptive subdivision exists to reduce
    ///    this, and reduces it rather than removing it.
    /// 4. **Ranges are atomic.** In-flight work is invisible until it lands, so the estimate is
    ///    pessimistic by up to one range's duration per worker.
    ///
    /// Treat it as an order of magnitude, not a departure time. It is `None` until at least
    /// [`ETA_MIN_FRACTION`] of the weight, or [`ETA_MIN_RANGES`] ranges, have completed.
    pub eta: Option<Duration>,
}

impl Progress {
    /// Whether every range has reached a terminal status.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.ranges_completed >= self.ranges_total
    }

    /// Ranges recorded with one terminal status.
    #[must_use]
    pub fn ranges_with(&self, status: RunStatus) -> u64 {
        self.ranges_by_status
            .get(status.as_str())
            .copied()
            .unwrap_or(0)
    }

    /// The projected finish time, given the instant the snapshot was taken.
    #[must_use]
    pub fn finishes_at(&self, now: Instant) -> Option<Instant> {
        self.eta.map(|eta| now + eta)
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

    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    #[test]
    fn met_011_progress_is_weighted_by_token_span_not_by_range_count() {
        // Three ranges: one holds 98% of the ring, two hold 1% each. Completing the two small
        // ones is two thirds of the *ranges* and one fiftieth of the *work*, and a progress bar
        // that says 67% here is lying to an operator who then plans their evening around it.
        let start = Instant::now();
        let ranges = [range(0, 979), range(980, 989), range(990, 999)];
        let progress = ProgressTracker::by_token_span(&ranges, start);

        progress.range_completed(ranges[1], RunStatus::Pass);
        progress.range_completed(ranges[2], RunStatus::Pass);

        let snapshot = progress.snapshot_at(start + Duration::from_secs(10));
        assert_eq!(snapshot.ranges_completed, 2);
        assert!((snapshot.ranges_fraction - 2.0 / 3.0).abs() < 1e-9);
        assert!((snapshot.weight_fraction - 0.02).abs() < 1e-9);
        assert!(!snapshot.is_complete());
    }

    #[test]
    fn met_011_estimates_reweight_the_plan_towards_the_dense_ranges() {
        let start = Instant::now();
        let ranges = [range(0, 99), range(100, 199)];
        // The two halves are the same width, but the first holds ninety times the partitions.
        let estimates = [
            RangeEstimate::new(range(0, 99), 9_000),
            RangeEstimate::new(range(100, 199), 100),
        ];
        let progress = ProgressTracker::by_estimates(&ranges, &estimates, start);
        assert_eq!(progress.total_weight(), 9_100);

        progress.range_completed(ranges[1], RunStatus::Pass);
        let snapshot = progress.snapshot_at(start + Duration::from_secs(1));
        assert!((snapshot.ranges_fraction - 0.5).abs() < 1e-9);
        assert!(
            snapshot.weight_fraction < 0.02,
            "the sparse half is barely any of the work: {}",
            snapshot.weight_fraction
        );
    }

    #[test]
    fn met_011_a_range_no_estimate_covers_is_weighted_by_the_observed_density() {
        let start = Instant::now();
        let ranges = [range(0, 99), range(100, 199)];
        let estimates = [RangeEstimate::new(range(0, 99), 500)];
        let progress = ProgressTracker::by_estimates(&ranges, &estimates, start);
        // The uncovered half is the same width as the covered one, so it inherits its density.
        assert_eq!(progress.total_weight(), 1_000);
    }

    #[test]
    fn met_011_estimates_that_miss_the_plan_entirely_fall_back_to_token_spans() {
        let start = Instant::now();
        let ranges = [range(0, 99), range(100, 199)];
        let estimates = [RangeEstimate::new(range(10_000, 20_000), 1_000_000)];
        let progress = ProgressTracker::by_estimates(&ranges, &estimates, start);
        assert_eq!(progress.total_weight(), 200);
    }

    #[test]
    fn met_011_an_eta_extrapolates_from_the_weight_already_done() {
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(4).unwrap();
        let progress = ProgressTracker::by_token_span(&ranges, start);

        progress.range_completed(ranges[0], RunStatus::Pass);
        let quarter = progress.snapshot_at(start + Duration::from_secs(30));
        assert_eq!(quarter.eta, Some(Duration::from_secs(90)));

        progress.range_completed(ranges[1], RunStatus::Pass);
        let half = progress.snapshot_at(start + Duration::from_secs(60));
        assert_eq!(half.eta, Some(Duration::from_secs(60)));
        assert_eq!(
            half.finishes_at(start + Duration::from_secs(60)),
            Some(start + Duration::from_secs(120))
        );
    }

    #[test]
    fn met_011_no_eta_is_offered_until_the_sample_is_worth_extrapolating_from() {
        let start = Instant::now();
        let ranges: Vec<TokenRange> = (0..1_000)
            .map(|index| range(i128::from(index) * 100, i128::from(index) * 100 + 99))
            .collect();
        let progress = ProgressTracker::by_token_span(&ranges, start);

        // Nothing done: no ETA, and no division by zero.
        assert_eq!(progress.snapshot_at(start).eta, None);
        assert_eq!(
            progress.snapshot_at(start + Duration::from_secs(5)).eta,
            None
        );

        // One range in a thousand is 0.1% — noise, not a forecast.
        progress.range_completed(ranges[0], RunStatus::Pass);
        assert_eq!(
            progress.snapshot_at(start + Duration::from_secs(5)).eta,
            None
        );

        // Twenty of a thousand is 2%, which is where the estimate starts being offered.
        for planned in ranges.iter().take(20).skip(1) {
            progress.range_completed(*planned, RunStatus::Pass);
        }
        let snapshot = progress.snapshot_at(start + Duration::from_secs(20));
        assert_eq!(snapshot.eta, Some(Duration::from_secs(980)));
    }

    #[test]
    fn met_011_a_small_plan_earns_its_eta_by_range_count() {
        // Sixteen ranges of a plan of a thousand is 1.6%, below `ETA_MIN_FRACTION`, but sixteen
        // observations is a perfectly good sample.
        let start = Instant::now();
        let ranges: Vec<TokenRange> = (0..1_000)
            .map(|index| range(i128::from(index) * 100, i128::from(index) * 100 + 99))
            .collect();
        let progress = ProgressTracker::by_token_span(&ranges, start);
        for planned in ranges.iter().take(usize::try_from(ETA_MIN_RANGES).unwrap()) {
            progress.range_completed(*planned, RunStatus::Pass);
        }
        assert!(progress
            .snapshot_at(start + Duration::from_secs(16))
            .eta
            .is_some());
    }

    #[test]
    fn met_011_a_finished_run_reports_no_time_remaining() {
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(2).unwrap();
        let progress = ProgressTracker::by_token_span(&ranges, start);
        for planned in &ranges {
            progress.range_completed(*planned, RunStatus::Pass);
        }
        let snapshot = progress.snapshot_at(start + Duration::from_secs(10));
        assert!(snapshot.is_complete());
        assert_eq!(snapshot.eta, Some(Duration::ZERO));
        assert!((snapshot.weight_fraction - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn met_010_ranges_are_reported_in_each_state() {
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(8).unwrap();
        let progress = ProgressTracker::by_token_span(&ranges, start);

        progress.range_started(ranges[0]);
        progress.range_started(ranges[1]);
        progress.range_completed(ranges[0], RunStatus::Pass);
        progress.range_completed(ranges[2], RunStatus::Fail);
        progress.range_completed(ranges[3], RunStatus::DiffCorrected);

        let snapshot = progress.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(snapshot.ranges_total, 8);
        assert_eq!(snapshot.ranges_completed, 3);
        assert_eq!(snapshot.ranges_in_flight, 1);
        assert_eq!(snapshot.ranges_pending, 4);
        assert_eq!(snapshot.ranges_with(RunStatus::Pass), 1);
        assert_eq!(snapshot.ranges_with(RunStatus::Fail), 1);
        assert_eq!(snapshot.ranges_with(RunStatus::DiffCorrected), 1);
        assert_eq!(snapshot.ranges_with(RunStatus::Diff), 0);
        // Keyed by the `TRK-012` spelling, and a `BTreeMap`, so the order is alphabetical rather
        // than the enum's — an exporter iterating it produces a stable series order either way.
        assert_eq!(
            snapshot.ranges_by_status.keys().collect::<Vec<_>>(),
            vec!["DIFF_CORRECTED", "FAIL", "PASS"]
        );
    }

    #[test]
    fn eng_008_a_failed_range_still_counts_as_progress() {
        // `ENG-008` keeps the run going after a range fails. A progress bar that ignored the
        // failure would stall at 99% on a run that has finished.
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(2).unwrap();
        let progress = ProgressTracker::by_token_span(&ranges, start);
        progress.range_completed(ranges[0], RunStatus::Fail);
        progress.range_completed(ranges[1], RunStatus::Pass);
        let snapshot = progress.snapshot_at(start + Duration::from_secs(1));
        assert!(snapshot.is_complete());
        assert_eq!(snapshot.ranges_with(RunStatus::Fail), 1);
    }

    #[test]
    fn eng_010_a_range_abandoned_by_shutdown_goes_back_to_pending() {
        // `ENG-010` leaves an abandoned range `STARTED`, which `TRK-031` treats as pending. It
        // has produced no outcome, so it must not count as progress.
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(4).unwrap();
        let progress = ProgressTracker::by_token_span(&ranges, start);

        progress.range_started(ranges[0]);
        progress.range_completed(ranges[0], RunStatus::Started);

        let snapshot = progress.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(snapshot.ranges_completed, 0);
        assert_eq!(snapshot.ranges_in_flight, 0);
        assert_eq!(snapshot.ranges_pending, 4);
    }

    #[test]
    fn met_011_progress_never_exceeds_one_however_often_a_range_is_reported() {
        // A reclaimed lease (`DST-012`) or a duplicated observer notification must not push the
        // bar past 100%.
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(2).unwrap();
        let progress = ProgressTracker::by_token_span(&ranges, start);
        for _ in 0..5 {
            progress.range_completed(ranges[0], RunStatus::Pass);
        }
        let snapshot = progress.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(snapshot.ranges_completed, 1);
        assert!((snapshot.weight_fraction - 0.5).abs() < 1e-9);

        // A range that was never planned is ignored rather than counted.
        progress.range_completed(range(-5, -1), RunStatus::Pass);
        progress.range_started(range(-5, -1));
        let after = progress.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(after.ranges_completed, 1);
        assert_eq!(after.ranges_in_flight, 0);
    }

    #[test]
    fn met_011_progress_round_trips_through_json() {
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(2).unwrap();
        let progress = ProgressTracker::by_token_span(&ranges, start);
        progress.range_completed(ranges[0], RunStatus::Pass);

        let snapshot = progress.snapshot_at(start + Duration::from_secs(4));
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"PASS\":1"), "{json}");
        assert_eq!(
            serde_json::from_str::<Progress>(&json).unwrap(),
            snapshot,
            "the API and the TUI read this back"
        );
    }
}
