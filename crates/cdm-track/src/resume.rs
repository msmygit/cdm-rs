//! Choosing a previous run and turning it into the work list of the next one
//! (`TRK-030`..`TRK-033`).
//!
//! # The one bias this module has
//!
//! **When in doubt, re-run the range.** Re-running a range that already completed costs time and
//! nothing else — migrate writes carry the origin's writetime, so a repeated upsert is a no-op at
//! the storage layer, and validate reads nothing it can damage. *Skipping* a range that did not
//! complete loses rows silently, and it loses them in a way no later run will notice, because the
//! range is recorded as done.
//!
//! Every ambiguity below therefore resolves towards re-running:
//!
//! * a range left `STARTED` is pending, not complete — the worker that claimed it may have died
//!   at any point (`TRK-031`);
//! * a range whose recorded status cdm-rs does not recognise is pending, not ignored — a status
//!   written by a future version, or by a Java build with a status this one has not heard of,
//!   must not be read as success;
//! * a run whose `run_info` cannot be parsed for `Partitions Failed:` counts as having failed
//!   partitions, where Java throws `NumberFormatException` and aborts the job;
//! * a previous run that cannot be found at all falls back to a **full** plan, not to an empty
//!   one (`TRK-032`).
//!
//! # The exception, and why it is structural
//!
//! Counter updates are not idempotent (`DST-015`). Re-running a counter range that partially
//! applied double-counts, and no writetime trick can undo that. For a counter table the pending
//! set is therefore *narrowed* rather than widened: only ranges that demonstrably never started
//! are re-planned, and everything else is quarantined for manual reconciliation and reported.
//!
//! This is not a filter applied after the fact. [`RerunPolicy::rerunnable_statuses`] returns the
//! whole set of statuses a policy may re-plan, and for a writing counter job that set is the
//! single element `NOT_STARTED`. There is no code path that adds to it, so no future edit can
//! reintroduce a counter rerun by forgetting a guard.

use cdm_core::{
    CdmError, ErrorKind, JobKind, RangeRecord, RunId, RunRecord, RunStatus, TokenRange,
};
use cdm_engine::planner::{shuffle_for_run, split_ring};

/// The prefix Java's metrics string uses for the failed-partition count (`MET-005`).
///
/// `TRK-030` reads this back out of `cdm_run_info.run_info` to decide whether a run that reached
/// `ENDED` nevertheless has work left. The string is `COMPAT-004`'s contract, so it is named once
/// here and matched against rather than reconstructed at each use.
pub const PARTITIONS_FAILED_PREFIX: &str = "Partitions Failed:";

/// Whether a run's ranges may be re-run, and which ones (`TRK-031`, `DST-015`).
///
/// The policy is a property of the *table and job*, not of the range: it is decided once, when
/// the resume is planned, from facts (`is this a counter table?`, `does this job write?`) that do
/// not vary between ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RerunPolicy {
    /// Whether the target table has counter columns.
    counter_table: bool,
    /// Whether this job writes to the target at all.
    writes_to_target: bool,
}

impl RerunPolicy {
    /// The statuses an ordinary, idempotent job re-plans: Java's four (`TRK-031`).
    const IDEMPOTENT: &'static [RunStatus] = &[
        RunStatus::NotStarted,
        RunStatus::Started,
        RunStatus::Fail,
        RunStatus::Diff,
    ];

    /// The statuses a counter-writing job may re-plan (`DST-015`).
    ///
    /// One element, and it is the only status that proves nothing was applied. A `STARTED`,
    /// `FAIL` or `DIFF` counter range may have applied some of its updates, and re-applying them
    /// would add to a counter that already moved.
    const COUNTER_SAFE: &'static [RunStatus] = &[RunStatus::NotStarted];

    /// The policy for a job against a non-counter table: everything Java re-plans.
    pub const fn idempotent() -> Self {
        Self {
            counter_table: false,
            writes_to_target: true,
        }
    }

    /// The policy for `job` against `table_has_counters`.
    ///
    /// A [`JobKind::Validate`] run that does not correct writes nothing, so its ranges are safe
    /// to re-read even on a counter table; `correcting` is the caller's
    /// `autocorrect.missing || autocorrect.mismatch || autocorrect.missing_counter`. A
    /// [`JobKind::Guardrail`] run never writes.
    pub const fn for_job(job: JobKind, table_has_counters: bool, correcting: bool) -> Self {
        let writes_to_target = match job {
            JobKind::Migrate => true,
            JobKind::Validate => correcting,
            JobKind::Guardrail => false,
        };
        Self {
            counter_table: table_has_counters,
            writes_to_target,
        }
    }

    /// Whether this policy is the restricted, counter-safe one.
    pub const fn is_counter_restricted(&self) -> bool {
        self.counter_table && self.writes_to_target
    }

    /// Every status this policy is willing to re-plan.
    ///
    /// The whole of the counter exclusion of `DST-015` lives in this `if`: a restricted policy
    /// returns a one-element slice, and [`plan_resume`] re-plans nothing that is not in the slice
    /// it returns.
    pub const fn rerunnable_statuses(&self) -> &'static [RunStatus] {
        if self.is_counter_restricted() {
            Self::COUNTER_SAFE
        } else {
            Self::IDEMPOTENT
        }
    }

    /// What to do with a range recorded in `status`.
    pub fn disposition(&self, status: Option<RunStatus>) -> RangeDisposition {
        let Some(status) = status else {
            // A status string this build does not recognise. Treating it as complete is the one
            // outcome that loses data, so it is treated as pending — unless the table is a
            // counter table, where "pending" could mean "partially applied".
            return if self.is_counter_restricted() {
                RangeDisposition::Quarantined(QuarantineReason::CounterPartiallyApplied)
            } else {
                RangeDisposition::Rerun
            };
        };
        if self.rerunnable_statuses().contains(&status) {
            RangeDisposition::Rerun
        } else if self.is_counter_restricted() && status.is_pending() {
            RangeDisposition::Quarantined(QuarantineReason::CounterPartiallyApplied)
        } else {
            RangeDisposition::Complete
        }
    }
}

/// What a resume does with one range of the previous run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeDisposition {
    /// The range finished; the resume leaves it alone.
    Complete,
    /// The range is unfinished and will be re-planned.
    Rerun,
    /// The range is unfinished but re-running it would be unsafe (`DST-015`).
    Quarantined(QuarantineReason),
}

/// Why a range was withheld from the resume rather than re-planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuarantineReason {
    /// A counter range that may have partially applied. Re-running it would double-count, so it
    /// needs manual reconciliation (`DST-015`).
    CounterPartiallyApplied,
}

impl QuarantineReason {
    /// The operator-facing explanation, which is also what goes in the run summary.
    pub const fn message(&self) -> &'static str {
        match self {
            Self::CounterPartiallyApplied => {
                "counter range may have partially applied; manual reconciliation required \
                 (re-running it would double-count)"
            }
        }
    }
}

/// A range the resume declined to re-plan, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuarantinedRange {
    /// The token bounds. Bounds only: a quarantine report never names a row (`SEC-002`).
    pub range: TokenRange,
    /// The status the previous run left it in, if it was one cdm-rs recognises.
    pub status: Option<RunStatus>,
    /// Why it was withheld.
    pub reason: QuarantineReason,
}

/// Why a resume gave up on the previous run and planned afresh (`TRK-032`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// No `cdm_run_info` row exists for the previous run id. Java's `RunNotStartedException`,
    /// "Run NOT FOUND".
    RunNotFound(RunId),
    /// The previous run's row exists but never left `NOT_STARTED`, so it planned nothing and
    /// there is nothing to resume. Java's `RunNotStartedException`, "Run NOT STARTED".
    RunNotStarted(RunId),
    /// The previous run recorded no range rows at all. Java would silently resume zero ranges,
    /// which migrates nothing and reports success; cdm-rs plans a full run instead.
    NoRangesRecorded(RunId),
}

impl FallbackReason {
    /// The warning logged when the fallback fires.
    pub fn message(&self) -> String {
        match self {
            Self::RunNotFound(id) => {
                format!("run {id} was not found in cdm_run_info; starting a new full run")
            }
            Self::RunNotStarted(id) => {
                format!("run {id} never left NOT_STARTED; starting a new full run")
            }
            Self::NoRangesRecorded(id) => format!(
                "run {id} recorded no token ranges, so nothing can be resumed from it; \
                 starting a new full run"
            ),
        }
    }
}

/// The outcome of planning a resume (`TRK-031`, `TRK-032`, `TRK-033`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePlan {
    ranges: Vec<TokenRange>,
    quarantined: Vec<QuarantinedRange>,
    fallback: Option<FallbackReason>,
    previous_run_id: Option<RunId>,
    considered: usize,
}

impl ResumePlan {
    /// A resume that produced no work list, because the previous run cannot be resumed
    /// (`TRK-032`). The caller must plan a full run.
    pub fn fallback(previous_run_id: RunId, reason: FallbackReason) -> Self {
        Self {
            ranges: Vec::new(),
            quarantined: Vec::new(),
            fallback: Some(reason),
            previous_run_id: Some(previous_run_id),
            considered: 0,
        }
    }

    /// The ranges to process, already shuffled and, if asked, subdivided.
    pub fn ranges(&self) -> &[TokenRange] {
        &self.ranges
    }

    /// The ranges that were unfinished but could not be safely re-run (`DST-015`).
    ///
    /// Never empty for a counter table whose previous run was interrupted mid-range, and the
    /// caller is expected to surface it: these ranges are the ones a human has to reconcile, and
    /// a resume that hid them would look like a clean recovery.
    pub fn quarantined(&self) -> &[QuarantinedRange] {
        &self.quarantined
    }

    /// Why the resume fell back to a full plan, if it did (`TRK-032`).
    ///
    /// `Some` means [`ResumePlan::ranges`] is empty *and the caller must plan the whole ring* —
    /// which is the opposite of what an empty range list would otherwise mean. Callers that
    /// ignore this field skip the entire table.
    pub fn fallback_reason(&self) -> Option<&FallbackReason> {
        self.fallback.as_ref()
    }

    /// Whether the caller must plan a fresh full run instead of using [`ResumePlan::ranges`].
    pub fn is_fallback(&self) -> bool {
        self.fallback.is_some()
    }

    /// The run this plan resumes.
    pub fn previous_run_id(&self) -> Option<RunId> {
        self.previous_run_id
    }

    /// How many rows of `cdm_run_details` the plan looked at.
    pub fn considered(&self) -> usize {
        self.considered
    }
}

/// Whether `run` is worth adopting as the previous run (`TRK-030`).
///
/// Java's rule, reproduced: a run is adopted if it did **not** reach `ENDED`, *or* if its
/// `run_info` reports failed partitions. The two cdm-rs statuses `INTERRUPTED` and `ABORTED` are
/// not `ENDED` either, so they adopt for free.
pub fn is_resumable(run: &RunRecord) -> bool {
    run.status != RunStatus::Ended || run.info.as_deref().is_some_and(has_failed_partitions)
}

/// Selects the run `auto_rerun` adopts (`TRK-030`).
///
/// `latest` is the newest `cdm_run_info` row for `(table_name, run_type)`. Returns `None` when
/// there is nothing to resume — no previous run, or one that finished cleanly — in which case the
/// caller plans a full run with no previous id, exactly as Java's `prevRunId = 0` does.
pub fn adopt_previous_run(latest: Option<&RunRecord>) -> Option<RunId> {
    latest.filter(|run| is_resumable(run)).map(|run| run.run_id)
}

/// Whether a metrics string reports at least one failed partition (`TRK-030`).
///
/// Java splits on `;`, trims, looks for the `Partitions Failed:` prefix, splits *that* on `:` and
/// parses the second half with `Integer.parseInt`. A malformed count therefore throws and kills
/// the job. cdm-rs treats an unparseable count as `true` instead: the string exists to say
/// whether there is work left, and if it cannot be read, assuming there is work re-runs ranges
/// that may already be done, while assuming there is none abandons ranges that are not.
pub fn has_failed_partitions(run_info: &str) -> bool {
    run_info.split(';').map(str::trim).any(|entry| {
        let Some(rest) = entry.strip_prefix(PARTITIONS_FAILED_PREFIX) else {
            return false;
        };
        // Java requires exactly two colon-separated halves; anything else it ignores. Here an
        // unreadable count is a reason to resume, not a reason to throw.
        rest.trim().parse::<i64>().map_or(true, |failed| failed > 0)
    })
}

/// Turns the previous run's recorded ranges into the next run's work list
/// (`TRK-031`, `TRK-032`, `TRK-033`).
///
/// * `previous` is the previous run's `cdm_run_info` row, `None` if it is absent;
/// * `records` is every `cdm_run_details` row of that run, in any order;
/// * `policy` decides which statuses may be re-planned (`DST-015`);
/// * `rerun_multiplier` subdivides each pending range (`TRK-033`);
/// * `run_id` seeds the shuffle, so the order of a resume is reproducible (`TOK-007`).
///
/// The result is shuffled — Java shuffles twice, and [`shuffle_for_run`] does the same with a
/// seeded generator — because consecutive ranges belong to the same replica set and processing
/// them back to back concentrates the load on one part of the ring.
///
/// # Errors
///
/// Returns [`ErrorKind::Tracking`] if a pending range cannot be subdivided, which can only happen
/// for a multiplier the configuration should already have rejected.
pub fn plan_resume(
    previous_run_id: RunId,
    previous: Option<&RunRecord>,
    records: &[RangeRecord],
    policy: RerunPolicy,
    rerun_multiplier: u32,
    run_id: RunId,
) -> Result<ResumePlan, CdmError> {
    // TRK-032, first half: Java's "Run NOT FOUND" and "Run NOT STARTED" paths. Both mean the
    // previous run planned nothing we can trust, and both fall back to a full plan rather than to
    // an empty one.
    let Some(previous) = previous else {
        return Ok(ResumePlan::fallback(
            previous_run_id,
            FallbackReason::RunNotFound(previous_run_id),
        ));
    };
    if previous.status == RunStatus::NotStarted {
        return Ok(ResumePlan::fallback(
            previous_run_id,
            FallbackReason::RunNotStarted(previous_run_id),
        ));
    }
    // TRK-032, second half — beyond Java. A run that reached STARTED but has no range rows is
    // indistinguishable, from here, from one whose range rows were lost. Java would resume zero
    // ranges and report a clean, empty success.
    if records.is_empty() {
        return Ok(ResumePlan::fallback(
            previous_run_id,
            FallbackReason::NoRangesRecorded(previous_run_id),
        ));
    }

    let mut pending: Vec<TokenRange> = Vec::new();
    let mut quarantined: Vec<QuarantinedRange> = Vec::new();
    for record in records {
        match policy.disposition(Some(record.status)) {
            RangeDisposition::Complete => {}
            RangeDisposition::Rerun => pending.push(record.range),
            RangeDisposition::Quarantined(reason) => quarantined.push(QuarantinedRange {
                range: record.range,
                status: Some(record.status),
                reason,
            }),
        }
    }

    // TRK-033. Java calls `SplitPartitions.getRandomSubPartitions(multiplier, min, max, 100, …)`,
    // which is the same splitter the initial plan uses at full coverage. Reusing `split_ring`
    // rather than dividing the span here is what keeps the two in step: the Java splitter has two
    // documented quirks (the `while (curMax <= max)` guard and the `partitionSize == 0` fallback)
    // that a second implementation would get subtly wrong.
    let multiplier = rerun_multiplier.max(1);
    if multiplier > 1 {
        let mut subdivided = Vec::with_capacity(pending.len().saturating_mul(multiplier as usize));
        for range in &pending {
            subdivided.extend(subdivide(*range, multiplier)?);
        }
        pending = subdivided;
    }

    // Deterministic within a run id, and independent of the order the rows came back in, so a
    // resume replanned on another node schedules the same ranges in the same order (`TOK-007`).
    pending.sort_unstable();
    shuffle_for_run(&mut pending, run_id);
    quarantined.sort_unstable_by_key(|q| q.range);

    Ok(ResumePlan {
        ranges: pending,
        quarantined,
        fallback: None,
        previous_run_id: Some(previous_run_id),
        considered: records.len(),
    })
}

/// Splits one pending range into `multiplier` sub-ranges at 100% coverage (`TRK-033`).
///
/// Full coverage, always: `filter.token_coverage_percent` was already applied when the range was
/// planned, and applying it again would shrink the range a second time and silently stop
/// re-reading part of what the previous run was supposed to cover.
///
/// # Errors
///
/// Returns whatever [`split_ring`] reports; with `multiplier >= 1` that is only the range-ceiling
/// check of `NFR-003`.
pub fn subdivide(range: TokenRange, multiplier: u32) -> Result<Vec<TokenRange>, CdmError> {
    if multiplier <= 1 {
        return Ok(vec![range]);
    }
    let parts = split_ring(range, u64::from(multiplier), 100).map_err(|err| {
        CdmError::new(
            ErrorKind::Tracking,
            format!("cannot subdivide pending range {range} by {multiplier}: {err}"),
        )
    })?;
    // The splitter never returns nothing, but if it ever did, the range would vanish from the
    // resume — the one failure this module exists to prevent. Falling back to the whole range
    // costs a re-read and loses nothing.
    if parts.is_empty() {
        return Ok(vec![range]);
    }
    Ok(parts)
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
    use cdm_core::TableRef;

    use super::*;

    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    fn record(min: i128, max: i128, status: RunStatus) -> RangeRecord {
        RangeRecord {
            range: range(min, max),
            status,
            started_at: None,
            info: None,
        }
    }

    fn run(status: RunStatus, info: Option<&str>) -> RunRecord {
        RunRecord {
            run_id: RunId::from_raw(100),
            previous_run_id: None,
            table: TableRef::new("ks", "t"),
            job: JobKind::Migrate,
            status,
            started_at: None,
            ended_at: None,
            info: info.map(str::to_owned),
        }
    }

    fn plan(records: &[RangeRecord], policy: RerunPolicy, multiplier: u32) -> ResumePlan {
        plan_resume(
            RunId::from_raw(100),
            Some(&run(RunStatus::Started, None)),
            records,
            policy,
            multiplier,
            RunId::from_raw(101),
        )
        .unwrap()
    }

    // -----------------------------------------------------------------------------------------
    // TRK-030 — adopting a previous run
    // -----------------------------------------------------------------------------------------

    #[test]
    fn trk_030_a_run_that_did_not_end_is_adopted() {
        for status in [
            RunStatus::NotStarted,
            RunStatus::Started,
            RunStatus::Interrupted,
            RunStatus::Aborted,
        ] {
            assert!(is_resumable(&run(status, None)), "{status} must be adopted");
        }
        assert_eq!(
            adopt_previous_run(Some(&run(RunStatus::Started, None))),
            Some(RunId::from_raw(100))
        );
    }

    #[test]
    fn trk_030_an_ended_run_is_adopted_only_when_it_reports_failed_partitions() {
        let clean = run(
            RunStatus::Ended,
            Some("Read: 10; Write: 10; Partitions Passed: 5; Partitions Failed: 0"),
        );
        assert!(!is_resumable(&clean));
        assert_eq!(adopt_previous_run(Some(&clean)), None);

        let dirty = run(
            RunStatus::Ended,
            Some("Read: 10; Write: 9; Partitions Passed: 4; Partitions Failed: 1"),
        );
        assert!(is_resumable(&dirty));
        assert_eq!(adopt_previous_run(Some(&dirty)), Some(RunId::from_raw(100)));
    }

    #[test]
    fn trk_030_no_previous_run_adopts_nothing() {
        assert_eq!(adopt_previous_run(None), None);
        assert!(!is_resumable(&run(RunStatus::Ended, None)));
    }

    #[test]
    fn trk_030_an_unparseable_failure_count_resumes_rather_than_throwing() {
        // Java's Integer.parseInt would throw here and kill the job.
        assert!(has_failed_partitions("Partitions Failed: not-a-number"));
        assert!(has_failed_partitions("Read: 1; Partitions Failed:"));
        // And a well-formed zero still means "nothing to do".
        assert!(!has_failed_partitions("Partitions Failed: 0"));
        assert!(!has_failed_partitions("Read: 1; Write: 1"));
        assert!(has_failed_partitions("Read: 1; Partitions Failed: 3"));
    }

    // -----------------------------------------------------------------------------------------
    // TRK-031 — the pending set
    // -----------------------------------------------------------------------------------------

    #[test]
    fn trk_031_only_the_four_java_statuses_are_replanned() {
        let records = [
            record(0, 9, RunStatus::NotStarted),
            record(10, 19, RunStatus::Started),
            record(20, 29, RunStatus::Fail),
            record(30, 39, RunStatus::Diff),
            record(40, 49, RunStatus::Pass),
            record(50, 59, RunStatus::DiffCorrected),
        ];
        let plan = plan(&records, RerunPolicy::idempotent(), 1);
        let mut ranges = plan.ranges().to_vec();
        ranges.sort_unstable();
        assert_eq!(
            ranges,
            vec![range(0, 9), range(10, 19), range(20, 29), range(30, 39)]
        );
        assert!(plan.quarantined().is_empty());
        assert_eq!(plan.considered(), 6);
    }

    #[test]
    fn trk_031_a_started_range_is_pending_because_the_worker_may_have_died() {
        let plan = plan(
            &[record(0, 9, RunStatus::Started)],
            RerunPolicy::idempotent(),
            1,
        );
        assert_eq!(plan.ranges(), [range(0, 9)]);
    }

    #[test]
    fn trk_031_the_resume_order_is_shuffled_and_reproducible() {
        let records: Vec<RangeRecord> = (0..64)
            .map(|i| record(i * 10, i * 10 + 9, RunStatus::Fail))
            .collect();
        let first = plan(&records, RerunPolicy::idempotent(), 1);
        let second = plan(&records, RerunPolicy::idempotent(), 1);
        assert_eq!(first.ranges(), second.ranges(), "same run id, same order");

        let mut sorted = first.ranges().to_vec();
        sorted.sort_unstable();
        assert_ne!(first.ranges(), sorted.as_slice(), "the order is permuted");
        assert_eq!(sorted.len(), 64, "and nothing was lost in permuting it");
    }

    #[test]
    fn trk_031_row_order_from_the_store_does_not_change_the_plan() {
        let forwards: Vec<RangeRecord> = (0..16)
            .map(|i| record(i * 10, i * 10 + 9, RunStatus::Fail))
            .collect();
        let mut backwards = forwards.clone();
        backwards.reverse();
        assert_eq!(
            plan(&forwards, RerunPolicy::idempotent(), 1).ranges(),
            plan(&backwards, RerunPolicy::idempotent(), 1).ranges()
        );
    }

    #[test]
    fn trk_031_an_unrecognised_status_is_treated_as_pending() {
        // `disposition(None)` is the "a status string this build cannot parse" case; the store
        // hands it through rather than dropping the row.
        assert_eq!(
            RerunPolicy::idempotent().disposition(None),
            RangeDisposition::Rerun
        );
    }

    // -----------------------------------------------------------------------------------------
    // TRK-032 — the fallback
    // -----------------------------------------------------------------------------------------

    #[test]
    fn trk_032_a_missing_previous_run_falls_back_to_a_full_plan() {
        let plan = plan_resume(
            RunId::from_raw(7),
            None,
            &[record(0, 9, RunStatus::Fail)],
            RerunPolicy::idempotent(),
            1,
            RunId::from_raw(8),
        )
        .unwrap();
        assert!(plan.is_fallback());
        assert_eq!(
            plan.fallback_reason(),
            Some(&FallbackReason::RunNotFound(RunId::from_raw(7)))
        );
        assert!(plan.ranges().is_empty());
        assert!(plan.fallback_reason().unwrap().message().contains('7'));
    }

    #[test]
    fn trk_032_a_previous_run_still_not_started_falls_back_to_a_full_plan() {
        let plan = plan_resume(
            RunId::from_raw(7),
            Some(&run(RunStatus::NotStarted, None)),
            &[record(0, 9, RunStatus::NotStarted)],
            RerunPolicy::idempotent(),
            1,
            RunId::from_raw(8),
        )
        .unwrap();
        assert_eq!(
            plan.fallback_reason(),
            Some(&FallbackReason::RunNotStarted(RunId::from_raw(7)))
        );
    }

    #[test]
    fn trk_032_a_previous_run_with_no_range_rows_falls_back_rather_than_migrating_nothing() {
        let plan = plan_resume(
            RunId::from_raw(7),
            Some(&run(RunStatus::Started, None)),
            &[],
            RerunPolicy::idempotent(),
            1,
            RunId::from_raw(8),
        )
        .unwrap();
        assert_eq!(
            plan.fallback_reason(),
            Some(&FallbackReason::NoRangesRecorded(RunId::from_raw(7)))
        );
        assert!(plan.is_fallback());
    }

    #[test]
    fn trk_032_a_completed_previous_run_yields_an_empty_plan_that_is_not_a_fallback() {
        // Everything passed: there is genuinely nothing to do, and the caller must *not* replan
        // the ring. The distinction between this and a fallback is the whole point of the flag.
        let plan = plan(
            &[record(0, 9, RunStatus::Pass)],
            RerunPolicy::idempotent(),
            1,
        );
        assert!(plan.ranges().is_empty());
        assert!(!plan.is_fallback());
    }

    // -----------------------------------------------------------------------------------------
    // TRK-033 — the rerun multiplier
    // -----------------------------------------------------------------------------------------

    #[test]
    fn trk_033_a_multiplier_subdivides_each_pending_range_at_full_coverage() {
        let plan = plan(
            &[record(0, 99, RunStatus::Fail)],
            RerunPolicy::idempotent(),
            4,
        );
        let mut ranges = plan.ranges().to_vec();
        ranges.sort_unstable();
        assert_eq!(ranges.len(), 4);
        assert_eq!(ranges[0].min(), 0);
        assert_eq!(
            ranges[3].max(),
            99,
            "the subdivision covers the whole range"
        );
        // Contiguity: every token of the original range is still covered by exactly one part.
        for pair in ranges.windows(2) {
            assert_eq!(pair[1].min(), pair[0].max() + 1);
        }
    }

    #[test]
    fn trk_033_subdivision_uses_the_shared_java_parity_splitter() {
        // The same call the planner makes for the initial plan, at 100% coverage. If this ever
        // diverges, a resume and a fresh plan disagree about where the range boundaries are.
        assert_eq!(
            subdivide(range(1, 100), 10).unwrap(),
            split_ring(range(1, 100), 10, 100).unwrap()
        );
    }

    #[test]
    fn trk_033_a_multiplier_of_one_or_zero_leaves_ranges_alone() {
        assert_eq!(subdivide(range(0, 99), 1).unwrap(), vec![range(0, 99)]);
        assert_eq!(subdivide(range(0, 99), 0).unwrap(), vec![range(0, 99)]);
        let plan = plan(
            &[record(0, 99, RunStatus::Fail)],
            RerunPolicy::idempotent(),
            1,
        );
        assert_eq!(plan.ranges(), [range(0, 99)]);
    }

    #[test]
    fn trk_033_subdivision_never_makes_a_pending_range_disappear() {
        for multiplier in [2_u32, 3, 7, 64] {
            let parts = subdivide(range(0, 5), multiplier).unwrap();
            assert!(!parts.is_empty());
            assert_eq!(parts[0].min(), 0);
            assert_eq!(parts[parts.len() - 1].max(), 5);
        }
    }

    // -----------------------------------------------------------------------------------------
    // DST-015 — counters
    // -----------------------------------------------------------------------------------------

    #[test]
    fn dst_015_a_counter_migrate_replans_only_ranges_that_never_started() {
        let policy = RerunPolicy::for_job(JobKind::Migrate, true, false);
        assert!(policy.is_counter_restricted());
        assert_eq!(policy.rerunnable_statuses(), &[RunStatus::NotStarted]);

        let records = [
            record(0, 9, RunStatus::NotStarted),
            record(10, 19, RunStatus::Started),
            record(20, 29, RunStatus::Fail),
            record(30, 39, RunStatus::Diff),
            record(40, 49, RunStatus::Pass),
        ];
        let plan = plan(&records, policy, 1);
        assert_eq!(plan.ranges(), [range(0, 9)]);
        assert_eq!(
            plan.quarantined()
                .iter()
                .map(|q| q.range)
                .collect::<Vec<_>>(),
            vec![range(10, 19), range(20, 29), range(30, 39)],
        );
        for quarantined in plan.quarantined() {
            assert_eq!(
                quarantined.reason,
                QuarantineReason::CounterPartiallyApplied
            );
            assert!(quarantined.reason.message().contains("double-count"));
        }
    }

    #[test]
    fn dst_015_no_multiplier_can_reintroduce_a_counter_rerun() {
        // Subdivision happens *after* the disposition filter, so a large multiplier multiplies
        // only the ranges that were already safe to re-run.
        let policy = RerunPolicy::for_job(JobKind::Migrate, true, false);
        let plan = plan(
            &[
                record(0, 99, RunStatus::Started),
                record(100, 199, RunStatus::NotStarted),
            ],
            policy,
            4,
        );
        assert_eq!(plan.ranges().len(), 4);
        for produced in plan.ranges().iter().copied() {
            assert!(
                produced.min() >= 100,
                "{produced} came from the STARTED counter range"
            );
        }
    }

    #[test]
    fn dst_015_a_counter_range_with_an_unknown_status_is_quarantined_not_rerun() {
        let policy = RerunPolicy::for_job(JobKind::Migrate, true, false);
        assert_eq!(
            policy.disposition(None),
            RangeDisposition::Quarantined(QuarantineReason::CounterPartiallyApplied)
        );
    }

    #[test]
    fn dst_015_a_read_only_job_on_a_counter_table_is_not_restricted() {
        // Validate without autocorrect writes nothing, so re-reading a counter range is safe;
        // guardrail never writes at all.
        assert!(!RerunPolicy::for_job(JobKind::Validate, true, false).is_counter_restricted());
        assert!(!RerunPolicy::for_job(JobKind::Guardrail, true, true).is_counter_restricted());
        // Turning autocorrect on makes validate a writer, and the restriction applies again.
        assert!(RerunPolicy::for_job(JobKind::Validate, true, true).is_counter_restricted());
        // And a non-counter table is never restricted, whatever the job.
        assert!(!RerunPolicy::for_job(JobKind::Migrate, false, true).is_counter_restricted());
    }

    #[test]
    fn sec_002_a_quarantine_report_names_token_bounds_and_nothing_else() {
        let policy = RerunPolicy::for_job(JobKind::Migrate, true, false);
        let plan = plan(&[record(10, 19, RunStatus::Started)], policy, 1);
        let rendered = format!("{:?}", plan.quarantined());
        assert!(rendered.contains("10"));
        assert!(!rendered.to_lowercase().contains("password"));
        assert_eq!(plan.quarantined()[0].status, Some(RunStatus::Started));
    }
}
