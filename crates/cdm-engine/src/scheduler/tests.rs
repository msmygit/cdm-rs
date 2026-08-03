//! End-to-end scheduler tests, driven by a fault-injecting [`RangeProcessor`] double
//! (`TST-040`).
//!
//! # Why a double rather than a cluster
//!
//! `ENG-008`, `ENG-009` and `ENG-013` are all about what happens when a range goes wrong, and the
//! interesting cases — a panic, a hang, a failure at exactly the row that trips the error limit —
//! are the ones a real cluster will not produce on demand. [`FaultProcessor`] produces them
//! exactly on demand, per range, which is what `TST-040` asks of a fault-injecting double at the
//! session level and is the same idea one layer up.
//!
//! # Why none of these tests can flake
//!
//! No test here observes real elapsed time. Anything involving a duration —
//! [`Behaviour::Hang`], the shutdown grace period, the rate limiters — runs under
//! `#[tokio::test(start_paused = true)]`, where the clock is virtual and only advances when every
//! task is parked. Elapsed time is then a deterministic function of the program, so the
//! assertions are equalities rather than tolerances, and a loaded CI machine changes nothing.
//!
//! Ordering is handled the same way: where a test needs a worker to have reached a particular
//! point, it waits on a [`Barrier`] or a [`Notify`], never on a sleep.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use cdm_core::{ErrorKind, TableRef};
use cdm_metrics::CounterKind;
use tokio::sync::{Barrier, Notify, Semaphore};

use crate::planner::{Partitioner, Planner, PlannerSettings};

use super::*;

// =================================================================================================
// The fault-injecting double (TST-040)
// =================================================================================================

/// How a range ends, when the double is asked to process it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// Complete with a verdict.
    Finish(RangeVerdict),
    /// Return an error, failing the range (`ENG-008`).
    Fail,
    /// Panic, which the scheduler must contain (`ENG-013`).
    Panic,
    /// Never finish. Only a kill ends this range (`ENG-010`).
    Hang,
    /// Wind down cleanly when cancelled, otherwise finish (`ENG-010`).
    Cooperative,
}

/// The rows a range pretends to move before its behaviour takes effect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Rows {
    read: u64,
    written: u64,
    skipped: u64,
}

impl Rows {
    const fn new(read: u64, written: u64, skipped: u64) -> Self {
        Self {
            read,
            written,
            skipped,
        }
    }
}

/// A [`RangeProcessor`] that fails, panics, hangs or succeeds on demand (`TST-040`).
#[derive(Debug)]
struct FaultProcessor {
    job: JobKind,
    default: Behaviour,
    rows: Rows,
    overrides: Mutex<BTreeMap<TokenRange, Behaviour>>,
    processed: Mutex<Vec<TokenRange>>,
    in_flight: AtomicUsize,
    peak_in_flight: AtomicUsize,
    entered: Arc<Notify>,
    gate: Mutex<Option<Arc<Barrier>>>,
    /// One ticket per range, handed out by the test. A processor with a ticket source cannot
    /// start a range until the test says so, which is what makes "pause *now*, with work in
    /// flight" a deterministic instruction rather than a hope about scheduling.
    tickets: Option<Arc<Semaphore>>,
}

impl FaultProcessor {
    fn new(job: JobKind) -> Self {
        Self {
            job,
            default: Behaviour::Finish(RangeVerdict::Pass),
            rows: Rows::default(),
            overrides: Mutex::new(BTreeMap::new()),
            processed: Mutex::new(Vec::new()),
            in_flight: AtomicUsize::new(0),
            peak_in_flight: AtomicUsize::new(0),
            entered: Arc::new(Notify::new()),
            gate: Mutex::new(None),
            tickets: None,
        }
    }

    fn migrate() -> Self {
        Self::new(JobKind::Migrate)
    }

    /// Requires one ticket from `tickets` before a range may be processed.
    fn with_tickets(mut self, tickets: Arc<Semaphore>) -> Self {
        self.tickets = Some(tickets);
        self
    }

    fn with_default(mut self, behaviour: Behaviour) -> Self {
        self.default = behaviour;
        self
    }

    fn with_rows(mut self, rows: Rows) -> Self {
        self.rows = rows;
        self
    }

    /// Every range that entered [`RangeProcessor::process`], in completion order.
    fn processed(&self) -> Vec<TokenRange> {
        self.processed.lock().clone()
    }

    fn peak_in_flight(&self) -> usize {
        self.peak_in_flight.load(Ordering::SeqCst)
    }

    /// Resolves once some range has entered `process`.
    ///
    /// `Notify::notify_one` stores a permit when nobody is waiting yet, so this cannot miss an
    /// entry that happened before the caller got round to asking — which `notify_waiters` would,
    /// and which would hang the test rather than fail it.
    async fn wait_for_entry(&self) {
        self.entered.notified().await;
    }

    fn behaviour_for(&self, range: TokenRange) -> Behaviour {
        self.overrides
            .lock()
            .get(&range)
            .copied()
            .unwrap_or(self.default)
    }

    /// Records the rows this range "moved", so `ENG-008` has something to account for.
    fn count_rows(&self, ctx: &RangeContext) -> Result<(), CdmError> {
        let counters = ctx.counters();
        for _ in 0..self.rows.read {
            counters.increment(counters.counter(CounterKind::Read)?);
        }
        for _ in 0..self.rows.written {
            counters.increment(counters.counter(CounterKind::Write)?);
        }
        for _ in 0..self.rows.skipped {
            counters.increment(counters.counter(CounterKind::Skipped)?);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl RangeProcessor for FaultProcessor {
    fn job(&self) -> JobKind {
        self.job
    }

    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
        let live = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_in_flight.fetch_max(live, Ordering::SeqCst);
        self.entered.notify_one();

        if let Some(tickets) = &self.tickets {
            if let Ok(ticket) = tickets.acquire().await {
                ticket.forget();
            }
        }

        let gate = self.gate.lock().clone();
        if let Some(gate) = gate {
            gate.wait().await;
        }

        self.count_rows(ctx)?;
        let behaviour = self.behaviour_for(ctx.range());

        // Every path but `Hang` records the range; a hung range never completes, so counting it
        // as processed would be a lie the abandonment tests would then assert.
        let finish = |this: &Self| {
            this.processed.lock().push(ctx.range());
            this.in_flight.fetch_sub(1, Ordering::SeqCst);
        };

        match behaviour {
            Behaviour::Finish(verdict) => {
                finish(self);
                Ok(verdict)
            }
            Behaviour::Fail => {
                finish(self);
                Err(CdmError::new(ErrorKind::Write, "injected write timeout")
                    .with_context(|c| c.with_range(ctx.range())))
            }
            Behaviour::Panic => {
                finish(self);
                panic!("injected panic in range {}", ctx.range().min())
            }
            Behaviour::Hang => {
                std::future::pending::<()>().await;
                unreachable!("a hung range is only ever dropped")
            }
            Behaviour::Cooperative => {
                ctx.cancelled().await;
                finish(self);
                Ok(RangeVerdict::Pass)
            }
        }
    }
}

/// An observer that records the `ENG-002` lifecycle callbacks.
#[derive(Debug, Default)]
struct RecordingObserver {
    started: Mutex<Vec<TokenRange>>,
    finished: Mutex<Vec<(TokenRange, RunStatus)>>,
}

impl RecordingObserver {
    fn started(&self) -> Vec<TokenRange> {
        self.started.lock().clone()
    }

    fn finished(&self) -> Vec<(TokenRange, RunStatus)> {
        self.finished.lock().clone()
    }
}

impl RangeObserver for RecordingObserver {
    fn on_range_started(&self, _run_id: RunId, range: TokenRange) {
        self.started.lock().push(range);
    }

    fn on_range_finished(&self, _run_id: RunId, outcome: &RangeOutcome) {
        self.finished.lock().push((outcome.range, outcome.status));
    }
}

// =================================================================================================
// Fixtures
// =================================================================================================

/// A plan of `parts` ranges over the full Murmur3 ring.
fn plan(parts: u64) -> TokenPlan {
    Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(parts))
        .plan(RunId::from_raw(1_712_345_678_901_234), None)
        .unwrap()
}

/// Settings with the rate limiters disabled, so a scheduler test measures scheduling only.
fn settings(workers: u32) -> SchedulerSettings {
    SchedulerSettings::default()
        .with_workers(workers)
        .with_ratelimits(0, 0)
        .with_node_id("node-under-test")
}

/// Yields enough times that every runnable task has certainly run.
///
/// Used only for the *negative* half of an assertion — "and then nothing more happened" — where
/// yielding more can never change a correct answer and no amount of yielding can rescue a wrong
/// one. Nothing here waits on a clock.
async fn settle() {
    for _ in 0..512 {
        tokio::task::yield_now().await;
    }
}

// =================================================================================================
// ENG-001: the work-stealing scheduler
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eng_001_every_range_is_processed_exactly_once_across_many_workers() {
    let plan = plan(500);
    let processor = Arc::new(FaultProcessor::migrate());
    let scheduler = Scheduler::new(settings(8)).unwrap();

    let report = scheduler
        .run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    let processed = processor.processed();
    assert_eq!(processed.len(), plan.len());
    assert_eq!(
        processed.iter().copied().collect::<BTreeSet<_>>(),
        plan.token_ranges().into_iter().collect::<BTreeSet<_>>(),
        "every planned range must be processed, and none twice"
    );
    assert_eq!(report.outcomes().len(), plan.len());
    assert_eq!(report.ranges_passed(), plan.len());
    assert_eq!(report.status(), RunStatus::Ended);
    assert!(report.unclaimed_ranges().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eng_001_workers_run_concurrently_rather_than_one_at_a_time() {
    // Every range parks on the barrier until all eight workers have arrived, so the run can only
    // finish if the eight really are concurrent. If the scheduler serialised them the barrier
    // would never trip and the test would hang rather than pass — a deadlock, not a flake.
    let plan = plan(64);
    let processor = Arc::new(FaultProcessor::migrate());
    *processor.gate.lock() = Some(Arc::new(Barrier::new(8)));

    let scheduler = Scheduler::new(settings(8)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    assert_eq!(report.outcomes().len(), plan.len());
    assert_eq!(processor.peak_in_flight(), 8);
}

#[tokio::test]
async fn eng_001_counters_total_across_every_range() {
    let plan = plan(20);
    let processor = Arc::new(FaultProcessor::migrate().with_rows(Rows::new(10, 8, 2)));
    let scheduler = Scheduler::new(settings(4)).unwrap();

    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();

    let ranges = u64::try_from(plan.len()).unwrap();
    let total = |kind| report.counters().count_of(kind, CounterView::Committed);
    assert_eq!(total(CounterKind::Read), 10 * ranges);
    assert_eq!(total(CounterKind::Write), 8 * ranges);
    assert_eq!(total(CounterKind::Skipped), 2 * ranges);
    assert_eq!(total(CounterKind::Error), 0);
    assert_eq!(total(CounterKind::PartitionsPassed), ranges);
    assert_eq!(total(CounterKind::PartitionsFailed), 0);
}

#[tokio::test]
async fn eng_001_an_empty_plan_finishes_immediately() {
    let plan = Planner::new(
        PlannerSettings::new(Partitioner::Murmur3)
            .with_num_parts(1)
            .with_coverage_percent(0),
    )
    .plan(RunId::from_raw(2), None)
    .unwrap();

    let scheduler = Scheduler::new(settings(4)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::new(FaultProcessor::migrate()),
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(report.ranges_passed(), plan.len());
}

#[tokio::test]
async fn eng_001_the_scheduler_exposes_what_it_resolved() {
    let scheduler = Scheduler::new(settings(3).with_fetch_size(77)).unwrap();
    assert_eq!(scheduler.settings().workers(), 3);
    assert_eq!(scheduler.settings().fetch_size(), 77);
    assert!(scheduler.limits().origin_rate().is_unlimited());
    assert!(!scheduler.control().is_stopping());
}

#[test]
fn eng_007_an_unusable_in_flight_bound_is_a_startup_error() {
    let error = Scheduler::new(settings(1).with_max_inflight_writes(0)).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config);
}

// =================================================================================================
// ENG-002: the range is the unit of tracking
// =================================================================================================

#[tokio::test]
async fn eng_002_a_range_is_reported_started_before_work_and_terminal_after_it() {
    let plan = plan(12);
    let observer = Arc::new(RecordingObserver::default());
    let scheduler = Scheduler::new(settings(2)).unwrap();

    scheduler
        .run(
            &plan,
            Arc::new(FaultProcessor::migrate().with_rows(Rows::new(4, 4, 0))),
            Arc::clone(&observer) as Arc<dyn RangeObserver>,
        )
        .await
        .unwrap();

    assert_eq!(observer.started().len(), plan.len());
    assert_eq!(
        observer.started().into_iter().collect::<BTreeSet<_>>(),
        plan.token_ranges().into_iter().collect::<BTreeSet<_>>()
    );
    let finished = observer.finished();
    assert_eq!(finished.len(), plan.len());
    assert!(finished
        .iter()
        .all(|(_, status)| *status == RunStatus::Pass));
}

#[tokio::test]
async fn eng_002_a_validate_range_records_diff_and_diff_corrected() {
    let plan = plan(4);
    let ranges = plan.token_ranges();
    let processor = Arc::new(FaultProcessor::new(JobKind::Validate));
    {
        let mut overrides = processor.overrides.lock();
        overrides.insert(ranges[0], Behaviour::Finish(RangeVerdict::Diff));
        overrides.insert(ranges[1], Behaviour::Finish(RangeVerdict::DiffCorrected));
    }

    let scheduler = Scheduler::new(settings(2)).unwrap();
    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();

    let status_of = |range: TokenRange| {
        report
            .outcomes()
            .iter()
            .find(|o| o.range == range)
            .unwrap()
            .status
    };
    assert_eq!(status_of(ranges[0]), RunStatus::Diff);
    assert_eq!(status_of(ranges[1]), RunStatus::DiffCorrected);
    assert_eq!(status_of(ranges[2]), RunStatus::Pass);
    // Java counts a DIFF range as a *passed* partition — the partition was processed; the rows
    // in it disagreed. `DiffJobSession` increments PARTITIONS_PASSED before it looks at hasDiff.
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::PartitionsPassed, CounterView::Committed),
        u64::try_from(plan.len()).unwrap()
    );
}

#[tokio::test]
async fn eng_002_each_range_reports_its_own_run_info_string() {
    let plan = plan(2);
    let processor = Arc::new(FaultProcessor::migrate().with_rows(Rows::new(10, 9, 1)));
    let scheduler = Scheduler::new(settings(1)).unwrap();

    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();

    // MET-005: the committed rendering, with `Unflushed` omitted, exactly as Java writes it to
    // `cdm_run_details.run_info`.
    assert_eq!(
        report.outcomes()[0].run_info,
        "Read: 10; Write: 9; Skipped: 1; Error: 0; Partitions Passed: 1; Partitions Failed: 0"
    );
}

#[tokio::test]
async fn eng_002_outcomes_are_reported_in_ring_order_regardless_of_completion_order() {
    let plan = plan(64);
    let scheduler = Scheduler::new(settings(8)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::new(FaultProcessor::migrate()),
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    let reported: Vec<_> = report.outcomes().iter().map(|o| o.range).collect();
    let mut sorted = reported.clone();
    sorted.sort_unstable();
    assert_eq!(reported, sorted);
    assert_eq!(reported, plan.ring_ordered());
}

// =================================================================================================
// ENG-003: the page size reaches the job
// =================================================================================================

#[tokio::test]
async fn eng_003_the_configured_fetch_size_reaches_every_range() {
    /// Asserts the page size in the only place it can be observed from: inside a range.
    #[derive(Debug)]
    struct FetchSizeSpy(AtomicUsize);

    #[async_trait::async_trait]
    impl RangeProcessor for FetchSizeSpy {
        fn job(&self) -> JobKind {
            JobKind::Migrate
        }

        async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
            assert_eq!(ctx.fetch_size(), 137);
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(RangeVerdict::Pass)
        }
    }

    let plan = plan(8);
    let spy = Arc::new(FetchSizeSpy(AtomicUsize::new(0)));
    let scheduler = Scheduler::new(settings(2).with_fetch_size(137)).unwrap();
    scheduler
        .run(
            &plan,
            Arc::clone(&spy) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    assert_eq!(spy.0.load(Ordering::SeqCst), plan.len());
}

// =================================================================================================
// ENG-004/005: rate limiting through the scheduler
// =================================================================================================

#[tokio::test(start_paused = true)]
async fn eng_004_a_run_is_paced_by_the_origin_rate_limit() {
    /// Reads ten rows through the origin limiter.
    #[derive(Debug)]
    struct TenReads;

    #[async_trait::async_trait]
    impl RangeProcessor for TenReads {
        fn job(&self) -> JobKind {
            JobKind::Migrate
        }

        async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
            let read = ctx.counters().counter(CounterKind::Read)?;
            for _ in 0..10 {
                ctx.acquire_read_rows(1).await;
                ctx.counters().increment(read);
            }
            Ok(RangeVerdict::Pass)
        }
    }

    // Ten ranges of ten rows is a hundred rows; at ten rows a second with a one-second burst
    // that is exactly nine seconds of virtual time, whatever order the four workers interleave
    // in — the limiter's reservation is global to the run.
    let plan = plan(10);
    let scheduler = Scheduler::new(settings(4).with_ratelimits(10, 0)).unwrap();
    let started = tokio::time::Instant::now();
    let report = scheduler
        .run(&plan, Arc::new(TenReads), Arc::new(NoopObserver))
        .await
        .unwrap();

    assert_eq!(started.elapsed(), Duration::from_secs(9));
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::Read, CounterView::Committed),
        100
    );
}

// =================================================================================================
// ENG-007: bounded memory
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eng_007_concurrent_ranges_cannot_exceed_the_in_flight_read_bound() {
    /// Holds an in-flight read slot across an await point, and records how many are ever held.
    #[derive(Debug, Default)]
    struct SlotHog {
        live: AtomicUsize,
        peak: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl RangeProcessor for SlotHog {
        fn job(&self) -> JobKind {
            JobKind::Migrate
        }

        async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
            let slot = ctx.read_slot().await?;
            let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(live, Ordering::SeqCst);
            tokio::task::yield_now().await;
            self.live.fetch_sub(1, Ordering::SeqCst);
            drop(slot);
            Ok(RangeVerdict::Pass)
        }
    }

    let plan = plan(200);
    let hog = Arc::new(SlotHog::default());
    let scheduler = Scheduler::new(settings(32).with_max_inflight_reads(3)).unwrap();
    scheduler
        .run(
            &plan,
            Arc::clone(&hog) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    assert!(
        hog.peak.load(Ordering::SeqCst) <= 3,
        "at most three reads may be in flight, saw {}",
        hog.peak.load(Ordering::SeqCst)
    );
    assert_eq!(scheduler.limits().available_read_slots(), 3);
}

// =================================================================================================
// ENG-008: per-range failure isolation
// =================================================================================================

#[tokio::test]
async fn eng_008_a_failing_range_does_not_abort_the_run() {
    let plan = plan(10);
    let ranges = plan.token_ranges();
    let processor = Arc::new(FaultProcessor::migrate().with_rows(Rows::new(10, 6, 1)));
    processor
        .overrides
        .lock()
        .insert(ranges[3], Behaviour::Fail);

    let scheduler = Scheduler::new(settings(3)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    // The run finished its plan; only one range failed.
    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(processor.processed().len(), plan.len());
    assert_eq!(report.ranges_failed(), 1);
    assert_eq!(report.ranges_passed(), plan.len() - 1);

    let failed = report
        .outcomes()
        .iter()
        .find(|o| o.range == ranges[3])
        .unwrap();
    assert_eq!(failed.status, RunStatus::Fail);
    assert!(!failed.abandoned);
    // ENG-008: the error is reported with the range bounds attached.
    let diagnostic = failed.diagnostic.as_ref().unwrap();
    assert!(
        diagnostic.title.contains("injected write timeout"),
        "{diagnostic:?}"
    );
}

#[tokio::test]
async fn eng_008_a_failed_range_increments_partitions_failed_and_error() {
    let plan = plan(4);
    let processor = Arc::new(
        FaultProcessor::migrate()
            .with_default(Behaviour::Fail)
            .with_rows(Rows::new(10, 6, 1)),
    );

    let scheduler = Scheduler::new(settings(2)).unwrap();
    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();

    let ranges = u64::try_from(plan.len()).unwrap();
    let total = |kind| report.counters().count_of(kind, CounterView::Committed);
    assert_eq!(total(CounterKind::PartitionsFailed), ranges);
    assert_eq!(total(CounterKind::PartitionsPassed), 0);
    // read − written − skipped = 10 − 6 − 1 = 3 rows lost per range.
    assert_eq!(total(CounterKind::Error), 3 * ranges);
    assert_eq!(total(CounterKind::Read), 10 * ranges);
}

#[tokio::test]
async fn eng_008_a_failed_validate_range_counts_the_rows_java_reports_as_zero() {
    /// Reads twenty rows, validates five of them, then fails.
    #[derive(Debug)]
    struct HalfValidatedThenFails;

    #[async_trait::async_trait]
    impl RangeProcessor for HalfValidatedThenFails {
        fn job(&self) -> JobKind {
            JobKind::Validate
        }

        async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
            let counters = ctx.counters();
            let read = counters.counter(CounterKind::Read)?;
            let valid = counters.counter(CounterKind::Valid)?;
            counters.increment_by(read, 20);
            counters.increment_by(valid, 5);
            Err(CdmError::new(ErrorKind::Read, "injected read timeout"))
        }
    }

    let plan = plan(3);
    let scheduler = Scheduler::new(settings(1)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::new(HalfValidatedThenFails),
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    // Java's `DiffJobSession` reads these terms at the committed level before `flush()`, so it
    // increments ERROR by 0 for every failed validate range. Fifteen rows per range really were
    // lost, and cdm-rs says so (ENG-008, [P+]).
    let ranges = u64::try_from(plan.len()).unwrap();
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::Error, CounterView::Committed),
        15 * ranges
    );
}

#[tokio::test]
async fn eng_008_a_failure_in_one_range_leaves_its_neighbours_counters_untouched() {
    let plan = plan(2);
    let ranges = plan.token_ranges();
    let processor = Arc::new(FaultProcessor::migrate().with_rows(Rows::new(10, 10, 0)));
    processor
        .overrides
        .lock()
        .insert(ranges[0], Behaviour::Fail);

    let scheduler = Scheduler::new(settings(1)).unwrap();
    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();

    let outcome_of = |range: TokenRange| {
        report
            .outcomes()
            .iter()
            .find(|o| o.range == range)
            .unwrap()
            .clone()
    };
    assert!(outcome_of(ranges[0])
        .run_info
        .contains("Partitions Failed: 1"));
    assert!(outcome_of(ranges[1])
        .run_info
        .contains("Partitions Passed: 1"));
    assert!(outcome_of(ranges[1]).run_info.contains("Error: 0"));
}

// =================================================================================================
// ENG-009: the error limit
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eng_009_the_error_limit_aborts_the_run_and_drains_in_flight_work() {
    let plan = plan(200);
    let processor = Arc::new(
        FaultProcessor::migrate()
            .with_default(Behaviour::Fail)
            .with_rows(Rows::new(10, 0, 0)),
    );

    // Ten lost rows per range, a limit of 25: the run must stop once the third range has failed.
    let scheduler = Scheduler::new(settings(4).with_error_limit(25)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    assert_eq!(report.status(), RunStatus::Aborted);
    assert_eq!(report.stopped_by(), Some(StopReason::ErrorLimit));
    assert!(
        report.outcomes().len() < plan.len(),
        "the run must stop before its plan is exhausted"
    );
    assert!(!report.unclaimed_ranges().is_empty());

    // Draining cleanly means every range that was claimed also finished and was accounted for:
    // nothing is abandoned, and no outcome is missing.
    assert_eq!(report.ranges_abandoned(), 0);
    assert_eq!(report.outcomes().len(), processor.processed().len());
    assert_eq!(report.ranges_failed(), report.outcomes().len());
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::Error, CounterView::Committed),
        10 * u64::try_from(report.outcomes().len()).unwrap()
    );
    assert_eq!(report.exit_code(), 1);
}

#[tokio::test]
async fn eng_009_the_limit_is_exceeded_not_merely_reached() {
    let plan = plan(4);
    let processor = Arc::new(
        FaultProcessor::migrate()
            .with_default(Behaviour::Fail)
            .with_rows(Rows::new(10, 0, 0)),
    );

    // One worker, ten errors per range, a limit of 40: the fourth range takes the total to 40,
    // which is not *more* than 40, so the plan finishes.
    let scheduler = Scheduler::new(settings(1).with_error_limit(40)).unwrap();
    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();

    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(report.outcomes().len(), plan.len());
    assert_eq!(report.exit_code(), 0);
}

#[tokio::test]
async fn eng_009_a_zero_error_limit_never_aborts() {
    let plan = plan(30);
    let processor = Arc::new(
        FaultProcessor::migrate()
            .with_default(Behaviour::Fail)
            .with_rows(Rows::new(1_000, 0, 0)),
    );

    let scheduler = Scheduler::new(settings(4).with_error_limit(0)).unwrap();
    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();

    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(report.outcomes().len(), plan.len());
    assert_eq!(report.ranges_failed(), plan.len());
}

#[tokio::test]
async fn eng_009_a_guardrail_run_has_no_error_counter_so_no_limit_to_exceed() {
    let plan = plan(6);
    let processor = Arc::new(
        FaultProcessor::new(JobKind::Guardrail)
            .with_default(Behaviour::Fail)
            .with_rows(Rows::new(100, 0, 0)),
    );

    let scheduler = Scheduler::new(settings(2).with_error_limit(1)).unwrap();
    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();

    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(report.ranges_failed(), plan.len());
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::PartitionsFailed, CounterView::Committed),
        u64::try_from(plan.len()).unwrap()
    );
}

// =================================================================================================
// ENG-010: graceful shutdown
// =================================================================================================

#[tokio::test]
async fn eng_010_a_signal_marks_the_run_interrupted_and_lets_in_flight_ranges_finish() {
    let plan = plan(500);
    let tickets = Arc::new(Semaphore::new(0));
    let processor = Arc::new(
        FaultProcessor::migrate()
            .with_rows(Rows::new(5, 5, 0))
            .with_tickets(Arc::clone(&tickets)),
    );
    let scheduler = Scheduler::new(settings(4)).unwrap();
    let control = scheduler.control();

    // The ticket gate removes the race this test would otherwise have with the run finishing
    // first: the workers cannot get past their first range until the signal has been delivered.
    let operator = async {
        processor.wait_for_entry().await;
        control.signalled();
        tickets.add_permits(Semaphore::MAX_PERMITS);
    };

    let (report, ()) = tokio::join!(
        scheduler.run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver)
        ),
        operator
    );
    let report = report.unwrap();

    assert_eq!(report.status(), RunStatus::Interrupted);
    assert_eq!(report.stopped_by(), Some(StopReason::Signal));
    assert_eq!(report.exit_code(), 1, "an interrupted run exits non-zero");

    // The ranges that were in flight when the signal landed all finished — nothing was abandoned
    // and nothing failed — and no more than the four workers had claimed one.
    assert_eq!(report.ranges_abandoned(), 0);
    assert_eq!(report.ranges_failed(), 0);
    assert_eq!(report.ranges_passed(), report.outcomes().len());
    assert!(
        (1..=4).contains(&report.outcomes().len()),
        "{:?}",
        report.outcomes().len()
    );

    // Nothing is lost between what was claimed and what a resume will re-plan.
    assert_eq!(
        report.outcomes().len() + report.unclaimed_ranges().len(),
        plan.len(),
        "no range may fall between the claimed and the unclaimed"
    );
}

#[tokio::test(start_paused = true)]
async fn eng_010_in_flight_ranges_finish_within_the_grace_period() {
    let plan = plan(8);
    let processor = Arc::new(FaultProcessor::migrate().with_default(Behaviour::Cooperative));
    let scheduler =
        Scheduler::new(settings(2).with_shutdown_grace(Duration::from_secs(30))).unwrap();
    let control = scheduler.control();

    let stopper = {
        let processor = Arc::clone(&processor);
        tokio::spawn(async move {
            processor.wait_for_entry().await;
            control.stop(StopReason::Signal);
        })
    };

    let started = tokio::time::Instant::now();
    let report = scheduler
        .run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();
    stopper.await.unwrap();

    // The cooperative ranges wind down when the grace period expires and the kill arrives, so
    // they complete rather than being abandoned — and the run takes exactly the grace period.
    assert_eq!(started.elapsed(), Duration::from_secs(30));
    assert_eq!(report.status(), RunStatus::Interrupted);
    assert_eq!(report.ranges_abandoned(), 0);
    assert_eq!(report.ranges_passed(), report.outcomes().len());
}

#[tokio::test(start_paused = true)]
async fn eng_010_the_shutdown_grace_bounds_a_range_that_will_not_stop() {
    let plan = plan(8);
    let processor = Arc::new(FaultProcessor::migrate().with_default(Behaviour::Hang));
    let scheduler =
        Scheduler::new(settings(2).with_shutdown_grace(Duration::from_secs(60))).unwrap();
    let control = scheduler.control();

    let stopper = {
        let processor = Arc::clone(&processor);
        tokio::spawn(async move {
            processor.wait_for_entry().await;
            control.stop(StopReason::Signal);
        })
    };

    let started = tokio::time::Instant::now();
    let report = scheduler
        .run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();
    stopper.await.unwrap();

    // A hung job cannot extend shutdown past the deadline.
    assert_eq!(started.elapsed(), Duration::from_secs(60));
    assert_eq!(report.status(), RunStatus::Interrupted);
    assert_eq!(report.ranges_abandoned(), 2, "both workers were mid-range");
    // An abandoned range is left STARTED, which TRK-031 counts as pending, so a resume re-plans
    // it. It is not a failure: nothing went wrong with the data.
    assert!(report
        .outcomes()
        .iter()
        .filter(|o| o.abandoned)
        .all(|o| o.status == RunStatus::Started && o.diagnostic.is_none()));
    assert_eq!(report.ranges_failed(), 0);
}

#[tokio::test(start_paused = true)]
async fn eng_010_a_second_signal_abandons_in_flight_work_immediately() {
    let plan = plan(8);
    let processor = Arc::new(FaultProcessor::migrate().with_default(Behaviour::Hang));
    // An hour of grace, which the second signal must make irrelevant.
    let scheduler =
        Scheduler::new(settings(2).with_shutdown_grace(Duration::from_secs(3_600))).unwrap();
    let control = scheduler.control();

    let stopper = {
        let processor = Arc::clone(&processor);
        tokio::spawn(async move {
            processor.wait_for_entry().await;
            control.signalled();
            control.signalled();
        })
    };

    let started = tokio::time::Instant::now();
    let report = scheduler
        .run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();
    stopper.await.unwrap();

    assert_eq!(
        started.elapsed(),
        Duration::ZERO,
        "the second signal does not wait"
    );
    assert_eq!(report.status(), RunStatus::Interrupted);
    assert_eq!(report.ranges_abandoned(), 2);
}

#[tokio::test]
async fn eng_010_metrics_are_flushed_and_reportable_after_an_interrupt() {
    let plan = plan(200);
    let tickets = Arc::new(Semaphore::new(0));
    let processor = Arc::new(
        FaultProcessor::migrate()
            .with_rows(Rows::new(7, 7, 0))
            .with_tickets(Arc::clone(&tickets)),
    );
    let scheduler = Scheduler::new(settings(2)).unwrap();
    let control = scheduler.control();

    let operator = async {
        processor.wait_for_entry().await;
        control.signalled();
        tickets.add_permits(Semaphore::MAX_PERMITS);
    };
    let (report, ()) = tokio::join!(
        scheduler.run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver)
        ),
        operator
    );
    let report = report.unwrap();

    let done = u64::try_from(report.outcomes().len()).unwrap();
    assert!(done > 0);
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::Read, CounterView::Committed),
        7 * done,
        "every completed range's counters must be committed into the run's totals"
    );
    // MET-006: the final block renders from the committed totals.
    report.log_final_block(Some(report.run_id()));
    assert!(report
        .counters()
        .final_block(Some(report.run_id()))
        .contains(&format!("Final Read Record Count: {}", 7 * done)));
}

// =================================================================================================
// ENG-011/012: the range span
// =================================================================================================

#[tokio::test]
async fn eng_011_a_job_runs_inside_the_range_span() {
    use crate::scheduler::span::tests::CapturingSubscriber;

    /// Records whether a span was entered around each of its `process` calls.
    #[derive(Debug)]
    struct SpanSpy {
        subscriber: CapturingSubscriber,
        depths: Mutex<Vec<isize>>,
    }

    #[async_trait::async_trait]
    impl RangeProcessor for SpanSpy {
        fn job(&self) -> JobKind {
            JobKind::Migrate
        }

        async fn process(&self, _ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
            self.depths.lock().push(self.subscriber.entered_depth());
            Ok(RangeVerdict::Pass)
        }
    }

    let captured = CapturingSubscriber::default();
    let plan = plan(4);
    let spy = Arc::new(SpanSpy {
        subscriber: captured.clone(),
        depths: Mutex::new(Vec::new()),
    });
    let scheduler = Scheduler::new(settings(1)).unwrap();

    let guard = tracing::subscriber::set_default(captured.clone());
    scheduler
        .run(
            &plan,
            Arc::clone(&spy) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();
    drop(guard);

    // ENG-011: the job's own code — and so everything it logs — runs with the range span
    // entered, and that span is `cdm.range` carrying the run, range and node identity.
    let depths = spy.depths.lock().clone();
    assert_eq!(depths.len(), plan.len());
    assert!(
        depths.iter().all(|depth| *depth > 0),
        "every range must be processed inside an entered span: {depths:?}"
    );
    assert_eq!(captured.entered_depth(), 0, "every span must be exited too");
    assert_eq!(captured.span_name(), Some("cdm.range"));
    assert_eq!(
        captured.field("node_id").as_deref(),
        Some("node-under-test")
    );
    assert_eq!(
        captured.field("run_id").as_deref(),
        Some(plan.run_id().as_i64().to_string().as_str())
    );
    assert!(
        captured.field("thread_label").is_some(),
        "the pretty format keeps Java's label"
    );
}

// =================================================================================================
// ENG-013: panics
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eng_013_a_panicking_range_fails_without_poisoning_the_run() {
    let plan = plan(50);
    let ranges = plan.token_ranges();
    let processor = Arc::new(FaultProcessor::migrate().with_rows(Rows::new(10, 4, 0)));
    {
        let mut overrides = processor.overrides.lock();
        for range in ranges.iter().take(5) {
            overrides.insert(*range, Behaviour::Panic);
        }
    }

    // The panics are expected; the default hook would print five backtraces into the test output.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let scheduler = Scheduler::new(settings(4)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();
    std::panic::set_hook(previous);

    // The run completed its plan: the pool is not poisoned, and no worker died.
    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(report.outcomes().len(), plan.len());
    assert_eq!(report.ranges_failed(), 5);
    assert_eq!(report.ranges_passed(), plan.len() - 5);

    // A panic is accounted for exactly like any other range failure (ENG-008).
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::PartitionsFailed, CounterView::Committed),
        5
    );
    assert_eq!(
        report
            .counters()
            .count_of(CounterKind::Error, CounterView::Committed),
        5 * (10 - 4)
    );

    let panicked = report
        .outcomes()
        .iter()
        .find(|o| o.range == ranges[0])
        .unwrap();
    assert_eq!(panicked.status, RunStatus::Fail);
    let diagnostic = panicked.diagnostic.as_ref().unwrap();
    assert!(diagnostic.title.contains("panicked"), "{diagnostic:?}");
    assert!(
        diagnostic.title.contains("injected panic"),
        "{diagnostic:?}"
    );
}

#[tokio::test]
async fn eng_013_a_run_of_nothing_but_panics_still_terminates() {
    let plan = plan(16);
    let processor = Arc::new(FaultProcessor::migrate().with_default(Behaviour::Panic));

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let scheduler = Scheduler::new(settings(4)).unwrap();
    let report = scheduler
        .run(&plan, processor, Arc::new(NoopObserver))
        .await
        .unwrap();
    std::panic::set_hook(previous);

    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(report.ranges_failed(), plan.len());
}

#[test]
fn eng_013_a_panic_payload_is_described_for_the_range_failure() {
    // The three payload shapes `catch_unwind` can hand back: a literal `panic!`, a formatted
    // one, and `panic_any`. They arrive boxed, which is the shape that matters — a `Box<dyn Any>`
    // is itself `Any`, so a downcast through the wrong reference silently misses every time.
    let boxed = |payload: Box<dyn Any + Send>| describe(payload.as_ref());
    assert_eq!(boxed(Box::new("a &str payload")), "a &str payload");
    assert_eq!(
        boxed(Box::new("a String payload".to_owned())),
        "a String payload"
    );
    assert_eq!(boxed(Box::new(42_u8)), "a panic with a non-string payload");
}

// =================================================================================================
// ENG-014: pause and resume
// =================================================================================================

#[tokio::test]
async fn eng_014_pause_stops_issuing_new_work_and_resume_continues_the_plan() {
    let plan = plan(64);
    let tickets = Arc::new(Semaphore::new(0));
    let processor = Arc::new(FaultProcessor::migrate().with_tickets(Arc::clone(&tickets)));
    let scheduler = Scheduler::new(settings(1)).unwrap();
    let control = scheduler.control();

    let operator = async {
        // Ten ranges' worth of tickets, so the run is demonstrably under way and demonstrably
        // unfinished when the pause lands. No sleeps: the ticket count, not the clock, decides
        // how far the run gets.
        tickets.add_permits(10);
        while processor.processed().len() < 10 {
            tokio::task::yield_now().await;
        }

        control.pause();
        assert!(control.is_paused());
        // Release every remaining ticket. A run that is merely *slow* would now finish its plan;
        // a paused one finishes the range already in flight and then claims nothing more.
        tickets.add_permits(Semaphore::MAX_PERMITS);
        settle().await;

        let paused_at = processor.processed().len();
        assert!(
            paused_at < 64,
            "the pause must bite before the plan ends, stopped at {paused_at}"
        );
        settle().await;
        assert_eq!(
            processor.processed().len(),
            paused_at,
            "a paused run must not claim new ranges however long it is left"
        );

        control.resume();
        paused_at
    };

    let (report, paused_at) = tokio::join!(
        scheduler.run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver)
        ),
        operator
    );
    let report = report.unwrap();

    // Nothing was lost: the plan resumed from the queue's cursor and ran to the end.
    assert!(paused_at < plan.len());
    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(report.outcomes().len(), plan.len());
    assert_eq!(
        processor.processed().into_iter().collect::<BTreeSet<_>>(),
        plan.token_ranges().into_iter().collect::<BTreeSet<_>>(),
        "every range must be processed exactly once across the pause"
    );
    assert!(!control.is_paused());
}

#[tokio::test]
async fn eng_014_stopping_a_paused_run_releases_its_workers() {
    let plan = plan(100);
    let tickets = Arc::new(Semaphore::new(0));
    let processor = Arc::new(FaultProcessor::migrate().with_tickets(Arc::clone(&tickets)));
    let scheduler = Scheduler::new(settings(2)).unwrap();
    let control = scheduler.control();

    let operator = async {
        processor.wait_for_entry().await;
        control.pause();
        tickets.add_permits(Semaphore::MAX_PERMITS);
        settle().await;
        // A paused run that is then stopped must not sit waiting for a resume that never comes.
        control.stop(StopReason::Operator);
    };

    let (report, ()) = tokio::join!(
        scheduler.run(
            &plan,
            Arc::clone(&processor) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver)
        ),
        operator
    );
    let report = report.unwrap();

    assert_eq!(report.status(), RunStatus::Aborted);
    assert_eq!(report.stopped_by(), Some(StopReason::Operator));
    assert!(!report.unclaimed_ranges().is_empty());
    assert!(!control.is_paused(), "a stop clears the pause");
}

// =================================================================================================
// The run report
// =================================================================================================

#[tokio::test]
async fn eng_002_the_report_describes_the_run_it_came_from() {
    let plan = plan(6);
    let scheduler = Scheduler::new(settings(2)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::new(FaultProcessor::migrate()),
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();

    assert_eq!(report.run_id(), plan.run_id());
    assert_eq!(report.job(), JobKind::Migrate);
    assert!(report.is_complete());
    assert_eq!(report.exit_code(), 0);
    assert_eq!(report.ranges_abandoned(), 0);
    // The report is `Debug`, because it is what a failing higher-level test prints.
    assert!(format!("{report:?}").contains("RunReport"));
}

#[tokio::test]
async fn eng_001_a_planner_settings_table_reference_does_not_change_scheduling() {
    // A plan built for a named table schedules identically: the geometry does not depend on the
    // data, and neither does the scheduler.
    let settings_with_table = PlannerSettings::new(Partitioner::Murmur3)
        .with_num_parts(8)
        .with_table(TableRef::new("ks", "tbl"));
    let plan = Planner::new(settings_with_table)
        .plan(RunId::from_raw(9), None)
        .unwrap();

    let scheduler = Scheduler::new(settings(2)).unwrap();
    let report = scheduler
        .run(
            &plan,
            Arc::new(FaultProcessor::migrate()),
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();
    assert_eq!(report.outcomes().len(), plan.len());
}
