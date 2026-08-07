//! Pause, resume, graceful stop and immediate abort (`ENG-009`, `ENG-010`, `ENG-014`).
//!
//! [`RunControl`] is the one handle that can change a running run's mind. It is cheap to clone,
//! `Send + Sync`, and every method is non-blocking, so the HTTP control plane, the signal
//! listener and the workers themselves all hold the same object.
//!
//! # Three states, not one flag
//!
//! ```text
//!            pause()                     stop(reason)                  kill()
//!   RUNNING ─────────► PAUSED ─────────────────────────► STOPPING ─────────────► KILLED
//!      ▲                 │                                   │  (grace expired,
//!      └─── resume() ────┘                                   │   or second signal)
//!                                                            │
//!                        workers stop claiming new ranges ───┘
//!                        in-flight ranges keep running
//! ```
//!
//! * **Paused** (`ENG-014`) — workers stop *claiming*, the plan is untouched, and
//!   [`RunControl::resume`] carries on from where the queue left off. Nothing is cancelled and
//!   nothing is lost.
//! * **Stopping** (`ENG-009`, `ENG-010`) — workers stop claiming and the run will end, but
//!   in-flight ranges are allowed to finish. This is what "draining in-flight work cleanly"
//!   means, and it is the same mechanism whether the trigger was a signal or the error limit;
//!   only the recorded [`StopReason`] differs, and with it the run's final status.
//! * **Killed** (`ENG-010`, second signal) — in-flight ranges are abandoned. The cancellation
//!   token reaches every [`RangeContext`](crate::scheduler::RangeContext) so a well-behaved job
//!   can wind down, and the scheduler stops waiting for one that does not.
//!
//! Pausing while stopping is meaningless and is ignored: a paused worker that can never be
//! resumed is a hung shutdown, and `ENG-010` gives shutdown a deadline.
//!
//! # Why the reason is recorded, not inferred
//!
//! `ENG-010` requires an interrupted run to be marked `INTERRUPTED`, and `ENG-009` requires an
//! error-limit abort to be distinguishable from it — an operator resuming a run needs to know
//! whether they stopped it or it stopped itself. The first reason recorded wins: a signal
//! arriving during an error-limit drain does not rewrite why the run ended.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Why a run stopped before its plan was exhausted (`ENG-009`, `ENG-010`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StopReason {
    /// `SIGINT` or `SIGTERM` (`ENG-010`). The run is marked `INTERRUPTED` and is resumable.
    Signal,
    /// Total `ERROR` exceeded `perfops.error_limit` (`ENG-009`). The run is marked `ABORTED`.
    ErrorLimit,
    /// A range failed with a fatal error kind (`ENG-015`). The run is marked `ABORTED`.
    ///
    /// Distinct from [`StopReason::ErrorLimit`] because the two say opposite things about the
    /// data. An error-limit abort means many rows failed and the run gave up on volume; a fatal
    /// abort means one condition of the run itself is wrong — a schema that changed, a credential
    /// that expired — and the row count is beside the point. An operator reading a run row needs
    /// to know which, because only one of them is fixed by looking at the data.
    Fatal,
    /// An operator asked for it, through the control plane or an embedding application.
    Operator,
}

impl StopReason {
    /// The status the run row carries when it stopped for this reason (`TRK-012`).
    #[must_use]
    pub const fn run_status(self) -> cdm_core::RunStatus {
        match self {
            Self::Signal => cdm_core::RunStatus::Interrupted,
            Self::ErrorLimit | Self::Fatal | Self::Operator => cdm_core::RunStatus::Aborted,
        }
    }

    /// A short phrase for the log line that announces the stop.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Signal => "signal",
            Self::ErrorLimit => "error limit exceeded",
            Self::Fatal => "a fatal error",
            Self::Operator => "operator request",
        }
    }
}

/// The shared state behind every [`RunControl`] clone.
#[derive(Debug)]
struct ControlInner {
    paused: watch::Sender<bool>,
    stopping: CancellationToken,
    killed: CancellationToken,
    reason: Mutex<Option<StopReason>>,
}

/// The pause, resume, stop and abort control of one run (`ENG-009`, `ENG-010`, `ENG-014`).
#[derive(Debug, Clone)]
pub struct RunControl {
    inner: Arc<ControlInner>,
}

impl Default for RunControl {
    fn default() -> Self {
        Self::new()
    }
}

impl RunControl {
    /// A control for a run that is running, not paused and not stopping.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ControlInner {
                paused: watch::Sender::new(false),
                stopping: CancellationToken::new(),
                killed: CancellationToken::new(),
                reason: Mutex::new(None),
            }),
        }
    }

    /// Stops issuing new work without losing the plan (`ENG-014`).
    ///
    /// In-flight ranges run to completion. Pausing a stopping run does nothing.
    pub fn pause(&self) {
        if self.is_stopping() {
            return;
        }
        self.inner.paused.send_replace(true);
    }

    /// Resumes a paused run from where its queue left off (`ENG-014`).
    pub fn resume(&self) {
        self.inner.paused.send_replace(false);
    }

    /// Whether new work is currently withheld (`ENG-014`).
    #[must_use]
    pub fn is_paused(&self) -> bool {
        *self.inner.paused.borrow()
    }

    /// Requests a graceful stop, recording why (`ENG-009`, `ENG-010`).
    ///
    /// Workers stop claiming immediately; in-flight ranges are left to finish. The first reason
    /// recorded is the one the run reports. A stop also clears a pause, so a run that was paused
    /// when the signal arrived still drains rather than sitting on a deadline it cannot meet.
    pub fn stop(&self, reason: StopReason) {
        {
            let mut recorded = self.inner.reason.lock();
            if recorded.is_none() {
                *recorded = Some(reason);
            }
        }
        self.inner.paused.send_replace(false);
        self.inner.stopping.cancel();
    }

    /// Abandons in-flight work immediately (`ENG-010`, second signal; grace expiry).
    ///
    /// Implies [`RunControl::stop`], so a kill that arrives first still records a reason.
    pub fn kill(&self) {
        self.stop(StopReason::Signal);
        self.inner.killed.cancel();
    }

    /// Applies one delivered signal: the first stops gracefully, the second aborts (`ENG-010`).
    ///
    /// Keeping the escalation here rather than in the signal listener is what makes it testable
    /// without raising a real signal at a real process — a test that would be unreproducible on
    /// Windows and racy everywhere else.
    pub fn signalled(&self) {
        if self.is_stopping() {
            self.kill();
        } else {
            self.stop(StopReason::Signal);
        }
    }

    /// Whether the run is draining: no new ranges are claimed (`ENG-009`, `ENG-010`).
    #[must_use]
    pub fn is_stopping(&self) -> bool {
        self.inner.stopping.is_cancelled()
    }

    /// Whether in-flight work has been abandoned (`ENG-010`).
    #[must_use]
    pub fn is_killed(&self) -> bool {
        self.inner.killed.is_cancelled()
    }

    /// Why the run stopped, or `None` while it is still running.
    #[must_use]
    pub fn stop_reason(&self) -> Option<StopReason> {
        *self.inner.reason.lock()
    }

    /// Resolves when a graceful stop is requested.
    pub async fn stop_requested(&self) {
        self.inner.stopping.cancelled().await;
    }

    /// Resolves when in-flight work is abandoned.
    pub async fn killed(&self) {
        self.inner.killed.cancelled().await;
    }

    /// The token handed to every range, cancelled when in-flight work is abandoned (`ENG-010`).
    ///
    /// A job that polls it can stop cleanly — releasing its statements, flushing what it can.
    /// A job that ignores it is dropped instead, which is why the scheduler's shutdown deadline
    /// does not depend on the job's cooperation.
    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.inner.killed.clone()
    }

    /// Waits while the run is paused, returning as soon as it resumes or starts stopping
    /// (`ENG-014`).
    ///
    /// Called by every worker before it claims a range, so pausing withholds *new* work only.
    pub async fn wait_while_paused(&self) {
        if !self.is_paused() {
            return;
        }
        let mut paused = self.inner.paused.subscribe();
        loop {
            if !*paused.borrow_and_update() || self.is_stopping() {
                return;
            }
            tokio::select! {
                () = self.inner.stopping.cancelled() => return,
                result = paused.changed() => {
                    // The sender lives in the `Arc` this method was called through, so it cannot
                    // have been dropped; treating the error as "no longer paused" is the safe
                    // reading either way.
                    if result.is_err() {
                        return;
                    }
                }
            }
        }
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use cdm_core::RunStatus;

    use super::*;

    #[test]
    fn eng_014_pause_and_resume_toggle_without_stopping_the_run() {
        let control = RunControl::new();
        assert!(!control.is_paused());
        control.pause();
        assert!(control.is_paused());
        assert!(!control.is_stopping());
        control.resume();
        assert!(!control.is_paused());
    }

    #[test]
    fn eng_014_pausing_a_stopping_run_is_ignored() {
        let control = RunControl::new();
        control.stop(StopReason::Operator);
        control.pause();
        assert!(!control.is_paused(), "a stopping run must not be pausable");
    }

    #[test]
    fn eng_010_a_stop_clears_a_pause_so_the_drain_can_finish() {
        let control = RunControl::new();
        control.pause();
        control.stop(StopReason::Signal);
        assert!(!control.is_paused());
        assert!(control.is_stopping());
    }

    #[test]
    fn eng_010_the_first_signal_stops_and_the_second_kills() {
        let control = RunControl::new();
        control.signalled();
        assert!(control.is_stopping());
        assert!(!control.is_killed());

        control.signalled();
        assert!(control.is_killed());
        assert_eq!(control.stop_reason(), Some(StopReason::Signal));
    }

    #[test]
    fn eng_009_the_first_recorded_reason_wins() {
        let control = RunControl::new();
        control.stop(StopReason::ErrorLimit);
        control.stop(StopReason::Signal);
        assert_eq!(control.stop_reason(), Some(StopReason::ErrorLimit));
        assert_eq!(
            control.stop_reason().unwrap().run_status(),
            RunStatus::Aborted
        );
    }

    #[test]
    fn eng_010_a_kill_records_a_reason_when_nothing_else_has() {
        let control = RunControl::new();
        control.kill();
        assert_eq!(control.stop_reason(), Some(StopReason::Signal));
        assert!(control.is_stopping());
    }

    #[test]
    fn eng_010_each_stop_reason_maps_to_the_status_the_run_row_carries() {
        assert_eq!(StopReason::Signal.run_status(), RunStatus::Interrupted);
        assert_eq!(StopReason::ErrorLimit.run_status(), RunStatus::Aborted);
        assert_eq!(StopReason::Fatal.run_status(), RunStatus::Aborted);
        assert_eq!(StopReason::Operator.run_status(), RunStatus::Aborted);
        assert_eq!(StopReason::Signal.as_str(), "signal");
        assert_eq!(StopReason::ErrorLimit.as_str(), "error limit exceeded");
        assert_eq!(StopReason::Fatal.as_str(), "a fatal error");
        assert_eq!(StopReason::Operator.as_str(), "operator request");
    }

    #[test]
    fn eng_010_a_fresh_control_has_not_stopped() {
        let control = RunControl::default();
        assert!(!control.is_stopping());
        assert!(!control.is_killed());
        assert_eq!(control.stop_reason(), None);
        assert!(!control.cancellation_token().is_cancelled());
    }

    #[tokio::test]
    async fn eng_014_waiting_while_paused_returns_as_soon_as_the_run_resumes() {
        let control = RunControl::new();
        control.pause();

        let resumed = Arc::new(AtomicBool::new(false));
        let waiter = tokio::spawn({
            let control = control.clone();
            let resumed = Arc::clone(&resumed);
            async move {
                control.wait_while_paused().await;
                resumed.store(true, Ordering::SeqCst);
            }
        });

        // Give the waiter a chance to park, then release it.
        tokio::task::yield_now().await;
        assert!(!resumed.load(Ordering::SeqCst));
        control.resume();
        waiter.await.unwrap();
        assert!(resumed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn eng_014_waiting_while_paused_returns_immediately_when_not_paused() {
        let control = RunControl::new();
        control.wait_while_paused().await;
        assert!(!control.is_paused());
    }

    #[tokio::test]
    async fn eng_010_waiting_while_paused_returns_when_the_run_starts_stopping() {
        let control = RunControl::new();
        control.pause();
        let waiter = tokio::spawn({
            let control = control.clone();
            async move { control.wait_while_paused().await }
        });
        tokio::task::yield_now().await;
        control.stop(StopReason::Signal);
        waiter.await.unwrap();
    }

    #[tokio::test]
    async fn eng_010_stop_and_kill_are_observable_as_futures() {
        let control = RunControl::new();
        let watcher = tokio::spawn({
            let control = control.clone();
            async move {
                control.stop_requested().await;
                control.killed().await;
                control.stop_reason()
            }
        });
        control.stop(StopReason::Operator);
        control.kill();
        assert_eq!(watcher.await.unwrap(), Some(StopReason::Operator));
    }

    #[tokio::test]
    async fn eng_010_the_cancellation_token_reaches_holders_taken_before_the_kill() {
        let control = RunControl::new();
        let token = control.cancellation_token();
        assert!(!token.is_cancelled());
        control.kill();
        assert!(token.is_cancelled());
        token.cancelled().await;
    }
}
