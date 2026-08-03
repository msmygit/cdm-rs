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
mod tests {
    use super::*;

    #[tokio::test]
    async fn eng_010_the_listener_can_be_installed_and_torn_down() {
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
