//! The work list workers pull ranges from (`ENG-001`).
//!
//! `ARCHITECTURE.md` §5.2 calls this an MPMC queue, and a channel would do. It is a shared cursor
//! over an immutable slice instead, for three reasons:
//!
//! * **Work stealing is the absence of assignment.** No range is bound to a worker, so a worker
//!   that finishes early takes the next one and a straggler never idles the fleet — which is what
//!   `ENG-001` asks for. Pre-partitioning the plan across workers would reintroduce the
//!   long-pole problem that Spark's `parallelize` gives Java CDM.
//! * **The plan survives a pause.** `ENG-014` requires pausing to withhold new work "without
//!   losing the plan". A cursor over the original list has nothing to lose; a drained channel
//!   would have to be rebuilt.
//! * **Progress is observable.** [`WorkQueue::claimed`] and [`WorkQueue::remaining`] answer "how
//!   far in?" without draining anything, which the run report and the control plane both need.
//!
//! One relaxed `fetch_add` per range is also the cheapest possible hand-off, and the ordering can
//! be relaxed because the slice is immutable and the counter is the only thing being
//! synchronised: nothing is published through it.

use std::sync::atomic::{AtomicUsize, Ordering};

use cdm_core::TokenRange;

/// A shared cursor over the planned ranges (`ENG-001`).
#[derive(Debug)]
pub struct WorkQueue {
    ranges: Vec<TokenRange>,
    next: AtomicUsize,
}

impl WorkQueue {
    /// A queue over the plan's ranges, in the order the planner shuffled them into (`TOK-006`).
    #[must_use]
    pub const fn new(ranges: Vec<TokenRange>) -> Self {
        Self {
            ranges,
            next: AtomicUsize::new(0),
        }
    }

    /// Takes the next unclaimed range, or `None` once the plan is exhausted.
    ///
    /// Every range is handed to exactly one caller, however many callers there are.
    pub fn claim(&self) -> Option<TokenRange> {
        let index = self.next.fetch_add(1, Ordering::Relaxed);
        self.ranges.get(index).copied()
    }

    /// How many ranges the plan holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// Whether the plan is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// How many ranges have been handed out.
    ///
    /// Saturated at [`WorkQueue::len`]: the cursor keeps advancing while exhausted callers race
    /// to discover there is nothing left, and those overshoots are not claims.
    #[must_use]
    pub fn claimed(&self) -> usize {
        self.next.load(Ordering::Relaxed).min(self.ranges.len())
    }

    /// How many ranges have never been claimed — the work a stop left on the table.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.ranges.len() - self.claimed()
    }

    /// The ranges no worker ever claimed, for the run report.
    #[must_use]
    pub fn unclaimed(&self) -> Vec<TokenRange> {
        self.ranges
            .get(self.claimed()..)
            .unwrap_or_default()
            .to_vec()
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
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use super::*;

    fn ranges(count: i128) -> Vec<TokenRange> {
        (0..count)
            .map(|i| TokenRange::new(i * 100, i * 100 + 99).unwrap())
            .collect()
    }

    #[test]
    fn eng_001_ranges_are_handed_out_in_plan_order() {
        let queue = WorkQueue::new(ranges(3));
        assert_eq!(queue.len(), 3);
        assert!(!queue.is_empty());
        assert_eq!(queue.claim().unwrap().min(), 0);
        assert_eq!(queue.claim().unwrap().min(), 100);
        assert_eq!(queue.claim().unwrap().min(), 200);
        assert_eq!(queue.claim(), None);
    }

    #[test]
    fn eng_001_an_exhausted_queue_keeps_returning_none() {
        let queue = WorkQueue::new(ranges(1));
        assert!(queue.claim().is_some());
        for _ in 0..10 {
            assert_eq!(queue.claim(), None);
        }
        // Overshooting the cursor must not inflate the progress figures.
        assert_eq!(queue.claimed(), 1);
        assert_eq!(queue.remaining(), 0);
        assert!(queue.unclaimed().is_empty());
    }

    #[test]
    fn eng_001_an_empty_plan_yields_nothing() {
        let queue = WorkQueue::new(Vec::new());
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
        assert_eq!(queue.claim(), None);
    }

    #[test]
    fn eng_014_the_unclaimed_remainder_is_the_plan_a_stop_left_behind() {
        let queue = WorkQueue::new(ranges(5));
        queue.claim();
        queue.claim();
        assert_eq!(queue.claimed(), 2);
        assert_eq!(queue.remaining(), 3);
        let left = queue.unclaimed();
        assert_eq!(left.len(), 3);
        assert_eq!(left[0].min(), 200);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn eng_001_every_range_is_claimed_exactly_once_across_many_workers() {
        let queue = Arc::new(WorkQueue::new(ranges(1_000)));
        let mut handles = Vec::new();
        for _ in 0..16 {
            let queue = Arc::clone(&queue);
            handles.push(tokio::spawn(async move {
                let mut mine = Vec::new();
                while let Some(range) = queue.claim() {
                    mine.push(range.min());
                }
                mine
            }));
        }

        let mut all = Vec::new();
        for handle in handles {
            all.extend(handle.await.unwrap());
        }

        assert_eq!(all.len(), 1_000, "no range may be claimed twice");
        assert_eq!(
            all.iter().copied().collect::<BTreeSet<_>>().len(),
            1_000,
            "no range may be missed"
        );
        assert_eq!(queue.remaining(), 0);
    }
}
