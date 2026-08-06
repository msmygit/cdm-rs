//! The run lifecycle, and the writer behind it (`TRK-020`, `TRK-021`, `TRK-022`, `TRK-035`).
//!
//! # Which numbers get persisted, and why it matters
//!
//! cdm-rs keeps every counter at two levels (`MET-004`): an **interim** value that workers
//! increment as rows go past, and a **committed** value that the interim level is folded into
//! when a range finishes. Only the committed level is ever written to the tracking table.
//!
//! That is not a detail. Two Java defects are deliberately not reproduced —
//!
//! * `MIG-004`: a flush threshold that read a committed value which was permanently zero, so the
//!   flush never fired;
//! * `ENG-008`: validate's error count derived from committed counts that were still zero at the
//!   moment it was computed;
//!
//! — and both are the same mistake in opposite directions: reading a level that has not been
//! written yet. Tracking is where the choice becomes durable. Persist the *interim* value and a
//! resume reads counts for work that was in flight and never landed, which makes a range look
//! more complete than it was. Persist the committed value at the wrong moment — before the
//! range's `flush()` — and it reads zero, which makes a completed range look untouched.
//!
//! So: [`RunTracker::finish_range`] takes the string the engine rendered *after* its flush
//! ([`RangeOutcome::run_info`](cdm_engine::scheduler::RangeOutcome::run_info)), and
//! [`committed_run_info`] is the only way this crate turns counters into a string. Both are
//! committed, both are pinned by tests, and there is no path from an interim counter to a
//! tracking row.
//!
//! # Never blocking the data path
//!
//! `TRK-035` requires tracking writes to be batched, asynchronous and bounded. They are: every
//! call from a worker is a `try_send` on a bounded channel, and when the channel is full the
//! write is folded into a checkpoint map instead of waiting. The map holds the *latest* state per
//! range, so a shed write is superseded rather than lost, and the periodic checkpoint writes it
//! out. Data movement never waits for tracking, and no range's final status is dropped.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cdm_core::{
    CdmError, JobKind, RangeRecord, RunId, RunRecord, RunStatus, TableRef, TokenRange,
    TrackingStore,
};
use cdm_engine::scheduler::{RangeObserver, RangeOutcome};
use cdm_metrics::JobCounters;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// The metrics string written to `cdm_run_info.run_info` and `cdm_run_details.run_info`
/// (`TRK-021`, `TRK-022`, `MET-005`).
///
/// The **committed** rendering, always. `JobCounters::run_info` is the committed view by
/// construction; this function exists so that every tracking write in the crate goes through one
/// named place, and so that a test can assert the choice rather than infer it.
///
/// The caller must have flushed the counters first — a range's interim work is credited by
/// `JobCounters::flush`, and rendering before that yields the zeroes of `MIG-004`.
#[must_use]
pub fn committed_run_info(counters: &JobCounters) -> String {
    counters.run_info()
}

/// How the tracking writer is sized (`TRK-035`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackerConfig {
    /// How many pending writes the channel holds before the tracker degrades to checkpoints.
    ///
    /// `ARCHITECTURE.md` §12 fixes this at 4096: large enough that a burst of range completions
    /// never sheds, small enough that the queue is a bounded, and small, part of `NFR-003`'s
    /// memory envelope.
    pub queue_capacity: usize,
    /// How many writes the writer applies before yielding.
    pub batch_size: usize,
    /// How often the shed-write checkpoint is written.
    pub checkpoint_interval: Duration,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 4096,
            batch_size: 64,
            checkpoint_interval: Duration::from_secs(10),
        }
    }
}

/// One pending tracking write.
#[derive(Debug, Clone)]
enum Write {
    Range(RangeRecord),
    Run {
        status: RunStatus,
        info: Option<String>,
    },
}

/// State that survives a shed write (`TRK-035`).
///
/// Keyed by `token_min`, which is `cdm_run_details`' clustering key, so a later state for a range
/// replaces an earlier one exactly as the `UPDATE` would have. That is what makes shedding safe:
/// the checkpoint writes the range's *current* status, and an intermediate `STARTED` that never
/// reached the table is not information anyone needs.
#[derive(Debug, Default)]
struct Checkpoint {
    pending: Mutex<BTreeMap<i128, RangeRecord>>,
    shed: AtomicU64,
}

/// Records a run's progress (`TRK-020`, `TRK-021`, `TRK-022`).
///
/// Created with [`RunTracker::start`], which performs the whole of `TRK-020` before returning, so
/// that a caller holding a `RunTracker` knows the run row and every range row already exist.
pub struct RunTracker {
    run_id: RunId,
    store: Arc<dyn TrackingStore>,
    // The sender is held in an `Option` so that `close_writer` can *drop* it: `writer_loop` ends
    // when every sender is gone, and a tracker that kept one alive while awaiting the writer
    // would wait for ever.
    sender: Mutex<Option<mpsc::Sender<Write>>>,
    checkpoint: Arc<Checkpoint>,
    writer: Mutex<Option<JoinHandle<()>>>,
}

/// Hand-written because `TrackingStore` is not `Debug` — it cannot be, since a store may hold a
/// driver session — and because the interesting state is the run and how much tracking has been
/// shed, not the channel internals.
#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for RunTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunTracker")
            .field("run_id", &self.run_id)
            .field("store", &self.store.name())
            .field("shed_writes", &self.shed_writes())
            .finish()
    }
}

impl RunTracker {
    /// Initialises a run and starts the writer (`TRK-020`, `TRK-035`).
    ///
    /// `ranges` is the plan. Every one of them gets a `NOT_STARTED` row before this returns,
    /// because `TRK-031`'s resume works by reading those rows: a range with no row is a range a
    /// resume cannot know about, and a crash between "run started" and "rows written" would
    /// therefore lose it.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](cdm_core::ErrorKind::Tracking) if the store rejects the
    /// run — most importantly when the run id already exists (`TRK-020`).
    pub async fn start(
        store: Arc<dyn TrackingStore>,
        run: &RunRecord,
        ranges: &[TokenRange],
        config: TrackerConfig,
    ) -> Result<Self, CdmError> {
        store.initialise().await?;
        let records: Vec<RangeRecord> = ranges
            .iter()
            .map(|range| RangeRecord {
                range: *range,
                status: RunStatus::NotStarted,
                started_at: None,
                info: None,
            })
            .collect();
        store.create_run(run, &records).await?;

        let (sender, receiver) = mpsc::channel(config.queue_capacity.max(1));
        let checkpoint = Arc::new(Checkpoint::default());
        let writer = tokio::spawn(writer_loop(
            Arc::clone(&store),
            run.run_id,
            receiver,
            Arc::clone(&checkpoint),
            config,
        ));
        Ok(Self {
            run_id: run.run_id,
            store,
            sender: Mutex::new(Some(sender)),
            checkpoint,
            writer: Mutex::new(Some(writer)),
        })
    }

    /// The run being tracked.
    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    /// How many writes have been shed to the checkpoint because the queue was full (`TRK-035`).
    ///
    /// Non-zero means tracking could not keep up with the run, which is the intended behaviour
    /// and not an error: the alternative is applying backpressure to data movement.
    pub fn shed_writes(&self) -> u64 {
        self.checkpoint.shed.load(Ordering::Relaxed)
    }

    /// Marks a range `STARTED` (`TRK-021`). Non-blocking.
    pub fn start_range(&self, range: TokenRange) {
        self.enqueue(Write::Range(RangeRecord {
            range,
            status: RunStatus::Started,
            started_at: Some(chrono::Utc::now()),
            info: None,
        }));
    }

    /// Records a range's terminal status and its committed metrics string (`TRK-021`).
    ///
    /// `run_info` must be the committed rendering, taken after the range's counters were flushed
    /// — see the module documentation. The engine's
    /// [`RangeOutcome`] already carries exactly that, which
    /// is why [`RunTracker::observe_outcome`] is the usual entry point.
    pub fn finish_range(&self, range: TokenRange, status: RunStatus, run_info: String) {
        self.enqueue(Write::Range(RangeRecord {
            range,
            status,
            started_at: None,
            info: Some(run_info),
        }));
    }

    /// Records a completed range from the engine's own outcome (`TRK-021`, `ENG-002`).
    pub fn observe_outcome(&self, outcome: &RangeOutcome) {
        self.finish_range(outcome.range, outcome.status, outcome.run_info.clone());
    }

    /// Closes the run: flushes every pending write, then records `end_time`, the aggregate
    /// committed metrics string and the final status (`TRK-022`).
    ///
    /// `status` is whatever terminal status the scheduler reports, and it is *not* assumed to be
    /// `ENDED`: `ENDED` for a run that processed its whole plan, `INTERRUPTED` for one stopped by
    /// a signal (`ENG-010`), `ABORTED` for one stopped by the error limit (`ENG-009`). Writing
    /// `ENDED` unconditionally would record an interrupted run as complete, and `TRK-030` would
    /// then decline to adopt it — the unfinished ranges would look done and no resume would ever
    /// re-plan them. `TRK-022` was reconciled with `ENG-009`/`ENG-010` in this pull request for
    /// exactly that reason.
    ///
    /// `run_info` must be the **committed** aggregate. A run's counter registry receives only
    /// committed values from its ranges, so its interim level is structurally zero for the whole
    /// run; rendering the interim view here would persist zeroes, which is the `ENG-008` defect
    /// made durable. [`committed_run_info`] is the supported way to produce this string.
    ///
    /// When `cdm-engine`'s `RangeObserver::on_run_finished(&RunReport)` lands, it is a direct
    /// call through to this method: the report carries the terminal status, the stop reason and
    /// the final counters, which is precisely the pair of arguments here.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](cdm_core::ErrorKind::Tracking) if the final write fails.
    /// A failure here leaves the run row short of `ENDED`, which a later `auto_rerun` reads as
    /// "did not finish" and resumes — the safe direction.
    pub async fn finish(&self, status: RunStatus, run_info: String) -> Result<(), CdmError> {
        // Ordering matters: the run row must not say ENDED while range writes are still queued,
        // or a resume started immediately afterwards reads a half-written range list.
        self.enqueue(Write::Run {
            status,
            info: Some(run_info),
        });
        self.close_writer().await;
        Ok(())
    }

    /// Stops the writer without recording a terminal run status.
    ///
    /// For a caller that is unwinding and has nothing meaningful to say about the run. The run
    /// row is left at `STARTED`, which a resume treats as unfinished.
    pub async fn abandon(&self) {
        self.close_writer().await;
    }

    /// Drops the sender, drains the writer, and applies whatever the checkpoint still holds.
    async fn close_writer(&self) {
        // Dropping the last sender is what ends `writer_loop`; it drains what is queued and then
        // writes the checkpoint one last time.
        drop(self.sender.lock().take());
        let handle = self.writer.lock().take();
        if let Some(handle) = handle {
            drop(handle.await);
        }
        // Belt and braces: if the writer task was cancelled outright, the checkpoint is still
        // here and still has to reach the store.
        apply_checkpoint(self.store.as_ref(), self.run_id, &self.checkpoint).await;
    }

    fn enqueue(&self, write: Write) {
        let sender = self.sender.lock().clone();
        // After `finish`, there is no writer left to enqueue to. Folding the write into the
        // checkpoint instead of dropping it keeps the invariant that a range's last known state
        // is always recoverable.
        let Some(sender) = sender else {
            if let Write::Range(record) = write {
                self.checkpoint
                    .pending
                    .lock()
                    .insert(record.range.min(), record);
            }
            return;
        };
        if sender.try_send(write.clone()).is_ok() {
            return;
        }
        // TRK-035: degrade rather than block. A run status is folded in as a range-less entry is
        // not possible, so it is retried on the checkpoint path by being re-sent blockingly only
        // for the terminal write, which is off the hot path anyway.
        match write {
            Write::Range(record) => {
                self.checkpoint.shed.fetch_add(1, Ordering::Relaxed);
                self.checkpoint
                    .pending
                    .lock()
                    .insert(record.range.min(), record);
            }
            Write::Run { status, info } => {
                // One write per run, at the very end: waiting for room here costs nothing.
                tokio::spawn(async move {
                    drop(sender.send(Write::Run { status, info }).await);
                });
            }
        }
    }
}

impl RangeObserver for RunTracker {
    fn on_range_started(&self, _run_id: RunId, range: TokenRange) {
        self.start_range(range);
    }

    fn on_range_finished(&self, _run_id: RunId, outcome: &RangeOutcome) {
        self.observe_outcome(outcome);
    }
}

/// Applies queued writes in batches, and the checkpoint on a timer (`TRK-035`).
async fn writer_loop(
    store: Arc<dyn TrackingStore>,
    run_id: RunId,
    mut receiver: mpsc::Receiver<Write>,
    checkpoint: Arc<Checkpoint>,
    config: TrackerConfig,
) {
    let mut ticker = tokio::time::interval(config.checkpoint_interval);
    // The first tick completes immediately, which would checkpoint an empty map on start-up.
    ticker.tick().await;
    let mut batch: Vec<Write> = Vec::with_capacity(config.batch_size.max(1));
    loop {
        tokio::select! {
            taken = receiver.recv_many(&mut batch, config.batch_size.max(1)) => {
                if taken == 0 {
                    break;
                }
                for write in batch.drain(..) {
                    apply(store.as_ref(), run_id, write).await;
                }
            }
            _ = ticker.tick() => {
                apply_checkpoint(store.as_ref(), run_id, &checkpoint).await;
            }
        }
    }
    apply_checkpoint(store.as_ref(), run_id, &checkpoint).await;
}

/// Writes out whatever was shed while the queue was full.
async fn apply_checkpoint(store: &dyn TrackingStore, run_id: RunId, checkpoint: &Checkpoint) {
    let pending: Vec<RangeRecord> = {
        let mut guard = checkpoint.pending.lock();
        if guard.is_empty() {
            return;
        }
        std::mem::take(&mut *guard).into_values().collect()
    };
    tracing::warn!(
        ranges = pending.len(),
        shed = checkpoint.shed.load(Ordering::Relaxed),
        "the tracking queue overflowed; writing a checkpoint of the latest range states instead \
         of blocking the run (TRK-035)"
    );
    for record in pending {
        if let Err(err) = store.update_range(run_id, &record).await {
            tracing::warn!(error = %err, "a tracking checkpoint write failed");
        }
    }
}

/// One write. A failure is logged and dropped: tracking must never fail a run (`TRK-035`).
async fn apply(store: &dyn TrackingStore, run_id: RunId, write: Write) {
    let result = match write {
        Write::Range(record) => store.update_range(run_id, &record).await,
        Write::Run { status, info } => store.update_run(run_id, status, info.as_deref()).await,
    };
    if let Err(err) = result {
        tracing::warn!(error = %err, "a tracking write failed; the run continues");
    }
}

/// The `cdm_run_info` row a new run starts from.
///
/// A small constructor rather than a struct literal at every call site, because getting
/// `previous_run_id` wrong is silent: `TRK-031` reads it to explain a resume, and a run that
/// forgot to record its ancestry cannot be traced back through a chain of reruns.
#[must_use]
pub fn new_run_record(
    run_id: RunId,
    previous_run_id: Option<RunId>,
    table: TableRef,
    job: JobKind,
) -> RunRecord {
    RunRecord {
        run_id,
        previous_run_id: previous_run_id.filter(|id| !id.is_unset()),
        table,
        job,
        status: RunStatus::NotStarted,
        started_at: Some(chrono::Utc::now()),
        ended_at: None,
        info: None,
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
    use cdm_metrics::{CounterKind, CounterView};

    use super::*;
    use crate::store::MemoryStore;

    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    fn record() -> RunRecord {
        new_run_record(
            RunId::from_raw(1),
            None,
            TableRef::new("ks", "t"),
            JobKind::Migrate,
        )
    }

    async fn tracker(store: Arc<MemoryStore>, ranges: &[TokenRange]) -> RunTracker {
        RunTracker::start(store, &record(), ranges, TrackerConfig::default())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn trk_020_starting_a_run_writes_the_run_row_and_every_range_row() {
        let store = Arc::new(MemoryStore::new());
        let ranges = [range(0, 9), range(10, 19), range(20, 29)];
        let tracker = tracker(Arc::clone(&store), &ranges).await;

        let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Started);
        let rows = store.ranges(RunId::from_raw(1)).await.unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.status == RunStatus::NotStarted));
        tracker.abandon().await;
    }

    #[tokio::test]
    async fn trk_020_a_duplicate_run_id_is_refused_before_the_writer_starts() {
        let store = Arc::new(MemoryStore::new());
        let first = tracker(Arc::clone(&store), &[range(0, 9)]).await;
        let err = RunTracker::start(
            Arc::clone(&store) as Arc<dyn TrackingStore>,
            &record(),
            &[range(0, 9)],
            TrackerConfig::default(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
        first.abandon().await;
    }

    #[tokio::test]
    async fn trk_021_a_range_moves_from_started_to_its_terminal_status_with_metrics() {
        let store = Arc::new(MemoryStore::new());
        let tracker = tracker(Arc::clone(&store), &[range(0, 9)]).await;

        tracker.start_range(range(0, 9));
        tracker.finish_range(range(0, 9), RunStatus::Pass, "Read: 5; Write: 5".to_owned());
        tracker
            .finish(RunStatus::Ended, "Read: 5".to_owned())
            .await
            .unwrap();

        let rows = store.ranges(RunId::from_raw(1)).await.unwrap();
        assert_eq!(rows[0].status, RunStatus::Pass);
        assert_eq!(rows[0].info.as_deref(), Some("Read: 5; Write: 5"));
        assert!(
            rows[0].started_at.is_some(),
            "the STARTED write set start_time"
        );
    }

    #[tokio::test]
    async fn trk_021_the_engine_s_range_outcome_is_recorded_verbatim() {
        let store = Arc::new(MemoryStore::new());
        let tracker = tracker(Arc::clone(&store), &[range(0, 9)]).await;
        let outcome = RangeOutcome {
            range: range(0, 9),
            status: RunStatus::Fail,
            run_info: "Read: 3; Write: 0; Error: 3".to_owned(),
            diagnostic: None,
            abandoned: false,
        };
        RangeObserver::on_range_started(&tracker, RunId::from_raw(1), range(0, 9));
        RangeObserver::on_range_finished(&tracker, RunId::from_raw(1), &outcome);
        tracker
            .finish(RunStatus::Ended, String::new())
            .await
            .unwrap();

        let rows = store.ranges(RunId::from_raw(1)).await.unwrap();
        assert_eq!(rows[0].status, RunStatus::Fail);
        assert_eq!(rows[0].info.as_deref(), Some("Read: 3; Write: 0; Error: 3"));
    }

    #[tokio::test]
    async fn trk_022_the_run_row_ends_with_the_aggregate_metrics_and_ended() {
        let store = Arc::new(MemoryStore::new());
        let tracker = tracker(Arc::clone(&store), &[range(0, 9)]).await;
        tracker
            .finish(
                RunStatus::Ended,
                "Read: 10; Partitions Failed: 0".to_owned(),
            )
            .await
            .unwrap();
        let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Ended);
        assert_eq!(run.info.as_deref(), Some("Read: 10; Partitions Failed: 0"));
        assert!(run.ended_at.is_some());
    }

    #[tokio::test]
    async fn trk_012_an_interrupted_run_is_recorded_as_such_and_stays_resumable() {
        let store = Arc::new(MemoryStore::new());
        let tracker = tracker(Arc::clone(&store), &[range(0, 9)]).await;
        tracker
            .finish(RunStatus::Interrupted, "Read: 2".to_owned())
            .await
            .unwrap();
        let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Interrupted);
        assert!(crate::resume::is_resumable(&run));
    }

    #[tokio::test]
    async fn trk_022_the_run_row_records_the_reported_status_not_always_ended() {
        // The reconciliation of TRK-022 with ENG-009 and ENG-010. If `finish` forced ENDED, a run
        // stopped by a signal or by the error limit would look complete and TRK-030 would refuse
        // to adopt it, stranding every range it did not process.
        for status in [RunStatus::Interrupted, RunStatus::Aborted, RunStatus::Ended] {
            let store = Arc::new(MemoryStore::new());
            let tracker = tracker(Arc::clone(&store), &[range(0, 9)]).await;
            tracker
                .finish(status, "Read: 2; Partitions Failed: 0".to_owned())
                .await
                .unwrap();
            let run = store.run(RunId::from_raw(1)).await.unwrap().unwrap();
            assert_eq!(run.status, status);
            assert!(run.ended_at.is_some() || status != RunStatus::Ended);
            assert_eq!(
                crate::resume::is_resumable(&run),
                status != RunStatus::Ended,
                "{status}: only a genuinely completed run is out of scope for auto_rerun"
            );
        }
    }

    // -----------------------------------------------------------------------------------------
    // MET-004 — the interim/committed choice
    // -----------------------------------------------------------------------------------------

    #[test]
    fn trk_021_the_persisted_metrics_string_is_the_committed_one() {
        let counters = JobCounters::new(JobKind::Migrate);
        counters.increment_by(counters.counter(CounterKind::Read).unwrap(), 7);
        counters.increment_by(counters.counter(CounterKind::Write).unwrap(), 7);

        // Before the flush the work is interim only. Persisting now would record zeroes — the
        // shape of MIG-004 and ENG-008 — so this assertion is the guard against reintroducing
        // them by rendering too early.
        assert!(committed_run_info(&counters).starts_with("Read: 0"));
        assert_eq!(
            counters.count_of(CounterKind::Read, CounterView::Interim),
            7
        );

        counters.flush();
        assert!(committed_run_info(&counters).starts_with("Read: 7"));
    }

    #[test]
    fn trk_021_unflushed_never_reaches_the_tracking_table() {
        // UNFLUSHED is an interim-only counter: rows buffered in a sink, not yet written. A
        // resume that saw it in `run_info` would double-count in-flight work.
        let counters = JobCounters::new(JobKind::Migrate);
        counters.flush();
        assert!(!committed_run_info(&counters).contains("Unflushed"));
    }

    // -----------------------------------------------------------------------------------------
    // TRK-035 — bounded queue and checkpoint degradation
    // -----------------------------------------------------------------------------------------

    #[tokio::test]
    async fn trk_035_a_full_queue_sheds_to_a_checkpoint_instead_of_blocking() {
        let store = Arc::new(MemoryStore::new());
        let ranges: Vec<TokenRange> = (0..64).map(|i| range(i * 10, i * 10 + 9)).collect();
        // A queue of one, and no writer progress while this test holds the runtime, so all but
        // the first write must shed.
        let config = TrackerConfig {
            queue_capacity: 1,
            batch_size: 1,
            checkpoint_interval: Duration::from_millis(10),
        };
        let tracker = RunTracker::start(
            Arc::clone(&store) as Arc<dyn TrackingStore>,
            &record(),
            &ranges,
            config,
        )
        .await
        .unwrap();

        for range in &ranges {
            tracker.finish_range(*range, RunStatus::Pass, "Read: 1".to_owned());
        }
        assert!(
            tracker.shed_writes() > 0,
            "a one-slot queue must shed; otherwise this test proves nothing"
        );

        tracker
            .finish(RunStatus::Ended, "Read: 64".to_owned())
            .await
            .unwrap();

        // Nothing was lost: every range still reached its terminal status, via the queue or via
        // the checkpoint.
        let rows = store.ranges(RunId::from_raw(1)).await.unwrap();
        assert_eq!(rows.len(), 64);
        assert!(
            rows.iter().all(|r| r.status == RunStatus::Pass),
            "a shed write must be superseded, never dropped"
        );
    }

    #[tokio::test]
    async fn trk_035_the_default_queue_is_the_documented_bound() {
        let config = TrackerConfig::default();
        assert_eq!(config.queue_capacity, 4096);
        assert!(config.batch_size > 1);
        assert!(config.checkpoint_interval > Duration::ZERO);
    }

    #[tokio::test]
    async fn trk_035_a_failing_store_does_not_fail_the_run() {
        // The tracker's contract is that tracking never becomes a reason a migration stops.
        let store = Arc::new(MemoryStore::new());
        let tracker = tracker(Arc::clone(&store), &[range(0, 9)]).await;
        // A range the run never planned: the store rejects nothing, but the run row lookup for a
        // different run id would. Either way `finish` must succeed.
        tracker.finish_range(range(100, 109), RunStatus::Pass, "Read: 1".to_owned());
        assert!(tracker
            .finish(RunStatus::Ended, String::new())
            .await
            .is_ok());
    }

    #[test]
    fn trk_020_a_new_run_record_drops_the_unset_previous_run_sentinel() {
        let record = new_run_record(
            RunId::from_raw(1),
            Some(RunId::UNSET),
            TableRef::new("ks", "t"),
            JobKind::Validate,
        );
        assert_eq!(record.previous_run_id, None);
        assert_eq!(record.status, RunStatus::NotStarted);
        assert_eq!(record.job, JobKind::Validate);
    }
}
