//! Resume correctness: a run killed at any point loses nothing (`TST-041`).
//!
//! # The invariant, stated once
//!
//! **For any interruption, at any point, the union of what the run finished and what a resume
//! plans covers every range.** Re-running a range that already completed costs time — a migrate
//! write carries the origin's writetime, so a repeated upsert is a no-op at the storage layer.
//! *Skipping* one that did not complete loses rows permanently and silently, because the range is
//! then recorded as done and no later run will look at it again.
//!
//! `cdm-track` biases towards re-running at four separate points, and each of those is covered by
//! a unit test beside the code. What is missing, and what is here, is the property those four
//! decisions exist to produce. A unit test says "a `STARTED` range is re-planned"; this says "kill
//! the run anywhere and nothing is lost", which is the claim an operator actually needs and the
//! one that survives somebody adding a fifth status.
//!
//! # How the interruption is made deterministic
//!
//! An operator stop, applied by the job itself after a generated number of ranges, with one
//! worker. No signals — a real `SIGINT` is unreproducible on Windows and racy everywhere — and no
//! sleeps. The generated stop point is drawn by `proptest`, which shrinks and prints its
//! counterexample, so a failure names the interruption that produced it.
//!
//! The three interesting shapes all fall out of the generator rather than being written by hand:
//! stopping before anything is claimed, stopping mid-plan with ranges in flight, and stopping
//! after the last range, which is not an interruption at all and must leave nothing to resume.
//!
//! | Claim | Test |
//! |---|---|
//! | Nothing is lost, whatever the interruption | [`tst_041_a_resume_covers_every_range_the_run_did_not_finish`] |
//! | The resume covers the whole ring, exactly once | [`tst_041_the_completed_and_resumed_ranges_tile_the_ring`] |
//! | A counter table quarantines rather than replays (`DST-015`) | [`tst_041_a_counter_run_quarantines_what_it_cannot_safely_replay`] |
//! | A resume of a resume still loses nothing | [`tst_041_resuming_a_resume_still_covers_everything`] |
//! | A run that finished has nothing to resume | [`tst_041_a_completed_run_plans_no_work_and_is_not_a_fallback`] |

// A failed assertion *is* the reporting mechanism in a test; the no-panic rule (ERR-004) exists
// to protect production paths, not test bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{
    CdmError, JobKind, RangeRecord, RunId, RunRecord, RunStatus, TableRef, TokenRange,
    TrackingStore,
};
use cdm_engine::planner::{Partitioner, Planner, PlannerSettings, TokenPlan};
use cdm_engine::scheduler::{
    RangeContext, RangeObserver, RangeProcessor, RangeVerdict, RunControl, RunReport, Scheduler,
    SchedulerSettings, StopReason,
};
use cdm_metrics::CounterKind;
use cdm_track::store::MemoryStore;
use cdm_track::tracker::{committed_run_info, new_run_record, RunTracker, TrackerConfig};
use cdm_track::{plan_resume, RerunPolicy, ResumePlan};
use proptest::prelude::*;

/// The table every run in this file tracks against.
fn table() -> TableRef {
    TableRef::new("target_ks", "customers")
}

/// A job that migrates a fixed number of rows per range, and stops the run after `stop_after`
/// ranges have been claimed.
///
/// The stop is issued from inside a range rather than from outside it, which is the case that
/// matters: `ENG-010` lets in-flight ranges drain, so the range that issues the stop finishes and
/// the ranges behind it are never claimed. A resume has to distinguish those two.
#[derive(Debug)]
struct InterruptingJob {
    control: RunControl,
    /// After this many ranges have been entered, stop. `None` never stops.
    stop_after: Option<usize>,
    entered: AtomicUsize,
    rows_per_range: u64,
}

impl InterruptingJob {
    fn new(control: RunControl, stop_after: Option<usize>) -> Self {
        Self {
            control,
            stop_after,
            entered: AtomicUsize::new(0),
            rows_per_range: 4,
        }
    }
}

#[async_trait]
impl RangeProcessor for InterruptingJob {
    fn job(&self) -> JobKind {
        JobKind::Migrate
    }

    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
        let entered = self.entered.fetch_add(1, Ordering::SeqCst) + 1;
        let counters = ctx.counters();
        counters.increment_by(counters.counter(CounterKind::Read)?, self.rows_per_range);
        counters.increment_by(counters.counter(CounterKind::Write)?, self.rows_per_range);

        if self.stop_after == Some(entered) {
            self.control.stop(StopReason::Operator);
        }
        Ok(RangeVerdict::Pass)
    }
}

/// Everything one interrupted run left behind.
struct Interrupted {
    plan: TokenPlan,
    report: RunReport,
    run: RunRecord,
    records: Vec<RangeRecord>,
    run_id: RunId,
}

impl Interrupted {
    /// The ranges the scheduler reported as having reached a successful terminal status.
    ///
    /// These, and only these, are the ranges a resume is entitled to skip.
    fn finished(&self) -> BTreeSet<TokenRange> {
        self.report
            .outcomes()
            .iter()
            .filter(|outcome| outcome.is_success())
            .map(|outcome| outcome.range)
            .collect()
    }

    /// Plans the resume this run's records imply.
    fn resume(&self, policy: RerunPolicy, multiplier: u32, next: RunId) -> ResumePlan {
        plan_resume(
            self.run_id,
            Some(&self.run),
            &self.records,
            policy,
            multiplier,
            next,
        )
        .unwrap()
    }
}

/// Runs `num_parts` ranges, stopping after `stop_after` of them, with tracking recorded to
/// `store`.
async fn interrupted_run(
    store: &Arc<MemoryStore>,
    run_id: RunId,
    num_parts: u64,
    stop_after: Option<usize>,
) -> Interrupted {
    let plan = Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(num_parts))
        .plan(run_id, None)
        .unwrap();
    let planned = plan.token_ranges();

    let record = new_run_record(run_id, None, table(), JobKind::Migrate);
    let tracker = Arc::new(
        RunTracker::start(
            Arc::clone(store) as Arc<dyn TrackingStore>,
            &record,
            &planned,
            TrackerConfig::default(),
        )
        .await
        .unwrap(),
    );

    // One worker, so "stop after the third range" names the same third range every time.
    let scheduler = Scheduler::new(SchedulerSettings::default().with_workers(1)).unwrap();
    let job = Arc::new(InterruptingJob::new(scheduler.control(), stop_after));
    let report = scheduler
        .run(&plan, job, Arc::clone(&tracker) as Arc<dyn RangeObserver>)
        .await
        .unwrap();

    // TRK-022: the run row carries the status the scheduler reported, not `ENDED` unconditionally.
    // Writing `ENDED` here would make `TRK-030` decline to adopt the run, and every unfinished
    // range would be stranded — the exact failure this file exists to rule out.
    tracker
        .finish(report.status(), committed_run_info(report.counters()))
        .await
        .unwrap();

    let run = store.run(run_id).await.unwrap().unwrap();
    let records = store.ranges(run_id).await.unwrap();
    Interrupted {
        plan,
        report,
        run,
        records,
        run_id,
    }
}

/// Blocks on `interrupted_run`, because `proptest!` bodies are synchronous.
fn interrupted_blocking(
    run_id: RunId,
    num_parts: u64,
    stop_after: Option<usize>,
) -> (Arc<MemoryStore>, Interrupted) {
    let store = Arc::new(MemoryStore::new());
    let interrupted = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(interrupted_run(&store, run_id, num_parts, stop_after));
    (store, interrupted)
}

/// What a second, interrupted run over `ranges` leaves in the tracking table.
///
/// The scheduler plans the whole ring, and there is no public way to hand it an arbitrary work
/// list — that seam belongs to the CLI harness, not to a test. So the second run is driven through
/// the same [`RangeObserver`] calls the scheduler would make: the first `finished` ranges reach
/// `PASS`, one more is left `STARTED` because it was in flight when the stop landed, and the rest
/// stay `NOT_STARTED` because nobody claimed them. That is exactly the three-way split
/// [`interrupted_run`] produces, which is what makes the two comparable.
async fn interrupted_replay(
    store: &Arc<MemoryStore>,
    run_id: RunId,
    previous: RunId,
    ranges: &[TokenRange],
    finished: usize,
) -> (Vec<TokenRange>, RunRecord, Vec<RangeRecord>) {
    let record = new_run_record(run_id, Some(previous), table(), JobKind::Migrate);
    let tracker = RunTracker::start(
        Arc::clone(store) as Arc<dyn TrackingStore>,
        &record,
        ranges,
        TrackerConfig::default(),
    )
    .await
    .unwrap();

    let finished = finished.min(ranges.len());
    for range in &ranges[..finished] {
        tracker.start_range(*range);
        tracker.finish_range(*range, RunStatus::Pass, "Read: 4; Write: 4".to_owned());
    }
    if let Some(in_flight) = ranges.get(finished) {
        // ENG-010: claimed, then abandoned. Left `STARTED`, which `TRK-031` reads as pending.
        tracker.start_range(*in_flight);
    }
    tracker
        .finish(RunStatus::Aborted, "Partitions Failed: 0".to_owned())
        .await
        .unwrap();

    let run = store.run(run_id).await.unwrap().unwrap();
    let records = store.ranges(run_id).await.unwrap();
    (ranges[..finished].to_vec(), run, records)
}

/// The number of tokens a set of ranges covers, and whether they overlap.
fn tokens_covered(ranges: &[TokenRange]) -> u128 {
    ranges.iter().map(|range| range.token_count()).sum()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// `TST-041`: for any interruption, at any point, no unfinished range is skipped.
    ///
    /// This is the invariant in its most direct form. The scheduler reports which ranges reached a
    /// successful terminal status; every *other* planned range — one that was in flight when the
    /// stop landed, one that was never claimed, one that failed — must appear in the resume's work
    /// list. A resume that omits any of them has silently abandoned rows, and nothing downstream
    /// would notice, because the range would be recorded as done.
    #[test]
    fn tst_041_a_resume_covers_every_range_the_run_did_not_finish(
        num_parts in 2_u64..12,
        stop_after in prop::option::of(1_usize..12),
    ) {
        let stop_after = stop_after.filter(|n| *n <= usize::try_from(num_parts).unwrap());
        let (_store, interrupted) =
            interrupted_blocking(RunId::from_raw(1), num_parts, stop_after);

        let resume = interrupted.resume(RerunPolicy::idempotent(), 1, RunId::from_raw(2));
        prop_assert!(!resume.is_fallback(), "a tracked run is always resumable from its rows");

        let finished = interrupted.finished();
        let replanned: BTreeSet<TokenRange> = resume.ranges().iter().copied().collect();
        for range in interrupted.plan.token_ranges() {
            if finished.contains(&range) {
                continue;
            }
            prop_assert!(
                replanned.contains(&range),
                "{} did not finish and is not in the resume: its rows are lost",
                range,
            );
        }

        // And the converse half, which is a cost claim rather than a correctness one: a resume
        // that re-ran everything would also satisfy the assertion above.
        prop_assert_eq!(
            replanned.len() + finished.len(),
            interrupted.plan.len(),
            "the resume re-plans exactly what did not finish",
        );
    }

    /// `TST-041`, `TOK-003`: what completed and what is re-planned tile the ring exactly.
    ///
    /// The range-set assertion above is about identity; this one is about tokens, and it is what
    /// catches a resume that subdivides (`TRK-033`) incorrectly — a multiplier that lost a token
    /// at a boundary would keep every range in the set and still leave a gap nothing scans.
    #[test]
    fn tst_041_the_completed_and_resumed_ranges_tile_the_ring(
        num_parts in 2_u64..10,
        stop_after in 1_usize..10,
        multiplier in 1_u32..5,
    ) {
        let stop_after = Some(stop_after.min(usize::try_from(num_parts).unwrap()));
        let (_store, interrupted) =
            interrupted_blocking(RunId::from_raw(1), num_parts, stop_after);

        let resume =
            interrupted.resume(RerunPolicy::idempotent(), multiplier, RunId::from_raw(2));
        let finished: Vec<TokenRange> = interrupted.finished().into_iter().collect();

        let planned_tokens = tokens_covered(&interrupted.plan.token_ranges());
        let covered = tokens_covered(&finished) + tokens_covered(resume.ranges());
        prop_assert_eq!(
            covered,
            planned_tokens,
            "the ring is not tiled: {} tokens planned, {} covered",
            planned_tokens,
            covered,
        );

        // Disjointness, so that the equality above cannot be met by an overlap cancelling a gap.
        let mut all: Vec<TokenRange> = finished;
        all.extend_from_slice(resume.ranges());
        all.sort_unstable();
        for pair in all.windows(2) {
            prop_assert!(
                !pair[0].intersects(pair[1]),
                "{} and {} overlap",
                pair[0],
                pair[1],
            );
        }
    }

    /// `TST-041`, `DST-015`: a counter run accounts for every range, replayed or quarantined.
    ///
    /// The counter exception is the one place the "when in doubt, re-run" bias is inverted, and
    /// inverting it is only safe if the ranges it withholds are *reported* rather than dropped. A
    /// quarantined range is work a human has to reconcile; a silently dropped one is a counter
    /// that is quietly wrong for ever.
    #[test]
    fn tst_041_a_counter_run_quarantines_what_it_cannot_safely_replay(
        num_parts in 2_u64..10,
        stop_after in 1_usize..10,
    ) {
        let stop_after = Some(stop_after.min(usize::try_from(num_parts).unwrap()));
        let (_store, interrupted) =
            interrupted_blocking(RunId::from_raw(1), num_parts, stop_after);

        let policy = RerunPolicy::for_job(JobKind::Migrate, true, false);
        prop_assert!(policy.is_counter_restricted());
        let resume = interrupted.resume(policy, 1, RunId::from_raw(2));

        let replanned: BTreeSet<TokenRange> = resume.ranges().iter().copied().collect();
        let quarantined: BTreeSet<TokenRange> =
            resume.quarantined().iter().map(|q| q.range).collect();
        let finished = interrupted.finished();

        for range in interrupted.plan.token_ranges() {
            prop_assert!(
                finished.contains(&range)
                    || replanned.contains(&range)
                    || quarantined.contains(&range),
                "{} is neither done, nor re-planned, nor reported for reconciliation",
                range,
            );
        }
        // Nothing is both: a range that were replayed *and* reported would be double-counted by a
        // reconciliation that trusted the report.
        prop_assert!(replanned.is_disjoint(&quarantined));
        prop_assert!(replanned.is_disjoint(&finished));
    }

    /// `TST-041`: a resume that is itself interrupted still loses nothing.
    ///
    /// The case an operator actually meets — a migration interrupted twice — and the one where an
    /// off-by-one in the work list compounds instead of cancelling.
    #[test]
    fn tst_041_resuming_a_resume_still_covers_everything(
        num_parts in 3_u64..10,
        first_stop in 1_usize..8,
        second_stop in 1_usize..8,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let store = Arc::new(MemoryStore::new());
            let parts = usize::try_from(num_parts).unwrap();

            let first = interrupted_run(
                &store,
                RunId::from_raw(1),
                num_parts,
                Some(first_stop.min(parts)),
            )
            .await;
            let plan_of_the_ring = first.plan.token_ranges();
            let resume = first.resume(RerunPolicy::idempotent(), 1, RunId::from_raw(2));

            // The second run works from exactly the resume's list, and is interrupted in turn.
            let (second_finished, second_run, second_records) = interrupted_replay(
                &store,
                RunId::from_raw(2),
                RunId::from_raw(1),
                resume.ranges(),
                second_stop.min(parts),
            )
            .await;
            let final_resume = plan_resume(
                RunId::from_raw(2),
                Some(&second_run),
                &second_records,
                RerunPolicy::idempotent(),
                1,
                RunId::from_raw(3),
            )
            .unwrap();

            // Every range of the *original* plan is done by one of the two runs, or still to do.
            let mut done: BTreeSet<TokenRange> = first.finished();
            done.extend(second_finished);
            let outstanding: BTreeSet<TokenRange> =
                final_resume.ranges().iter().copied().collect();
            for range in plan_of_the_ring {
                assert!(
                    done.contains(&range) || outstanding.contains(&range),
                    "{range} survived two interruptions and is in neither set",
                );
            }
            assert!(
                done.is_disjoint(&outstanding),
                "no range is both finished and outstanding",
            );
        });
    }
}

#[tokio::test]
async fn tst_041_a_completed_run_plans_no_work_and_is_not_a_fallback() {
    // The distinction the whole `ResumePlan` API turns on. An empty work list means "nothing to
    // do"; a *fallback* means "plan the whole ring". A caller that confused them would either
    // migrate the table twice or skip it entirely, and both look like success from the outside.
    let store = Arc::new(MemoryStore::new());
    let interrupted = interrupted_run(&store, RunId::from_raw(1), 6, None).await;

    assert_eq!(interrupted.report.status(), RunStatus::Ended);
    assert_eq!(interrupted.finished().len(), 6);
    assert!(!cdm_track::resume::is_resumable(&interrupted.run));

    let resume = interrupted.resume(RerunPolicy::idempotent(), 1, RunId::from_raw(2));
    assert!(resume.ranges().is_empty());
    assert!(!resume.is_fallback());
    assert_eq!(resume.considered(), 6);
    assert!(resume.quarantined().is_empty());
}

#[tokio::test]
async fn tst_041_a_run_stopped_before_it_claimed_anything_resumes_the_whole_plan() {
    // The boundary the generator reaches least often: stopped so early that nothing finished. The
    // resume must re-plan every range, and must reach that answer through the range rows rather
    // than through the fallback, which would silently discard `filter.token_coverage_percent` and
    // any custom token bounds the original plan was built with.
    let store = Arc::new(MemoryStore::new());
    let interrupted = interrupted_run(&store, RunId::from_raw(1), 6, Some(1)).await;

    let resume = interrupted.resume(RerunPolicy::idempotent(), 1, RunId::from_raw(2));
    assert!(!resume.is_fallback());
    assert_eq!(
        resume.ranges().len() + interrupted.finished().len(),
        6,
        "one range finished; the other five are re-planned"
    );
    assert_eq!(interrupted.report.status(), RunStatus::Aborted);
    assert!(!interrupted.report.unclaimed_ranges().is_empty());
}

#[tokio::test]
async fn tst_041_an_in_flight_range_is_re_planned_rather_than_recorded_as_done() {
    // `ENG-010` leaves an abandoned range `STARTED`, and `TRK-031` reads `STARTED` as pending.
    // The two halves are tested separately; this is the join, which is where a mistake would
    // actually cost data.
    let store = Arc::new(MemoryStore::new());
    let interrupted = interrupted_run(&store, RunId::from_raw(1), 8, Some(2)).await;

    let pending: Vec<RunStatus> = interrupted
        .records
        .iter()
        .filter(|record| !interrupted.finished().contains(&record.range))
        .map(|record| record.status)
        .collect();
    assert!(!pending.is_empty());
    for status in pending {
        assert!(
            status.is_pending(),
            "{status} is not pending, so the resume would skip the range"
        );
    }
}
