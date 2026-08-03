//! The execution scheduler (`ENG-001`..`ENG-014`).
//!
//! `perfops.workers` Tokio tasks pull ranges from a shared work list, process each to completion,
//! record its outcome, and repeat until the plan is exhausted or the run is stopped. There is no
//! Spark, no JVM and no external scheduler: a cdm-rs run is one process, and this module is its
//! main loop.
//!
//! ```text
//!                    TokenPlan (already split and shuffled — TOK-003, TOK-006)
//!                                    │
//!                              ┌─────▼─────┐
//!                              │ WorkQueue │  shared cursor, work stealing by construction
//!                              └─────┬─────┘
//!            ┌───────────────────────┼───────────────────────┐
//!       ┌────▼────┐             ┌────▼────┐             ┌────▼────┐
//!       │ worker 1│             │ worker 2│      ...    │ worker N│
//!       └────┬────┘             └────┬────┘             └────┬────┘
//!            │  RunControl: pause? stop? kill?  (ENG-009/010/014)
//!            │
//!            ▼   per range:  tracing span (ENG-011/012)
//!                            RangeContext: rate limits (ENG-004/005),
//!                                          in-flight semaphores (ENG-007)
//!                            RangeProcessor::process  ── panics caught (ENG-013)
//!                            accounting + flush + merge (ENG-002, ENG-008, MET-004)
//!                            error limit? (ENG-009)
//! ```
//!
//! # What this module is not
//!
//! It is not a job. Nothing here knows what a row is, what CQL is, or which cluster it is talking
//! to; the entire job surface is [`RangeProcessor`], one method wide. Migrate, validate and
//! guardrail arrive in PRs #21–#24 and change nothing here. That separation is not tidiness — it
//! is why the `ENG-008` accounting exists once instead of three times, which is the specific
//! shape of the bug Java CDM has in `DiffJobSession` but not in `CopyJobSession`.
//!
//! # Three levels of isolation
//!
//! `ARCHITECTURE.md` §13 names them, and this module implements the outer two:
//!
//! | Level | Meaning | Where |
//! |---|---|---|
//! | Record | one bad row does not fail a range | the job (`MIG`, `VAL`) |
//! | **Range** | one bad range does not fail the run | [`Scheduler`], `ENG-008`, `ENG-013` |
//! | **Run** | too many lost rows stops the run cleanly | [`Scheduler`], `ENG-009` |
//!
//! A range failure — an error, or a panic — is caught, accounted for, logged with the range
//! bounds, and left behind. Because the range is the unit of tracking (`ENG-002`), everything a
//! failed range touched is re-runnable.
//!
//! # Stopping
//!
//! Three things stop a run early, and they differ only in what the run row ends up saying:
//!
//! | Trigger | Requirement | In-flight ranges | Final status |
//! |---|---|---|---|
//! | `SIGINT` / `SIGTERM` | `ENG-010` | finish, within `shutdown_grace` | `INTERRUPTED` |
//! | Total `ERROR` > `error_limit` | `ENG-009` | finish, within `shutdown_grace` | `ABORTED` |
//! | Operator request | `ENG-014` | finish, within `shutdown_grace` | `ABORTED` |
//!
//! In every case workers stop *claiming* immediately and in-flight ranges drain. A second signal,
//! or the expiry of the grace period, abandons them: the cancellation token in every
//! [`RangeContext`] fires, and the scheduler stops waiting for a job that does not check it. That
//! last point is what makes the deadline real — a hung range cannot extend shutdown past
//! `shutdown_grace` no matter what it is doing.
//!
//! Pausing (`ENG-014`) is the same mechanism minus the finality: workers stop claiming, the plan
//! is untouched, and resuming carries on from the queue's cursor.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//!
//! use cdm_core::{CdmError, JobKind, RunId, TokenRange};
//! use cdm_engine::planner::{Planner, PlannerSettings, Partitioner};
//! use cdm_engine::scheduler::{
//!     NoopObserver, RangeContext, RangeProcessor, RangeVerdict, Scheduler, SchedulerSettings,
//! };
//! use cdm_metrics::CounterKind;
//!
//! /// A job that reads ten rows from every range and writes them all.
//! struct TenRows;
//!
//! #[async_trait::async_trait]
//! impl RangeProcessor for TenRows {
//!     fn job(&self) -> JobKind { JobKind::Migrate }
//!
//!     async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
//!         let read = ctx.counters().counter(CounterKind::Read)?;
//!         let write = ctx.counters().counter(CounterKind::Write)?;
//!         for _ in 0..10 {
//!             ctx.acquire_read_rows(1).await;          // ENG-004/005
//!             ctx.counters().increment(read);
//!             let _slot = ctx.write_slot().await?;     // ENG-007
//!             ctx.acquire_write_rows(1).await;
//!             ctx.counters().increment(write);
//!         }
//!         Ok(RangeVerdict::Pass)
//!     }
//! }
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), CdmError> {
//! let settings = PlannerSettings::new(Partitioner::Murmur3).with_num_parts(8);
//! let plan = Planner::new(settings).plan(RunId::from_raw(1), None)?;
//!
//! let scheduler = Scheduler::new(SchedulerSettings::default().with_workers(4))?;
//! let report = scheduler
//!     .run(&plan, Arc::new(TenRows), Arc::new(NoopObserver))
//!     .await?;
//!
//! assert_eq!(report.outcomes().len(), plan.len());
//! assert_eq!(report.counters().count_of(CounterKind::Read, cdm_metrics::CounterView::Committed), 80);
//! # Ok(())
//! # }
//! ```

pub mod control;
pub mod failure;
pub mod limits;
pub mod processor;
pub mod queue;
pub mod ratelimit;
pub mod settings;
pub mod signal;
pub mod span;

use std::any::Any;
use std::sync::Arc;

use cdm_core::{CdmError, ErrorKind, JobKind, RunId, RunStatus, TokenRange};
use cdm_metrics::{CounterKind, CounterView, JobCounters};
use futures::FutureExt;
use parking_lot::Mutex;
use tracing::Instrument;

pub use control::{RunControl, StopReason};
pub use limits::{InflightPermit, RuntimeLimits};
pub use processor::{
    NoopObserver, RangeContext, RangeObserver, RangeOutcome, RangeProcessor, RangeVerdict,
};
pub use queue::WorkQueue;
pub use ratelimit::RateLimiter;
pub use settings::{SchedulerSettings, DEFAULT_SHUTDOWN_GRACE};
pub use signal::spawn_signal_listener;
pub use span::{java_thread_label, range_span};

use crate::planner::TokenPlan;

/// The work-stealing range scheduler (`ENG-001`).
///
/// One per run. Its rate limiters and in-flight semaphores are built once, at construction, so
/// that a misconfigured bound is a startup error rather than a stall discovered an hour in.
#[derive(Debug)]
pub struct Scheduler {
    settings: SchedulerSettings,
    control: RunControl,
    limits: Arc<RuntimeLimits>,
}

impl Scheduler {
    /// Builds a scheduler.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if an in-flight bound is zero or beyond the runtime's maximum
    /// (`ENG-007`).
    pub fn new(settings: SchedulerSettings) -> Result<Self, CdmError> {
        let limits = Arc::new(RuntimeLimits::new(&settings)?);
        Ok(Self {
            settings,
            control: RunControl::new(),
            limits,
        })
    }

    /// The settings this scheduler resolved at construction.
    #[must_use]
    pub const fn settings(&self) -> &SchedulerSettings {
        &self.settings
    }

    /// A handle for pausing, resuming and stopping the run (`ENG-009`, `ENG-010`, `ENG-014`).
    #[must_use]
    pub fn control(&self) -> RunControl {
        self.control.clone()
    }

    /// The rate limiters and in-flight semaphores (`ENG-004`, `ENG-007`).
    #[must_use]
    pub fn limits(&self) -> &Arc<RuntimeLimits> {
        &self.limits
    }

    /// Runs a plan to completion, or until it is stopped (`ENG-001`).
    ///
    /// # Errors
    ///
    /// Only for failures of the scheduler itself. A failing range is not an error: `ENG-008`
    /// requires the run to continue, and the failure appears in the returned
    /// [`RunReport::outcomes`].
    pub async fn run(
        &self,
        plan: &TokenPlan,
        processor: Arc<dyn RangeProcessor>,
        observer: Arc<dyn RangeObserver>,
    ) -> Result<RunReport, CdmError> {
        let run_id = plan.run_id();
        let job = processor.job();
        let queue = Arc::new(WorkQueue::new(plan.token_ranges()));
        let shared = Arc::new(WorkerShared {
            run_id,
            node_id: Arc::from(self.settings.node_id()),
            settings: self.settings.clone(),
            control: self.control.clone(),
            limits: Arc::clone(&self.limits),
            queue: Arc::clone(&queue),
            processor,
            observer,
            run_counters: Arc::new(JobCounters::new(job)),
            outcomes: Arc::new(Mutex::new(Vec::with_capacity(plan.len()))),
        });

        tracing::info!(
            target: "cdm::engine",
            run_id = run_id.as_i64(),
            job = job.as_str(),
            ranges = plan.len(),
            workers = self.settings.workers(),
            node_id = self.settings.node_id(),
            "starting the range scheduler"
        );

        // ENG-010: the grace period is armed by the stop, not by the scheduler's start, so a run
        // that is never stopped never sleeps on it.
        let watchdog = tokio::spawn(grace_watchdog(
            self.control.clone(),
            self.settings.shutdown_grace(),
        ));

        let workers = usize::try_from(self.settings.workers())
            .unwrap_or(usize::MAX)
            .max(1);
        let mut handles = Vec::with_capacity(workers);
        for index in 0..workers {
            handles.push(tokio::spawn(worker(Arc::clone(&shared), index)));
        }
        for handle in handles {
            // A worker task cannot panic: `ENG-013` catches the processor's panics inside it, and
            // everything else it does is infallible. If one somehow did, the remaining workers
            // must still be joined, so the run is not left half-finished.
            if let Err(join) = handle.await {
                tracing::error!(
                    target: "cdm::engine",
                    run_id = run_id.as_i64(),
                    error = %join,
                    "a scheduler worker task ended abnormally"
                );
            }
        }
        watchdog.abort();

        let stopped_by = self.control.stop_reason();
        let status = stopped_by.map_or(RunStatus::Ended, StopReason::run_status);
        let mut outcomes = std::mem::take(&mut *shared.outcomes.lock());
        // Workers finish in whatever order the runtime gives them; ring order makes the report
        // reproducible and matches how `cdm plan` and the tracking table render a plan.
        outcomes.sort_by_key(|outcome| outcome.range);

        let run_counters =
            Arc::try_unwrap(Arc::clone(&shared.run_counters)).unwrap_or_else(|shared| {
                // Every worker has been joined, so the only surviving reference is ours; the
                // fallback merges into a fresh registry rather than reporting an impossible
                // failure.
                let owned = JobCounters::new(shared.job());
                let _ = owned.add(&shared);
                owned
            });

        tracing::info!(
            target: "cdm::engine",
            run_id = run_id.as_i64(),
            status = status.as_str(),
            ranges_completed = outcomes.len(),
            ranges_unclaimed = queue.remaining(),
            "the range scheduler finished"
        );

        Ok(RunReport {
            run_id,
            job,
            status,
            stopped_by,
            counters: run_counters,
            outcomes,
            unclaimed: queue.unclaimed(),
        })
    }
}

/// Everything the workers of one run share.
struct WorkerShared {
    run_id: RunId,
    node_id: Arc<str>,
    settings: SchedulerSettings,
    control: RunControl,
    limits: Arc<RuntimeLimits>,
    queue: Arc<WorkQueue>,
    processor: Arc<dyn RangeProcessor>,
    observer: Arc<dyn RangeObserver>,
    run_counters: Arc<JobCounters>,
    outcomes: Arc<Mutex<Vec<RangeOutcome>>>,
}

impl std::fmt::Debug for WorkerShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerShared")
            .field("run_id", &self.run_id)
            .field("node_id", &self.node_id)
            .field("ranges", &self.queue.len())
            .finish_non_exhaustive()
    }
}

/// One worker: claim, process, account, repeat (`ENG-001`).
async fn worker(shared: Arc<WorkerShared>, index: usize) {
    loop {
        // ENG-014: withhold new work while paused, without touching the plan.
        shared.control.wait_while_paused().await;
        // ENG-009, ENG-010: a stopping run claims nothing more. Checked after the pause wait as
        // well as before the claim, because a stop is what releases a paused worker.
        if shared.control.is_stopping() {
            break;
        }
        let Some(range) = shared.queue.claim() else {
            break;
        };

        let outcome = process_range(&shared, range).await;
        let failed = outcome.is_failure();
        shared.observer.on_range_finished(shared.run_id, &outcome);
        shared.outcomes.lock().push(outcome);

        if failed {
            enforce_error_limit(&shared);
        }
    }
    tracing::debug!(
        target: "cdm::engine",
        run_id = shared.run_id.as_i64(),
        worker = index,
        "worker finished"
    );
}

/// Processes one range, isolating every way it can go wrong (`ENG-002`, `ENG-008`, `ENG-013`).
async fn process_range(shared: &WorkerShared, range: TokenRange) -> RangeOutcome {
    let counters = Arc::new(JobCounters::new(shared.processor.job()));
    let ctx = RangeContext::new(
        shared.run_id,
        Arc::clone(&shared.node_id),
        range,
        shared.settings.fetch_size(),
        Arc::clone(&counters),
        Arc::clone(&shared.limits),
        shared.control.cancellation_token(),
    );

    // ENG-002: STARTED is recorded before any work begins, so a run killed mid-range still shows
    // which ranges were in flight.
    shared.observer.on_range_started(shared.run_id, range);

    let span = range_span(
        shared.run_id,
        range,
        &shared.node_id,
        shared.settings.java_thread_label(),
    );

    // ENG-013: a panic in a job is a range failure, not a poisoned pool. `AssertUnwindSafe` is
    // sound here because the panicking future is dropped immediately and nothing it borrowed is
    // observed again: the counters are atomics, and the range's registry is discarded below.
    let processing = std::panic::AssertUnwindSafe(shared.processor.process(&ctx))
        .catch_unwind()
        .instrument(span.clone());

    // ENG-010: a job that does not notice the cancellation token is dropped rather than waited
    // for, so `shutdown_grace` is a real deadline and not a request.
    let result = tokio::select! {
        biased;
        result = processing => Some(result),
        () = shared.control.killed() => None,
    };

    let _guard = span.enter();
    let outcome = match result {
        Some(Ok(Ok(verdict))) => {
            failure::record_range_success(&counters);
            counters.flush();
            RangeOutcome {
                range,
                status: verdict.status(),
                run_info: counters.run_info(),
                diagnostic: None,
                abandoned: false,
            }
        }
        Some(Ok(Err(error))) => fail_range(&counters, range, &error),
        // ENG-013: convert the panic into an ordinary range failure and carry on.
        Some(Err(payload)) => {
            let error = CdmError::new(
                ErrorKind::Internal,
                format!(
                    "the job panicked while processing the range: {}",
                    // `payload.as_ref()`, not `&payload`: `Box<dyn Any + Send>` is itself `Any`,
                    // so `&payload` would unsize-coerce to a `dyn Any` whose concrete type is the
                    // box, and every downcast below would miss.
                    describe(payload.as_ref())
                ),
            )
            .with_context(|ctx| ctx.with_range(range));
            fail_range(&counters, range, &error)
        }
        None => {
            // Abandoned by a kill. Nothing failed, so nothing is counted as failed; the range
            // stays STARTED, which `TRK-031` treats as pending so a resume re-plans it. The rows
            // it did read are still merged into the run's totals, because they really were read.
            counters.flush();
            tracing::warn!(
                target: "cdm::engine",
                "abandoning the range: the run was stopped and its grace period has expired"
            );
            RangeOutcome {
                range,
                status: RunStatus::Started,
                run_info: counters.run_info(),
                diagnostic: None,
                abandoned: true,
            }
        }
    };

    if let Err(error) = shared.run_counters.add(&counters) {
        tracing::error!(
            target: "cdm::engine",
            error = %error,
            "cannot merge a range's counters into the run's totals"
        );
    }
    outcome
}

/// Applies `ENG-008` to a failed range: account, log, and hand back the outcome.
fn fail_range(counters: &Arc<JobCounters>, range: TokenRange, error: &CdmError) -> RangeOutcome {
    let lost = failure::record_range_failure(counters);
    // Java logs the interim metrics here (`logger.error("Error stats " + getMetrics(true))`), and
    // so do we — the interim rendering is the one that includes `Unflushed`, which is exactly the
    // number an operator wants when a migrate range dies with writes buffered.
    tracing::error!(
        target: "cdm::engine",
        range_min = %range.min(),
        range_max = %range.max(),
        lost_rows = lost,
        error = %error,
        stats = %counters.metrics(CounterView::Interim),
        "the range failed; the run continues"
    );
    counters.flush();
    RangeOutcome {
        range,
        status: RunStatus::Fail,
        run_info: counters.run_info(),
        diagnostic: Some(error.to_diagnostic()),
        abandoned: false,
    }
}

/// `ENG-009`: stop the run once the total `ERROR` count exceeds `perfops.error_limit`.
fn enforce_error_limit(shared: &WorkerShared) {
    let limit = shared.settings.error_limit();
    if limit == 0 || shared.control.is_stopping() {
        return;
    }
    let errors = shared
        .run_counters
        .count_of(CounterKind::Error, CounterView::Committed);
    if errors > limit {
        tracing::error!(
            target: "cdm::engine",
            run_id = shared.run_id.as_i64(),
            errors,
            error_limit = limit,
            "the error limit has been exceeded; draining in-flight ranges and stopping the run"
        );
        shared.control.stop(StopReason::ErrorLimit);
    }
}

/// `ENG-010`: once a stop is requested, in-flight ranges have `grace` to finish.
async fn grace_watchdog(control: RunControl, grace: std::time::Duration) {
    control.stop_requested().await;
    tokio::time::sleep(grace).await;
    if !control.is_killed() {
        tracing::warn!(
            target: "cdm::engine",
            grace_secs = grace.as_secs(),
            "the shutdown grace period expired; abandoning in-flight ranges"
        );
        control.kill();
    }
}

/// The message carried by a panic payload, for the range failure it becomes.
fn describe(payload: &(dyn Any + Send)) -> String {
    payload.downcast_ref::<&'static str>().map_or_else(
        || {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "a panic with a non-string payload".to_owned())
        },
        |message| (*message).to_owned(),
    )
}

/// What a finished run reports (`ENG-002`, `ENG-009`, `ENG-010`, `MET-004`, `MET-006`).
#[derive(Debug)]
pub struct RunReport {
    run_id: RunId,
    job: JobKind,
    status: RunStatus,
    stopped_by: Option<StopReason>,
    counters: JobCounters,
    outcomes: Vec<RangeOutcome>,
    unclaimed: Vec<TokenRange>,
}

impl RunReport {
    /// The run this report describes.
    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    /// The job that ran.
    #[must_use]
    pub const fn job(&self) -> JobKind {
        self.job
    }

    /// The run's terminal status: `ENDED`, `INTERRUPTED` (`ENG-010`) or `ABORTED` (`ENG-009`).
    #[must_use]
    pub const fn status(&self) -> RunStatus {
        self.status
    }

    /// Why the run stopped early, or `None` if it finished its plan.
    #[must_use]
    pub const fn stopped_by(&self) -> Option<StopReason> {
        self.stopped_by
    }

    /// The run's committed totals (`MET-004`).
    #[must_use]
    pub const fn counters(&self) -> &JobCounters {
        &self.counters
    }

    /// Every range that was claimed, in ring order.
    #[must_use]
    pub fn outcomes(&self) -> &[RangeOutcome] {
        &self.outcomes
    }

    /// The ranges no worker ever claimed, because the run was stopped first (`ENG-010`).
    #[must_use]
    pub fn unclaimed_ranges(&self) -> &[TokenRange] {
        &self.unclaimed
    }

    /// How many ranges failed (`ENG-008`).
    #[must_use]
    pub fn ranges_failed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.is_failure()).count()
    }

    /// How many ranges reached a successful terminal status.
    #[must_use]
    pub fn ranges_passed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.is_success()).count()
    }

    /// How many ranges shutdown abandoned mid-flight (`ENG-010`).
    #[must_use]
    pub fn ranges_abandoned(&self) -> usize {
        self.outcomes.iter().filter(|o| o.abandoned).count()
    }

    /// Whether the run finished its plan without being stopped.
    ///
    /// Failed *ranges* do not make a run unsuccessful — `ENG-008` is explicit that they must not
    /// abort it — so this asks only whether the run reached the end of its work.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self.status, RunStatus::Ended)
    }

    /// The process exit code (`ENG-010`: an interrupted run exits non-zero).
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        if self.is_complete() {
            0
        } else {
            1
        }
    }

    /// Emits Java's final metrics block (`MET-006`).
    ///
    /// `run_id` is `Some` when run tracking is enabled, which is when Java prints the `RunId:`
    /// line.
    pub fn log_final_block(&self, run_id: Option<RunId>) {
        self.counters.log_final_block(run_id);
    }
}

#[cfg(test)]
mod tests;
