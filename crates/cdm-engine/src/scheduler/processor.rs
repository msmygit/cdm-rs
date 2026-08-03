//! The seam between the scheduler and a job (`ENG-001`, `ENG-002`, `ENG-008`).
//!
//! The scheduler knows how to claim a range, pace it, bound it, isolate its failures and account
//! for them. It knows nothing about CQL. Everything a *job* does — migrate (`MIG`), validate
//! (`VAL`), guardrail (`GRD`) — arrives through [`RangeProcessor`]:
//!
//! ```text
//!   Scheduler                                       RangeProcessor
//!   ─────────                                       ──────────────
//!   claim range          ──►  on_range_started  ──► (observer)
//!   build RangeContext   ──►  process(&ctx)     ──► read, transform, write,
//!   catch panic          ◄──  Ok(verdict) | Err     incrementing ctx.counters()
//!   account, flush, merge ─►  on_range_finished ──► (observer)
//! ```
//!
//! The contract is deliberately narrow, because everything outside it is the scheduler's job and
//! must not be re-implemented per job — which is precisely how Java CDM ended up with the
//! `ENG-008` accounting bug in `DiffJobSession` but not in `CopyJobSession`: the failure path is
//! copy-pasted per job there, and one copy is wrong. Here there is one copy.
//!
//! # What a processor must do
//!
//! * Increment [`RangeContext::counters`] as it works, at the **interim** level (`MET-004`). The
//!   scheduler flushes and merges; a processor that flushes on its own would double-count.
//! * Acquire a rate-limit permit before each row it reads and each row it writes
//!   (`ENG-004`, `ENG-005`), and an in-flight slot around each outstanding request (`ENG-007`).
//! * Return `Err` for anything that fails the range. It must **not** try to contain a range
//!   failure itself; `ENG-008` accounting happens in exactly one place.
//! * Notice [`RangeContext::is_cancelled`] and wind down when it is set (`ENG-010`). A processor
//!   that ignores it is dropped instead, so shutdown is bounded either way.
//!
//! # What a processor must not do
//!
//! Panic. It may — `ENG-013` catches it at the range boundary and turns it into a range failure —
//! but a panic loses the range's diagnostics and is never the intended path.

use std::sync::Arc;

use cdm_core::{CdmError, Diagnostic, JobKind, RunId, RunStatus, TokenRange};
use cdm_metrics::JobCounters;
use tokio_util::sync::CancellationToken;

use crate::scheduler::limits::{InflightPermit, RuntimeLimits};

/// How a range ended when it did not fail (`ENG-002`).
///
/// Failure is not a variant: it arrives as `Err` from [`RangeProcessor::process`], because the
/// error itself is what `ENG-008` logs and what the tracking row records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RangeVerdict {
    /// Migrated, or validated with no differences.
    #[default]
    Pass,
    /// Validation found differences that were not corrected.
    Diff,
    /// Validation found differences and corrected all of them.
    DiffCorrected,
}

impl RangeVerdict {
    /// The tracking status this verdict is recorded as (`ENG-002`, `TRK-012`).
    #[must_use]
    pub const fn status(self) -> RunStatus {
        match self {
            Self::Pass => RunStatus::Pass,
            Self::Diff => RunStatus::Diff,
            Self::DiffCorrected => RunStatus::DiffCorrected,
        }
    }
}

/// Everything one range's processing is given (`ENG-004`, `ENG-007`, `ENG-010`, `ENG-011`).
///
/// Built by the scheduler, borrowed by the processor, dropped when the range ends. It is
/// `Send + Sync`, so a processor may hand it to as many concurrent sub-tasks as it likes; the
/// counters and the limits are shared, which is the point.
#[derive(Debug)]
pub struct RangeContext {
    run_id: RunId,
    node_id: Arc<str>,
    range: TokenRange,
    fetch_size: u32,
    counters: Arc<JobCounters>,
    limits: Arc<RuntimeLimits>,
    cancel: CancellationToken,
}

impl RangeContext {
    /// Assembles a context. The scheduler is the only caller; jobs receive one, never build one.
    pub(crate) const fn new(
        run_id: RunId,
        node_id: Arc<str>,
        range: TokenRange,
        fetch_size: u32,
        counters: Arc<JobCounters>,
        limits: Arc<RuntimeLimits>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            run_id,
            node_id,
            range,
            fetch_size,
            counters,
            limits,
            cancel,
        }
    }

    /// The run this range belongs to.
    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    /// The node processing it (`ENG-011`).
    #[must_use]
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// The tokens to process.
    #[must_use]
    pub const fn range(&self) -> TokenRange {
        self.range
    }

    /// The origin page size, in rows (`ENG-003`).
    #[must_use]
    pub const fn fetch_size(&self) -> u32 {
        self.fetch_size
    }

    /// This range's counters, at the interim level (`MET-004`).
    ///
    /// The scheduler flushes them and merges them into the run's totals when the range ends.
    #[must_use]
    pub fn counters(&self) -> &Arc<JobCounters> {
        &self.counters
    }

    /// The run's rate limiters and in-flight semaphores (`ENG-004`, `ENG-007`).
    #[must_use]
    pub fn limits(&self) -> &Arc<RuntimeLimits> {
        &self.limits
    }

    /// Whether in-flight work has been abandoned and this range should wind down (`ENG-010`).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Resolves when in-flight work is abandoned, for a `select!` inside a job (`ENG-010`).
    pub async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }

    /// The cancellation token itself, for a job that spawns sub-tasks.
    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Waits until `rows` more rows may be read from the origin (`ENG-004`, `ENG-005`).
    pub async fn acquire_read_rows(&self, rows: u32) {
        self.limits.acquire_read_rows(rows).await;
    }

    /// Waits until `rows` more rows may be written to the target (`ENG-004`, `ENG-005`).
    pub async fn acquire_write_rows(&self, rows: u32) {
        self.limits.acquire_write_rows(rows).await;
    }

    /// Claims one in-flight origin read slot, held until dropped (`ENG-007`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`](cdm_core::ErrorKind::Internal) if the semaphore has been closed,
    /// which the scheduler never does.
    pub async fn read_slot(&self) -> Result<InflightPermit, CdmError> {
        self.limits.read_slot().await
    }

    /// Claims one in-flight target write slot, held until dropped (`ENG-007`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`](cdm_core::ErrorKind::Internal) if the semaphore has been closed,
    /// which the scheduler never does.
    pub async fn write_slot(&self) -> Result<InflightPermit, CdmError> {
        self.limits.write_slot().await
    }
}

/// What a job does with one token range (`ENG-001`).
///
/// Implemented once per job kind: `MIG` in PR #21, `VAL` in #22, `GRD` in #24. The scheduler in
/// this crate is complete without any of them, and is tested against fault-injecting doubles.
#[async_trait::async_trait]
pub trait RangeProcessor: Send + Sync {
    /// Which job's counters this processor keeps, which fixes the counters it may use
    /// (`MET-002`) and how a failed range's `ERROR` term is computed (`ENG-008`).
    fn job(&self) -> JobKind;

    /// Processes one range to completion.
    ///
    /// # Errors
    ///
    /// Any error fails the range and only the range (`ENG-008`): the scheduler marks it `FAIL`,
    /// accounts for the rows that were lost, logs the error with the range bounds, and moves on.
    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError>;
}

/// How one range ended (`ENG-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeOutcome {
    /// The tokens processed.
    pub range: TokenRange,
    /// The status recorded for the range: `PASS`, `DIFF`, `DIFF_CORRECTED`, `FAIL`, or `STARTED`
    /// for a range abandoned by a shutdown that ran out of grace.
    pub status: RunStatus,
    /// The range's `run_info` string, rendered from the committed counters after the flush
    /// (`MET-004`, `MET-005`, `TRK-021`).
    pub run_info: String,
    /// Why it failed, when it did.
    pub diagnostic: Option<Diagnostic>,
    /// Whether shutdown abandoned it mid-flight (`ENG-010`). An abandoned range did not fail: it
    /// is left `STARTED`, which `TRK-031` counts as pending, so a resume re-plans it.
    pub abandoned: bool,
}

impl RangeOutcome {
    /// Whether the range reached a successful terminal state.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(
            self.status,
            RunStatus::Pass | RunStatus::Diff | RunStatus::DiffCorrected
        )
    }

    /// Whether the range failed (`ENG-008`).
    #[must_use]
    pub const fn is_failure(&self) -> bool {
        matches!(self.status, RunStatus::Fail)
    }
}

/// Notified as each range starts and finishes (`ENG-002`).
///
/// This is the seam the tracking store of `TRK-020` plugs into: `ENG-002` requires a range to be
/// marked `STARTED` *before* work begins and to reach a terminal status on completion, and those
/// two writes are the whole of the scheduler's interest in tracking. Implementations must be
/// cheap and non-blocking — `cdm-track` batches through a bounded channel
/// (`ARCHITECTURE.md` §12) rather than writing inline.
pub trait RangeObserver: Send + Sync {
    /// Called before a range's processing starts (`ENG-002`).
    fn on_range_started(&self, run_id: RunId, range: TokenRange);

    /// Called once a range has reached a terminal status and its counters are committed
    /// (`ENG-002`, `MET-004`).
    fn on_range_finished(&self, run_id: RunId, outcome: &RangeOutcome);
}

/// The observer used when nobody is watching — run tracking disabled, or an embedded run.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopObserver;

impl RangeObserver for NoopObserver {
    fn on_range_started(&self, _run_id: RunId, _range: TokenRange) {}
    fn on_range_finished(&self, _run_id: RunId, _outcome: &RangeOutcome) {}
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
    use crate::scheduler::settings::SchedulerSettings;

    use super::*;

    fn context(cancel: CancellationToken) -> RangeContext {
        let settings = SchedulerSettings::default().with_ratelimits(0, 0);
        RangeContext::new(
            RunId::from_raw(7),
            Arc::from("node-1"),
            TokenRange::new(-10, 10).unwrap(),
            250,
            Arc::new(JobCounters::new(JobKind::Migrate)),
            Arc::new(RuntimeLimits::new(&settings).unwrap()),
            cancel,
        )
    }

    #[test]
    fn eng_002_a_verdict_maps_to_the_status_the_tracking_row_carries() {
        assert_eq!(RangeVerdict::Pass.status(), RunStatus::Pass);
        assert_eq!(RangeVerdict::Diff.status(), RunStatus::Diff);
        assert_eq!(
            RangeVerdict::DiffCorrected.status(),
            RunStatus::DiffCorrected
        );
        assert_eq!(RangeVerdict::default(), RangeVerdict::Pass);
    }

    #[test]
    fn eng_002_an_outcome_reports_success_and_failure() {
        let base = RangeOutcome {
            range: TokenRange::new(0, 1).unwrap(),
            status: RunStatus::Pass,
            run_info: String::new(),
            diagnostic: None,
            abandoned: false,
        };
        assert!(base.is_success() && !base.is_failure());

        let failed = RangeOutcome {
            status: RunStatus::Fail,
            ..base.clone()
        };
        assert!(failed.is_failure() && !failed.is_success());

        let abandoned = RangeOutcome {
            status: RunStatus::Started,
            abandoned: true,
            ..base
        };
        assert!(!abandoned.is_success() && !abandoned.is_failure());
    }

    #[tokio::test]
    async fn eng_011_the_context_carries_the_range_identity_a_job_needs() {
        let ctx = context(CancellationToken::new());
        assert_eq!(ctx.run_id(), RunId::from_raw(7));
        assert_eq!(ctx.node_id(), "node-1");
        assert_eq!(ctx.range().min(), -10);
        assert_eq!(ctx.fetch_size(), 250);
        assert_eq!(ctx.counters().job(), JobKind::Migrate);
    }

    #[tokio::test]
    async fn eng_010_a_context_reports_and_awaits_cancellation() {
        let cancel = CancellationToken::new();
        let ctx = context(cancel.clone());
        assert!(!ctx.is_cancelled());
        cancel.cancel();
        assert!(ctx.is_cancelled());
        assert!(ctx.cancellation_token().is_cancelled());
        ctx.cancelled().await;
    }

    #[tokio::test]
    async fn eng_007_the_context_hands_out_rate_permits_and_in_flight_slots() {
        let ctx = context(CancellationToken::new());
        ctx.acquire_read_rows(1).await;
        ctx.acquire_write_rows(1).await;
        let read = ctx.read_slot().await.unwrap();
        let write = ctx.write_slot().await.unwrap();
        assert_eq!(
            ctx.limits().available_read_slots(),
            usize::try_from(SchedulerSettings::default().max_inflight_reads()).unwrap() - 1
        );
        drop((read, write));
        assert_eq!(
            ctx.limits().available_read_slots(),
            usize::try_from(SchedulerSettings::default().max_inflight_reads()).unwrap()
        );
    }

    #[test]
    fn eng_002_the_noop_observer_observes_nothing() {
        let observer = NoopObserver;
        observer.on_range_started(RunId::from_raw(1), TokenRange::new(0, 1).unwrap());
        observer.on_range_finished(
            RunId::from_raw(1),
            &RangeOutcome {
                range: TokenRange::new(0, 1).unwrap(),
                status: RunStatus::Pass,
                run_info: String::new(),
                diagnostic: None,
                abandoned: false,
            },
        );
    }
}
