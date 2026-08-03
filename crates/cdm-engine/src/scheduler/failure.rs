//! Per-range failure accounting (`ENG-008`, `ENG-013`).
//!
//! When a range fails, `ENG-008` requires four things and only these four: the range is marked
//! `FAIL`, `PARTITIONS_FAILED` is incremented, `ERROR` is incremented by the number of rows the
//! range read but did not account for, and the error is logged with the range bounds. The run
//! carries on.
//!
//! # The lost-rows term
//!
//! `ERROR` answers one question — *how many rows did this range read and then lose?* — so it is
//! read minus everything the range can account for:
//!
//! | Job | `ERROR` increment |
//! |---|---|
//! | migrate | `READ − WRITE − SKIPPED` |
//! | validate | `READ − VALID − MISSING − MISMATCH − SKIPPED` |
//! | guardrail | none: `MET-002` does not register `ERROR` for it |
//!
//! # Interim, not committed
//!
//! Every term is read at the **interim** level, before [`JobCounters::flush`] runs. This is where
//! Java CDM is wrong, and specifically where it is wrong twice over:
//!
//! * `CopyJobSession` reads all three terms with `getCount(type, true)` — interim — and gets the
//!   right answer.
//! * `DiffJobSession` reads all five with `getCount(type)` — committed — on a path where
//!   `flush()` has not yet run. Every committed counter is therefore still `0`, the expression
//!   evaluates to `0 − 0 − 0 − 0 − 0`, and **a failed validate range always increments `ERROR` by
//!   exactly zero**. The counter whose entire purpose is to report lost rows reports none, for
//!   the job whose entire purpose is to find discrepancies.
//!
//! `ENG-008` marks this `[P+]`: cdm-rs uses interim counts for both jobs, and `--compat-java`
//! does not restore the bug. Reproducing a counter that is silently always zero has no legitimate
//! use — there is no script that depends on being lied to.
//!
//! # Saturation
//!
//! Java's terms are signed `long`s and the subtraction can go negative: a migrate range that
//! fails *during* a flush can have credited more writes than the interim `READ` it is measured
//! against, and Java would then subtract from `ERROR`. cdm-rs counters are unsigned and the
//! arithmetic saturates at zero. "Minus three rows were lost" is not a fact about the world, and
//! a negative contribution would corrupt the run total that `ENG-009` compares against
//! `error_limit`.

use cdm_core::JobKind;
use cdm_metrics::{CounterKind, CounterView, JobCounters};

/// The rows a failed range read but could not account for (`ENG-008`).
///
/// Read at the interim level, so this must be called *before* the range's counters are flushed.
#[must_use]
pub(crate) fn lost_rows(counters: &JobCounters) -> u64 {
    let count = |kind: CounterKind| counters.count_of(kind, CounterView::Interim);
    let read = count(CounterKind::Read);
    match counters.job() {
        JobKind::Migrate => read
            .saturating_sub(count(CounterKind::Write))
            .saturating_sub(count(CounterKind::Skipped)),
        JobKind::Validate => read
            .saturating_sub(count(CounterKind::Valid))
            .saturating_sub(count(CounterKind::Missing))
            .saturating_sub(count(CounterKind::Mismatch))
            .saturating_sub(count(CounterKind::Skipped)),
        // MET-002 does not register ERROR for guardrail, so there is nothing to increment; Java's
        // GuardrailCheckJobSession likewise counts only the failed partition.
        JobKind::Guardrail => 0,
    }
}

/// Applies `ENG-008`'s accounting to a failed range's counters, and returns the `ERROR`
/// increment for the log line.
///
/// Increments `ERROR` by [`lost_rows`] and `PARTITIONS_FAILED` by one, at the interim level. The
/// caller flushes afterwards, exactly as `CopyJobSession` does.
pub(crate) fn record_range_failure(counters: &JobCounters) -> u64 {
    let lost = lost_rows(counters);
    if let Ok(error) = counters.counter(CounterKind::Error) {
        if lost > 0 {
            counters.increment_by(error, lost);
        }
    }
    if let Ok(failed) = counters.counter(CounterKind::PartitionsFailed) {
        counters.increment(failed);
    }
    lost
}

/// Applies the success half of `ENG-002`: one more partition passed.
pub(crate) fn record_range_success(counters: &JobCounters) {
    if let Ok(passed) = counters.counter(CounterKind::PartitionsPassed) {
        counters.increment(passed);
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
    use super::*;

    fn counters(job: JobKind, counts: &[(CounterKind, u64)]) -> JobCounters {
        let registry = JobCounters::new(job);
        for &(kind, by) in counts {
            let counter = registry.counter(kind).unwrap();
            registry.increment_by(counter, by);
        }
        registry
    }

    #[test]
    fn eng_008_migrate_loses_the_rows_it_read_but_neither_wrote_nor_skipped() {
        let registry = counters(
            JobKind::Migrate,
            &[
                (CounterKind::Read, 100),
                (CounterKind::Write, 70),
                (CounterKind::Skipped, 5),
            ],
        );
        assert_eq!(lost_rows(&registry), 25);
    }

    #[test]
    fn eng_008_validate_loses_the_rows_it_read_but_did_not_classify() {
        let registry = counters(
            JobKind::Validate,
            &[
                (CounterKind::Read, 100),
                (CounterKind::Valid, 60),
                (CounterKind::Missing, 10),
                (CounterKind::Mismatch, 5),
                (CounterKind::Skipped, 2),
            ],
        );
        assert_eq!(lost_rows(&registry), 23);
    }

    #[test]
    fn eng_008_a_failed_validate_range_counts_lost_rows_where_java_counts_zero() {
        // The whole of Java's bug, in one test. `DiffJobSession` reads these five terms at the
        // committed level, where they are all still zero because `flush()` has not run, so its
        // ERROR increment is always 0. Ours is the number of rows actually lost.
        let registry = counters(
            JobKind::Validate,
            &[(CounterKind::Read, 40), (CounterKind::Valid, 10)],
        );

        let java_would_increment_by = {
            let count = |kind| registry.count_of(kind, CounterView::Committed);
            count(CounterKind::Read)
                .saturating_sub(count(CounterKind::Valid))
                .saturating_sub(count(CounterKind::Missing))
                .saturating_sub(count(CounterKind::Mismatch))
                .saturating_sub(count(CounterKind::Skipped))
        };
        assert_eq!(java_would_increment_by, 0, "Java's failure path is blind");
        assert_eq!(lost_rows(&registry), 30, "ours is not");
    }

    #[test]
    fn eng_008_terms_are_read_before_the_flush_not_after() {
        let registry = counters(
            JobKind::Migrate,
            &[(CounterKind::Read, 10), (CounterKind::Write, 4)],
        );
        assert_eq!(lost_rows(&registry), 6);
        // After a flush the interim level is empty, which is exactly the state Java's validate
        // path reads its committed terms in.
        registry.flush();
        assert_eq!(lost_rows(&registry), 0);
    }

    #[test]
    fn eng_008_the_lost_row_count_saturates_rather_than_going_negative() {
        let registry = counters(
            JobKind::Migrate,
            &[(CounterKind::Read, 5), (CounterKind::Write, 9)],
        );
        assert_eq!(lost_rows(&registry), 0);
    }

    #[test]
    fn eng_008_guardrail_registers_no_error_counter_so_loses_no_rows() {
        let registry = counters(JobKind::Guardrail, &[(CounterKind::Read, 100)]);
        assert_eq!(lost_rows(&registry), 0);

        // The failed partition is still counted; only the ERROR term is absent.
        assert_eq!(record_range_failure(&registry), 0);
        assert_eq!(
            registry.count_of(CounterKind::PartitionsFailed, CounterView::Interim),
            1
        );
        assert_eq!(
            registry.count_of(CounterKind::Error, CounterView::Interim),
            0
        );
    }

    #[test]
    fn eng_008_a_failure_increments_error_and_the_failed_partition_counter() {
        let registry = counters(
            JobKind::Migrate,
            &[(CounterKind::Read, 8), (CounterKind::Skipped, 1)],
        );
        assert_eq!(record_range_failure(&registry), 7);
        assert_eq!(
            registry.count_of(CounterKind::Error, CounterView::Interim),
            7
        );
        assert_eq!(
            registry.count_of(CounterKind::PartitionsFailed, CounterView::Interim),
            1
        );
        assert_eq!(
            registry.count_of(CounterKind::PartitionsPassed, CounterView::Interim),
            0
        );
    }

    #[test]
    fn eng_008_a_failure_that_lost_no_rows_leaves_error_alone() {
        let registry = counters(JobKind::Migrate, &[]);
        assert_eq!(record_range_failure(&registry), 0);
        assert_eq!(
            registry.count_of(CounterKind::Error, CounterView::Interim),
            0
        );
        assert_eq!(
            registry.count_of(CounterKind::PartitionsFailed, CounterView::Interim),
            1
        );
    }

    #[test]
    fn eng_002_a_success_increments_the_passed_partition_counter() {
        let registry = counters(JobKind::Validate, &[]);
        record_range_success(&registry);
        assert_eq!(
            registry.count_of(CounterKind::PartitionsPassed, CounterView::Interim),
            1
        );
        assert_eq!(
            registry.count_of(CounterKind::PartitionsFailed, CounterView::Interim),
            0
        );
    }
}
