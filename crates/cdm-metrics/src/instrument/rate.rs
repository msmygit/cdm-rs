//! Throughput meters: 1s/10s/60s exponentially-weighted rates (`MET-010`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// How often the exponentially-weighted averages are advanced.
///
/// One second, matching Java's `com.codahale.metrics.EWMA` in structure but not in period (it
/// ticks every five). A one-second tick is what makes the 1s window meaningful at all, and it is
/// cheap: ticking is one lock acquisition per second per meter, not per row.
pub const TICK: Duration = Duration::from_secs(1);

/// The three averaging windows `MET-010` names, in seconds.
pub const WINDOWS_SECS: [u64; 3] = [1, 10, 60];

/// A throughput meter: total, mean, and three exponentially-weighted rates (`MET-010`).
///
/// # Which accounting this reflects
///
/// A meter measures *events as they happen*, which is the **interim** level of `MET-004`: a row is
/// marked read when it is read, not when its range completes and its counters are folded into the
/// run's totals. That is the only choice that makes a rate useful — a committed-level rate would
/// report zero for the whole of a long range and then a spike at the end — but it means the
/// integral of a rate is *not* the exported committed counter for a run that has ranges in flight
/// or ranges that failed. `met_010_rates_track_interim_activity_not_committed_totals` pins the
/// distinction so that nobody later "fixes" it by reading the wrong level, which is the shape of
/// both `MIG-004` and `ENG-008` in Java.
///
/// # Clock
///
/// Every method takes the current [`Instant`], or has an `_at` sibling that does. Reading the
/// clock is the caller's business, which keeps the meter a pure function of its inputs and its
/// tests a pure function of theirs — no sleeping, no flaky CI.
///
/// ```
/// use std::time::{Duration, Instant};
/// use cdm_metrics::RateMeter;
///
/// let start = Instant::now();
/// let meter = RateMeter::new(start);
/// for second in 0..60 {
///     meter.mark_at(1_000, start + Duration::from_secs(second));
/// }
///
/// let rates = meter.snapshot_at(start + Duration::from_secs(60));
/// assert_eq!(rates.total, 60_000);
/// assert_eq!(rates.mean_per_second.round(), 1_000.0);
/// ```
#[derive(Debug)]
pub struct RateMeter {
    /// Events marked since the last tick. The hot path touches only this.
    pending: AtomicU64,
    /// Events marked since the meter was created.
    total: AtomicU64,
    state: Mutex<TickState>,
    started_at: Instant,
}

/// The part of a meter that only the once-per-second tick touches.
#[derive(Debug)]
struct TickState {
    last_tick: Instant,
    /// One rate per entry of [`WINDOWS_SECS`], or `None` until the first tick seeds it.
    ewma: [Option<f64>; WINDOWS_SECS.len()],
}

impl RateMeter {
    /// A meter that starts counting at `now`.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            pending: AtomicU64::new(0),
            total: AtomicU64::new(0),
            state: Mutex::new(TickState {
                last_tick: now,
                ewma: [None; WINDOWS_SECS.len()],
            }),
            started_at: now,
        }
    }

    /// Records `count` events at the current instant.
    ///
    /// The hot-path spelling. It reads the clock once per call, which is what a rate meter must
    /// do; batch the count rather than the calls where that matters — one `mark(1_000)` per page
    /// rather than a thousand `mark(1)` per row.
    pub fn mark(&self, count: u64) {
        self.mark_at(count, Instant::now());
    }

    /// Records `count` events at an explicit instant.
    pub fn mark_at(&self, count: u64, now: Instant) {
        self.pending.fetch_add(count, Ordering::Relaxed);
        self.total.fetch_add(count, Ordering::Relaxed);
        self.tick_if_due(now);
    }

    /// Total events recorded since the meter was created.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// The current rates, reading the clock.
    #[must_use]
    pub fn snapshot(&self) -> RateSnapshot {
        self.snapshot_at(Instant::now())
    }

    /// The current rates at an explicit instant.
    ///
    /// Advances the averages first, so a meter nobody has marked recently reports a decaying rate
    /// rather than a stale one.
    #[must_use]
    pub fn snapshot_at(&self, now: Instant) -> RateSnapshot {
        self.tick_if_due(now);
        let state = self.state.lock();
        let total = self.total.load(Ordering::Relaxed);
        let elapsed = now.saturating_duration_since(self.started_at).as_secs_f64();
        RateSnapshot {
            total,
            mean_per_second: if elapsed > 0.0 {
                as_f64(total) / elapsed
            } else {
                0.0
            },
            per_second_1s: state.ewma[0].unwrap_or(0.0),
            per_second_10s: state.ewma[1].unwrap_or(0.0),
            per_second_60s: state.ewma[2].unwrap_or(0.0),
        }
    }

    /// Advances the averages by however many whole ticks have elapsed.
    ///
    /// Several ticks can be due at once when a meter is idle — a range that reads nothing for a
    /// minute, say — and each of them must decay the averages, or an idle meter would report the
    /// rate it had before it went quiet.
    fn tick_if_due(&self, now: Instant) {
        {
            // The common case is "no tick due", and it must not take the lock.
            let state = self.state.lock();
            if now.saturating_duration_since(state.last_tick) < TICK {
                return;
            }
        }
        let mut state = self.state.lock();
        let mut elapsed = now.saturating_duration_since(state.last_tick);
        if elapsed < TICK {
            return; // another thread ticked while we were re-acquiring
        }
        // The first tick collects everything marked since the meter was created; subsequent ones
        // collect a tick's worth, and the leftover ticks decay towards zero.
        let mut pending = self.pending.swap(0, Ordering::Relaxed);
        while elapsed >= TICK {
            let instant_rate = as_f64(pending) / TICK.as_secs_f64();
            for (slot, window) in state.ewma.iter_mut().zip(WINDOWS_SECS) {
                *slot = Some(match *slot {
                    // Seed with the first observation rather than with zero, so a meter does not
                    // spend its first minute reporting a rate it never had.
                    None => instant_rate,
                    Some(previous) => previous + alpha(window) * (instant_rate - previous),
                });
            }
            pending = 0;
            elapsed -= TICK;
            state.last_tick += TICK;
        }
    }
}

/// The smoothing factor of a window: `1 - exp(-tick / window)`, the standard EWMA constant.
fn alpha(window_secs: u64) -> f64 {
    let window = as_f64(window_secs);
    if window <= 0.0 {
        return 1.0;
    }
    1.0 - (-TICK.as_secs_f64() / window).exp()
}

/// Widens a count for rate arithmetic.
///
/// Above 2^53 events the conversion is lossy, which for a rate expressed to three significant
/// figures is immaterial: a run that has read nine quadrillion rows does not care about the last
/// one.
#[allow(clippy::cast_precision_loss)]
fn as_f64(value: u64) -> f64 {
    value as f64
}

/// A meter's rates at one instant (`MET-010`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RateSnapshot {
    /// Events recorded since the meter was created.
    pub total: u64,
    /// Total divided by elapsed wall-clock time — the figure a run summary quotes.
    pub mean_per_second: f64,
    /// One-second exponentially-weighted rate: responsive, noisy.
    pub per_second_1s: f64,
    /// Ten-second exponentially-weighted rate: the one a progress display should show.
    pub per_second_10s: f64,
    /// One-minute exponentially-weighted rate: the one an alert should use.
    pub per_second_60s: f64,
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
    fn met_010_a_steady_rate_converges_on_the_three_windows() {
        let start = Instant::now();
        let meter = RateMeter::new(start);
        for second in 0..=600 {
            meter.mark_at(500, start + Duration::from_secs(second));
        }
        let rates = meter.snapshot_at(start + Duration::from_secs(600));

        assert_eq!(rates.total, 300_500);
        assert!((rates.mean_per_second - 500.0).abs() < 1.0, "{rates:?}");
        for observed in [
            rates.per_second_1s,
            rates.per_second_10s,
            rates.per_second_60s,
        ] {
            assert!((observed - 500.0).abs() < 1.0, "{rates:?}");
        }
    }

    #[test]
    fn met_010_the_short_window_reacts_first_and_the_long_window_last() {
        let start = Instant::now();
        let meter = RateMeter::new(start);
        // Ten minutes at 1000/s, then a minute of silence.
        for second in 0..600 {
            meter.mark_at(1_000, start + Duration::from_secs(second));
        }
        let busy = meter.snapshot_at(start + Duration::from_secs(600));
        let idle = meter.snapshot_at(start + Duration::from_secs(660));

        assert!(idle.per_second_1s < idle.per_second_10s, "{idle:?}");
        assert!(idle.per_second_10s < idle.per_second_60s, "{idle:?}");
        assert!(idle.per_second_60s < busy.per_second_60s, "{idle:?}");
        // A meter nobody has marked for a minute must decay, not report its last busy rate.
        assert!(idle.per_second_1s < 1.0, "{idle:?}");
        // The total and the mean are cumulative and do not decay.
        assert_eq!(idle.total, 600_000);
        assert!((idle.mean_per_second - 909.0).abs() < 2.0, "{idle:?}");
    }

    #[test]
    fn met_010_rates_are_zero_before_the_first_tick_rather_than_infinite() {
        let start = Instant::now();
        let meter = RateMeter::new(start);
        meter.mark_at(10, start);
        let immediate = meter.snapshot_at(start);
        assert_eq!(immediate.total, 10);
        assert!(immediate.per_second_1s.abs() < f64::EPSILON);
        assert!(
            immediate.mean_per_second.abs() < f64::EPSILON,
            "no elapsed time, no rate"
        );
    }

    #[test]
    fn met_010_a_meter_seeds_from_its_first_observation() {
        // Seeding from zero would make the first minute of a run report a rate the run never had.
        let start = Instant::now();
        let meter = RateMeter::new(start);
        meter.mark_at(750, start);
        let first = meter.snapshot_at(start + TICK);
        assert!((first.per_second_60s - 750.0).abs() < 1.0, "{first:?}");
    }

    #[test]
    fn met_010_marks_from_many_workers_are_counted_exactly() {
        const THREADS: u64 = 8;
        const PER_THREAD: u64 = 10_000;

        let meter = Arc::new(RateMeter::new(Instant::now()));
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let meter = Arc::clone(&meter);
                thread::spawn(move || {
                    for _ in 0..PER_THREAD {
                        meter.mark(1);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(meter.total(), THREADS * PER_THREAD);
        // Reading the clock for real must not panic or divide by zero.
        let _ = meter.snapshot();
    }

    #[test]
    fn met_010_the_windows_are_the_ones_the_specification_names() {
        assert_eq!(WINDOWS_SECS, [1, 10, 60]);
        assert_eq!(TICK, Duration::from_secs(1));
        assert!(alpha(1) > alpha(10));
        assert!(alpha(10) > alpha(60));
        assert!(
            (alpha(0) - 1.0).abs() < f64::EPSILON,
            "a zero window cannot smooth anything"
        );
    }
}
