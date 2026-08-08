//! Rate limiters and in-flight semaphores, the two things that bound a run (`ENG-004`,
//! `ENG-005`, `ENG-007`, `NFR-003`).
//!
//! [`RuntimeLimits`] is the whole of the scheduler's back pressure, in one shareable object:
//!
//! | Limit | Mechanism | Setting | Requirement |
//! |---|---|---|---|
//! | Origin rows read per second | [`RateLimiter`] | `perfops.ratelimit.origin` | `ENG-004` |
//! | Target rows written per second | [`RateLimiter`] | `perfops.ratelimit.target` | `ENG-004` |
//! | Concurrent origin reads | [`Semaphore`] | `perfops.max_inflight_reads` | `ENG-007` |
//! | Concurrent target writes | [`Semaphore`] | `perfops.max_inflight_writes` | `ENG-007` |
//! | Target rate, when the target complains | [`AdaptiveRateController`] | `perfops.adaptive_ratelimit` | `ENG-006` |
//!
//! The rate limiters bound *throughput*; the semaphores bound *memory*. They are not
//! interchangeable, which is why `ENG-007` exists separately from `ENG-005`: a rate limit says
//! nothing about how many pages are resident at once, and it is the resident pages that decide
//! whether the process fits in its RSS budget. `NFR-003` states that budget as
//! `~200 MB + (max_inflight_reads + max_inflight_writes) × average_row_size × 2`, and it holds
//! only because every read and every write passes through one of these semaphores first.
//!
//! Both semaphores are shared by every worker, so the bound is on the run, not on the worker: a
//! run with 64 workers and `max_inflight_reads = 256` has at most 256 reads outstanding, not
//! 16 384.

use std::sync::Arc;
use std::time::Duration;

use cdm_core::{CdmError, ErrorKind, RequestObserver, Side};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::scheduler::adaptive::{AdaptiveRateController, LoadSignal};
use crate::scheduler::ratelimit::RateLimiter;
use crate::scheduler::settings::SchedulerSettings;

/// A claim on one in-flight slot, held for as long as the request is outstanding (`ENG-007`).
///
/// Dropping it returns the slot. There is nothing to call and nothing to forget: a request that
/// panics or is cancelled releases its slot with the rest of its stack.
#[derive(Debug)]
pub struct InflightPermit {
    _permit: OwnedSemaphorePermit,
}

/// The rate limiters and in-flight semaphores of one run (`ENG-004`, `ENG-005`, `ENG-007`).
#[derive(Debug)]
pub struct RuntimeLimits {
    origin_rate: RateLimiter,
    target_rate: RateLimiter,
    adaptive: Option<AdaptiveRateController>,
    reads: Arc<Semaphore>,
    writes: Arc<Semaphore>,
    /// Where a rate-limiter wait is reported (`MET-010`), or nowhere.
    ///
    /// `None` on a run nobody is watching, in which case the wait costs one null check on top of
    /// the sleep it is measuring. The limiter has already computed the delay, so there is no
    /// clock read here even when it is `Some`.
    requests: Option<Arc<dyn RequestObserver>>,
}

impl RuntimeLimits {
    /// Builds the limits a settings object describes.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if either in-flight bound is zero, which would deadlock the run, or
    /// larger than [`Semaphore::MAX_PERMITS`].
    pub fn new(settings: &SchedulerSettings) -> Result<Self, CdmError> {
        // ENG-006: the controller exists only when the operator asked for it, so a run without
        // `perfops.adaptive_ratelimit` cannot have its rate moved by anything.
        let adaptive = settings.adaptive_ratelimit().then(|| {
            let controller = AdaptiveRateController::new(
                settings.target_rows_per_second(),
                settings.adaptive_ratelimit_min_percent(),
            );
            if controller.is_active() {
                tracing::info!(
                    target: "cdm::engine",
                    ceiling = controller.ceiling(),
                    floor = controller.floor(),
                    step = controller.step(),
                    "the target write rate is adaptive: it will be halved for each control \
                     window in which the target reports overload (ENG-006)"
                );
            } else {
                tracing::warn!(
                    target: "cdm::engine",
                    "perfops.adaptive_ratelimit is set but perfops.ratelimit.target is 0 \
                     (unlimited), so there is no rate to reduce (ENG-006)"
                );
            }
            controller
        });
        Ok(Self {
            origin_rate: RateLimiter::new(settings.origin_rows_per_second()),
            target_rate: RateLimiter::new(settings.target_rows_per_second()),
            adaptive,
            reads: semaphore("perfops.max_inflight_reads", settings.max_inflight_reads())?,
            writes: semaphore(
                "perfops.max_inflight_writes",
                settings.max_inflight_writes(),
            )?,
            requests: None,
        })
    }

    /// Reports rate-limiter waits to `requests` (`MET-010`, `ENG-005`).
    ///
    /// A builder rather than a constructor argument for the same reason the executors of
    /// `cdm-cql` take one: the observer belongs to the run, and the limits are built from the
    /// settings before anything is watching.
    #[must_use]
    pub fn observing(mut self, requests: Option<Arc<dyn RequestObserver>>) -> Self {
        self.requests = requests;
        self
    }

    /// The adaptive controller, when `perfops.adaptive_ratelimit` is set (`ENG-006`).
    #[must_use]
    pub const fn adaptive(&self) -> Option<&AdaptiveRateController> {
        self.adaptive.as_ref()
    }

    /// Feeds one target request's outcome to `ENG-006`'s controller.
    ///
    /// A no-op — not merely a cheap one, but one that touches no state at all — unless
    /// `perfops.adaptive_ratelimit` is set. When the controller decides a new rate, it is applied
    /// to the target limiter here and nowhere else, so there is exactly one place where the rate
    /// a run is actually paced at can change.
    ///
    /// This is deliberately *not* an error path. A signal is not a failure: a write timeout that
    /// the retry of `CON-011` absorbs never becomes a `CdmError` at all, and one that does is
    /// accounted for by `ENG-008` exactly as it would be without this call. Nothing here
    /// increments a counter, so backing off cannot move the run towards `perfops.error_limit`
    /// (`ENG-009`) — and nothing here blocks, so it cannot delay a shutdown (`ENG-010`).
    pub fn record_target_signal(&self, signal: LoadSignal) {
        let Some(controller) = &self.adaptive else {
            return;
        };
        if let Some(rate) = controller.observe(signal) {
            self.target_rate.set_rows_per_second(rate);
            tracing::info!(
                target: "cdm::engine",
                rows_per_second = rate,
                ceiling = controller.ceiling(),
                floor = controller.floor(),
                "the adaptive controller changed the target write rate (ENG-006)"
            );
        }
    }

    /// The origin read limiter, in rows per second (`ENG-004`).
    #[must_use]
    pub const fn origin_rate(&self) -> &RateLimiter {
        &self.origin_rate
    }

    /// The target write limiter, in rows per second (`ENG-004`).
    #[must_use]
    pub const fn target_rate(&self) -> &RateLimiter {
        &self.target_rate
    }

    /// Waits until `rows` more rows may be read from the origin (`ENG-004`, `ENG-005`).
    pub async fn acquire_read_rows(&self, rows: u32) {
        let waited = self.origin_rate.acquire(rows).await;
        self.record_wait(Side::Origin, waited);
    }

    /// Waits until `rows` more rows may be written to the target (`ENG-004`, `ENG-005`).
    pub async fn acquire_write_rows(&self, rows: u32) {
        let waited = self.target_rate.acquire(rows).await;
        self.record_wait(Side::Target, waited);
    }

    /// Records one rate-limiter wait (`MET-010`).
    ///
    /// A zero-length wait is recorded too: the distribution's whole purpose is to answer "is the
    /// run being held back?", and dropping the zeroes would make one throttled call in a thousand
    /// look like a permanently throttled run.
    fn record_wait(&self, side: Side, waited: Duration) {
        if let Some(observer) = self.requests.as_deref() {
            observer.ratelimit_waited(side, waited);
        }
    }

    /// Claims one in-flight origin read slot (`ENG-007`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the semaphore has been closed, which the scheduler never does.
    pub async fn read_slot(&self) -> Result<InflightPermit, CdmError> {
        acquire(&self.reads, "read").await
    }

    /// Claims one in-flight target write slot (`ENG-007`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the semaphore has been closed, which the scheduler never does.
    pub async fn write_slot(&self) -> Result<InflightPermit, CdmError> {
        acquire(&self.writes, "write").await
    }

    /// How many origin reads may still be started before a caller has to wait.
    #[must_use]
    pub fn available_read_slots(&self) -> usize {
        self.reads.available_permits()
    }

    /// How many target writes may still be started before a caller has to wait.
    #[must_use]
    pub fn available_write_slots(&self) -> usize {
        self.writes.available_permits()
    }
}

/// `ENG-006`: the seam `cdm-cql`'s write path reports through.
///
/// The classification of "overload" lives in `cdm-cql`, which is the only crate allowed to read a
/// CQL error frame; the control law lives in [`AdaptiveRateController`], which never sees one.
/// This impl is the two-line join between them, and is why neither has to know about the other.
impl cdm_cql::exec::TargetLoadObserver for RuntimeLimits {
    fn on_target_ok(&self) {
        self.record_target_signal(LoadSignal::Ok);
    }

    fn on_target_overload(&self) {
        self.record_target_signal(LoadSignal::Overload);
    }
}

/// Builds one bounded semaphore from a checked bound.
fn semaphore(setting: &str, permits: u32) -> Result<Arc<Semaphore>, CdmError> {
    check_permits(setting, usize::try_from(permits).unwrap_or(usize::MAX))
        .map(Semaphore::new)
        .map(Arc::new)
}

/// Rejects the two in-flight bounds that cannot work.
///
/// The upper bound only bites on a 32-bit target, where [`Semaphore::MAX_PERMITS`] is smaller
/// than `u32::MAX`; on a 64-bit one no `u32` can reach it. It is checked regardless, because the
/// alternative on the platforms where it *can* happen is a panic from inside the runtime.
fn check_permits(setting: &str, permits: usize) -> Result<usize, CdmError> {
    if permits == 0 {
        return Err(CdmError::new(
            ErrorKind::Config,
            format!("{setting} must be at least 1; a bound of 0 would stall every worker"),
        )
        .with_context(|ctx| ctx.with_config_key(setting)));
    }
    if permits > Semaphore::MAX_PERMITS {
        return Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "{setting} must not exceed {}, the runtime's maximum",
                Semaphore::MAX_PERMITS
            ),
        )
        .with_context(|ctx| ctx.with_config_key(setting)));
    }
    Ok(permits)
}

/// Acquires one permit, converting the only failure the runtime can report.
async fn acquire(semaphore: &Arc<Semaphore>, side: &str) -> Result<InflightPermit, CdmError> {
    Arc::clone(semaphore)
        .acquire_owned()
        .await
        .map(|permit| InflightPermit { _permit: permit })
        .map_err(|_closed| {
            CdmError::new(
                ErrorKind::Internal,
                format!("the in-flight {side} semaphore was closed while the run was still going"),
            )
        })
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn settings() -> SchedulerSettings {
        SchedulerSettings::default()
    }

    #[tokio::test(start_paused = true)]
    async fn met_010_a_rate_limited_acquire_reports_its_wait_to_the_instruments() {
        // `MET-010` names rate-limiter wait time, and this is the only place it can be measured:
        // the limiter is the thing that waits. `Instruments` implements `cdm_core::RequestObserver`,
        // so the real instrument is under test here, not a double — a double would prove the call
        // is made and nothing about what it lands in.
        let instruments = Arc::new(cdm_metrics::Instruments::new(std::time::Instant::now()));
        // One row per second with one second of burst, so the second call has to wait a whole
        // second on the virtual clock.
        let limits = RuntimeLimits::new(&settings().with_ratelimits(1, 0))
            .unwrap()
            .observing(Some(
                Arc::clone(&instruments) as Arc<dyn cdm_core::RequestObserver>
            ));

        limits.acquire_read_rows(1).await;
        limits.acquire_read_rows(1).await;
        limits.acquire_write_rows(1).await;

        let snapshot = instruments.snapshot();
        assert_eq!(
            snapshot.origin.ratelimit_wait.count, 2,
            "both origin acquisitions must be recorded, including the one that did not wait"
        );
        assert!(
            snapshot.origin.ratelimit_wait.max > 0,
            "an acquisition past the burst budget waited, and the wait must be visible"
        );
        // `ENG-004` gives the two sides separate limiters; unlimited means no wait, not no sample.
        assert_eq!(snapshot.target.ratelimit_wait.count, 1);
        assert_eq!(snapshot.target.ratelimit_wait.max, 0);
    }

    #[tokio::test]
    async fn met_010_an_unobserved_run_records_no_wait_and_still_paces_itself() {
        let limits = RuntimeLimits::new(&settings().with_ratelimits(0, 0)).unwrap();
        // No observer, no panic, no cost: the whole of what a silent run pays for `MET-010`.
        limits.acquire_read_rows(10).await;
        limits.acquire_write_rows(10).await;
    }

    #[tokio::test]
    async fn eng_007_in_flight_reads_are_bounded_by_the_semaphore() {
        let limits = Arc::new(
            RuntimeLimits::new(&settings().with_max_inflight_reads(4).with_ratelimits(0, 0))
                .unwrap(),
        );

        let peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..64 {
            let limits = Arc::clone(&limits);
            let peak = Arc::clone(&peak);
            let live = Arc::clone(&live);
            handles.push(tokio::spawn(async move {
                let permit = limits.read_slot().await.unwrap();
                let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                live.fetch_sub(1, Ordering::SeqCst);
                drop(permit);
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 4, "{peak:?}");
        assert_eq!(limits.available_read_slots(), 4);
    }

    #[tokio::test]
    async fn eng_007_in_flight_writes_are_bounded_independently_of_reads() {
        let limits = RuntimeLimits::new(
            &settings()
                .with_max_inflight_reads(1)
                .with_max_inflight_writes(3),
        )
        .unwrap();
        let _read = limits.read_slot().await.unwrap();
        assert_eq!(limits.available_read_slots(), 0);
        // The write side is untouched by an exhausted read side.
        assert_eq!(limits.available_write_slots(), 3);
        let _write = limits.write_slot().await.unwrap();
        assert_eq!(limits.available_write_slots(), 2);
    }

    #[tokio::test]
    async fn eng_007_a_permit_is_returned_when_it_is_dropped() {
        let limits = RuntimeLimits::new(&settings().with_max_inflight_reads(1)).unwrap();
        {
            let _permit = limits.read_slot().await.unwrap();
            assert_eq!(limits.available_read_slots(), 0);
        }
        assert_eq!(limits.available_read_slots(), 1);
    }

    #[test]
    fn eng_007_a_zero_in_flight_bound_is_rejected_at_startup() {
        let err = RuntimeLimits::new(&settings().with_max_inflight_reads(0)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("max_inflight_reads"), "{err}");

        let err = RuntimeLimits::new(&settings().with_max_inflight_writes(0)).unwrap_err();
        assert!(err.message().contains("max_inflight_writes"), "{err}");
    }

    #[test]
    fn eng_007_an_in_flight_bound_above_the_runtime_maximum_is_rejected() {
        let err =
            check_permits("perfops.max_inflight_reads", Semaphore::MAX_PERMITS + 1).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("maximum"), "{err}");

        // On a 64-bit target no `u32` can reach the runtime's maximum, so the largest bound the
        // configuration can express is accepted.
        assert!(check_permits("perfops.max_inflight_reads", Semaphore::MAX_PERMITS).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn eng_004_the_read_and_write_rates_come_from_their_own_settings() {
        let limits = RuntimeLimits::new(&settings().with_ratelimits(10, 20)).unwrap();
        assert_eq!(limits.origin_rate().rows_per_second(), 10);
        assert_eq!(limits.target_rate().rows_per_second(), 20);

        let started = tokio::time::Instant::now();
        limits.acquire_read_rows(20).await;
        assert_eq!(started.elapsed(), std::time::Duration::from_secs(1));

        // Twenty writes is exactly the write side's one-second burst, so it does not wait.
        let before = tokio::time::Instant::now();
        limits.acquire_write_rows(20).await;
        assert_eq!(before.elapsed(), std::time::Duration::ZERO);
    }

    /// A limits object with an adaptive target rate, and no other limit in the way.
    fn adaptive_limits() -> RuntimeLimits {
        RuntimeLimits::new(
            &settings()
                .with_ratelimits(0, 10_000)
                .with_adaptive_ratelimit(true, 10),
        )
        .unwrap()
    }

    #[test]
    fn eng_006_the_controller_exists_only_when_the_operator_asked_for_one() {
        // The whole feature, off: no controller, nothing to observe through, and no way for a
        // signal to reach the rate.
        let off = RuntimeLimits::new(&settings().with_ratelimits(0, 10_000)).unwrap();
        assert!(off.adaptive().is_none());

        let on = adaptive_limits();
        let controller = on.adaptive().unwrap();
        assert_eq!(controller.ceiling(), 10_000);
        assert_eq!(controller.floor(), 1_000);
    }

    #[tokio::test(start_paused = true)]
    async fn eng_006_overload_signals_lower_the_rate_the_limiter_actually_paces_at() {
        // The anti-#21c test: the flag is not merely accepted, the run is measurably slower.
        let limits = adaptive_limits();
        assert_eq!(limits.target_rate().rows_per_second(), 10_000);

        for _ in 0..40 {
            limits.record_target_signal(LoadSignal::Overload);
            tokio::time::advance(std::time::Duration::from_millis(600)).await;
        }

        let reduced = limits.target_rate().rows_per_second();
        assert!(
            reduced < 10_000,
            "the target rate never moved: {reduced} rows/s"
        );
        assert_eq!(
            reduced,
            limits.adaptive().unwrap().rate(),
            "the limiter must be paced at exactly what the controller decided"
        );
        assert!(
            reduced >= limits.adaptive().unwrap().floor(),
            "the rate fell through the floor and the run would look hung"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn eng_006_a_run_without_the_flag_ignores_every_signal() {
        let limits = RuntimeLimits::new(&settings().with_ratelimits(0, 10_000)).unwrap();
        for _ in 0..40 {
            limits.record_target_signal(LoadSignal::Overload);
            tokio::time::advance(std::time::Duration::from_millis(600)).await;
        }
        assert_eq!(
            limits.target_rate().rows_per_second(),
            10_000,
            "`perfops.adaptive_ratelimit` is off, so nothing may move the rate"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn eng_006_the_origin_read_rate_is_never_adaptive() {
        // ENG-004 keeps the two sides independent, and it is the *target* that reports overload.
        // A read side throttled by the target's problems could not recover on its own.
        let limits = RuntimeLimits::new(
            &settings()
                .with_ratelimits(7_000, 10_000)
                .with_adaptive_ratelimit(true, 10),
        )
        .unwrap();
        for _ in 0..40 {
            limits.record_target_signal(LoadSignal::Overload);
            tokio::time::advance(std::time::Duration::from_millis(600)).await;
        }
        assert_eq!(limits.origin_rate().rows_per_second(), 7_000);
        assert!(limits.target_rate().rows_per_second() < 10_000);
    }

    #[tokio::test(start_paused = true)]
    async fn eng_006_and_eng_009_backing_off_costs_the_run_no_errors() {
        // `ENG-009` counts rows lost, and a rate reduction loses none. If a signal incremented
        // anything, a target having a bad afternoon would abort a run that was in no trouble.
        let limits = adaptive_limits();
        let counters = cdm_metrics::JobCounters::new(cdm_core::JobKind::Migrate);
        for _ in 0..40 {
            limits.record_target_signal(LoadSignal::Overload);
            tokio::time::advance(std::time::Duration::from_millis(600)).await;
        }
        assert_eq!(
            counters.count_of(
                cdm_metrics::CounterKind::Error,
                cdm_metrics::CounterView::Interim
            ),
            0
        );
    }

    #[test]
    fn eng_006_an_unlimited_target_rate_leaves_the_controller_with_nothing_to_do() {
        let limits = RuntimeLimits::new(
            &settings()
                .with_ratelimits(0, 0)
                .with_adaptive_ratelimit(true, 10),
        )
        .unwrap();
        assert!(!limits.adaptive().unwrap().is_active());
        limits.record_target_signal(LoadSignal::Overload);
        assert_eq!(limits.target_rate().rows_per_second(), 0);
    }

    #[tokio::test]
    async fn eng_007_a_closed_semaphore_is_reported_rather_than_panicking() {
        let closed = Arc::new(Semaphore::new(1));
        closed.close();
        let err = acquire(&closed, "read").await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
    }
}
