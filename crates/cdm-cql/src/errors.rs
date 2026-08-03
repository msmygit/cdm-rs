//! Constructors for the [`CdmError`] variants this crate raises (`ERR-001`).
//!
//! Every failure in `cdm-cql` is one of four kinds — `Config`, `Connect`, `Auth` or `Tls` for the
//! connection path, `SchemaMismatch` for introspection — and each carries the side it concerns so
//! that a message reads `origin` or `target` without the caller having to say so again. These
//! helpers exist to keep that context attached at the point of failure rather than bolted on
//! three frames later.

use cdm_core::{CdmError, ErrorKind, Side};

/// A boxable underlying cause.
pub(crate) type Cause = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Builds an error of `kind` for `side`.
pub(crate) fn side_error(kind: ErrorKind, side: Side, message: impl Into<String>) -> CdmError {
    CdmError::new(kind, message).with_context(|c| c.with_side(side))
}

/// Builds an error of `kind` for `side`, preserving the underlying cause.
pub(crate) fn side_error_from(
    kind: ErrorKind,
    side: Side,
    message: impl Into<String>,
    cause: impl Into<Cause>,
) -> CdmError {
    side_error(kind, side, message).with_source(cause.into())
}

/// A TLS material failure (`CON-006`, `CON-007`).
pub(crate) fn tls_error(side: Side, message: impl Into<String>) -> CdmError {
    side_error(ErrorKind::Tls, side, message)
}

/// A TLS material failure that wraps a parser or filesystem error.
pub(crate) fn tls_error_from(
    side: Side,
    message: impl Into<String>,
    cause: impl Into<Cause>,
) -> CdmError {
    side_error_from(ErrorKind::Tls, side, message, cause)
}

/// A configuration failure detected while building a session (`CON-002`).
pub(crate) fn config_error(side: Side, message: impl Into<String>, key: &str) -> CdmError {
    CdmError::new(ErrorKind::Config, message)
        .with_context(|c| c.with_side(side).with_config_key(key.to_owned()))
}

/// A connection failure (`CON-001`, `CON-022`, `CON-026`).
pub(crate) fn connect_error(side: Side, message: impl Into<String>) -> CdmError {
    side_error(ErrorKind::Connect, side, message)
}

/// A connection failure that wraps a driver or transport error.
pub(crate) fn connect_error_from(
    side: Side,
    message: impl Into<String>,
    cause: impl Into<Cause>,
) -> CdmError {
    side_error_from(ErrorKind::Connect, side, message, cause)
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
    fn err_001_errors_carry_the_side_they_concern() {
        let err = tls_error(Side::Origin, "bad truststore");
        assert_eq!(err.kind(), ErrorKind::Tls);
        assert!(err.to_string().contains("origin"));
    }

    #[test]
    fn err_001_config_errors_name_the_property() {
        let err = config_error(Side::Target, "both host and scb", "connect.target.scb");
        assert!(err.to_string().contains("connect.target.scb"));
    }

    #[test]
    fn err_001_causes_are_preserved() {
        let cause = std::io::Error::other("disk on fire");
        let err = connect_error_from(Side::Origin, "cannot connect", cause);
        assert!(std::error::Error::source(&err).is_some());
    }
}
