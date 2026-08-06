//! Deterministic, seeded test data, and the seed reporting that makes it reproducible
//! (`TST-101`).
//!
//! Randomised test data finds bugs that hand-written data does not, and costs nothing — provided
//! a failure can be replayed. `TST-101` therefore makes two demands, and this module exists to
//! make both mechanical rather than a matter of remembering:
//!
//! * generation is a pure function of a [`Seed`], so the same seed always produces the same
//!   bytes, on every platform and in every ordering;
//! * a failing test prints its seed, so the reader of a CI log can reproduce it locally without
//!   guessing.
//!
//! The second is the one that gets forgotten, because it only matters on the failure path — the
//! path nobody exercises while writing the test. [`SeedGuard`] therefore prints on `Drop` during
//! a panic, so the reporting happens whether or not the test author thought about it.

use std::fmt;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

/// The environment variable that pins the seed, so a failure can be replayed.
pub const SEED_ENV: &str = "CDM_TEST_SEED";

/// The seed every generator in this crate derives from (`TST-101`).
///
/// A `Seed` is a plain 64-bit number, and the generator it produces is `StdRng`, whose output is
/// stable for a given seed across platforms and architectures. That is the whole reason a
/// dedicated type exists rather than a bare `u64`: it is the place the stability guarantee, the
/// environment override and the failure banner are written down together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seed(u64);

impl Seed {
    /// A seed with an explicit value — the form a replay uses.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The seed named by `CDM_TEST_SEED`, or a fresh one derived from the wall clock.
    ///
    /// A malformed `CDM_TEST_SEED` is ignored rather than fatal: a typo in an environment
    /// variable must not turn every test in the suite red with the same unrelated error, and the
    /// seed that was actually used is printed by [`Seed::banner`] either way.
    pub fn from_env_or_entropy() -> Self {
        if let Ok(raw) = std::env::var(SEED_ENV) {
            if let Ok(value) = raw.trim().parse::<u64>() {
                return Self(value);
            }
        }
        Self(Self::entropy())
    }

    /// A seed from the wall clock, mixed with the process id and a per-process counter.
    ///
    /// Cryptographic quality is beside the point; distinctness is all that is wanted, and a clock
    /// reading that fails is simply another number. The counter is not redundant with the clock:
    /// two calls in quick succession can read the same value, because the clock's *resolution* is
    /// not its *precision*.
    fn entropy() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| {
                u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or_default()
            });
        nanos
            ^ (u64::from(std::process::id()) << 32)
            ^ NEXT.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
    }

    /// The numeric value.
    pub const fn value(self) -> u64 {
        self.0
    }

    /// A generator seeded from this value.
    ///
    /// Calling it twice yields two generators that produce the same sequence, which is what makes
    /// "generate the origin data, then generate the expected target data" reproducible without
    /// threading one generator through both.
    pub fn rng(self) -> StdRng {
        StdRng::seed_from_u64(self.0)
    }

    /// A derived seed, for a second independent stream that must still be reproducible.
    ///
    /// Two streams from `seed.derive("origin")` and `seed.derive("target")` are independent of
    /// each other but both fixed by the parent seed.
    #[must_use]
    pub fn derive(self, label: &str) -> Self {
        // FNV-1a over the label, folded into the parent. Deliberately a written-out hash rather
        // than `DefaultHasher`, whose output std explicitly does not promise to keep stable
        // between releases — which would silently break reproducibility across toolchains.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in label.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Self(self.0.rotate_left(17) ^ hash)
    }

    /// The line a failing test prints so the run can be reproduced (`TST-101`).
    pub fn banner(self) -> String {
        format!(
            "test data seed: {} — reproduce this run with {SEED_ENV}={}",
            self.0, self.0
        )
    }

    /// A guard that prints [`Seed::banner`] if the enclosing test panics (`TST-101`).
    ///
    /// ```
    /// use cdm_testkit::Seed;
    ///
    /// let seed = Seed::from_env_or_entropy();
    /// let _report_on_failure = seed.report_on_panic();
    /// let mut rng = seed.rng();
    /// // ... assertions that would otherwise fail without saying which data they used ...
    /// ```
    pub const fn report_on_panic(self) -> SeedGuard {
        SeedGuard { seed: self }
    }
}

impl fmt::Display for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for Seed {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// Prints its seed if the enclosing test is unwinding (`TST-101`).
///
/// Only on a panic: a passing test that announced its seed would bury the signal in noise, and
/// `cargo test` hides stdout for passing tests anyway.
///
/// Deliberately neither [`Copy`] nor [`Clone`]: a copy would report the seed twice, and the guard
/// exists precisely to be dropped exactly once, at the end of the test that owns it.
#[derive(Debug)]
pub struct SeedGuard {
    seed: Seed,
}

impl SeedGuard {
    /// The seed this guard would report.
    pub const fn seed(&self) -> Seed {
        self.seed
    }
}

impl Drop for SeedGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("{}", self.seed.banner());
        }
    }
}

/// Draws a value in `0..len`, for generators choosing among alternatives.
///
/// A free function rather than a method so that both [`Seed`] and a borrowed `StdRng` can be the
/// caller's unit of reproducibility. Returns `None` for an empty range, which is the only way it
/// can fail and is a caller error rather than a panic.
pub(crate) fn choose(rng: &mut StdRng, len: usize) -> Option<usize> {
    (len > 0).then(|| rng.gen_range(0..len))
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

    fn draw(seed: Seed, count: usize) -> Vec<u64> {
        let mut rng = seed.rng();
        (0..count).map(|_| rng.gen()).collect()
    }

    #[test]
    fn tst_101_the_same_seed_always_produces_the_same_data() {
        assert_eq!(draw(Seed::new(42), 16), draw(Seed::new(42), 16));
        // And across `Seed` values that compare equal, however they were built.
        assert_eq!(draw(Seed::from(7_u64), 8), draw(Seed::new(7), 8));
    }

    #[test]
    fn tst_101_different_seeds_produce_different_data() {
        assert_ne!(draw(Seed::new(1), 16), draw(Seed::new(2), 16));
    }

    #[test]
    fn tst_101_the_expected_values_are_pinned_so_a_toolchain_change_cannot_move_them() {
        // If this fails, `StdRng`'s algorithm changed and every recorded seed in every past CI
        // log stopped meaning what it meant. That is a breaking change to the reproducibility
        // contract, not a test to update casually.
        assert_eq!(
            draw(Seed::new(0), 4),
            vec![
                13_486_662_071_293_341_567,
                14_267_822_071_968_393_595,
                476_749_353_381_333_526,
                10_775_836_403_224_147_664,
            ]
        );
    }

    #[test]
    fn tst_101_derived_seeds_are_independent_but_still_reproducible() {
        let parent = Seed::new(99);
        let origin = parent.derive("origin");
        let target = parent.derive("target");

        assert_ne!(origin, target);
        assert_ne!(origin, parent);
        assert_eq!(origin, Seed::new(99).derive("origin"));
        assert_ne!(draw(origin, 8), draw(target, 8));
    }

    #[test]
    fn tst_101_the_banner_names_the_variable_that_replays_the_run() {
        let banner = Seed::new(1234).banner();
        assert!(banner.contains("1234"), "{banner}");
        assert!(banner.contains("CDM_TEST_SEED=1234"), "{banner}");
        assert_eq!(SEED_ENV, "CDM_TEST_SEED");
        assert_eq!(Seed::new(1234).to_string(), "1234");
        assert_eq!(Seed::new(1234).value(), 1234);
    }

    #[test]
    fn tst_101_a_guard_carries_the_seed_it_would_report() {
        let seed = Seed::new(5);
        let guard = seed.report_on_panic();
        assert_eq!(guard.seed(), seed);
        // Dropping without a panic in flight must print nothing and must not itself panic.
        drop(guard);
    }

    #[test]
    fn tst_101_a_guard_prints_the_seed_when_the_test_panics() {
        // Exercises the branch that only runs while unwinding, which no ordinary assertion can
        // reach: catch a panic raised while a guard is alive.
        let outcome = std::panic::catch_unwind(|| {
            let _guard = Seed::new(777).report_on_panic();
            panic!("deliberate");
        });
        assert!(outcome.is_err());
    }

    #[test]
    fn tst_101_entropy_seeds_differ_between_calls() {
        assert_ne!(Seed::entropy(), 0);
        assert_ne!(Seed::entropy(), Seed::entropy());
    }

    #[test]
    fn tst_101_choose_is_none_for_an_empty_range_and_in_bounds_otherwise() {
        let mut rng = Seed::new(3).rng();
        assert_eq!(choose(&mut rng, 0), None);
        for _ in 0..100 {
            let index = choose(&mut rng, 5).unwrap();
            assert!(index < 5);
        }
    }
}
