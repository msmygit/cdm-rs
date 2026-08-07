//! Fault injection against the engine's failure accounting (`TST-040`, `ENG-008`, `ENG-009`).
//!
//! # What a fault suite is for
//!
//! `ENG-008` says an error must not abort the run: the range is marked `FAIL`,
//! `PARTITIONS_FAILED` goes up by one, `ERROR` goes up by the rows the range read and could not
//! account for, and the worker takes the next range. Every clause of that is a statement about
//! what happens when a cluster misbehaves, and a cluster will not misbehave to order — least of
//! all *halfway through a range*, which is the only place the `ERROR` term is interesting.
//!
//! [`FaultySession`](cdm_testkit::FaultySession) misbehaves exactly on order. The job double here
//! is a migrate loop in miniature — read a row, write a row, count both — that issues its reads
//! and writes through a faulty session, so a fault lands between two rows and the accounting has
//! something real to be wrong about.
//!
//! | Claim | Test |
//! |---|---|
//! | A mid-range fault fails one range and the run continues (`ENG-008`) | [`tst_040_a_write_fault_fails_one_range_and_the_run_carries_on`] |
//! | `ERROR` is the rows the range lost, not zero (`ENG-008`) | [`tst_040_the_error_count_is_the_rows_the_range_could_not_account_for`] |
//! | Each of the six faults is contained the same way | [`tst_040_every_transport_fault_is_contained_at_the_range_boundary`] |
//! | A schema change is not contained — every range fails (`SCH-009`) | [`tst_040_a_schema_change_fails_every_range_rather_than_one`] |
//! | Enough injected faults abort the run (`ENG-009`) | [`tst_040_injected_faults_that_lose_enough_rows_abort_the_run`] |
//! | A counter range is never retried past a fault (`CON-012`) | [`tst_040_a_counter_fault_fails_the_range_without_a_second_attempt`] |
//! | A record-level fault costs one row, not the range (`ERR-005`) | [`tst_040_a_record_level_fault_costs_one_row_and_not_the_range`] |
//!
//! # Determinism
//!
//! Faults fire on numbered statements, never on a clock or a random draw. The workers are
//! deliberately set to **one**: the accounting under test is per range, and one worker fixes the
//! order in which ranges reach the session, so "the seventh write fails" names the same write on
//! every machine. Where a test needs concurrency it says so and asserts only order-independent
//! facts.

// A failed assertion *is* the reporting mechanism in a test; the no-panic rule (ERR-004) exists
// to protect production paths, not test bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{CdmError, JobKind, RunId, RunStatus};
use cdm_engine::planner::{Partitioner, Planner, PlannerSettings};
use cdm_engine::scheduler::{
    NoopObserver, RangeContext, RangeProcessor, RangeVerdict, RunReport, Scheduler,
    SchedulerSettings, StopReason,
};
use cdm_metrics::{CounterKind, CounterView};
use cdm_testkit::{Fault, FaultKind, FaultPlan, FaultySession, TestSession};

/// How many ranges every run in this file plans.
const RANGES: u64 = 8;
/// How many rows each range pretends to hold.
const ROWS_PER_RANGE: usize = 5;

/// Whether a failed statement costs the row or the range (`ARCHITECTURE.md` §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Isolation {
    /// A bad row: `ERROR` goes up by one and the loop continues (`ERR-005`).
    Record,
    /// A bad cluster: the range fails and the scheduler accounts for it (`ENG-008`).
    Range,
}

/// A migrate loop in miniature, reading and writing through a faulty session (`TST-040`).
///
/// Small on purpose. It counts `READ` before the write and `WRITE` after it, which is the one
/// property of the real loop that `ENG-008`'s `READ − WRITE − SKIPPED` term depends on: a row that
/// was read and then lost has to be visible as a read with no matching write.
#[derive(Debug)]
struct FaultyJob {
    session: FaultySession,
    isolation: Isolation,
    /// A counter table's write is issued once and never retried (`CON-012`).
    counter_table: bool,
    write_attempts: AtomicUsize,
}

impl FaultyJob {
    fn new(plan: FaultPlan, isolation: Isolation) -> Self {
        Self {
            session: FaultySession::new(plan),
            isolation,
            counter_table: false,
            write_attempts: AtomicUsize::new(0),
        }
    }

    const fn counters(mut self) -> Self {
        self.counter_table = true;
        self
    }

    fn injected(&self) -> usize {
        self.session.injected_count()
    }

    fn write_attempts(&self) -> usize {
        self.write_attempts.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RangeProcessor for FaultyJob {
    fn job(&self) -> JobKind {
        JobKind::Migrate
    }

    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
        let counters = ctx.counters();
        let read = counters.counter(CounterKind::Read)?;
        let write = counters.counter(CounterKind::Write)?;
        let error = counters.counter(CounterKind::Error)?;

        // SCH-009: the schema check every job makes before its first read. A fault here fails the
        // range before a single row is read, which is what makes it visible as a *fatal* fault
        // rather than as one bad row.
        self.session.execute("SELECT schema_version").await?;

        for row in 0..ROWS_PER_RANGE {
            ctx.acquire_read_rows(1).await;
            counters.increment(read);

            self.write_attempts.fetch_add(1, Ordering::SeqCst);
            let statement = if self.counter_table {
                format!("UPDATE ks.t SET n = n + 1 WHERE k = {row}")
            } else {
                format!("INSERT INTO ks.t (k) VALUES ({row})")
            };
            match self.session.execute(&statement).await {
                Ok(_) => {
                    ctx.acquire_write_rows(1).await;
                    counters.increment(write);
                }
                // ARCHITECTURE.md §13: a bad row costs one ERROR and the range continues; a bad
                // cluster fails the range and lets ENG-008 do the accounting.
                Err(_) if self.isolation == Isolation::Record => counters.increment(error),
                Err(failure) => return Err(failure),
            }
        }
        Ok(RangeVerdict::Pass)
    }
}

/// Runs `job` over a plan of [`RANGES`] ranges with one worker, so statement order is fixed.
async fn run(job: Arc<FaultyJob>, settings: SchedulerSettings) -> RunReport {
    let plan = Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(RANGES))
        .plan(RunId::from_raw(1), None)
        .unwrap();
    Scheduler::new(settings.with_workers(1))
        .unwrap()
        .run(&plan, job, Arc::new(NoopObserver))
        .await
        .unwrap()
}

/// The committed total of `kind` for the whole run.
fn total(report: &RunReport, kind: CounterKind) -> u64 {
    report.counters().count_of(kind, CounterView::Committed)
}

#[tokio::test(flavor = "multi_thread")]
async fn tst_040_a_write_fault_fails_one_range_and_the_run_carries_on() {
    // The seventh write times out. That lands in the middle of the second range — five rows per
    // range — so one range fails with rows already written and the other seven finish.
    let job = Arc::new(FaultyJob::new(
        FaultPlan::none().on_attempts("INSERT", Fault::target(FaultKind::WriteTimeout), [7usize]),
        Isolation::Range,
    ));
    let report = run(Arc::clone(&job), SchedulerSettings::default()).await;

    assert_eq!(job.injected(), 1, "exactly one fault fired");
    assert_eq!(report.ranges_failed(), 1, "ENG-008: one range, not the run");
    assert_eq!(
        report.ranges_passed(),
        usize::try_from(RANGES).unwrap() - 1,
        "the other ranges must be unaffected"
    );
    assert_eq!(report.status(), RunStatus::Ended, "the run is not aborted");
    assert_eq!(report.stopped_by(), None);
    assert_eq!(report.outcomes().len(), usize::try_from(RANGES).unwrap());

    let failed = report
        .outcomes()
        .iter()
        .find(|outcome| outcome.is_failure())
        .unwrap();
    assert_eq!(failed.status, RunStatus::Fail);
    assert!(!failed.abandoned, "a failure is not an abandonment");
    assert!(failed.diagnostic.is_some(), "ENG-008 records why");
}

#[tokio::test(flavor = "multi_thread")]
async fn tst_040_the_error_count_is_the_rows_the_range_could_not_account_for() {
    // The number Java's validate path reports as zero. The failing range read two rows, wrote
    // one, and then died: exactly one row was lost, and `ERROR` has to say so.
    let job = Arc::new(FaultyJob::new(
        FaultPlan::none().on_attempts("INSERT", Fault::target(FaultKind::WriteTimeout), [2usize]),
        Isolation::Range,
    ));
    let report = run(Arc::clone(&job), SchedulerSettings::default()).await;

    assert_eq!(report.ranges_failed(), 1);
    assert_eq!(
        total(&report, CounterKind::Error),
        1,
        "READ(2) − WRITE(1) − SKIPPED(0): the row that was read and lost"
    );
    assert_eq!(
        total(&report, CounterKind::PartitionsFailed),
        1,
        "ENG-008 increments the failed-partition counter exactly once"
    );
    assert_eq!(total(&report, CounterKind::PartitionsPassed), RANGES - 1);

    // The whole run: seven clean ranges of five rows, plus the two the failing range read.
    assert_eq!(total(&report, CounterKind::Read), (RANGES - 1) * 5 + 2);
    assert_eq!(total(&report, CounterKind::Write), (RANGES - 1) * 5 + 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn tst_040_every_transport_fault_is_contained_at_the_range_boundary() {
    // Five different failures, one containment rule. If any of them escaped, the run would end
    // with fewer outcomes than ranges rather than with a failed one.
    for kind in [
        FaultKind::ReadTimeout,
        FaultKind::WriteTimeout,
        FaultKind::Unavailable,
        FaultKind::Overloaded,
        FaultKind::ConnectionDropped,
    ] {
        let job = Arc::new(FaultyJob::new(
            FaultPlan::none().on_attempts("INSERT", Fault::target(kind), [3usize]),
            Isolation::Range,
        ));
        let report = run(Arc::clone(&job), SchedulerSettings::default()).await;

        assert_eq!(report.ranges_failed(), 1, "{kind} escaped its range");
        assert_eq!(report.outcomes().len(), usize::try_from(RANGES).unwrap());
        assert_eq!(report.status(), RunStatus::Ended, "{kind} aborted the run");
        assert_eq!(
            total(&report, CounterKind::Error),
            1,
            "{kind}: the range read three rows, wrote two, and lost the third"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tst_040_a_schema_change_aborts_the_run_rather_than_failing_every_range() {
    // `SCH-009` is the fault that must not be contained, and `ENG-015` is what stops it being.
    // The distinction matters because both behaviours end with nothing written: the difference is
    // whether the operator gets one error or `num_parts` copies of it, and whether the run row
    // says `ABORTED` — which is the only durable record that the schema, not the data, was wrong.
    // This is the shape `sch_009_a_schema_change_aborts_the_run` sees against a real node.
    let job = Arc::new(FaultyJob::new(
        FaultPlan::none().always(
            "SELECT schema_version",
            Fault::origin(FaultKind::SchemaChanged),
        ),
        Isolation::Range,
    ));
    let report = run(Arc::clone(&job), SchedulerSettings::default()).await;

    assert_eq!(report.status(), RunStatus::Aborted);
    assert_eq!(report.stopped_by(), Some(StopReason::Fatal));
    assert_eq!(report.ranges_passed(), 0);
    // One worker, so the first range to hit it is the only one that can: the rest are never
    // claimed and are left for a resume once the schema is settled.
    assert_eq!(report.ranges_failed(), 1);
    assert_eq!(
        report.unclaimed_ranges().len(),
        usize::try_from(RANGES).unwrap() - 1
    );
    assert_eq!(job.write_attempts(), 0, "not one row was written");
    assert_eq!(
        total(&report, CounterKind::Error),
        0,
        "a range that failed before reading anything lost nothing"
    );
    assert_eq!(total(&report, CounterKind::PartitionsFailed), 1);
    assert_eq!(report.exit_code(), 1, "and it is not a retryable exit");
}

#[tokio::test(flavor = "multi_thread")]
async fn tst_040_injected_faults_that_lose_enough_rows_abort_the_run() {
    // `ENG-009`: the error limit counts *rows*, across the run, and is checked after every range
    // rather than only after a failed one. Every range here loses its second row, so the limit is
    // reached by attrition — six ranges, six lost rows — which is the case a check that only ran
    // on the failure path would still catch, and a check that compared per-range counts would not.
    let job = Arc::new(FaultyJob::new(
        FaultPlan::none().every_nth("INSERT", Fault::target(FaultKind::Overloaded), 2),
        Isolation::Range,
    ));
    let report = run(
        Arc::clone(&job),
        SchedulerSettings::default().with_error_limit(5),
    )
    .await;

    assert_eq!(report.status(), RunStatus::Aborted);
    assert_eq!(report.stopped_by(), Some(StopReason::ErrorLimit));
    assert_eq!(
        report.exit_code(),
        1,
        "an error-limit abort is not retryable"
    );
    assert_eq!(
        total(&report, CounterKind::Error),
        6,
        "the limit is exceeded strictly, and the run stops at the first range that does so"
    );
    assert_eq!(report.outcomes().len(), 6);
    assert!(
        !report.unclaimed_ranges().is_empty(),
        "ENG-010: what the run did not claim is left for a resume"
    );
    assert_eq!(
        report.outcomes().len() + report.unclaimed_ranges().len(),
        usize::try_from(RANGES).unwrap(),
        "TST-041: every range is either accounted for or left unclaimed",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tst_040_a_counter_fault_fails_the_range_without_a_second_attempt() {
    // `CON-012`: a counter update that timed out may or may not have been applied, so the one
    // thing that must not happen is a second attempt. The job issues one statement per row and
    // fails the range on the first refusal; what this asserts is that the fault fired once and
    // the range stopped there, rather than the range re-issuing the update.
    let job = Arc::new(
        FaultyJob::new(
            FaultPlan::none().on_attempts(
                "UPDATE",
                Fault::target(FaultKind::WriteTimeout),
                [2usize],
            ),
            Isolation::Range,
        )
        .counters(),
    );
    let report = run(Arc::clone(&job), SchedulerSettings::default()).await;

    assert_eq!(job.injected(), 1, "the fault fired once");
    assert_eq!(report.ranges_failed(), 1);
    assert_eq!(
        job.write_attempts(),
        (usize::try_from(RANGES).unwrap() - 1) * ROWS_PER_RANGE + 2,
        "the failing range issued two updates and stopped: no retry of the second (CON-012)"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn tst_040_a_record_level_fault_costs_one_row_and_not_the_range() {
    // The innermost of the three levels of isolation. A row the job itself rejects — a bind that
    // cannot be built, a value that will not convert — is one `ERROR` and the loop continues, and
    // the range still passes. Getting this wrong in the other direction is the failure mode
    // `eng_008_a_record_failure_and_a_range_failure_are_different_values` guards: a target that is
    // refusing every write would otherwise look like a run with a great many bad rows.
    let job = Arc::new(FaultyJob::new(
        FaultPlan::none().every_nth("INSERT", Fault::target(FaultKind::WriteTimeout), 4),
        Isolation::Record,
    ));
    let report = run(Arc::clone(&job), SchedulerSettings::default()).await;

    assert_eq!(report.ranges_failed(), 0, "no range failed");
    assert_eq!(report.ranges_passed(), usize::try_from(RANGES).unwrap());
    assert_eq!(total(&report, CounterKind::PartitionsFailed), 0);

    let rows = RANGES * u64::try_from(ROWS_PER_RANGE).unwrap();
    assert_eq!(total(&report, CounterKind::Read), rows);
    assert_eq!(total(&report, CounterKind::Error), rows / 4);
    assert_eq!(total(&report, CounterKind::Write), rows - rows / 4);
    assert_eq!(u64::try_from(job.injected()).unwrap(), rows / 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn tst_040_a_run_with_no_faults_injected_is_the_baseline_it_is_compared_against() {
    // A fault suite whose clean case does not pass proves nothing about its dirty ones.
    let job = Arc::new(FaultyJob::new(FaultPlan::none(), Isolation::Range));
    let report = run(Arc::clone(&job), SchedulerSettings::default()).await;

    assert_eq!(job.injected(), 0);
    assert_eq!(report.ranges_failed(), 0);
    assert!(report.is_complete());
    assert_eq!(total(&report, CounterKind::Error), 0);
    assert_eq!(
        total(&report, CounterKind::Write),
        RANGES * u64::try_from(ROWS_PER_RANGE).unwrap()
    );
}
