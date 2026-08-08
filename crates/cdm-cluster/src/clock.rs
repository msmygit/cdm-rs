//! The clock a lease is judged by (`DST-012`).
//!
//! Lease expiry is a comparison between a timestamp another node wrote and the time this node
//! believes it is. That makes the clock a *parameter of correctness*, not an ambient service, so
//! it is named here and injected rather than read from `Utc::now()` at the point of use. Two
//! things follow, and both matter:
//!
//! * a test can decide that a lease has expired without waiting for it to, so the expiry,
//!   renewal and contention suites contain no sleep whose duration decides their outcome;
//! * the place where clock skew enters the system is a single trait, which is where the
//!   documentation about skew belongs.

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};

use chrono::{DateTime, TimeZone as _, Utc};

/// What the coordinator asks when it needs to know whether a lease has expired.
pub trait Clock: fmt::Debug + Send + Sync + 'static {
    /// The current instant, as this node understands it.
    fn now(&self) -> DateTime<Utc>;
}

/// The host's wall clock — the only clock a real run uses.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that only moves when a test moves it.
///
/// Public rather than test-only on purpose: `DST-019`'s node-death harness (#52) needs to make a
/// lease expire without waiting a minute for it, and so does anyone writing a plugin against
/// [`Coordinator`](crate::Coordinator). It is inert in production — nothing constructs one unless
/// asked to.
#[derive(Debug)]
pub struct ManualClock {
    millis: AtomicI64,
}

impl ManualClock {
    /// A clock reading `at`.
    #[must_use]
    pub fn new(at: DateTime<Utc>) -> Self {
        Self {
            millis: AtomicI64::new(at.timestamp_millis()),
        }
    }

    /// A clock reading the Unix epoch, which is the tidiest start for arithmetic in a test.
    #[must_use]
    pub fn epoch() -> Self {
        Self::new(DateTime::UNIX_EPOCH)
    }

    /// Moves the clock forward by `by`.
    ///
    /// Saturating, and forward-only: a clock that can be wound back would let a test construct a
    /// history no run can observe.
    pub fn advance(&self, by: std::time::Duration) {
        let millis = i64::try_from(by.as_millis()).unwrap_or(i64::MAX);
        self.millis.fetch_add(millis, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        let millis = self.millis.load(Ordering::SeqCst);
        // `timestamp_millis_opt` is `None` only outside chrono's representable range, which this
        // clock cannot reach: it starts at a real instant and only ever moves forward by a
        // `Duration`, saturating at `i64::MAX` milliseconds — year 292 million.
        Utc.timestamp_millis_opt(millis)
            .single()
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
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
    use std::time::Duration;

    use super::*;

    #[test]
    fn dst_012_a_manual_clock_moves_only_when_it_is_told_to() {
        let clock = ManualClock::epoch();
        let start = clock.now();
        assert_eq!(start, clock.now(), "a manual clock does not drift");
        clock.advance(Duration::from_secs(90));
        assert_eq!(clock.now(), start + chrono::Duration::seconds(90));
    }

    #[test]
    fn dst_012_the_system_clock_reads_the_host() {
        let before = Utc::now();
        let now = SystemClock.now();
        assert!(now >= before - chrono::Duration::seconds(1));
    }
}
