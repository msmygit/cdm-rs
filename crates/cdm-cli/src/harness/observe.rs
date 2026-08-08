//! The bridge from the scheduler to everything that watches a run (`MET-030`, `MET-031`).
//!
//! `ENG-002` gives the scheduler one seam — [`RangeObserver`] — and calls it twice per range, once
//! before the work starts and once after its counters are committed. That is enough to feed
//! everything a live display needs, and this is the type that does it:
//!
//! | Called | What [`LiveRun`] does with it |
//! |---|---|
//! | `on_range_started` | marks the range in flight (`MET-011`), publishes `RangeStarted`, starts its clock |
//! | `on_range_finished` | records the terminal status, the range's duration, its counter deltas, and publishes `RangeCompleted` — plus `Error` when the range failed |
//! | `on_run_finished` | publishes `RunCompleted` with the committed counters |
//!
//! A run has more than one watcher, though, and the seam is singular: tracking (`TRK-021`) uses
//! the same two callbacks to write the rows a resume is planned from. [`Observers`] is what lets
//! both be installed at once, and its documentation says why the order it calls them in is a
//! safety property rather than a preference.
//!
//! # Why the display does not read progress off the bus
//!
//! Both are fed here, and they are fed differently on purpose. The [`ProgressTracker`] and the
//! [`Instruments`] are *shared state*, updated synchronously: an update cannot be lost. The
//! [`EventBus`] is a bounded broadcast that drops rather than blocking (`MET-030`), because
//! nothing an observer does may be allowed to slow a migration down. A progress bar read from the
//! bus would be permanently short by whatever the bus dropped; a progress bar read from the
//! tracker cannot be. `cdm_metrics::dashboard` has the full table.
//!
//! # Cost
//!
//! Everything here happens twice per range, not per row. At the default `perfops.num_parts` of
//! 5000 that is ten thousand calls over the life of a run, so a mutex, a clock read and a string
//! parse are all affordable — and none of them is on the path a row takes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cdm_core::{JobKind, RunId, Side, TokenRange};
use cdm_engine::scheduler::{NoopObserver, RangeObserver, RangeOutcome, RunReport};
use cdm_metrics::{
    CounterKind, CounterView, DashboardState, EventBus, Instruments, ProgressTracker, RangeTimings,
};

/// Everything a running job publishes about itself (`MET-030`, `MET-031`).
///
/// Built once per run, before the scheduler starts, and handed to it as the [`RangeObserver`].
/// Cheap to build and inert when nobody is watching: with no subscriber the bus's sends fail fast
/// and are counted, and the tracker and instruments are a few atomics.
#[derive(Debug)]
pub struct LiveRun {
    job: JobKind,
    run_id: RunId,
    node_id: String,
    /// The structured event stream (`MET-030`).
    pub events: Arc<EventBus>,
    /// Weighted progress and the ETA (`MET-011`).
    pub progress: Arc<ProgressTracker>,
    /// Throughput, request latency, in-flight requests, retries and rate-limiter waits
    /// (`MET-010`).
    ///
    /// Shared with the executors that issue the requests: `Instruments` implements
    /// [`cdm_core::RequestObserver`], so `cdm-cql` records a page request and `cdm-engine` records
    /// a rate-limiter wait straight into this value, without either of them naming a metrics
    /// type. That is why it is constructed by the caller and handed in rather than created here —
    /// the jobs are built before the observer is.
    pub instruments: Arc<Instruments>,
    /// How long ranges are taking (`MET-031`).
    pub timings: Arc<RangeTimings>,
    /// When each in-flight range was claimed, so that its duration can be measured.
    ///
    /// A `std::sync::Mutex` rather than `parking_lot`'s: this is locked twice per range and
    /// `cdm-cli` has no reason to take a dependency for it. A poisoned lock is treated as an empty
    /// map — a duration is a diagnostic, and `ERR-004` leaves no room for the panic an `unwrap`
    /// would put on a production path.
    started: Mutex<BTreeMap<TokenRange, Instant>>,
}

impl LiveRun {
    /// Prepares the observers for a run over `ranges`, recording into `instruments`.
    ///
    /// `instruments` is a parameter because the jobs are built before the run is: `MET-010`'s
    /// request latencies are recorded by the executors inside those jobs, which have to be handed
    /// the very same value this observer later reports.
    #[must_use]
    pub fn new(
        job: JobKind,
        run_id: RunId,
        node_id: impl Into<String>,
        events: Arc<EventBus>,
        instruments: Arc<Instruments>,
        ranges: &[TokenRange],
        now: Instant,
    ) -> Self {
        Self {
            job,
            run_id,
            node_id: node_id.into(),
            events,
            progress: Arc::new(ProgressTracker::by_token_span(ranges, now)),
            instruments,
            timings: Arc::new(RangeTimings::new()),
            started: Mutex::new(BTreeMap::new()),
        }
    }

    /// The view model a display folds events into (`MET-031`).
    #[must_use]
    pub fn dashboard(&self) -> DashboardState {
        DashboardState::new(
            self.job,
            self.run_id,
            self.node_id.clone(),
            Arc::clone(&self.progress),
            Arc::clone(&self.instruments),
            Arc::clone(&self.timings),
        )
    }
}

/// Every observer one run has, presented to the scheduler as one (`MET-031`, `TRK-021`).
///
/// A run needs two things watched at once and they are unrelated to each other: the live display's
/// feed ([`LiveRun`]) and the durable record a resume is planned from (`cdm-track`'s `RunTracker`,
/// reached through `super::Tracking`). `Scheduler::run` takes a single observer, so either one
/// alone silently drops the other — a tracked run with no display, or a display over a run that
/// records nothing and can never be resumed. Neither failure says anything at the time.
///
/// # Order is a safety property, not a preference
///
/// Observers are called in the order they were added, and `super::run` adds **tracking first**.
/// Both are cheap and neither blocks — a tracking write is a `try_send` that degrades to a
/// checkpoint (`TRK-035`), a display update is a few atomics and a bounded broadcast that drops
/// (`MET-030`) — so the order cannot matter for throughput. It matters for what survives: an
/// observer that panicked would take the scheduler's worker with it (`ENG-013` catches the
/// *processor*'s panics, not an observer's), and of the two, the one whose loss is unrecoverable
/// is the durable record. Enqueue that first and a later panic costs a frame of display rather
/// than a range that no resume will know to re-run.
#[derive(Default)]
pub struct Observers {
    observers: Vec<Arc<dyn RangeObserver>>,
}

impl std::fmt::Debug for Observers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `RangeObserver` is not `Debug` — it cannot be, since an implementation may hold a driver
        // session — so the count is the honest thing to print.
        f.debug_struct("Observers")
            .field("observers", &self.observers.len())
            .finish()
    }
}

impl Observers {
    /// No observers. The scheduler sees a run nobody is watching.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            observers: Vec::new(),
        }
    }

    /// Adds `observer` if there is one, so a caller can write the optional cases in a chain.
    #[must_use]
    pub fn and(mut self, observer: Option<Arc<dyn RangeObserver>>) -> Self {
        self.observers.extend(observer);
        self
    }

    /// How many observers will be called per range.
    #[must_use]
    pub fn len(&self) -> usize {
        self.observers.len()
    }

    /// Whether nothing is watching.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.observers.is_empty()
    }

    /// The observer to hand the scheduler.
    ///
    /// Collapses the two cases that do not need a fan-out: nothing watching becomes the
    /// `NoopObserver` a silent run has always had, and a single watcher is handed over directly,
    /// so the common paths pay neither an allocation nor an indirection.
    #[must_use]
    pub fn into_observer(mut self) -> Arc<dyn RangeObserver> {
        match self.observers.len() {
            0 => Arc::new(NoopObserver),
            // `pop` cannot fail on a length of one; `map_or_else` keeps `ERR-004` satisfied
            // without an index or an `unwrap` on a production path.
            1 => self.observers.pop().map_or_else(
                || Arc::new(NoopObserver) as Arc<dyn RangeObserver>,
                |one| one,
            ),
            _ => Arc::new(self),
        }
    }
}

impl RangeObserver for Observers {
    fn on_range_started(&self, run_id: RunId, range: TokenRange) {
        for observer in &self.observers {
            observer.on_range_started(run_id, range);
        }
    }

    fn on_range_finished(&self, run_id: RunId, outcome: &RangeOutcome) {
        for observer in &self.observers {
            observer.on_range_finished(run_id, outcome);
        }
    }

    fn on_run_finished(&self, report: &RunReport) {
        for observer in &self.observers {
            observer.on_run_finished(report);
        }
    }
}

impl RangeObserver for LiveRun {
    fn on_range_started(&self, _run_id: RunId, range: TokenRange) {
        self.progress.range_started(range);
        if let Ok(mut started) = self.started.lock() {
            started.insert(range, Instant::now());
        }
        self.events.range_started(chrono::Utc::now(), range);
    }

    fn on_range_finished(&self, _run_id: RunId, outcome: &RangeOutcome) {
        let now = Instant::now();
        self.progress.range_completed(outcome.range, outcome.status);

        if let Ok(mut started) = self.started.lock() {
            if let Some(claimed) = started.remove(&outcome.range) {
                self.timings.record(now.saturating_duration_since(claimed));
            }
        }

        // `MET-005`'s string is the only account of the range the observer is given. Read is the
        // origin's rows and Write the target's, which is exactly what `MET-010`'s two meters mean.
        let counts = cdm_metrics::parse_run_info(&outcome.run_info);
        if let Some(read) = counts.get(CounterKind::Read.index()) {
            self.instruments.rows(Side::Origin).mark_at(*read, now);
        }
        if let Some(written) = counts.get(CounterKind::Write.index()) {
            self.instruments.rows(Side::Target).mark_at(*written, now);
        }

        let at = chrono::Utc::now();
        self.events
            .range_completed(at, outcome.range, outcome.status, outcome.run_info.clone());
        // `ENG-008`: a failed range does not stop the run, which is exactly why it has to be
        // visible. The diagnostic is published as it stands; `SEC-002` is enforced where the
        // diagnostic is built and again in what a display is willing to draw.
        if let Some(diagnostic) = &outcome.diagnostic {
            self.events
                .error(at, diagnostic.clone(), Some(outcome.range));
        }
    }

    fn on_run_finished(&self, report: &RunReport) {
        let counters = report
            .counters()
            .registered()
            .iter()
            .map(|&kind| {
                (
                    kind.as_str().to_owned(),
                    report.counters().count_of(kind, CounterView::Committed),
                )
            })
            .collect();
        self.events.run_completed(
            chrono::Utc::now(),
            report.status(),
            counters,
            report.elapsed(),
        );
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
    use cdm_core::{Diagnostic, RunStatus};
    use cdm_metrics::EventPayload;

    use super::*;

    /// An observer that records which callbacks it saw, and in which order it was called relative
    /// to its siblings through the shared log.
    #[derive(Debug)]
    struct Recorder {
        name: &'static str,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Recorder {
        fn new(name: &'static str, log: &Arc<Mutex<Vec<String>>>) -> Arc<Self> {
            Arc::new(Self {
                name,
                log: Arc::clone(log),
            })
        }

        fn note(&self, event: &str) {
            if let Ok(mut log) = self.log.lock() {
                log.push(format!("{}:{event}", self.name));
            }
        }
    }

    impl RangeObserver for Recorder {
        fn on_range_started(&self, _run_id: RunId, _range: TokenRange) {
            self.note("started");
        }

        fn on_range_finished(&self, _run_id: RunId, _outcome: &RangeOutcome) {
            self.note("finished");
        }

        fn on_run_finished(&self, _report: &RunReport) {
            self.note("run_finished");
        }
    }

    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    #[test]
    fn trk_021_met_031_every_observer_sees_every_range_in_the_order_it_was_added() {
        // The whole reason this type exists: a run is displayed *and* recorded, and `Scheduler`
        // takes one observer. Whichever was handed over alone would silently drop the other — a
        // display over a run that records nothing is the defect `TRK-038` exists to prevent, and
        // it says nothing at the time.
        let log = Arc::new(Mutex::new(Vec::new()));
        let tracking = Recorder::new("tracking", &log);
        let display = Recorder::new("display", &log);

        let observers = Observers::new()
            .and(Some(Arc::clone(&tracking) as Arc<dyn RangeObserver>))
            .and(Some(Arc::clone(&display) as Arc<dyn RangeObserver>));
        assert_eq!(observers.len(), 2);
        assert!(!observers.is_empty());

        let fanned = observers.into_observer();
        fanned.on_range_started(RunId::from_raw(1), range(0, 9));
        fanned.on_range_finished(
            RunId::from_raw(1),
            &outcome(range(0, 9), RunStatus::Pass, "Read: 1; Write: 1"),
        );

        // Tracking first, always: the durable record is the one whose loss cannot be recovered.
        assert_eq!(
            log.lock().unwrap().as_slice(),
            [
                "tracking:started",
                "display:started",
                "tracking:finished",
                "display:finished",
            ]
        );
    }

    #[test]
    fn trk_021_a_run_with_one_watcher_or_none_pays_for_no_fan_out() {
        // The two paths that existed before composition must be exactly what they were: a silent
        // untracked run hands over the `NoopObserver`, and a run with a single watcher hands that
        // watcher over directly rather than wrapping it.
        assert!(Observers::new().is_empty());
        assert_eq!(Observers::new().len(), 0);
        assert!(Observers::new().and(None).is_empty());

        let log = Arc::new(Mutex::new(Vec::new()));
        let only = Recorder::new("only", &log);
        let single = Observers::new()
            .and(Some(Arc::clone(&only) as Arc<dyn RangeObserver>))
            .into_observer();
        // Handed over unwrapped: the same allocation went in and came out.
        assert!(Arc::ptr_eq(
            &(Arc::clone(&only) as Arc<dyn RangeObserver>),
            &single
        ));
        single.on_range_started(RunId::from_raw(1), range(0, 9));
        assert_eq!(log.lock().unwrap().as_slice(), ["only:started"]);
    }

    fn live(ranges: &[TokenRange]) -> (LiveRun, Arc<EventBus>) {
        let bus = Arc::new(EventBus::new(RunId::from_raw(7), "node-a"));
        let now = Instant::now();
        let run = LiveRun::new(
            JobKind::Migrate,
            RunId::from_raw(7),
            "node-a",
            Arc::clone(&bus),
            Arc::new(Instruments::new(now)),
            ranges,
            now,
        );
        (run, bus)
    }

    fn outcome(range: TokenRange, status: RunStatus, run_info: &str) -> RangeOutcome {
        RangeOutcome {
            range,
            status,
            run_info: run_info.to_owned(),
            diagnostic: None,
            abandoned: false,
        }
    }

    #[tokio::test]
    async fn met_031_a_finished_range_moves_the_bar_and_the_throughput_meters() {
        let ranges = TokenRange::MURMUR3_FULL.split(4).unwrap();
        let (run, _bus) = live(&ranges);

        run.on_range_started(RunId::from_raw(7), ranges[0]);
        assert_eq!(run.progress.snapshot().ranges_in_flight, 1);

        run.on_range_finished(
            RunId::from_raw(7),
            &outcome(ranges[0], RunStatus::Pass, "Read: 1000; Write: 900"),
        );

        let progress = run.progress.snapshot();
        assert_eq!(progress.ranges_completed, 1);
        assert_eq!(progress.ranges_in_flight, 0);
        assert!((progress.weight_fraction - 0.25).abs() < 1e-9);

        let instruments = run.instruments.snapshot();
        assert_eq!(instruments.origin.rows.total, 1_000);
        assert_eq!(instruments.target.rows.total, 900);
        // The range was bracketed, so its duration was measured.
        assert_eq!(run.timings.snapshot().count, 1);
    }

    #[tokio::test]
    async fn met_030_the_range_lifecycle_reaches_the_bus() {
        let ranges = TokenRange::MURMUR3_FULL.split(2).unwrap();
        let (run, bus) = live(&ranges);
        let mut events = bus.subscribe();

        run.on_range_started(RunId::from_raw(7), ranges[0]);
        run.on_range_finished(
            RunId::from_raw(7),
            &outcome(ranges[0], RunStatus::Pass, "Read: 5"),
        );

        assert_eq!(
            events.try_recv().unwrap().unwrap().payload.kind(),
            "range_started"
        );
        assert_eq!(
            events.try_recv().unwrap().unwrap().payload.kind(),
            "range_completed"
        );
    }

    #[tokio::test]
    async fn eng_008_a_failed_range_publishes_an_error_and_still_counts_as_progress() {
        let ranges = TokenRange::MURMUR3_FULL.split(2).unwrap();
        let (run, bus) = live(&ranges);
        let mut events = bus.subscribe();

        let mut failed = outcome(ranges[0], RunStatus::Fail, "Read: 5; Error: 5");
        failed.diagnostic = Some(Diagnostic::error("CDM-CQL", "the range read timed out"));
        run.on_range_finished(RunId::from_raw(7), &failed);

        // `ENG-008` keeps the run going; a bar that ignored the failure would stall at 99% on a
        // run that has finished.
        assert_eq!(run.progress.snapshot().ranges_completed, 1);

        let mut kinds = Vec::new();
        while let Ok(Some(event)) = events.try_recv() {
            kinds.push(event.payload.kind());
        }
        assert_eq!(kinds, vec!["range_completed", "error"]);
    }

    #[tokio::test]
    async fn eng_010_a_range_abandoned_by_shutdown_goes_back_to_pending() {
        let ranges = TokenRange::MURMUR3_FULL.split(4).unwrap();
        let (run, _bus) = live(&ranges);

        run.on_range_started(RunId::from_raw(7), ranges[0]);
        run.on_range_finished(
            RunId::from_raw(7),
            &outcome(ranges[0], RunStatus::Started, "Read: 0"),
        );

        let progress = run.progress.snapshot();
        assert_eq!(progress.ranges_completed, 0);
        assert_eq!(progress.ranges_in_flight, 0);
        assert_eq!(progress.ranges_pending, 4);
    }

    /// A processor that reads ten rows per range and nothing else.
    struct TenRows;

    #[async_trait::async_trait]
    impl cdm_engine::scheduler::RangeProcessor for TenRows {
        fn job(&self) -> JobKind {
            JobKind::Migrate
        }

        async fn process(
            &self,
            ctx: &cdm_engine::scheduler::RangeContext,
        ) -> Result<cdm_engine::scheduler::RangeVerdict, cdm_core::CdmError> {
            let counters = ctx.counters();
            counters.increment_by(counters.counter(CounterKind::Read)?, 10);
            counters.increment_by(counters.counter(CounterKind::Write)?, 10);
            Ok(cdm_engine::scheduler::RangeVerdict::Pass)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn met_031_a_real_run_through_the_real_scheduler_fills_the_whole_display() {
        // The end-to-end proof that `--tui` is wired to something rather than to nothing: a real
        // `Scheduler`, a real `TokenPlan`, this observer, and afterwards a dashboard that has the
        // bar, the counters and the terminal status in it. A unit test on the observer's methods
        // alone would still pass if the harness never handed it to the scheduler.
        use cdm_engine::planner::{Partitioner, Planner, PlannerSettings};
        use cdm_engine::scheduler::{Scheduler, SchedulerSettings};

        let settings = PlannerSettings {
            num_parts: 8,
            ..PlannerSettings::new(Partitioner::Murmur3)
        };
        let plan = Planner::new(settings)
            .plan(RunId::from_raw(7), None)
            .unwrap();
        let bus = Arc::new(EventBus::new(RunId::from_raw(7), "node-a"));
        let mut events = bus.subscribe();
        let now = Instant::now();
        let live = Arc::new(LiveRun::new(
            JobKind::Migrate,
            RunId::from_raw(7),
            "node-a",
            Arc::clone(&bus),
            Arc::new(Instruments::new(now)),
            &plan.token_ranges(),
            now,
        ));
        let mut dashboard = live.dashboard();

        let scheduler = Scheduler::new(SchedulerSettings::default().with_workers(2)).unwrap();
        let report = scheduler
            .run(
                &plan,
                Arc::new(TenRows),
                Arc::clone(&live) as Arc<dyn RangeObserver>,
            )
            .await
            .unwrap();
        assert_eq!(report.status(), RunStatus::Ended);

        while let Ok(Some(event)) = events.try_recv() {
            dashboard.apply(&event);
        }
        let view = dashboard.snapshot();

        assert_eq!(view.progress.ranges_total, 8);
        assert_eq!(view.progress.ranges_completed, 8);
        assert!((view.progress.weight_fraction - 1.0).abs() < 1e-9);
        assert_eq!(view.progress.eta, Some(std::time::Duration::ZERO));
        assert_eq!(view.instruments.origin.rows.total, 80);
        assert_eq!(view.instruments.target.rows.total, 80);
        assert_eq!(view.range_latency.count, 8);
        assert_eq!(view.status, Some(RunStatus::Ended));
        assert_eq!(view.errors_total, 0);
    }

    #[tokio::test]
    async fn met_030_the_run_ends_with_its_committed_counters_on_the_bus() {
        use cdm_engine::planner::{Partitioner, Planner, PlannerSettings};
        use cdm_engine::scheduler::{Scheduler, SchedulerSettings};

        let settings = PlannerSettings {
            num_parts: 2,
            ..PlannerSettings::new(Partitioner::Murmur3)
        };
        let plan = Planner::new(settings)
            .plan(RunId::from_raw(7), None)
            .unwrap();
        let bus = Arc::new(EventBus::new(RunId::from_raw(7), "node-a"));
        let mut events = bus.subscribe();
        let now = Instant::now();
        let live = Arc::new(LiveRun::new(
            JobKind::Migrate,
            RunId::from_raw(7),
            "node-a",
            Arc::clone(&bus),
            Arc::new(Instruments::new(now)),
            &plan.token_ranges(),
            now,
        ));

        Scheduler::new(SchedulerSettings::default())
            .unwrap()
            .run(&plan, Arc::new(TenRows), live as Arc<dyn RangeObserver>)
            .await
            .unwrap();

        let mut completed = None;
        while let Ok(Some(event)) = events.try_recv() {
            if let EventPayload::RunCompleted {
                status, counters, ..
            } = event.payload
            {
                completed = Some((status, counters));
            }
        }
        let (status, counters) = completed.expect("a run_completed event");
        assert_eq!(status, RunStatus::Ended);
        assert_eq!(counters.get("READ"), Some(&20));
        assert_eq!(counters.get("WRITE"), Some(&20));
    }
}
