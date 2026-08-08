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
use cdm_engine::scheduler::{RangeObserver, RangeOutcome, RunReport};
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
    /// Throughput (`MET-010`).
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
    /// Prepares the observers for a run over `ranges`.
    #[must_use]
    pub fn new(
        job: JobKind,
        run_id: RunId,
        node_id: impl Into<String>,
        events: Arc<EventBus>,
        ranges: &[TokenRange],
        now: Instant,
    ) -> Self {
        Self {
            job,
            run_id,
            node_id: node_id.into(),
            events,
            progress: Arc::new(ProgressTracker::by_token_span(ranges, now)),
            instruments: Arc::new(Instruments::new(now)),
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

    fn live(ranges: &[TokenRange]) -> (LiveRun, Arc<EventBus>) {
        let bus = Arc::new(EventBus::new(RunId::from_raw(7), "node-a"));
        let run = LiveRun::new(
            JobKind::Migrate,
            RunId::from_raw(7),
            "node-a",
            Arc::clone(&bus),
            ranges,
            Instant::now(),
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
        let live = Arc::new(LiveRun::new(
            JobKind::Migrate,
            RunId::from_raw(7),
            "node-a",
            Arc::clone(&bus),
            &plan.token_ranges(),
            Instant::now(),
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
        let live = Arc::new(LiveRun::new(
            JobKind::Migrate,
            RunId::from_raw(7),
            "node-a",
            Arc::clone(&bus),
            &plan.token_ranges(),
            Instant::now(),
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
