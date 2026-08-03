//! Process exit codes (`CLI-004`).
//!
//! Exit codes are part of the tool's contract with whatever runs it. A migration is typically
//! driven by a scheduler or a CI pipeline that can only see the code, so "non-zero" is not enough:
//! a configuration typo and a mid-run interruption need different responses, and only one of them
//! is worth retrying automatically.

use std::process::ExitCode;

use cdm_core::{CdmError, ErrorKind};

/// The documented exit codes.
///
/// The numbers are a public contract (`CLI-004`) and MUST NOT be renumbered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    /// The command did what was asked.
    Success = 0,
    /// The command ran to completion but found failures or discrepancies.
    ///
    /// A validate run that found mismatches lands here, as does a migrate run with failed ranges.
    /// The distinction from [`Exit::Internal`] matters: the tool worked, the data did not agree.
    Completed = 1,
    /// The configuration is invalid; nothing was attempted.
    Config = 2,
    /// A cluster could not be reached or authenticated to.
    Connect = 3,
    /// The run was interrupted by a signal and stopped cleanly (`ENG-010`).
    Interrupted = 4,
    /// A defect in cdm-rs.
    Internal = 5,
}

impl Exit {
    /// The exit code as the process should report it.
    pub fn code(self) -> ExitCode {
        ExitCode::from(self as u8)
    }

    /// Whether a supervisor should consider retrying the invocation unchanged.
    ///
    /// Only an interruption is worth an automatic retry: the run is resumable and nothing about the
    /// request was wrong. Retrying a configuration error just fails again, and retrying a
    /// completed-with-discrepancies run hides the discrepancies.
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::Interrupted)
    }

    /// The exit code for an error, chosen from its kind.
    pub fn for_error(error: &CdmError) -> Self {
        match error.kind() {
            ErrorKind::Config | ErrorKind::SchemaMismatch => Self::Config,
            ErrorKind::Connect | ErrorKind::Auth | ErrorKind::Tls => Self::Connect,
            ErrorKind::Cancelled => Self::Interrupted,
            _ => Self::Internal,
        }
    }
}

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
    fn cli_004_codes_are_the_documented_numbers() {
        // These are a public contract. Changing one silently breaks every pipeline that reads it.
        assert_eq!(Exit::Success as u8, 0);
        assert_eq!(Exit::Completed as u8, 1);
        assert_eq!(Exit::Config as u8, 2);
        assert_eq!(Exit::Connect as u8, 3);
        assert_eq!(Exit::Interrupted as u8, 4);
        assert_eq!(Exit::Internal as u8, 5);
    }

    #[test]
    fn cli_004_configuration_errors_are_distinguishable_from_defects() {
        let config = CdmError::new(ErrorKind::Config, "bad key");
        let internal = CdmError::new(ErrorKind::Internal, "unreachable");

        assert_eq!(Exit::for_error(&config), Exit::Config);
        assert_eq!(Exit::for_error(&internal), Exit::Internal);
    }

    #[test]
    fn cli_004_connection_failures_share_one_code() {
        for kind in [ErrorKind::Connect, ErrorKind::Auth, ErrorKind::Tls] {
            let error = CdmError::new(kind, "unreachable");
            assert_eq!(
                Exit::for_error(&error),
                Exit::Connect,
                "{kind:?} is a connectivity problem from the operator's point of view"
            );
        }
    }

    #[test]
    fn cli_004_only_an_interruption_invites_an_automatic_retry() {
        assert!(Exit::Interrupted.is_retryable());
        for exit in [
            Exit::Success,
            Exit::Completed,
            Exit::Config,
            Exit::Connect,
            Exit::Internal,
        ] {
            assert!(!exit.is_retryable(), "{exit:?} must not be retried blindly");
        }
    }
}
