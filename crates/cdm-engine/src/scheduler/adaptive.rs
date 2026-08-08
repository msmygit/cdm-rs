//! The adaptive rate-limit controller (`ENG-006`).
//!
//! `ENG-005`'s token bucket paces a run at whatever number the operator typed. That number is a
//! guess: nobody knows what a production target will absorb at three in the morning while a
//! compaction is running. `perfops.adaptive_ratelimit = true` turns the configured
//! `perfops.ratelimit.target` from *the rate* into *the ceiling*, and lets the target's own
//! complaints choose the rate underneath it.
//!
//! # Why AIMD
//!
//! Additive-increase/multiplicative-decrease is the control law of TCP congestion avoidance, and
//! Chiu and Jain's 1989 analysis is the reason it is the one to copy: of the four linear
//! increase/decrease pairs, AIMD is the only one that converges to an efficient *and* fair
//! operating point from every starting state. The asymmetry is the whole point — a controller
//! must shed load faster than it acquires it, or it spends its life discovering the same cliff.
//!
//! The alternatives were considered and rejected:
//!
//! | Scheme | Why not |
//! |---|---|
//! | Multiplicative increase | Doubling back towards a cliff guarantees a limit cycle of the same amplitude as the capacity itself. It oscillates by construction. |
//! | Proportional control on a latency error | Needs a set-point nobody can state: "the right p99" is a property of the target's SLA, not of the migration. |
//! | Fixed back-off on error, no recovery | A single transient timeout would pin a petabyte migration at the floor for the rest of the week. |
//!
//! # The law, exactly
//!
//! Time is divided into fixed **control windows** (one second — the same interval as `ENG-005`'s
//! burst, so at most one adjustment per burst of budget). At the end of each window:
//!
//! * if the window saw **any** overload signal → `rate ← max(floor, rate / 2)`;
//! * otherwise → `rate ← min(ceiling, rate + step)`, where `step` is
//!   [`INCREASE_PERCENT_OF_CEILING`]% of the ceiling.
//!
//! "**any**, once per window" is load-bearing rather than an optimisation. A run has
//! `perfops.workers` workers and up to `perfops.max_inflight_writes` requests outstanding; when a
//! target starts timing out, hundreds of signals arrive at once, all reporting the *same*
//! congestion event. A controller that halved per signal would reach the floor in one round trip
//! and stay there. This is TCP's "one reduction per round-trip time" rule, and
//! `eng_006_the_rate_falls_once_per_window_however_many_signals_arrive` is its test.
//!
//! # What stability means here, and what is actually proved
//!
//! Under a target whose true capacity `C` sits strictly between the floor and the ceiling, AIMD
//! does not settle on a single number: its steady state is a sawtooth between `C/2` and `C`. That
//! is not a defect, it is what probing costs, and the property worth asserting is that the
//! sawtooth's **envelope does not grow** — the controller neither diverges nor ratchets. Where
//! `C` is outside the bounds the controller has a genuine fixed point and reaches it:
//!
//! | Capacity | Steady state | Test |
//! |---|---|---|
//! | above the ceiling | exactly the ceiling, forever | `eng_006_a_target_that_never_complains_settles_at_the_ceiling` |
//! | below the floor | exactly the floor, forever | `eng_006_relentless_overload_settles_at_the_floor_not_at_zero` |
//! | between the two | bounded sawtooth, non-growing envelope | `eng_006_the_controller_converges_to_a_bounded_envelope_rather_than_oscillating` |
//!
//! # Time is an argument
//!
//! [`AdaptiveRateController::observe_at`] takes the current instant, in nanoseconds, as a
//! parameter and returns the new rate. Every rule above is therefore testable with no clock, no
//! sleep and no race; [`AdaptiveRateController::observe`] is the thin wrapper that reads
//! `tokio::time::Instant`.

use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

/// How long one control window lasts.
///
/// One second, matching `ENG-005`'s burst allowance: a shorter window would react to a single
/// slow request, a longer one would spend minutes overloading a target that said so immediately.
pub const CONTROL_WINDOW: Duration = Duration::from_secs(1);

/// The additive increase, as a percentage of the ceiling, applied per quiet window.
///
/// Five percent means a full recovery from the floor takes twenty seconds — slow enough that the
/// controller cannot chase its own back-off, fast enough that a transient blip does not cost a
/// petabyte migration an hour.
pub const INCREASE_PERCENT_OF_CEILING: u32 = 5;

/// The multiplicative decrease: the rate is divided by this on an overloaded window.
pub const DECREASE_DIVISOR: u32 = 2;

/// The most windows one observation may fast-forward through.
///
/// A run that is paused (`ENG-014`) or whose target has gone quiet can leave an arbitrary gap
/// between observations. Recovering the whole gap in one step is correct — nothing complained
/// during it — but it must not be an unbounded loop, and beyond a few minutes the arithmetic has
/// already saturated at the ceiling anyway.
const MAX_CATCHUP_WINDOWS: u128 = 600;

/// What one target request told the controller (`ENG-006`).
///
/// Deliberately two-valued. The controller's job is to decide a rate, not to diagnose a cluster;
/// everything richer belongs in the error that is already being logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadSignal {
    /// The target absorbed the request without complaint.
    Ok,
    /// The target reported overload: a write timeout, `OVERLOADED`, a write failure, or a request
    /// that exceeded the configured `perfops.request_timeout`.
    Overload,
}

/// The mutable half of the controller, behind one lock.
#[derive(Debug)]
struct State {
    /// The rate in force, in rows per second.
    effective: u32,
    /// The start of the window now being accumulated, in nanoseconds since the controller began.
    window_start: u128,
    /// Whether the window now being accumulated has seen an overload signal.
    overloaded: bool,
}

/// An AIMD controller over the target write rate (`ENG-006`).
///
/// Construct one with [`AdaptiveRateController::new`] and feed it with
/// [`AdaptiveRateController::observe`]; [`RuntimeLimits`](super::limits::RuntimeLimits) owns the
/// one a run uses and applies its decisions to the target [`RateLimiter`](super::RateLimiter).
#[derive(Debug)]
pub struct AdaptiveRateController {
    ceiling: u32,
    floor: u32,
    step: u32,
    window_nanos: u128,
    origin: Instant,
    state: Mutex<State>,
}

impl AdaptiveRateController {
    /// A controller whose ceiling is `ceiling` rows per second and whose floor is
    /// `min_percent` of it.
    ///
    /// The floor is clamped into `1..=ceiling`: a floor of zero would let the controller stop the
    /// run without ever saying it had, and a floor above the ceiling is a configuration the
    /// operator cannot have meant. A `ceiling` of `0` — `perfops.ratelimit.target` unlimited —
    /// is a controller with nothing to control; [`AdaptiveRateController::is_active`] reports
    /// `false` and every observation is ignored.
    #[must_use]
    pub fn new(ceiling: u32, min_percent: u8) -> Self {
        let floor = floor_for(ceiling, min_percent);
        Self {
            ceiling,
            floor,
            step: step_for(ceiling),
            window_nanos: CONTROL_WINDOW.as_nanos(),
            origin: Instant::now(),
            state: Mutex::new(State {
                effective: ceiling,
                window_start: 0,
                overloaded: false,
            }),
        }
    }

    /// The ceiling: the configured `perfops.ratelimit.target`, which the rate never exceeds.
    #[must_use]
    pub const fn ceiling(&self) -> u32 {
        self.ceiling
    }

    /// The floor: the rate the controller will not reduce below, never less than one row/s.
    #[must_use]
    pub const fn floor(&self) -> u32 {
        self.floor
    }

    /// The additive increase applied per quiet window, in rows per second.
    #[must_use]
    pub const fn step(&self) -> u32 {
        self.step
    }

    /// Whether this controller can change anything.
    ///
    /// `false` when the target rate is unlimited, which leaves nothing to reduce.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.ceiling > 0
    }

    /// The rate in force, in rows per second.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.state.lock().effective
    }

    /// Records one target request's outcome on the runtime clock.
    ///
    /// Returns the new rate when a control window closed *and* the rate changed, and `None`
    /// otherwise — so the caller writes to the rate limiter only when there is something to say.
    pub fn observe(&self, signal: LoadSignal) -> Option<u32> {
        self.observe_at(signal, self.origin.elapsed().as_nanos())
    }

    /// The pure core: record `signal` at `now_nanos` and return the new rate if it changed.
    ///
    /// Nanoseconds since this controller was built, so a test can drive years of a run through it
    /// without a clock. All of `ENG-006`'s behaviour is here; [`Self::observe`] adds only the
    /// clock read.
    pub fn observe_at(&self, signal: LoadSignal, now_nanos: u128) -> Option<u32> {
        self.roll(Some(signal), now_nanos)
    }

    /// Closes any control windows that have elapsed, without recording a signal.
    ///
    /// Separated from [`Self::observe_at`] because the two are different statements: "a request
    /// finished, and this is what the target said" versus "time has passed". A run whose target
    /// has gone quiet — paused (`ENG-014`), or between ranges — recovers on the second, and a
    /// test that wants to reason about one window at a time needs to be able to advance the clock
    /// without also asserting something about a request.
    pub fn tick_at(&self, now_nanos: u128) -> Option<u32> {
        self.roll(None, now_nanos)
    }

    /// The window machinery both entry points share.
    fn roll(&self, signal: Option<LoadSignal>, now_nanos: u128) -> Option<u32> {
        if !self.is_active() {
            return None;
        }
        let mut state = self.state.lock();

        let elapsed = now_nanos.saturating_sub(state.window_start);
        if elapsed < self.window_nanos {
            // Still inside the open window: remember an overload, but change nothing. One
            // congestion event must cost one halving, however many requests observed it.
            state.overloaded |= signal == Some(LoadSignal::Overload);
            return None;
        }

        let windows = (elapsed / self.window_nanos).min(MAX_CATCHUP_WINDOWS);
        let before = state.effective;

        // The window that just closed decides the first step...
        let mut rate = if state.overloaded {
            self.decrease(before)
        } else {
            self.increase(before)
        };
        // ...and any windows that elapsed with no observation at all were, by definition, quiet.
        let quiet = u32::try_from(windows.saturating_sub(1)).unwrap_or(u32::MAX);
        rate = self.increase_by(rate, quiet);

        state.effective = rate;
        state.window_start = if windows >= MAX_CATCHUP_WINDOWS {
            now_nanos
        } else {
            state
                .window_start
                .saturating_add(windows.saturating_mul(self.window_nanos))
        };
        // The signal that closed the window belongs to the window it opened.
        state.overloaded = signal == Some(LoadSignal::Overload);

        (rate != before).then_some(rate)
    }

    /// Multiplicative decrease, floored.
    fn decrease(&self, rate: u32) -> u32 {
        (rate / DECREASE_DIVISOR).max(self.floor)
    }

    /// One additive increase, capped at the ceiling.
    fn increase(&self, rate: u32) -> u32 {
        rate.saturating_add(self.step).min(self.ceiling)
    }

    /// `times` additive increases, capped at the ceiling.
    fn increase_by(&self, rate: u32, times: u32) -> u32 {
        rate.saturating_add(self.step.saturating_mul(times))
            .min(self.ceiling)
    }
}

/// The floor a ceiling and a percentage imply.
fn floor_for(ceiling: u32, min_percent: u8) -> u32 {
    if ceiling == 0 {
        return 0;
    }
    let percent = u64::from(min_percent).min(100);
    let floor = u64::from(ceiling).saturating_mul(percent) / 100;
    u32::try_from(floor).unwrap_or(u32::MAX).clamp(1, ceiling)
}

/// The additive step a ceiling implies, never zero for a limited ceiling.
fn step_for(ceiling: u32) -> u32 {
    if ceiling == 0 {
        return 0;
    }
    (ceiling / 100)
        .saturating_mul(INCREASE_PERCENT_OF_CEILING)
        .max(1)
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

    /// One control window, in the nanoseconds `observe_at` counts.
    const WINDOW: u128 = 1_000_000_000;

    /// A controller with a round ceiling: step 500/s, floor 1 000/s.
    fn controller() -> AdaptiveRateController {
        AdaptiveRateController::new(10_000, 10)
    }

    /// Drives `windows` control windows and returns the rate in force during each.
    ///
    /// The ordering is the one a run really has, and it matters: the window is rolled *first*, so
    /// the rate the target is asked to absorb during window `w` is the one the controller decided
    /// at the end of window `w − 1`, and `signal` is then asked what the target made of *that*
    /// rate. Deciding the signal before rolling would model a target answering a question it had
    /// not been asked yet, and would put a spurious one-window lag into every assertion below.
    fn drive(
        controller: &AdaptiveRateController,
        windows: usize,
        mut signal: impl FnMut(u32) -> LoadSignal,
    ) -> Vec<u32> {
        let mut rates = Vec::with_capacity(windows);
        for window in 0..windows {
            let start = window as u128 * WINDOW;
            controller.tick_at(start);
            let in_force = controller.rate();
            controller.observe_at(signal(in_force), start + WINDOW / 2);
            rates.push(in_force);
        }
        rates
    }

    #[test]
    fn eng_006_the_bounds_come_from_the_ceiling_and_the_configured_percentage() {
        let controller = controller();
        assert_eq!(controller.ceiling(), 10_000);
        assert_eq!(controller.floor(), 1_000);
        assert_eq!(controller.step(), 500);
        assert_eq!(
            controller.rate(),
            10_000,
            "a run starts at the rate it was told"
        );
        assert!(controller.is_active());
    }

    #[test]
    fn eng_006_the_floor_is_never_zero_and_never_above_the_ceiling() {
        // A percentage that rounds to nothing still leaves a rate a run can make progress at: a
        // node pinned at zero rows per second is indistinguishable from a hung one.
        assert_eq!(AdaptiveRateController::new(5, 0).floor(), 1);
        assert_eq!(AdaptiveRateController::new(5, 1).floor(), 1);
        // And a nonsensical percentage cannot raise the floor through the ceiling.
        assert_eq!(AdaptiveRateController::new(100, 255).floor(), 100);
    }

    #[test]
    fn eng_006_an_unlimited_target_rate_leaves_nothing_to_control() {
        let controller = AdaptiveRateController::new(0, 10);
        assert!(!controller.is_active());
        assert_eq!(
            controller.observe_at(LoadSignal::Overload, 10 * WINDOW),
            None
        );
        assert_eq!(
            controller.rate(),
            0,
            "0 means unlimited, and stays unlimited"
        );
    }

    #[test]
    fn eng_006_sustained_overload_reduces_the_rate() {
        let controller = controller();
        let rates = drive(&controller, 4, |_| LoadSignal::Overload);
        assert_eq!(rates, vec![10_000, 5_000, 2_500, 1_250]);
    }

    #[test]
    fn eng_006_the_rate_falls_once_per_window_however_many_signals_arrive() {
        let controller = controller();
        // A thousand workers all seeing the same congestion event, inside one window.
        for i in 0_u128..1_000 {
            controller.observe_at(LoadSignal::Overload, WINDOW / 2 + i);
        }
        assert_eq!(
            controller.rate(),
            10_000,
            "nothing changes until the window closes"
        );
        controller.observe_at(LoadSignal::Ok, WINDOW);
        assert_eq!(
            controller.rate(),
            5_000,
            "one congestion event costs exactly one halving"
        );
    }

    #[test]
    fn eng_006_the_rate_recovers_when_the_overload_stops() {
        let controller = controller();
        // Three overloaded windows, then a target that has stopped complaining. The first quiet
        // window still pays for the last overloaded one — the rule is applied when the window it
        // describes closes, not when the operator would like it to.
        let mut window = 0_u128;
        let mut rates = Vec::new();
        for _ in 0..3 {
            controller.tick_at(window * WINDOW);
            rates.push(controller.rate());
            controller.observe_at(LoadSignal::Overload, window * WINDOW + WINDOW / 2);
            window += 1;
        }
        assert_eq!(rates, vec![10_000, 5_000, 2_500]);

        // Additive increase, 500/s per quiet window, never past the ceiling.
        let mut recovery = Vec::new();
        for _ in 0..30 {
            controller.tick_at(window * WINDOW);
            recovery.push(controller.rate());
            controller.observe_at(LoadSignal::Ok, window * WINDOW + WINDOW / 2);
            window += 1;
        }
        assert_eq!(&recovery[..3], &[1_250, 1_750, 2_250]);
        assert_eq!(*recovery.last().unwrap(), 10_000);
        assert!(
            recovery.windows(2).all(|pair| pair[1] >= pair[0]),
            "recovery must be monotone: {recovery:?}"
        );
    }

    #[test]
    fn eng_006_a_target_that_never_complains_settles_at_the_ceiling() {
        let controller = controller();
        let rates = drive(&controller, 200, |_| LoadSignal::Ok);
        assert!(
            rates.iter().all(|rate| *rate == 10_000),
            "a quiet target must never push the rate above what was configured"
        );
    }

    #[test]
    fn eng_006_relentless_overload_settles_at_the_floor_not_at_zero() {
        let controller = controller();
        let rates = drive(&controller, 200, |_| LoadSignal::Overload);
        assert_eq!(*rates.last().unwrap(), controller.floor());
        // A genuine fixed point: once at the floor it stays there, rather than creeping to zero.
        assert!(
            rates[50..].iter().all(|rate| *rate == 1_000),
            "{:?}",
            &rates[50..60]
        );
    }

    #[test]
    fn eng_006_the_controller_converges_to_a_bounded_envelope_rather_than_oscillating() {
        // A target that absorbs 3 000 rows/s and complains above that. `C` sits strictly between
        // the floor (1 000) and the ceiling (10 000), which is the only regime in which AIMD has
        // no single fixed point — so this is where "stable" has to mean something other than
        // "constant".
        const CAPACITY: u32 = 3_000;
        let controller = controller();
        let rates = drive(&controller, 600, |rate| {
            if rate > CAPACITY {
                LoadSignal::Overload
            } else {
                LoadSignal::Ok
            }
        });

        // 1. It never diverges: after the initial descent the rate stays inside the envelope AIMD
        //    promises — never above capacity plus one probe, never below half of it.
        let settled = &rates[20..];
        let high = *settled.iter().max().unwrap();
        let low = *settled.iter().min().unwrap();
        assert!(
            high <= CAPACITY + controller.step(),
            "overshoot of {high} exceeds one probing step above capacity"
        );
        assert!(
            low >= CAPACITY / 2,
            "the rate collapsed to {low}, below the half-capacity AIMD guarantees"
        );

        // 2. The envelope does not grow. This is the property a multiplicative-increase
        //    controller fails: its swings widen until they hit the bounds.
        let early = envelope(&rates[20..220]);
        let late = envelope(&rates[400..600]);
        assert!(
            late <= early,
            "the oscillation widened from {early} to {late}: the loop is not stable"
        );

        // 3. And it is *useful*: the average rate is a large fraction of the real capacity,
        //    rather than a controller that has parked itself safely at the floor.
        let mean = settled.iter().map(|r| u64::from(*r)).sum::<u64>() / settled.len() as u64;
        assert!(
            mean > u64::from(CAPACITY) * 6 / 10,
            "the settled mean of {mean} wastes most of the target's capacity"
        );
    }

    #[test]
    fn eng_006_convergence_does_not_depend_on_where_the_capacity_sits() {
        // The same envelope argument, swept across capacities, so the previous test is not a
        // single lucky arithmetic coincidence.
        for capacity in [1_100_u32, 2_000, 4_444, 7_500, 9_900] {
            let controller = controller();
            let rates = drive(&controller, 600, |rate| {
                if rate > capacity {
                    LoadSignal::Overload
                } else {
                    LoadSignal::Ok
                }
            });
            let settled = &rates[20..];
            assert!(
                settled
                    .iter()
                    .all(|rate| *rate <= capacity + controller.step()),
                "capacity {capacity}: the rate escaped above the target's capacity"
            );
            assert!(
                settled.iter().all(|rate| *rate >= controller.floor()),
                "capacity {capacity}: the rate fell through the floor"
            );
            assert!(
                envelope(&rates[400..600]) <= envelope(&rates[20..220]),
                "capacity {capacity}: the oscillation widened"
            );
        }
    }

    #[test]
    fn eng_006_a_gap_between_observations_recovers_the_windows_it_covers() {
        let controller = controller();
        drive(&controller, 3, |_| LoadSignal::Overload);
        controller.tick_at(3 * WINDOW);
        assert_eq!(controller.rate(), 1_250);

        // Ten seconds of silence — a paused run (`ENG-014`), or a target nobody was writing to —
        // is ten quiet windows, not one.
        controller.observe_at(LoadSignal::Ok, 13 * WINDOW);
        assert_eq!(controller.rate(), 1_250 + 10 * 500);

        // And an arbitrarily long silence saturates at the ceiling rather than looping.
        controller.observe_at(LoadSignal::Ok, 10_000_000 * WINDOW);
        assert_eq!(controller.rate(), 10_000);
    }

    #[test]
    fn eng_006_only_a_closed_window_that_changed_the_rate_reports_a_new_one() {
        let controller = controller();
        // Inside the first window: nothing to apply.
        assert_eq!(controller.observe_at(LoadSignal::Ok, WINDOW / 2), None);
        // A quiet window at the ceiling changes nothing, so there is nothing to write.
        assert_eq!(controller.observe_at(LoadSignal::Overload, WINDOW), None);
        // The overload it recorded closes the next window with a real change.
        assert_eq!(
            controller.observe_at(LoadSignal::Ok, 2 * WINDOW),
            Some(5_000)
        );
    }

    /// The peak-to-trough spread of a slice, which is the amplitude of the sawtooth.
    fn envelope(rates: &[u32]) -> u32 {
        let high = rates.iter().copied().max().unwrap_or(0);
        let low = rates.iter().copied().min().unwrap_or(0);
        high - low
    }
}
