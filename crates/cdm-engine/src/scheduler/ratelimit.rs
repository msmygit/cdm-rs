//! The token-bucket rate limiter (`ENG-004`, `ENG-005`).
//!
//! Two of these exist per run: one counting rows read from the origin, one counting rows written
//! to the target. They are independent, as `ENG-004` requires — a slow target must not throttle
//! the read side into a state the read side cannot recover from, and vice versa.
//!
//! # Why a reservation, not a bucket of tokens
//!
//! The obvious implementation refills a counter on a timer and makes waiters retry. That either
//! spins or starves: with `perfops.workers` tasks contending for the same counter, whichever task
//! happens to wake first wins, and a task can be passed over indefinitely.
//!
//! This limiter instead keeps a single *theoretical arrival time* — the virtual instant at which
//! the work reserved so far will have been paid for. Acquiring `n` rows advances that instant by
//! `n / rate` seconds and returns how long the caller must sleep before its own reservation comes
//! due. The reservation is taken under the lock and never given back, so:
//!
//! * every caller is served in the order it arrived (`ENG-005`: backpressure, never dropping);
//! * a caller sleeps exactly once and then proceeds (never spinning);
//! * the aggregate rate is the configured rate regardless of how the callers interleave.
//!
//! This is the GCRA formulation of a token bucket. The burst allowance is one second of budget,
//! exactly as `ENG-005` specifies: a run may issue `rate` rows instantly, and pays for everything
//! after that.
//!
//! # Time and testability
//!
//! Both the clock and the sleep come from `tokio::time`, so a test that pauses the runtime clock
//! (`tokio::test(start_paused = true)`) drives this limiter through hours of virtual time in
//! microseconds, deterministically. The arithmetic itself is in a private `reserve` method, which is
//! pure: it takes the current instant as an argument and returns a delay, so the pacing rules are
//! tested without any clock at all.
//!
//! # Units
//!
//! Everything is picoseconds in `u128`. Nanoseconds would round `1 / rate` badly for small rates
//! (at 3 rows/s the per-row cost is 333 333 333.33 ns), and floating point would make the
//! reservation non-associative across threads. A picosecond is small enough that the truncation
//! error is below one part in `10^12` per row, and `u128` cannot overflow: a run would have to
//! last `10^26` seconds.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

/// Picoseconds in one second.
const PICOS_PER_SECOND: u128 = 1_000_000_000_000;

/// Picoseconds in one nanosecond.
const PICOS_PER_NANO: u128 = 1_000;

/// A rows-per-second token bucket with one second of burst (`ENG-004`, `ENG-005`).
///
/// A rate of `0` means *unlimited*: [`RateLimiter::acquire`] returns immediately and no state is
/// touched. This follows the convention `perfops.error_limit` already uses, where `0` disables
/// the check; Java's Guava `RateLimiter` rejects a rate of zero outright, which leaves an
/// operator who wants no limit with no way to say so.
///
/// # Why the rate is not a constant
///
/// `ENG-006` lets an operator hand the rate over to a controller that lowers it when the target
/// says it is overloaded. The rate therefore lives in an atomic and is read once per reservation
/// rather than being baked into a precomputed per-row cost. The cost of that is one integer
/// division per `acquire`, which is nothing beside the mutex the reservation already takes; the
/// benefit is that [`RateLimiter::set_rows_per_second`] cannot be forgotten by a caller that
/// changes the rate. The theoretical arrival time is deliberately *not* rewound when the rate
/// changes: work already reserved has already been promised a slot, and taking it back would let
/// a rate cut overtake requests that were admitted under the old one.
#[derive(Debug)]
pub struct RateLimiter {
    rows_per_second: AtomicU32,
    burst_picos: u128,
    origin: Instant,
    tat: Mutex<u128>,
}

impl RateLimiter {
    /// A limiter admitting `rows_per_second` rows per second, with one second of burst.
    #[must_use]
    pub fn new(rows_per_second: u32) -> Self {
        Self {
            rows_per_second: AtomicU32::new(rows_per_second),
            burst_picos: PICOS_PER_SECOND,
            origin: Instant::now(),
            tat: Mutex::new(0),
        }
    }

    /// The rate in force, in rows per second. `0` means unlimited.
    #[must_use]
    pub fn rows_per_second(&self) -> u32 {
        self.rows_per_second.load(Ordering::Relaxed)
    }

    /// Sets the rate in force, in rows per second (`ENG-006`).
    ///
    /// Only `ENG-006`'s adaptive controller calls this; a run without
    /// `perfops.adaptive_ratelimit` keeps the rate it was constructed with for its whole life.
    pub fn set_rows_per_second(&self, rows_per_second: u32) {
        self.rows_per_second
            .store(rows_per_second, Ordering::Relaxed);
    }

    /// Whether this limiter admits everything immediately.
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.rows_per_second() == 0
    }

    /// The burst allowance, in rows — one second of budget (`ENG-005`).
    #[must_use]
    pub fn burst_rows(&self) -> u32 {
        self.rows_per_second()
    }

    /// Waits until `rows` rows may be processed, then returns how long that took (`ENG-005`).
    ///
    /// Backpressure is applied by awaiting. Nothing is ever dropped, and the caller is never
    /// asked to retry.
    ///
    /// The returned [`Duration`] is `MET-010`'s rate-limiter wait time, and it costs nothing to
    /// produce: the limiter has already computed the delay in order to sleep for it, so the
    /// caller is handed the number rather than timing the call and reading the clock twice more.
    /// [`Duration::ZERO`] means the reservation came due immediately, which is every call on an
    /// unlimited limiter.
    pub async fn acquire(&self, rows: u32) -> Duration {
        let delay = self.reserve(rows, self.now_picos());
        if delay == 0 {
            return Duration::ZERO;
        }
        let waited = picos_to_duration(delay);
        tokio::time::sleep(waited).await;
        waited
    }

    /// Picoseconds since this limiter was created, on the runtime clock.
    fn now_picos(&self) -> u128 {
        self.origin.elapsed().as_nanos() * PICOS_PER_NANO
    }

    /// The pure core: reserve `rows` at `now_picos` and return the delay, in picoseconds, the
    /// caller must wait before its reservation comes due.
    ///
    /// The reservation is unconditional — the theoretical arrival time advances whether or not
    /// the caller has to wait — which is what makes the limiter first-come-first-served rather
    /// than a scramble.
    fn reserve(&self, rows: u32, now_picos: u128) -> u128 {
        let rate = self.rows_per_second();
        if rate == 0 || rows == 0 {
            return 0;
        }
        let picos_per_row = PICOS_PER_SECOND / u128::from(rate);
        let cost = picos_per_row.saturating_mul(u128::from(rows));
        let mut tat = self.tat.lock();
        let due = (*tat).max(now_picos).saturating_add(cost);
        *tat = due;
        due.saturating_sub(self.burst_picos)
            .saturating_sub(now_picos)
    }
}

/// Picoseconds to a [`Duration`], rounding up so a sub-nanosecond debt still yields a real wait.
fn picos_to_duration(picos: u128) -> Duration {
    let nanos = picos.div_ceil(PICOS_PER_NANO);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
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

    use super::*;

    /// One second of budget, in picoseconds — the unit the pure tests reason in.
    const SECOND: u128 = PICOS_PER_SECOND;

    #[test]
    fn eng_005_the_first_second_of_budget_is_free() {
        let limiter = RateLimiter::new(10);
        // Ten rows at ten rows a second is exactly one second of budget: the burst.
        for _ in 0..10 {
            assert_eq!(limiter.reserve(1, 0), 0);
        }
    }

    #[test]
    fn eng_005_work_beyond_the_burst_is_paced_at_the_configured_rate() {
        let limiter = RateLimiter::new(10);
        for _ in 0..10 {
            assert_eq!(limiter.reserve(1, 0), 0);
        }
        // The eleventh row is one tenth of a second past the burst, the twelfth two tenths.
        assert_eq!(limiter.reserve(1, 0), SECOND / 10);
        assert_eq!(limiter.reserve(1, 0), SECOND / 5);
    }

    #[test]
    fn eng_005_the_burst_is_exactly_one_second_of_budget() {
        let limiter = RateLimiter::new(1_000);
        assert_eq!(limiter.burst_rows(), 1_000);
        // A single acquisition of the whole burst is free; one row more is not.
        assert_eq!(limiter.reserve(1_000, 0), 0);
        assert_eq!(limiter.reserve(1, 0), SECOND / 1_000);
    }

    #[test]
    fn eng_005_elapsed_real_time_repays_the_debt() {
        let limiter = RateLimiter::new(10);
        assert_eq!(limiter.reserve(30, 0), 2 * SECOND);
        // Two seconds later the reservation has come due and the bucket has refilled.
        assert_eq!(limiter.reserve(1, 3 * SECOND), 0);
    }

    #[test]
    fn eng_005_a_reservation_is_never_given_back_so_callers_are_served_in_order() {
        let limiter = RateLimiter::new(10);
        let first = limiter.reserve(20, 0);
        let second = limiter.reserve(1, 0);
        // The second caller waits strictly longer than the first: no overtaking, no starvation.
        assert!(second > first, "{second} must exceed {first}");
    }

    #[test]
    fn eng_005_a_zero_rate_limit_is_unlimited() {
        let limiter = RateLimiter::new(0);
        assert!(limiter.is_unlimited());
        assert_eq!(limiter.reserve(u32::MAX, 0), 0);
    }

    #[test]
    fn eng_005_acquiring_zero_rows_costs_nothing() {
        let limiter = RateLimiter::new(1);
        assert_eq!(limiter.reserve(0, 0), 0);
        assert_eq!(limiter.reserve(1, 0), 0);
    }

    #[test]
    fn eng_005_a_rate_that_does_not_divide_a_second_evenly_still_paces() {
        let limiter = RateLimiter::new(3);
        // Three rows is the burst; the fourth waits a third of a second.
        assert_eq!(limiter.reserve(3, 0), 0);
        let delay = limiter.reserve(1, 0);

        // A third of a second is not a whole number of picoseconds, and the truncation shows: a
        // row costs 333_333_333_333 ps rather than 333_333_333_333.33, so three of them fall a
        // picosecond short of the second-long burst and the fourth row's wait is a picosecond
        // shorter than a naive third. That is three parts in 10^12 — a nanosecond every five
        // minutes — and it errs towards admitting work rather than withholding it.
        assert_eq!(delay, SECOND / 3 - 1);
        assert!(
            SECOND / 3 - delay < 1_000,
            "the pacing error must stay below a nanosecond"
        );
    }

    #[test]
    fn eng_005_picoseconds_round_up_to_a_whole_nanosecond() {
        assert_eq!(picos_to_duration(1), Duration::from_nanos(1));
        assert_eq!(picos_to_duration(1_000), Duration::from_nanos(1));
        assert_eq!(picos_to_duration(1_001), Duration::from_nanos(2));
        assert_eq!(picos_to_duration(0), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn eng_005_backpressure_awaits_rather_than_dropping_or_spinning() {
        // Virtual time: this test does not sleep for real, and its assertion is exact rather
        // than a tolerance, because the runtime clock only advances when every task is idle.
        let limiter = RateLimiter::new(10);
        let started = Instant::now();
        for _ in 0..30 {
            limiter.acquire(1).await;
        }
        // Ten rows of burst, then twenty more at ten a second.
        assert_eq!(started.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn eng_004_the_two_limiters_are_independent() {
        let origin = Arc::new(RateLimiter::new(10));
        let target = Arc::new(RateLimiter::new(1_000));
        let started = Instant::now();

        let reader = tokio::spawn({
            let origin = Arc::clone(&origin);
            async move {
                for _ in 0..20 {
                    origin.acquire(1).await;
                }
            }
        });
        let writer = tokio::spawn({
            let target = Arc::clone(&target);
            async move {
                for _ in 0..1_000 {
                    target.acquire(1).await;
                }
                Instant::now()
            }
        });

        let writer_finished = writer.await.unwrap();
        reader.await.unwrap();

        // The target side burnt only its own budget: a thousand rows is one second of burst for
        // it, so it finished immediately even though the origin side was throttled to one more
        // second of work.
        assert_eq!(writer_finished.duration_since(started), Duration::ZERO);
        assert_eq!(started.elapsed(), Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn eng_005_concurrent_acquirers_share_the_configured_rate() {
        let limiter = Arc::new(RateLimiter::new(100));
        let started = Instant::now();
        let mut handles = Vec::new();
        for _ in 0..10 {
            let limiter = Arc::clone(&limiter);
            handles.push(tokio::spawn(async move {
                for _ in 0..30 {
                    limiter.acquire(1).await;
                }
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        // 300 rows: 100 of burst plus 200 paced at 100/s. The aggregate rate is the configured
        // rate no matter how many tasks contend.
        assert_eq!(started.elapsed(), Duration::from_secs(2));
    }

    #[test]
    fn eng_006_a_changed_rate_paces_the_work_that_comes_after_it() {
        let limiter = RateLimiter::new(10);
        // Burn the burst so the pacing is visible, then measure a row at the old rate.
        assert_eq!(limiter.reserve(10, 0), 0);
        assert_eq!(limiter.reserve(1, 0), SECOND / 10);

        // ENG-006: the controller halves the rate, and a row immediately costs twice as much.
        limiter.set_rows_per_second(5);
        assert_eq!(limiter.rows_per_second(), 5);
        assert_eq!(limiter.burst_rows(), 5);
        assert_eq!(limiter.reserve(1, 0), SECOND / 10 + SECOND / 5);

        // And raising it back makes the next row cheap again.
        limiter.set_rows_per_second(20);
        assert_eq!(
            limiter.reserve(1, 0),
            SECOND / 10 + SECOND / 5 + SECOND / 20
        );
    }

    #[test]
    fn eng_006_lowering_the_rate_does_not_rescind_a_reservation_already_made() {
        let limiter = RateLimiter::new(10);
        let promised = limiter.reserve(30, 0);
        limiter.set_rows_per_second(1);
        // The waiter that was told to sleep two seconds still comes due in two seconds: a rate
        // cut may not retroactively lengthen a wait somebody is already in the middle of.
        assert_eq!(promised, 2 * SECOND);
        assert_eq!(limiter.reserve(0, 0), 0);
    }

    #[test]
    fn eng_006_a_rate_may_be_set_to_unlimited_and_back() {
        let limiter = RateLimiter::new(10);
        limiter.set_rows_per_second(0);
        assert!(limiter.is_unlimited());
        assert_eq!(limiter.reserve(1_000_000, 0), 0);
        limiter.set_rows_per_second(10);
        assert!(!limiter.is_unlimited());
    }

    #[tokio::test(start_paused = true)]
    async fn eng_006_a_reduced_rate_really_slows_the_run_down() {
        // The anti-#21c test at the limiter level: not "the number changed" but "the work took
        // longer". Virtual time, so the assertion is exact rather than a tolerance.
        let limiter = RateLimiter::new(100);
        let started = Instant::now();
        for _ in 0..200 {
            limiter.acquire(1).await;
        }
        // 100 of burst, 100 more at 100/s.
        assert_eq!(started.elapsed(), Duration::from_secs(1));

        limiter.set_rows_per_second(10);
        let after = Instant::now();
        for _ in 0..20 {
            limiter.acquire(1).await;
        }
        assert_eq!(after.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn eng_005_an_unlimited_limiter_never_sleeps() {
        let limiter = RateLimiter::new(0);
        let started = Instant::now();
        for _ in 0..10_000 {
            limiter.acquire(1).await;
        }
        assert_eq!(started.elapsed(), Duration::ZERO);
    }
}
