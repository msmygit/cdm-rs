//! The lock-free counter registry (`MET-001`..`MET-004`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use cdm_core::{CdmError, ErrorKind, JobKind, MetricsSnapshot, RunId};
use chrono::{DateTime, Utc};

use crate::counter::{registered_counters, CounterKind};

/// Which of the two levels of `MET-004` a read or a rendering refers to.
///
/// Java expresses this as a `boolean interim` parameter; naming it removes the call sites that
/// read `getMetrics(true)` and leave the reader guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CounterView {
    /// Work accumulated since the last [`JobCounters::flush`] — the current range's contribution,
    /// not yet folded into the totals.
    Interim,
    /// Work folded into the totals by [`JobCounters::flush`]. This is what the final block and
    /// the `run_info` strings report.
    #[default]
    Committed,
}

/// Proof that a counter is registered for a job (`MET-003`).
///
/// The only way to obtain one is [`JobCounters::counter`], which fails if the job does not
/// register the counter. Every subsequent operation takes the token and is infallible, so the hot
/// path has no error branch and no failure mode: where Java throws `IllegalArgumentException` on
/// the millionth row, cdm-rs refuses to start.
///
/// The token is [`Copy`] and eight bytes at most; hold one per counter for the life of the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Counter {
    kind: CounterKind,
    job: JobKind,
}

impl Counter {
    /// Which counter this token refers to.
    pub const fn kind(self) -> CounterKind {
        self.kind
    }

    /// The job whose registry issued the token.
    pub const fn job(self) -> JobKind {
        self.job
    }
}

/// One counter's two levels: interim and committed (`MET-004`, Java's `CounterUnit`).
#[derive(Debug, Default)]
struct CounterUnit {
    interim: AtomicU64,
    committed: AtomicU64,
}

impl CounterUnit {
    /// Relaxed ordering throughout. Counters are independent of one another and of any other
    /// memory: nothing is published by incrementing one, and the only totals anybody reads are
    /// taken after the owning range has been joined, which establishes happens-before by itself.
    const ORDER: Ordering = Ordering::Relaxed;

    fn increment(&self, by: u64) {
        self.interim.fetch_add(by, Self::ORDER);
    }

    fn get(&self, view: CounterView) -> u64 {
        match view {
            CounterView::Interim => self.interim.load(Self::ORDER),
            CounterView::Committed => self.committed.load(Self::ORDER),
        }
    }

    /// Java's `CounterUnit.addToCount`: fold the interim value into the total and clear it. The
    /// swap makes the pair atomic per counter even when a flush races an increment; the increment
    /// is then simply attributed to the next flush.
    fn flush(&self) {
        let interim = self.interim.swap(0, Self::ORDER);
        if interim != 0 {
            self.committed.fetch_add(interim, Self::ORDER);
        }
    }

    /// Java's `CounterUnit.reset`: discard the interim value without crediting it.
    fn reset_interim(&self) {
        self.interim.store(0, Self::ORDER);
    }

    fn add_committed(&self, by: u64) {
        self.committed.fetch_add(by, Self::ORDER);
    }

    fn clear(&self) {
        self.interim.store(0, Self::ORDER);
        self.committed.store(0, Self::ORDER);
    }
}

/// The counters of one job, or of one range within it (`MET-001`, `MET-004`).
///
/// # Shape
///
/// Thirteen [`AtomicU64`] pairs in a fixed array indexed by [`CounterKind::index`]
/// (`ARCHITECTURE.md` §9). Incrementing is one relaxed `fetch_add` against a slot chosen at
/// compile time — no map lookup, no lock, no allocation. Which slots a job *uses* is decided by
/// `MET-002`; the unused slots cost 16 bytes each and are never read, which is cheaper than
/// making the array's length depend on the job.
///
/// # Two levels
///
/// Every counter has an *interim* and a *committed* value (`MET-004`). Workers increment the
/// interim level; [`JobCounters::flush`] folds it into the committed level when a range
/// completes. Java's structure is reproduced exactly: one `JobCounters` per range, whose
/// committed values are merged into the run's own `JobCounters` with [`JobCounters::add`] — the
/// job of `CDMMetricsAccumulator` in Spark.
///
/// # Concurrency
///
/// `JobCounters` is `Send + Sync` and every method takes `&self`, so a range's counters can be
/// shared by however many tasks process it. Put it in an `Arc` and clone the handle.
///
/// ```
/// use cdm_core::JobKind;
/// use cdm_metrics::{CounterKind, CounterView, JobCounters};
///
/// let range = JobCounters::new(JobKind::Migrate);
/// // Resolve the tokens once, at startup (MET-003).
/// let read = range.counter(CounterKind::Read)?;
/// let write = range.counter(CounterKind::Write)?;
///
/// range.increment_by(read, 10);
/// range.increment_by(write, 9);
/// assert_eq!(range.count(read, CounterView::Interim), 10);
/// assert_eq!(range.count(read, CounterView::Committed), 0);
///
/// range.flush();
/// assert_eq!(range.count(read, CounterView::Committed), 10);
/// assert_eq!(range.metrics(CounterView::Committed), "Read: 10; Write: 9; Skipped: 0; Error: 0; Partitions Passed: 0; Partitions Failed: 0");
///
/// // A counter this job does not register cannot be obtained at all.
/// assert!(range.counter(CounterKind::Mismatch).is_err());
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug)]
pub struct JobCounters {
    job: JobKind,
    units: [CounterUnit; CounterKind::COUNT],
}

impl JobCounters {
    /// Registers exactly the counters `MET-002` gives this job.
    #[must_use]
    pub fn new(job: JobKind) -> Self {
        Self {
            job,
            units: std::array::from_fn(|_| CounterUnit::default()),
        }
    }

    /// The job whose registration set these counters follow.
    pub const fn job(&self) -> JobKind {
        self.job
    }

    /// The counters this job registers, in rendering order (`MET-002`).
    pub const fn registered(&self) -> &'static [CounterKind] {
        registered_counters(self.job)
    }

    /// Whether this job registers `kind`.
    pub const fn is_registered(&self, kind: CounterKind) -> bool {
        kind.is_registered_for(self.job)
    }

    /// Resolves a counter to a token, once, at startup (`MET-003`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if this job does not register `kind`. Asking for a counter a job
    /// does not have is a programming error, and this is where it surfaces: before any data
    /// moves, instead of on an arbitrary row as it does in Java.
    pub fn counter(&self, kind: CounterKind) -> Result<Counter, CdmError> {
        if !kind.is_registered_for(self.job) {
            return Err(CdmError::new(
                ErrorKind::Internal,
                format!(
                    "counter {kind} is not registered for the {} job; it registers {}",
                    self.job,
                    self.registered()
                        .iter()
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        Ok(Counter {
            kind,
            job: self.job,
        })
    }

    /// Resolves every counter this job registers, in rendering order.
    ///
    /// A convenience for callers that hold on to all of them; it cannot fail, because it only
    /// asks for counters the job has.
    #[must_use]
    pub fn all_counters(&self) -> Vec<Counter> {
        self.registered()
            .iter()
            .map(|&kind| Counter {
                kind,
                job: self.job,
            })
            .collect()
    }

    /// Adds one to a counter's interim value. The hot path: one relaxed `fetch_add`.
    pub fn increment(&self, counter: Counter) {
        self.increment_by(counter, 1);
    }

    /// Adds `by` to a counter's interim value.
    pub fn increment_by(&self, counter: Counter, by: u64) {
        self.unit(counter.kind).increment(by);
    }

    /// Reads one level of one counter.
    #[must_use]
    pub fn count(&self, counter: Counter, view: CounterView) -> u64 {
        self.unit(counter.kind).get(view)
    }

    /// Reads one level of a counter by kind, reporting zero for a counter this job does not
    /// register — the shape [`MetricsSnapshot::counter`] and the REST API need.
    #[must_use]
    pub fn count_of(&self, kind: CounterKind, view: CounterView) -> u64 {
        if self.is_registered(kind) {
            self.unit(kind).get(view)
        } else {
            0
        }
    }

    /// Discards a counter's interim value without crediting it to the total.
    ///
    /// Java calls this `reset(CounterType)`, and uses it for exactly one thing: clearing
    /// `UNFLUSHED` once the buffered writes it counted have been flushed and credited to `WRITE`
    /// (`MIG-004`).
    pub fn reset(&self, counter: Counter) {
        self.unit(counter.kind).reset_interim();
    }

    /// Folds every interim value into its total and clears it (`MET-004`).
    ///
    /// Called once per range, when the range reaches a terminal state — after which
    /// [`JobCounters::metrics`] renders the range's `run_info` string (`TRK-021`) and
    /// [`JobCounters::add`] merges the range into the run.
    pub fn flush(&self) {
        for &kind in self.registered() {
            self.unit(kind).flush();
        }
    }

    /// Merges another set of counters' *committed* values into this one's (`MET-004`).
    ///
    /// This is Java's `JobCounter.add`, the operation behind `CDMMetricsAccumulator`: interim
    /// values are not merged, so the caller must [`JobCounters::flush`] the range first, exactly
    /// as `CopyJobSession` and `DiffJobSession` do.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the two sets belong to different jobs and therefore register
    /// different counters.
    pub fn add(&self, other: &Self) -> Result<(), CdmError> {
        if other.job != self.job {
            return Err(CdmError::new(
                ErrorKind::Internal,
                format!(
                    "cannot merge {} counters into {} counters",
                    other.job, self.job
                ),
            ));
        }
        for &kind in self.registered() {
            let value = other.unit(kind).get(CounterView::Committed);
            if value != 0 {
                self.unit(kind).add_committed(value);
            }
        }
        Ok(())
    }

    /// Whether no registered counter has recorded anything at either level.
    ///
    /// Java's `isZero`, which Spark uses to decide whether an accumulator is worth shipping.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.registered().iter().all(|&kind| {
            self.unit(kind).get(CounterView::Interim) == 0
                && self.unit(kind).get(CounterView::Committed) == 0
        })
    }

    /// Clears every registered counter at both levels.
    pub fn clear(&self) {
        for &kind in self.registered() {
            self.unit(kind).clear();
        }
    }

    /// A point-in-time view of the committed counters, for the exporters of `PLG-006`.
    ///
    /// Keys are the `SCREAMING_SNAKE_CASE` names of `MET-001`; only registered counters appear,
    /// and [`MetricsSnapshot::counter`] reports the rest as zero.
    #[must_use]
    pub fn snapshot(&self, run_id: RunId, taken_at: DateTime<Utc>) -> MetricsSnapshot {
        MetricsSnapshot {
            run_id,
            job: self.job,
            taken_at,
            counters: self
                .registered()
                .iter()
                .map(|&kind| {
                    (
                        kind.as_str().to_owned(),
                        self.unit(kind).get(CounterView::Committed),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        }
    }

    /// The atomic pair backing a counter.
    //
    // SAFETY-INVARIANT: `CounterKind::index` returns the variant's position in
    // `CounterKind::ALL`, which is `0..CounterKind::COUNT`, and `units` has exactly
    // `CounterKind::COUNT` elements. `met_001_every_kind_indexes_its_own_slot` proves the bound
    // for every variant, so the index can never be out of range and there is no failure to
    // report. Using `get()` here would put an `Option` on the hot path for an impossible case.
    #[allow(clippy::indexing_slicing)]
    fn unit(&self, kind: CounterKind) -> &CounterUnit {
        &self.units[kind.index()]
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
    use std::sync::Arc;
    use std::thread;

    use super::*;

    #[test]
    fn met_002_a_job_registers_exactly_its_own_counters() {
        for job in JobKind::ALL {
            let counters = JobCounters::new(job);
            assert_eq!(counters.job(), job);
            assert_eq!(counters.registered(), registered_counters(job));
            for kind in CounterKind::ALL {
                assert_eq!(
                    counters.counter(kind).is_ok(),
                    counters.is_registered(kind),
                    "{job}/{kind}"
                );
            }
            assert_eq!(counters.all_counters().len(), counters.registered().len());
        }
    }

    #[test]
    fn met_003_an_unregistered_counter_cannot_be_obtained_and_says_what_is_available() {
        let migrate = JobCounters::new(JobKind::Migrate);
        let err = migrate.counter(CounterKind::Mismatch).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        let message = err.to_string();
        assert!(message.contains("MISMATCH"), "{message}");
        assert!(message.contains("migrate"), "{message}");
        assert!(message.contains("READ, WRITE, SKIPPED"), "{message}");
    }

    #[test]
    fn met_003_the_error_is_fatal_so_it_cannot_be_swallowed_as_a_range_failure() {
        let err = JobCounters::new(JobKind::Guardrail)
            .counter(CounterKind::Error)
            .unwrap_err();
        assert!(err.kind().is_fatal());
        assert!(!err.kind().is_retryable());
    }

    #[test]
    fn met_003_a_token_carries_its_kind_and_its_job() {
        let counters = JobCounters::new(JobKind::Validate);
        let valid = counters.counter(CounterKind::Valid).unwrap();
        assert_eq!(valid.kind(), CounterKind::Valid);
        assert_eq!(valid.job(), JobKind::Validate);
    }

    #[test]
    fn met_004_increments_land_on_the_interim_level_until_flushed() {
        let counters = JobCounters::new(JobKind::Migrate);
        let read = counters.counter(CounterKind::Read).unwrap();

        counters.increment(read);
        counters.increment_by(read, 4);
        assert_eq!(counters.count(read, CounterView::Interim), 5);
        assert_eq!(counters.count(read, CounterView::Committed), 0);

        counters.flush();
        assert_eq!(counters.count(read, CounterView::Interim), 0);
        assert_eq!(counters.count(read, CounterView::Committed), 5);

        // A second range's worth of work accumulates on top.
        counters.increment_by(read, 2);
        counters.flush();
        assert_eq!(counters.count(read, CounterView::Committed), 7);
    }

    #[test]
    fn met_004_reset_discards_the_interim_value_without_crediting_it() {
        // Java's UNFLUSHED cycle (MIG-004): count buffered writes, credit them to WRITE on
        // flush, then reset UNFLUSHED so they are not counted twice.
        let counters = JobCounters::new(JobKind::Migrate);
        let write = counters.counter(CounterKind::Write).unwrap();
        let unflushed = counters.counter(CounterKind::Unflushed).unwrap();

        counters.increment_by(unflushed, 100);
        counters.increment_by(write, counters.count(unflushed, CounterView::Interim));
        counters.reset(unflushed);

        assert_eq!(counters.count(unflushed, CounterView::Interim), 0);
        counters.flush();
        assert_eq!(counters.count(unflushed, CounterView::Committed), 0);
        assert_eq!(counters.count(write, CounterView::Committed), 100);
    }

    #[test]
    fn met_004_ranges_fold_into_the_run_totals_on_completion() {
        let run = JobCounters::new(JobKind::Validate);
        let run_read = run.counter(CounterKind::Read).unwrap();
        let run_passed = run.counter(CounterKind::PartitionsPassed).unwrap();

        for rows in [3_u64, 5, 11] {
            let range = JobCounters::new(JobKind::Validate);
            let read = range.counter(CounterKind::Read).unwrap();
            let passed = range.counter(CounterKind::PartitionsPassed).unwrap();
            range.increment_by(read, rows);
            range.increment(passed);
            range.flush();
            run.add(&range).unwrap();
        }

        assert_eq!(run.count(run_read, CounterView::Committed), 19);
        assert_eq!(run.count(run_passed, CounterView::Committed), 3);
    }

    #[test]
    fn met_004_add_ignores_unflushed_interim_work_exactly_as_java_does() {
        let run = JobCounters::new(JobKind::Migrate);
        let range = JobCounters::new(JobKind::Migrate);
        let read = range.counter(CounterKind::Read).unwrap();
        range.increment_by(read, 42);

        run.add(&range).unwrap();
        assert_eq!(
            run.count(
                run.counter(CounterKind::Read).unwrap(),
                CounterView::Committed
            ),
            0,
            "a range that was never flushed contributes nothing"
        );
    }

    #[test]
    fn met_004_merging_across_jobs_is_rejected() {
        let migrate = JobCounters::new(JobKind::Migrate);
        let validate = JobCounters::new(JobKind::Validate);
        let err = migrate.add(&validate).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert!(err.to_string().contains("validate"));
    }

    #[test]
    fn met_001_counters_start_zero_and_can_be_cleared() {
        let counters = JobCounters::new(JobKind::Guardrail);
        assert!(counters.is_zero());

        let large = counters.counter(CounterKind::Large).unwrap();
        counters.increment(large);
        assert!(!counters.is_zero(), "interim work counts as non-zero");
        counters.flush();
        assert!(!counters.is_zero());

        counters.clear();
        assert!(counters.is_zero());
        assert_eq!(counters.count(large, CounterView::Committed), 0);
    }

    #[test]
    fn met_001_an_unregistered_counter_reads_as_zero_by_kind() {
        let counters = JobCounters::new(JobKind::Guardrail);
        assert_eq!(
            counters.count_of(CounterKind::Error, CounterView::Committed),
            0
        );
        let read = counters.counter(CounterKind::Read).unwrap();
        counters.increment_by(read, 6);
        assert_eq!(
            counters.count_of(CounterKind::Read, CounterView::Interim),
            6
        );
    }

    #[test]
    fn met_001_concurrent_increments_are_exact() {
        const THREADS: u64 = 16;
        const PER_THREAD: u64 = 25_000;

        let counters = Arc::new(JobCounters::new(JobKind::Migrate));
        let read = counters.counter(CounterKind::Read).unwrap();
        let write = counters.counter(CounterKind::Write).unwrap();

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let counters = Arc::clone(&counters);
                thread::spawn(move || {
                    for _ in 0..PER_THREAD {
                        counters.increment(read);
                        counters.increment_by(write, 2);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(
            counters.count(read, CounterView::Interim),
            THREADS * PER_THREAD
        );
        counters.flush();
        assert_eq!(
            counters.count(read, CounterView::Committed),
            THREADS * PER_THREAD
        );
        assert_eq!(
            counters.count(write, CounterView::Committed),
            2 * THREADS * PER_THREAD
        );
    }

    #[test]
    fn met_004_a_flush_racing_increments_loses_nothing() {
        const THREADS: u64 = 8;
        const PER_THREAD: u64 = 20_000;

        let counters = Arc::new(JobCounters::new(JobKind::Migrate));
        let read = counters.counter(CounterKind::Read).unwrap();

        let mut handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let counters = Arc::clone(&counters);
                thread::spawn(move || {
                    for _ in 0..PER_THREAD {
                        counters.increment(read);
                    }
                })
            })
            .collect();
        let flusher = {
            let counters = Arc::clone(&counters);
            thread::spawn(move || {
                for _ in 0..1_000 {
                    counters.flush();
                }
            })
        };
        handles.push(flusher);
        for handle in handles {
            handle.join().unwrap();
        }

        counters.flush();
        assert_eq!(
            counters.count(read, CounterView::Committed),
            THREADS * PER_THREAD,
            "interleaved flushes must neither lose nor duplicate an increment"
        );
        assert_eq!(counters.count(read, CounterView::Interim), 0);
    }

    #[test]
    fn plg_006_a_snapshot_holds_the_registered_counters_under_their_java_names() {
        let counters = JobCounters::new(JobKind::Guardrail);
        let large = counters.counter(CounterKind::Large).unwrap();
        counters.increment_by(large, 3);
        counters.flush();

        let snapshot = counters.snapshot(RunId::from_raw(7), DateTime::UNIX_EPOCH);
        assert_eq!(snapshot.run_id, RunId::from_raw(7));
        assert_eq!(snapshot.job, JobKind::Guardrail);
        assert_eq!(snapshot.counter("LARGE"), 3);
        assert_eq!(snapshot.counter("READ"), 0);
        // Counters guardrail does not register are absent, and read back as zero.
        assert!(!snapshot.counters.contains_key("WRITE"));
        assert_eq!(snapshot.counter("WRITE"), 0);
        assert_eq!(snapshot.counters.len(), 6);
    }

    #[test]
    fn met_004_the_default_view_is_the_committed_one() {
        assert_eq!(CounterView::default(), CounterView::Committed);
    }
}
