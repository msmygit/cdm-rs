//! Telling an adaptive rate limiter that the target is overloaded (`ENG-006`).
//!
//! `ENG-006` requires the effective rate to fall when "the target reports overload (write
//! timeouts, `OVERLOADED`, rising p99 latency)". Deciding whether a given failure *is* that is a
//! question about a CQL error frame, and `ARCHITECTURE.md` §3 makes this the only crate allowed
//! to look at one. The controller itself — the thing that decides what rate to move to — is
//! driver-agnostic and lives in `cdm-engine::scheduler::adaptive`; the two meet at
//! [`TargetLoadObserver`], which carries a verdict and nothing else.
//!
//! # What counts as overload, and what deliberately does not
//!
//! | Condition | Fed to the controller? | Why |
//! |---|---|---|
//! | `WriteTimeout` | **yes** | Named by `ENG-006`. The replicas did not acknowledge in time: the target is behind. |
//! | `Overloaded` | **yes** | Named by `ENG-006`. The coordinator said so in as many words. |
//! | `WriteFailure` | **yes** | Replicas failed the write outright, which is what a saturated commitlog looks like. |
//! | `ExecutionError::RequestTimeout` | **yes** | This is `ENG-006`'s "rising p99 latency", read off the deadline the operator already set. `perfops.request_timeout` is a *statement* of the latency budget; a request that blows through it is a request in the tail. Estimating a p99 separately would need a second threshold nobody could state. |
//! | `Unavailable` | no | Replicas are *down*, not slow. Slowing down does not bring them back, and hiding an outage inside a rate reduction delays the page. |
//! | `IsBootstrapping` | no | A node joining. Transient by nature and unrelated to load; `CON-011` already retries it elsewhere. |
//! | `ReadTimeout` | no | The read side is paced by its own limiter (`ENG-004`); this observer is only ever attached to target writes. |
//! | Syntax, `Unauthorized`, serialization | no | Deterministic. No rate makes them succeed. |
//! | Anything [`ErrorKind::is_fatal`] | **never** | A fatal error stops the run (`ENG-015`). Absorbing one as backpressure would convert an actionable failure into a slower run that fails anyway, which is the single worst thing a controller of this kind can do. [`is_target_overload`] checks this first, before it looks at the frame at all. |
//!
//! [`ErrorKind::is_fatal`]: cdm_core::ErrorKind::is_fatal

use cdm_core::CdmError;
use scylla::errors::{DbError, ExecutionError, RequestAttemptError};

/// Notified of every target write attempt, so an adaptive rate limiter can see what the target
/// is saying (`ENG-006`).
///
/// Implemented by `cdm-engine`'s `RuntimeLimits`. Every attempt reports exactly one outcome,
/// including the attempts `CON-011` retries and eventually succeeds at — those are the most
/// valuable signal there is, because they are the ones the target is complaining about *before*
/// anything has failed.
///
/// Implementations are called on the write hot path and must not block.
pub trait TargetLoadObserver: Send + Sync + std::fmt::Debug {
    /// The attempt finished without the target reporting overload.
    fn on_target_ok(&self);

    /// The target reported overload. See the [module documentation](self) for exactly which
    /// conditions reach this.
    fn on_target_overload(&self);
}

/// Whether a driver failure is the target saying it is overloaded (`ENG-006`).
///
/// The table in the [module documentation](self) is normative; this function is it in code.
#[must_use]
pub fn is_overload(error: &ExecutionError) -> bool {
    match error {
        // The operator's own latency budget, exceeded. `ENG-006`'s p99 signal.
        ExecutionError::RequestTimeout(_) => true,
        ExecutionError::LastAttemptError(attempt) => is_attempt_overload(attempt),
        _ => false,
    }
}

/// Whether one attempt's error is an overload condition.
fn is_attempt_overload(error: &RequestAttemptError) -> bool {
    matches!(
        error,
        RequestAttemptError::DbError(
            DbError::WriteTimeout { .. } | DbError::Overloaded | DbError::WriteFailure { .. },
            _,
        )
    )
}

/// Whether a [`CdmError`] the write path produced carries an overload condition (`ENG-006`).
///
/// Used by callers that have already lost the driver error inside a [`CdmError`]. A fatal kind is
/// rejected outright and before anything else: `ENG-015` requires a fatal failure to stop the
/// run, and a controller that quietly turned one into a lower rate would defeat it.
#[must_use]
pub fn is_target_overload(error: &CdmError) -> bool {
    if error.kind().is_fatal() {
        return false;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> =
        std::error::Error::source(error as &dyn std::error::Error);
    while let Some(cause) = source {
        if let Some(execution) = cause.downcast_ref::<ExecutionError>() {
            return is_overload(execution);
        }
        if let Some(attempt) = cause.downcast_ref::<RequestAttemptError>() {
            return is_attempt_overload(attempt);
        }
        source = cause.source();
    }
    false
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
    use cdm_core::{ErrorKind, Side};
    use scylla::statement::Consistency;

    use crate::errors::side_error_from;

    use super::*;

    fn db(error: DbError) -> ExecutionError {
        ExecutionError::LastAttemptError(RequestAttemptError::DbError(error, String::new()))
    }

    fn write_timeout() -> ExecutionError {
        db(DbError::WriteTimeout {
            consistency: Consistency::LocalQuorum,
            received: 1,
            required: 2,
            write_type: scylla::errors::WriteType::Simple,
        })
    }

    #[test]
    fn eng_006_a_write_timeout_and_an_overloaded_response_are_overload_signals() {
        assert!(is_overload(&write_timeout()));
        assert!(is_overload(&db(DbError::Overloaded)));
        assert!(is_overload(&db(DbError::WriteFailure {
            consistency: Consistency::LocalQuorum,
            received: 1,
            required: 2,
            numfailures: 1,
            write_type: scylla::errors::WriteType::Simple,
        })));
    }

    #[test]
    fn eng_006_a_request_that_exceeds_the_configured_timeout_is_the_latency_signal() {
        assert!(is_overload(&ExecutionError::RequestTimeout(
            std::time::Duration::from_secs(30)
        )));
    }

    #[test]
    fn eng_006_a_node_that_is_down_or_joining_is_not_an_overload_signal() {
        // Backing off does not bring a replica back, and a rate reduction that hides an outage
        // delays the alert an operator actually needs.
        assert!(!is_overload(&db(DbError::Unavailable {
            consistency: Consistency::LocalQuorum,
            required: 2,
            alive: 1,
        })));
        assert!(!is_overload(&db(DbError::IsBootstrapping)));
    }

    #[test]
    fn eng_006_a_deterministic_failure_is_not_an_overload_signal() {
        assert!(!is_overload(&db(DbError::SyntaxError)));
        assert!(!is_overload(&db(DbError::Unauthorized)));
        assert!(!is_overload(&db(DbError::ReadTimeout {
            consistency: Consistency::LocalQuorum,
            received: 1,
            required: 2,
            data_present: false,
        })));
    }

    #[test]
    fn eng_006_an_overload_survives_the_trip_through_a_cdm_error() {
        let error = side_error_from(
            ErrorKind::Write,
            Side::Target,
            "the target write failed".to_owned(),
            write_timeout(),
        );
        assert!(is_target_overload(&error));

        let benign = side_error_from(
            ErrorKind::Write,
            Side::Target,
            "the target write failed".to_owned(),
            db(DbError::SyntaxError),
        );
        assert!(!is_target_overload(&benign));
    }

    #[test]
    fn eng_006_and_eng_015_a_fatal_error_is_never_absorbed_as_backpressure() {
        // Contrived on purpose: even if a fatal error somehow carried a write timeout underneath
        // it, it must stop the run rather than quietly slow it down.
        for kind in ErrorKind::ALL.into_iter().filter(ErrorKind::is_fatal) {
            let error = side_error_from(
                kind,
                Side::Target,
                "a run-level failure".to_owned(),
                write_timeout(),
            );
            assert!(
                !is_target_overload(&error),
                "{kind} is fatal (ENG-015) and must not be read as backpressure"
            );
        }
    }

    #[test]
    fn eng_006_an_error_with_no_driver_cause_is_not_a_signal() {
        assert!(!is_target_overload(&CdmError::new(
            ErrorKind::Write,
            "a write failed for a reason nobody wrote down"
        )));
    }
}
