//! Diagnostic constructors shared by every feature.
//!
//! A feature reports what is wrong with a configuration by returning [`Diagnostic`]s rather than by
//! failing (`PLG-002`), so that all of a run's findings reach the operator in one pass (`CFG-021`).
//! The two codes below are the only ones the features here emit — `CDM-CONFIG` for "you asked for
//! something contradictory" and `CDM-SCHEMA-MISMATCH` for "the cluster does not have what you asked
//! for" — and centralising their construction keeps a feature from inventing a third code that has
//! no page in `docs/errors/` (`ERR-003`).

use cdm_core::{Diagnostic, ErrorKind, Severity};

/// A blocking finding about the configuration itself.
pub(crate) fn config_error(title: impl Into<String>) -> Diagnostic {
    Diagnostic::error(ErrorKind::Config.diagnostic_code(), title)
}

/// A blocking finding about the configuration as it meets the schema.
pub(crate) fn schema_error(title: impl Into<String>) -> Diagnostic {
    Diagnostic::error(ErrorKind::SchemaMismatch.diagnostic_code(), title)
}

/// A non-blocking finding the operator should read before the run gets far.
pub(crate) fn config_warning(title: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        ErrorKind::Config.diagnostic_code(),
        Severity::Warning,
        title,
    )
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
    fn fea_011_findings_use_the_documented_error_codes() {
        assert_eq!(config_error("t").code, "CDM-CONFIG");
        assert_eq!(schema_error("t").code, "CDM-SCHEMA-MISMATCH");
        assert!(config_error("t").is_blocking());
        assert!(!config_warning("t").is_blocking());
        assert_eq!(config_warning("t").severity, Severity::Warning);
    }
}
