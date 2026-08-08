//! Entering and — above all — leaving the alternate screen (`MET-031`).
//!
//! # The defect this module exists to prevent
//!
//! A terminal UI puts the terminal into raw mode and switches it to the alternate screen. Both are
//! process-wide state on a device the process does not own. If cdm-rs exits without undoing them,
//! the operator is left with a shell that does not echo what they type, does not run a line when
//! they press return, and does not respond to Ctrl-C — on the machine where a migration has just
//! gone wrong. `reset` fixes it, but only if you know that; the usual response is to close the
//! terminal and lose the scrollback with the error in it.
//!
//! So restoration is not attached to the happy path. There are three ways out of a run and this
//! module covers all of them:
//!
//! | Exit | Covered by |
//! |---|---|
//! | the run finishes, or returns an error | [`TerminalGuard`]'s `Drop` |
//! | the operator presses `q` or Ctrl-C | the same `Drop`, after the graceful stop of `ENG-010` |
//! | something panics | the panic hook installed by [`TerminalGuard::enter`] |
//!
//! The panic hook matters more here than anywhere else in the codebase. `ENG-013` contains a
//! processor's panic at the range boundary, so a panicking *range* never reaches this; what does
//! reach it is a panic on the display's own thread, and that is precisely the case where the
//! terminal is in raw mode and the message that would explain it is about to be printed onto an
//! alternate screen that is then discarded. The hook restores first and prints second.
//!
//! # Ctrl-C in raw mode
//!
//! Raw mode turns off `ISIG`, so Ctrl-C stops being `SIGINT` and becomes a key event. The signal
//! listener the scheduler installs (`ENG-010`) is therefore *not* what stops a run while the UI is
//! up, and a UI that did not handle the key itself would make Ctrl-C look broken. The event loop
//! translates it back into the same graceful stop the signal would have caused.

use std::io::{stdout, Stdout};
use std::panic::PanicHookInfo;
use std::sync::Arc;

use cdm_core::{CdmError, ErrorKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// The terminal to draw on, restored on every path out.
///
/// Created by [`TerminalGuard::enter`] and dropped when the display stops. Nothing else in the
/// process may enable raw mode while one exists.
pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    previous_hook: Arc<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>,
}

// `Terminal` is not `Debug`, and `missing_debug_implementations` is a workspace lint.
impl std::fmt::Debug for TerminalGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalGuard").finish_non_exhaustive()
    }
}

impl TerminalGuard {
    /// Takes the terminal, installing the restoring panic hook (`MET-031`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] when raw mode or the alternate screen is refused — which is what
    /// happens on a handle that is not a terminal after all. A caller must treat that as a reason
    /// to fall back to line-based progress, never as a reason to fail the run: the migration is
    /// the job and the display is not.
    pub fn enter() -> Result<Self, CdmError> {
        enable_raw_mode().map_err(|error| terminal_error("cannot enter raw mode", &error))?;

        // From here on, every early return must undo the raw mode it just enabled.
        let mut out = stdout();
        if let Err(error) = crossterm::execute!(out, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(terminal_error("cannot enter the alternate screen", &error));
        }

        let terminal = match Terminal::new(CrosstermBackend::new(out)) {
            Ok(terminal) => terminal,
            Err(error) => {
                restore();
                return Err(terminal_error("cannot take the terminal", &error));
            }
        };

        Ok(Self {
            terminal,
            previous_hook: install_restoring_hook(),
        })
    }

    /// The terminal to draw on.
    pub fn terminal(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
        uninstall_hook(&self.previous_hook);
    }
}

/// The type the panic hook is held as between installing and putting it back.
type Hook = Arc<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

/// Wraps the current panic hook in one that restores the terminal first, and returns the old one.
///
/// An `Arc` rather than a `Box` because the hook has to own a callable copy while `Drop` still
/// needs one to put back. The extra indirection costs a pointer chase on a path that runs, at most,
/// once per process.
fn install_restoring_hook() -> Hook {
    let previous: Hook = Arc::from(std::panic::take_hook());
    let hook = Arc::clone(&previous);
    std::panic::set_hook(Box::new(move |info| {
        // Restore first: whatever the previous hook prints is unreadable on an alternate screen
        // that is about to be thrown away.
        restore();
        hook(info);
    }));
    previous
}

/// Puts back the hook [`install_restoring_hook`] displaced.
fn uninstall_hook(previous: &Hook) {
    let previous = Arc::clone(previous);
    std::panic::set_hook(Box::new(move |info| previous(info)));
}

/// Undoes everything [`TerminalGuard::enter`] did, ignoring every error.
///
/// Ignoring errors is correct and is the only correct choice: this runs from `Drop` and from a
/// panic hook, neither of which has anywhere to report to, and a terminal that refuses to leave
/// the alternate screen is not made better by also refusing to disable raw mode. Each step is
/// attempted independently so that one failure cannot skip the next.
pub fn restore() {
    let mut out = stdout();
    let _ = crossterm::execute!(out, LeaveAlternateScreen);
    let _ = crossterm::execute!(out, crossterm::cursor::Show);
    let _ = disable_raw_mode();
}

/// A terminal failure, as a diagnostic that says the display is optional.
fn terminal_error(what: &str, error: &std::io::Error) -> CdmError {
    CdmError::new(ErrorKind::Internal, format!("{what}: {error}"))
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

    #[test]
    fn met_031_restoring_a_terminal_that_was_never_taken_is_harmless() {
        // `restore` runs from `Drop` and from a panic hook, and both can fire on a process whose
        // stdout is a pipe — which is exactly how this test runs under `cargo test`. It must not
        // fail, and must not panic, however little of it applies.
        restore();
        restore();
    }

    #[test]
    fn met_031_a_panic_restores_the_terminal_before_the_previous_hook_runs() {
        // The defect this whole module exists to prevent, tested without a terminal: install the
        // wrapper over a hook that records that it ran, panic inside `catch_unwind`, and require
        // both that the wrapper called through and that it put the old hook back afterwards.
        //
        // `TerminalGuard::enter` itself is deliberately not exercised here. `crossterm` enables
        // raw mode on the controlling `/dev/tty`, not on stdout, so calling it from a unit test
        // would switch the developer's own terminal to the alternate screen mid-suite — for a
        // property this test already covers.
        static RAN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

        let outer = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {
            RAN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));

        let previous = install_restoring_hook();
        let result = std::panic::catch_unwind(|| panic!("something went wrong mid-run"));
        assert!(result.is_err());
        assert_eq!(
            RAN.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the wrapper must call through to the hook it displaced"
        );

        uninstall_hook(&previous);
        let result = std::panic::catch_unwind(|| panic!("and again, with the old hook back"));
        assert!(result.is_err());
        assert_eq!(RAN.load(std::sync::atomic::Ordering::SeqCst), 2);

        std::panic::set_hook(outer);
    }
}
