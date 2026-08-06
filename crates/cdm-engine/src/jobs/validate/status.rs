//! Resolving a validated range's status (`VAL-016`).
//!
//! Four outcomes, and the distinction between the middle two is the whole point of running validate
//! with autocorrect at all:
//!
//! | Condition | Status | What an operator does |
//! |---|---|---|
//! | no discrepancy | `PASS` | nothing |
//! | every discrepancy was corrected | `DIFF_CORRECTED` | nothing, but the range *was* wrong |
//! | a discrepancy remains | `DIFF` | investigate, and re-run this range |
//! | the range failed | `FAIL` | investigate, and re-run this range |
//!
//! `TRK-031` re-plans `DIFF` and `FAIL` ranges on a resume and leaves `PASS` and `DIFF_CORRECTED`
//! alone, so getting this wrong does not merely mislabel a range — it decides whether the range is
//! ever looked at again.
//!
//! # Which counters this reads, and why it is the interim level
//!
//! `MET-004` gives every counter an interim and a committed level. A job increments the interim
//! level and the *scheduler* flushes, which is what moves interim into committed
//! (`scheduler::processor`). At the moment a range's verdict is decided the flush has not happened
//! yet, so the committed level is still zero and the interim level holds everything the range did.
//!
//! That is not a detail: reading the committed level here would make
//! `MISSING == CORRECTED_MISSING` trivially true — `0 == 0` — and **every** range with a
//! discrepancy would be reported `DIFF_CORRECTED`, including one where nothing was corrected at
//! all. It is the same shape of defect as `ENG-008`, where Java's validate failure path reads five
//! counters at a level where they are structurally always zero and therefore always computes `0`.
//! `val_016_the_verdict_reads_the_level_that_has_the_counts_in_it` asserts the two levels differ
//! rather than merely asserting that the verdict is right.

use cdm_metrics::{CounterKind, CounterView, JobCounters};

use crate::scheduler::RangeVerdict;

/// The verdict for a range that completed, from its **interim** counters (`VAL-016`).
///
/// `had_discrepancy` is `true` if any record in the range compared as missing or mismatched. It is
/// tracked separately rather than derived from the counters because Java tracks it separately
/// (`AtomicBoolean hasDiff`), and because the two are not the same question: a range whose every
/// discrepancy was corrected still *had* one, and must not be reported `PASS`.
#[must_use]
pub fn verdict(counters: &JobCounters, had_discrepancy: bool) -> RangeVerdict {
    if !had_discrepancy {
        return RangeVerdict::Pass;
    }
    let count = |kind| counters.count_of(kind, CounterView::Interim);
    let all_corrected = count(CounterKind::Missing) == count(CounterKind::CorrectedMissing)
        && count(CounterKind::Mismatch) == count(CounterKind::CorrectedMismatch);
    if all_corrected {
        RangeVerdict::DiffCorrected
    } else {
        RangeVerdict::Diff
    }
}
