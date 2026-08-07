//! Property-based tests over a whole scheduler run (`TST-010`).
//!
//! # Which properties, and why these
//!
//! `TST-010` names four families. Three of them already have homes beside the code they cover:
//! the token splitter in `planner::split` and `planner::shuffle`, codec round-trips in
//! `cdm-codec`'s `codec_properties.rs`, and the configuration round-trip in `cdm-config`. What is
//! here is the pair that only exists once several pieces are assembled:
//!
//! | Property | Why it earns its keep |
//! |---|---|
//! | [`tst_010_a_runs_committed_totals_are_the_sum_of_its_ranges`] | Three defects so far — `MIG-004`, `ENG-008`, `VAL-016` — are one mistake: reading a counter at the level it was not written to. A property relating the two levels across a whole run catches the fourth. |
//! | [`tst_010_a_run_never_accumulates_anything_at_the_interim_level`] | The structural half of the same claim. The run registry is only ever merged into, and `JobCounters::add` merges committed values, so its interim level is *permanently zero* — which is exactly why `ENG-009`'s error limit must not read it. |
//! | [`tst_010_every_range_is_accounted_for_exactly_once`] | The precondition `TST-041` is built on: no range may be silently dropped between the plan and the report. |
//!
//! # Why a property rather than a case
//!
//! The counter defects are not wrong for a particular number of rows. They are wrong for *every*
//! number of rows, in a way that a hand-written case with one range and ten rows can easily miss —
//! `MIG-004`'s threshold comparison is correct whenever the interim and committed values happen to
//! agree, which is exactly what they do when there is only one range and it has already flushed.
//! Generating the shape of the run is what makes the disagreement between the two levels
//! observable.
//!
//! # Determinism
//!
//! `proptest`'s generator is seeded and its counterexample is shrunk and printed, so a failure
//! here replays. Nothing observes elapsed time. Where a run has more than one worker the
//! assertions are sums and set-equalities, which do not depend on the order the workers finish in.

// A failed assertion *is* the reporting mechanism in a test; the no-panic rule (ERR-004) exists
// to protect production paths, not test bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{CdmError, ErrorKind, JobKind, RunId, TokenRange};
use cdm_engine::planner::{Partitioner, Planner, PlannerSettings};
use cdm_engine::scheduler::{
    NoopObserver, RangeContext, RangeProcessor, RangeVerdict, RunReport, Scheduler,
    SchedulerSettings,
};
use cdm_metrics::{CounterKind, CounterView};
use cdm_testkit::parse_metrics_string;
use parking_lot::Mutex;
use proptest::prelude::*;

/// What one range does before it ends, as the generator draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RangeProgram {
    /// Rows read from the origin.
    read: u64,
    /// Rows written to the target. Never more than `read`.
    written: u64,
    /// Rows a filter rejected. Never more than `read - written`.
    skipped: u64,
    /// Whether the range then fails (`ENG-008`).
    fails: bool,
}

impl RangeProgram {
    /// The rows `ENG-008` says a failure of this range loses.
    const fn lost(self) -> u64 {
        if self.fails {
            self.read
                .saturating_sub(self.written)
                .saturating_sub(self.skipped)
        } else {
            0
        }
    }
}

/// A generator for one range's program, with the arithmetic constraint the real loop obeys:
/// a row is written, skipped, or lost, never two of those.
fn range_program() -> impl Strategy<Value = RangeProgram> {
    (0_u64..40, 0_u64..100, 0_u64..100, any::<bool>()).prop_map(
        |(read, written_pct, skipped_pct, fails)| {
            let written = read * written_pct / 100;
            let skipped = (read - written) * skipped_pct / 100;
            RangeProgram {
                read,
                written,
                skipped,
                fails,
            }
        },
    )
}

/// A job that runs whichever program the generator drew for the range it is handed.
///
/// Programs are assigned in ring order rather than in claim order, so the same generated input
/// produces the same run whatever the workers do.
#[derive(Debug)]
struct ProgrammedJob {
    programs: BTreeMap<TokenRange, RangeProgram>,
    seen: Mutex<Vec<TokenRange>>,
}

impl ProgrammedJob {
    fn new(ranges: &[TokenRange], programs: &[RangeProgram]) -> Self {
        Self {
            programs: ranges
                .iter()
                .copied()
                .zip(programs.iter().copied())
                .collect(),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<TokenRange> {
        self.seen.lock().clone()
    }

    /// The totals the generator asked for, summed over every range.
    fn expected(&self) -> BTreeMap<CounterKind, u64> {
        let mut totals = BTreeMap::new();
        for program in self.programs.values() {
            *totals.entry(CounterKind::Read).or_default() += program.read;
            *totals.entry(CounterKind::Write).or_default() += program.written;
            *totals.entry(CounterKind::Skipped).or_default() += program.skipped;
            *totals.entry(CounterKind::Error).or_default() += program.lost();
            let partition = if program.fails {
                CounterKind::PartitionsFailed
            } else {
                CounterKind::PartitionsPassed
            };
            *totals.entry(partition).or_default() += 1;
        }
        totals.entry(CounterKind::Unflushed).or_default();
        totals
    }
}

#[async_trait]
impl RangeProcessor for ProgrammedJob {
    fn job(&self) -> JobKind {
        JobKind::Migrate
    }

    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
        self.seen.lock().push(ctx.range());
        let program = self.programs.get(&ctx.range()).copied().unwrap_or(
            // A range the generator did not describe would silently contribute nothing, which is
            // the one way this fixture could make a broken run look correct.
            RangeProgram {
                read: 0,
                written: 0,
                skipped: 0,
                fails: false,
            },
        );

        let counters = ctx.counters();
        counters.increment_by(counters.counter(CounterKind::Read)?, program.read);
        counters.increment_by(counters.counter(CounterKind::Write)?, program.written);
        counters.increment_by(counters.counter(CounterKind::Skipped)?, program.skipped);

        if program.fails {
            return Err(CdmError::new(
                ErrorKind::Write,
                "the generated program fails this range",
            ));
        }
        Ok(RangeVerdict::Pass)
    }
}

/// Plans `programs.len()` ranges and runs them, returning the job and the report.
async fn run(programs: &[RangeProgram], workers: u32) -> (Arc<ProgrammedJob>, RunReport) {
    let plan = Planner::new(
        PlannerSettings::new(Partitioner::Murmur3)
            .with_num_parts(u64::try_from(programs.len()).unwrap_or(1)),
    )
    .plan(RunId::from_raw(1), None)
    .unwrap();
    let job = Arc::new(ProgrammedJob::new(&plan.ring_ordered(), programs));
    let report = Scheduler::new(SchedulerSettings::default().with_workers(workers))
        .unwrap()
        .run(
            &plan,
            Arc::clone(&job) as Arc<dyn RangeProcessor>,
            Arc::new(NoopObserver),
        )
        .await
        .unwrap();
    (job, report)
}

/// A blocking `run`, because `proptest!` bodies are synchronous.
fn run_blocking(programs: &[RangeProgram], workers: u32) -> (Arc<ProgrammedJob>, RunReport) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run(programs, workers))
}

proptest! {
    // The scheduler spawns a task per range, so a case is not free; a hundred is enough to
    // explore the shape space without turning `cargo test` into a coffee break.
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// `TST-010`, `MET-004`: a run's committed totals are exactly the sum of what its ranges did.
    ///
    /// The property the interim/committed defects violate. Each range increments its own registry
    /// at the interim level, flushes when it ends, and is merged into the run's registry at the
    /// committed level; if any step of that reads or writes the wrong level, the run's totals stop
    /// being the sum of the ranges' — and the numbers an operator signs a migration off on stop
    /// meaning what they say.
    #[test]
    fn tst_010_a_runs_committed_totals_are_the_sum_of_its_ranges(
        programs in proptest::collection::vec(range_program(), 1..12),
    ) {
        let (job, report) = run_blocking(&programs, 1);

        for (kind, expected) in job.expected() {
            prop_assert_eq!(
                report.counters().count_of(kind, CounterView::Committed),
                expected,
                "{} disagrees with the sum of the ranges",
                kind,
            );
        }
        prop_assert_eq!(report.ranges_failed(), programs.iter().filter(|p| p.fails).count());
        prop_assert_eq!(report.ranges_passed(), programs.iter().filter(|p| !p.fails).count());
    }

    /// `TST-010`, `MET-004`: the run's interim level is empty, whatever the ranges did.
    ///
    /// Not an incidental fact. `JobCounters::add` merges committed values only, and the run's
    /// registry is never incremented directly, so its interim level is structurally zero for the
    /// whole run. That is precisely why `ENG-009`'s error limit reads the *committed* level: read
    /// the interim one and the comparison is `0 > limit` on every call, and the limit silently
    /// never fires — the shape of the `ENG-008` defect, one requirement along.
    #[test]
    fn tst_010_a_run_never_accumulates_anything_at_the_interim_level(
        programs in proptest::collection::vec(range_program(), 1..12),
    ) {
        let (_, report) = run_blocking(&programs, 1);

        for kind in CounterKind::ALL {
            prop_assert_eq!(
                report.counters().count_of(kind, CounterView::Interim),
                0,
                "{} has an interim value at the run level, so a check that read it would be \
                 reading a number nobody writes",
                kind,
            );
        }
        // And the committed level is not vacuously zero too, or the assertion above proves
        // nothing: some counter moved, unless the generator drew a run that did nothing at all.
        let did_something = programs.iter().any(|p| p.read > 0) || !programs.is_empty();
        prop_assert_eq!(
            did_something,
            !report.counters().is_zero(),
            "a run that did something must have committed something",
        );
    }

    /// `TST-010`, `MET-004`, `TRK-021`: the per-range `run_info` strings sum to the run's totals.
    ///
    /// The two levels meet in exactly one durable place — the tracking table — and this is the
    /// relation that makes a resume's arithmetic trustworthy. `RangeOutcome::run_info` is rendered
    /// from a range's *committed* counters, after its flush; the run's totals are those same
    /// values merged. If the range rendered before its flush it would report zeroes (`MIG-004`),
    /// and if it rendered the interim level it would report work that was in flight and never
    /// landed. Either way the two sides of this equality come apart.
    #[test]
    fn tst_010_the_range_run_info_strings_sum_to_the_runs_totals(
        programs in proptest::collection::vec(range_program(), 1..12),
    ) {
        let (_, report) = run_blocking(&programs, 1);

        let mut summed: BTreeMap<CounterKind, u64> = BTreeMap::new();
        for outcome in report.outcomes() {
            let parsed = parse_metrics_string(&outcome.run_info).unwrap();
            for (kind, value) in parsed {
                *summed.entry(kind).or_default() += value;
            }
        }
        for kind in [
            CounterKind::Read,
            CounterKind::Write,
            CounterKind::Skipped,
            CounterKind::Error,
            CounterKind::PartitionsPassed,
            CounterKind::PartitionsFailed,
        ] {
            prop_assert_eq!(
                summed.get(&kind).copied().unwrap_or_default(),
                report.counters().count_of(kind, CounterView::Committed),
                "{}: the tracking rows and the run row disagree",
                kind,
            );
        }
        // UNFLUSHED is interim-only bookkeeping and must never appear in a committed rendering.
        prop_assert!(!summed.contains_key(&CounterKind::Unflushed));
    }

    /// `TST-010`, `ENG-002`: every planned range is reported exactly once.
    ///
    /// The precondition `TST-041` rests on. A resume works from the recorded outcome of every
    /// range, so a range that the scheduler processed and did not report is a range no resume can
    /// know about — and it would be *silently* lost, because the run's counters would still add
    /// up if the range had nothing in it.
    #[test]
    fn tst_010_every_range_is_accounted_for_exactly_once(
        programs in proptest::collection::vec(range_program(), 1..12),
        workers in 1_u32..5,
    ) {
        let (job, report) = run_blocking(&programs, workers);

        let planned: BTreeSet<TokenRange> = job.programs.keys().copied().collect();
        let reported: BTreeSet<TokenRange> =
            report.outcomes().iter().map(|outcome| outcome.range).collect();
        prop_assert_eq!(&planned, &reported, "the report is not the plan");
        prop_assert_eq!(report.outcomes().len(), planned.len(), "a range was reported twice");

        let processed = job.seen();
        prop_assert_eq!(processed.len(), planned.len(), "a range was processed twice");
        prop_assert!(report.unclaimed_ranges().is_empty(), "a completed run claims everything");
    }
}
