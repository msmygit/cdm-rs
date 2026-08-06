//! `SIGINT` / `SIGTERM` delivery (`ENG-010`).
//!
//! This module is deliberately almost empty. Everything a signal *means* — first one drains, the
//! second abandons — lives in [`RunControl::signalled`], which is ordinary code with ordinary
//! tests. All that is left here is the platform plumbing that turns an operating-system signal
//! into a call to it.
//!
//! Splitting it this way is the difference between a tested requirement and an untestable one. A
//! test that raises a real `SIGINT` at its own process races every other test in the binary,
//! cannot run on Windows, and interacts badly with the harness's own handlers. A test that calls
//! `signalled()` twice asserts precisely the behaviour `ENG-010` specifies, deterministically,
//! everywhere.
//!
//! # Behaviour
//!
//! * First `SIGINT` or `SIGTERM` — graceful shutdown: stop claiming, let in-flight ranges finish
//!   within `shutdown_grace`, mark the run `INTERRUPTED`.
//! * Second — abandon in-flight work immediately.
//! * The listener then stops, so a third signal gets the default disposition and the operator can
//!   always kill the process.

use crate::scheduler::control::RunControl;

/// Spawns the signal listener for a run (`ENG-010`).
///
/// The returned handle stops on its own once a second signal has been delivered; aborting it is
/// how a run that finished normally tears the listener down.
pub fn spawn_signal_listener(control: RunControl) -> tokio::task::JoinHandle<()> {
    tokio::spawn(listen(control))
}

/// Delivers each signal to the control until in-flight work has been abandoned.
#[cfg(unix)]
async fn listen(control: RunControl) {
    use tokio::signal::unix::{signal, SignalKind};

    let Ok((mut interrupt, mut terminate)) = signal(SignalKind::interrupt())
        .and_then(|interrupt| signal(SignalKind::terminate()).map(|term| (interrupt, term)))
    else {
        tracing::warn!(
            target: "cdm::engine",
            "cannot install signal handlers; the run will not shut down gracefully on \
             SIGINT or SIGTERM (ENG-010)"
        );
        return;
    };

    loop {
        tokio::select! {
            received = interrupt.recv() => if received.is_none() { return },
            received = terminate.recv() => if received.is_none() { return },
        }
        control.signalled();
        if control.is_killed() {
            return;
        }
    }
}

/// The non-Unix listener. Windows has no `SIGTERM`; `Ctrl-C` is the whole of the contract.
#[cfg(not(unix))]
async fn listen(control: RunControl) {
    while tokio::signal::ctrl_c().await.is_ok() {
        control.signalled();
        if control.is_killed() {
            return;
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
pub(crate) mod tests {
    use super::*;

    /// Serialises every test in this crate that touches process-global signal state (`ENG-010`).
    ///
    /// A signal is delivered to the *process*, and Tokio hands it to every signal stream
    /// registered anywhere in the test binary. One test raising a `SIGINT` is therefore visible
    /// to any other test that has a listener installed at that moment, which is how a suite grows
    /// a failure that only appears when two tests happen to overlap. Every test that raises a
    /// signal, and every test that asserts a signal did *not* arrive, holds this lock.
    ///
    /// A Tokio mutex rather than a `std` one, for two reasons: the guard is deliberately held
    /// across the whole test, awaits included, which is exactly what `clippy::await_holding_lock`
    /// exists to forbid for the blocking kind; and it has no poisoning, so a test that fails while
    /// holding it does not turn every later signal test into a re-report of the first failure.
    #[cfg(unix)]
    static SIGNAL_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Takes the signal-test lock for the rest of the test.
    #[cfg(unix)]
    pub(crate) async fn serialised() -> tokio::sync::MutexGuard<'static, ()> {
        SIGNAL_TESTS.lock().await
    }

    /// Sends one `SIGINT` to this process.
    #[cfg(unix)]
    pub(crate) fn raise_sigint() {
        let status = std::process::Command::new("kill")
            .arg("-INT")
            .arg(std::process::id().to_string())
            .status()
            .expect("the test host must have kill(1)");
        assert!(status.success(), "kill -INT failed: {status}");
    }

    /// Raises `SIGINT` until `stopped` reports true, or gives up after a bounded number of tries.
    ///
    /// A listener registers asynchronously, and a signal raised before it does is simply not
    /// delivered to it — so one raise would be a race with the runtime, and repeating until the
    /// control has noticed is not.
    #[cfg(unix)]
    pub(crate) async fn sigint_until(stopped: impl Fn() -> bool) -> bool {
        for _ in 0..100 {
            if stopped() {
                return true;
            }
            raise_sigint();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        stopped()
    }

    #[tokio::test]
    async fn eng_010_the_listener_can_be_installed_and_torn_down() {
        // Holds the lock even though it raises nothing: it asserts that the control was *not*
        // stopped, which a `SIGINT` from a concurrent test would falsify.
        #[cfg(unix)]
        let _serialised = serialised().await;
        // The listener's own logic is one call to `signalled()`, which `control::tests` covers
        // exhaustively. What is worth asserting here is that installing it succeeds under the
        // runtime and that a finished run can dismantle it without waiting for a signal that
        // will never come.
        let control = RunControl::new();
        let handle = spawn_signal_listener(control.clone());
        assert!(!handle.is_finished());
        handle.abort();
        assert!(handle.await.unwrap_err().is_cancelled());
        assert!(!control.is_stopping());
    }
}
