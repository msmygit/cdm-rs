//! The error model (`ERR-001`, `ERR-004`) and user-facing diagnostics (`ERR-002`, `ERR-003`).
//!
//! There is exactly one error type in cdm-rs, [`CdmError`]. Its variants are the stable
//! [`ErrorKind`] codes required by `ERR-001`, and every one of them carries an [`ErrorContext`]
//! naming the side, keyspace, table, column, token range and primary key involved, so an error
//! surfaced from a worker is actionable without a stack trace.
//!
//! [`Diagnostic`] is the *presentation* of an error (or of a validation finding that is not an
//! error at all). `ERR-002` requires the same value to render as CLI text, as `problem+json` and
//! as an SSE event; this crate owns the value and its CLI rendering, while the JSON and SSE
//! renderings are the transports' business in `cdm-api`.

pub mod diagnostic;

pub use diagnostic::{Diagnostic, Severity};

use std::fmt;

use crate::domain::{PrimaryKey, Side, TableRef, TokenRange};

/// A boxed underlying cause. Kept behind an alias because every variant carries one.
pub type BoxSource = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The stable, documented error codes of `ERR-001`.
///
/// The set is closed and the string forms are a public contract: they appear in `problem+json`
/// bodies, in log records and in the `docs/errors/` page names (`ERR-003`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorKind {
    /// Invalid, missing or contradictory configuration. Detected before any data moves.
    Config,
    /// A cluster could not be reached, or the session could not be established.
    Connect,
    /// Credentials were rejected, or none were supplied where the cluster requires them.
    Auth,
    /// TLS material could not be loaded, parsed or verified.
    Tls,
    /// Origin and target schemas cannot be reconciled for the requested job.
    SchemaMismatch,
    /// A value could not be converted between the origin and target CQL types.
    TypeConversion,
    /// A read against the origin (or, when validating, the target) failed.
    Read,
    /// A write against the target failed.
    Write,
    /// The configured rate limiter or the cluster rejected the request as overloaded.
    RateLimited,
    /// The run-tracking store could not be read or written.
    Tracking,
    /// A distributed-mode lease could not be acquired, renewed, or was lost.
    Lease,
    /// The run was cancelled by the operator or by a shutdown signal.
    Cancelled,
    /// An invariant of cdm-rs itself was violated. Always a bug.
    Internal,
}

impl ErrorKind {
    /// Every kind, in declaration order. Exhaustive by construction: adding a variant without
    /// adding it here fails `err_001_all_lists_every_kind`.
    pub const ALL: [Self; 13] = [
        Self::Config,
        Self::Connect,
        Self::Auth,
        Self::Tls,
        Self::SchemaMismatch,
        Self::TypeConversion,
        Self::Read,
        Self::Write,
        Self::RateLimited,
        Self::Tracking,
        Self::Lease,
        Self::Cancelled,
        Self::Internal,
    ];

    /// The stable string form, identical to the variant name.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Config => "Config",
            Self::Connect => "Connect",
            Self::Auth => "Auth",
            Self::Tls => "Tls",
            Self::SchemaMismatch => "SchemaMismatch",
            Self::TypeConversion => "TypeConversion",
            Self::Read => "Read",
            Self::Write => "Write",
            Self::RateLimited => "RateLimited",
            Self::Tracking => "Tracking",
            Self::Lease => "Lease",
            Self::Cancelled => "Cancelled",
            Self::Internal => "Internal",
        }
    }

    /// The diagnostic code this kind maps to, e.g. `CDM-SCHEMA-MISMATCH` (`ERR-003`).
    ///
    /// One page per kind, not per call site: a code has to have a documentation page, and a page
    /// per call site would be neither writable nor readable.
    pub const fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::Config => "CDM-CONFIG",
            Self::Connect => "CDM-CONNECT",
            Self::Auth => "CDM-AUTH",
            Self::Tls => "CDM-TLS",
            Self::SchemaMismatch => "CDM-SCHEMA-MISMATCH",
            Self::TypeConversion => "CDM-TYPE-CONVERSION",
            Self::Read => "CDM-READ",
            Self::Write => "CDM-WRITE",
            Self::RateLimited => "CDM-RATE-LIMITED",
            Self::Tracking => "CDM-TRACKING",
            Self::Lease => "CDM-LEASE",
            Self::Cancelled => "CDM-CANCELLED",
            Self::Internal => "CDM-INTERNAL",
        }
    }

    /// Whether the operation may usefully be retried with backoff.
    ///
    /// This is the `Kind` decision of the failure-isolation flowchart in `ARCHITECTURE.md` §13:
    /// transport-level failures are retried, everything else is not. Whether a *particular* retry
    /// is safe additionally requires idempotence — counters are never retried (`MIG-032`) — which
    /// is the engine's call, not the error's.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Read | Self::Write | Self::RateLimited)
    }

    /// Whether encountering this kind must abort the whole run rather than fail one range.
    ///
    /// `ARCHITECTURE.md` §13 names `Config`, `Auth`, `Tls` and `SchemaMismatch` as never
    /// transient. `Connect` joins them: cdm-rs establishes its sessions at startup, so a
    /// connection error is a misconfiguration rather than a blip — in-run node failures surface
    /// as `Read`/`Write` instead. `Internal` aborts because the process is in an unknown state.
    pub const fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::Config
                | Self::Connect
                | Self::Auth
                | Self::Tls
                | Self::SchemaMismatch
                | Self::Internal
        )
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The context every [`CdmError`] carries (`ERR-001`).
///
/// Each field is optional because not every kind has every dimension — a config error has no
/// token range, a lease error has no column. Fields are populated as an error travels outwards:
/// a codec reports the column, the range worker adds the range and primary key.
///
/// Values are deliberately *not* a field here. `SEC-002` forbids logging row values outside the
/// validate diff path; the primary key is the documented exception (`ARCHITECTURE.md` §13,
/// record-level isolation) and is rendered as hex, never as text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorContext {
    /// Which cluster the failure concerns.
    pub side: Option<Side>,
    /// The keyspace and table involved.
    pub table: Option<TableRef>,
    /// The column involved.
    pub column: Option<String>,
    /// The unit of work being processed.
    pub range: Option<TokenRange>,
    /// The primary key of the row being processed.
    pub primary_key: Option<PrimaryKey>,
    /// The configuration property responsible, e.g. `spark.cdm.perfops.numParts`.
    pub config_key: Option<String>,
}

impl ErrorContext {
    /// An empty context.
    pub const fn new() -> Self {
        Self {
            side: None,
            table: None,
            column: None,
            range: None,
            primary_key: None,
            config_key: None,
        }
    }

    /// Whether no dimension has been recorded, in which case rendering it contributes nothing.
    pub const fn is_empty(&self) -> bool {
        self.side.is_none()
            && self.table.is_none()
            && self.column.is_none()
            && self.range.is_none()
            && self.primary_key.is_none()
            && self.config_key.is_none()
    }

    /// Records the cluster the failure concerns.
    #[must_use]
    pub fn with_side(mut self, side: Side) -> Self {
        self.side = Some(side);
        self
    }

    /// Records the table involved.
    #[must_use]
    pub fn with_table(mut self, table: TableRef) -> Self {
        self.table = Some(table);
        self
    }

    /// Records the column involved.
    #[must_use]
    pub fn with_column(mut self, column: impl Into<String>) -> Self {
        self.column = Some(column.into());
        self
    }

    /// Records the token range being processed.
    #[must_use]
    pub fn with_range(mut self, range: TokenRange) -> Self {
        self.range = Some(range);
        self
    }

    /// Records the primary key of the row being processed.
    #[must_use]
    pub fn with_primary_key(mut self, key: PrimaryKey) -> Self {
        self.primary_key = Some(key);
        self
    }

    /// Records the configuration property responsible.
    #[must_use]
    pub fn with_config_key(mut self, key: impl Into<String>) -> Self {
        self.config_key = Some(key.into());
        self
    }
}

impl fmt::Display for ErrorContext {
    /// Renders as ` [origin ks.tbl column=c range=[0, 9] pk=(0x01)]`, or as nothing when empty,
    /// so it can be appended unconditionally to any message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return Ok(());
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(side) = self.side {
            parts.push(side.to_string());
        }
        if let Some(table) = &self.table {
            parts.push(table.to_string());
        }
        if let Some(column) = &self.column {
            parts.push(format!("column={column}"));
        }
        if let Some(range) = &self.range {
            parts.push(format!("range={range}"));
        }
        if let Some(key) = &self.primary_key {
            parts.push(format!("pk={key}"));
        }
        if let Some(key) = &self.config_key {
            parts.push(format!("key={key}"));
        }
        write!(f, " [{}]", parts.join(" "))
    }
}

/// The one error type of cdm-rs (`ERR-001`).
///
/// Construct with [`CdmError::new`] and enrich as the error propagates:
///
/// ```
/// use cdm_core::{CdmError, ErrorKind, Side, TableRef};
///
/// let err = CdmError::new(ErrorKind::Write, "write timeout after 3 attempts")
///     .with_context(|c| c.with_side(Side::Target).with_table(TableRef::new("ks", "tbl")));
///
/// assert_eq!(err.kind(), ErrorKind::Write);
/// assert!(err.is_retryable());
/// assert!(err.to_string().contains("target ks.tbl"));
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CdmError {
    /// See [`ErrorKind::Config`].
    #[error("configuration error: {message}{context}")]
    Config {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Connect`].
    #[error("connection error: {message}{context}")]
    Connect {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Auth`].
    #[error("authentication error: {message}{context}")]
    Auth {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Tls`].
    #[error("TLS error: {message}{context}")]
    Tls {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::SchemaMismatch`].
    #[error("schema mismatch: {message}{context}")]
    SchemaMismatch {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::TypeConversion`].
    #[error("type conversion error: {message}{context}")]
    TypeConversion {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Read`].
    #[error("read error: {message}{context}")]
    Read {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Write`].
    #[error("write error: {message}{context}")]
    Write {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::RateLimited`].
    #[error("rate limited: {message}{context}")]
    RateLimited {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Tracking`].
    #[error("run tracking error: {message}{context}")]
    Tracking {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Lease`].
    #[error("lease error: {message}{context}")]
    Lease {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Cancelled`].
    #[error("cancelled: {message}{context}")]
    Cancelled {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
    /// See [`ErrorKind::Internal`].
    #[error("internal error: {message}{context}")]
    Internal {
        /// What went wrong.
        message: String,
        /// Where it went wrong. Boxed so that `CdmError` stays small enough to return by
        /// value on the hot path without bloating every `Result`.
        context: Box<ErrorContext>,
        /// The underlying cause, if any.
        #[source]
        source: Option<BoxSource>,
    },
}

/// Expands to a `match` over every variant, binding the three common fields. Keeps the thirteen
/// accessors below honest: a new variant that is not added here fails to compile.
macro_rules! for_each_variant {
    ($self:expr, |$message:ident, $context:ident, $source:ident| $body:expr) => {
        match $self {
            CdmError::Config {
                $message,
                $context,
                $source,
            }
            | CdmError::Connect {
                $message,
                $context,
                $source,
            }
            | CdmError::Auth {
                $message,
                $context,
                $source,
            }
            | CdmError::Tls {
                $message,
                $context,
                $source,
            }
            | CdmError::SchemaMismatch {
                $message,
                $context,
                $source,
            }
            | CdmError::TypeConversion {
                $message,
                $context,
                $source,
            }
            | CdmError::Read {
                $message,
                $context,
                $source,
            }
            | CdmError::Write {
                $message,
                $context,
                $source,
            }
            | CdmError::RateLimited {
                $message,
                $context,
                $source,
            }
            | CdmError::Tracking {
                $message,
                $context,
                $source,
            }
            | CdmError::Lease {
                $message,
                $context,
                $source,
            }
            | CdmError::Cancelled {
                $message,
                $context,
                $source,
            }
            | CdmError::Internal {
                $message,
                $context,
                $source,
            } => $body,
        }
    };
}

impl CdmError {
    /// Creates an error of the given kind with an empty context and no cause.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        let message = message.into();
        let context = Box::new(ErrorContext::new());
        let source = None;
        match kind {
            ErrorKind::Config => Self::Config {
                message,
                context,
                source,
            },
            ErrorKind::Connect => Self::Connect {
                message,
                context,
                source,
            },
            ErrorKind::Auth => Self::Auth {
                message,
                context,
                source,
            },
            ErrorKind::Tls => Self::Tls {
                message,
                context,
                source,
            },
            ErrorKind::SchemaMismatch => Self::SchemaMismatch {
                message,
                context,
                source,
            },
            ErrorKind::TypeConversion => Self::TypeConversion {
                message,
                context,
                source,
            },
            ErrorKind::Read => Self::Read {
                message,
                context,
                source,
            },
            ErrorKind::Write => Self::Write {
                message,
                context,
                source,
            },
            ErrorKind::RateLimited => Self::RateLimited {
                message,
                context,
                source,
            },
            ErrorKind::Tracking => Self::Tracking {
                message,
                context,
                source,
            },
            ErrorKind::Lease => Self::Lease {
                message,
                context,
                source,
            },
            ErrorKind::Cancelled => Self::Cancelled {
                message,
                context,
                source,
            },
            ErrorKind::Internal => Self::Internal {
                message,
                context,
                source,
            },
        }
    }

    /// The stable kind code.
    pub const fn kind(&self) -> ErrorKind {
        match self {
            Self::Config { .. } => ErrorKind::Config,
            Self::Connect { .. } => ErrorKind::Connect,
            Self::Auth { .. } => ErrorKind::Auth,
            Self::Tls { .. } => ErrorKind::Tls,
            Self::SchemaMismatch { .. } => ErrorKind::SchemaMismatch,
            Self::TypeConversion { .. } => ErrorKind::TypeConversion,
            Self::Read { .. } => ErrorKind::Read,
            Self::Write { .. } => ErrorKind::Write,
            Self::RateLimited { .. } => ErrorKind::RateLimited,
            Self::Tracking { .. } => ErrorKind::Tracking,
            Self::Lease { .. } => ErrorKind::Lease,
            Self::Cancelled { .. } => ErrorKind::Cancelled,
            Self::Internal { .. } => ErrorKind::Internal,
        }
    }

    /// The message, without the rendered context.
    pub fn message(&self) -> &str {
        for_each_variant!(self, |message, context, source| {
            let _ = (context, source);
            message
        })
    }

    /// The context accumulated so far.
    pub fn context(&self) -> &ErrorContext {
        for_each_variant!(self, |message, context, source| {
            let _ = (message, source);
            context
        })
    }

    /// Enriches the context in place, e.g. when an error crosses a range boundary.
    #[must_use]
    pub fn with_context(mut self, f: impl FnOnce(ErrorContext) -> ErrorContext) -> Self {
        for_each_variant!(&mut self, |message, context, source| {
            let _ = (message, source);
            **context = f(std::mem::take(&mut **context));
        });
        self
    }

    /// Attaches the underlying cause, replacing any previous one.
    #[must_use]
    pub fn with_source(mut self, cause: impl Into<BoxSource>) -> Self {
        for_each_variant!(&mut self, |message, context, source| {
            let _ = (message, context);
            *source = Some(cause.into());
        });
        self
    }

    /// See [`ErrorKind::is_retryable`].
    pub const fn is_retryable(&self) -> bool {
        self.kind().is_retryable()
    }

    /// See [`ErrorKind::is_fatal`].
    pub const fn is_fatal(&self) -> bool {
        self.kind().is_fatal()
    }

    /// Renders the error as the user-facing [`Diagnostic`] required by `ERR-002`.
    ///
    /// The title is the message; the detail is the underlying cause, if any; the location is the
    /// rendered context; and `docs_url` points at the kind's page (`ERR-003`).
    pub fn to_diagnostic(&self) -> Diagnostic {
        let code = self.kind().diagnostic_code();
        let context = self.context();
        let mut diagnostic = Diagnostic::error(code, self.message());
        if let Some(source) = std::error::Error::source(self) {
            diagnostic = diagnostic.with_detail(source.to_string());
        }
        if !context.is_empty() {
            diagnostic = diagnostic.with_location(context.to_string().trim());
        }
        if let Some(key) = &context.config_key {
            diagnostic = diagnostic.with_rule(key.clone());
        }
        diagnostic
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
    use std::collections::BTreeSet;
    use std::error::Error as _;

    use super::*;
    use crate::domain::RawCell;

    #[test]
    fn err_001_all_lists_every_kind_exactly_once() {
        let unique: BTreeSet<_> = ErrorKind::ALL.iter().collect();
        assert_eq!(unique.len(), ErrorKind::ALL.len());
        // The thirteen codes named in ERR-001.
        let names: Vec<&str> = ErrorKind::ALL.iter().map(ErrorKind::as_str).collect();
        assert_eq!(
            names,
            vec![
                "Config",
                "Connect",
                "Auth",
                "Tls",
                "SchemaMismatch",
                "TypeConversion",
                "Read",
                "Write",
                "RateLimited",
                "Tracking",
                "Lease",
                "Cancelled",
                "Internal",
            ]
        );
    }

    #[test]
    fn err_001_every_kind_round_trips_through_a_variant() {
        for kind in ErrorKind::ALL {
            let err = CdmError::new(kind, "boom");
            assert_eq!(err.kind(), kind, "{kind} did not round-trip");
            assert_eq!(err.message(), "boom");
            assert!(err.context().is_empty());
            assert!(err.source().is_none());
            assert!(err.to_string().contains("boom"));
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn err_001_every_variant_carries_the_full_context() {
        let pk = PrimaryKey::new(vec![RawCell::from_static(&[0x01, 0xab])]);
        for kind in ErrorKind::ALL {
            let err = CdmError::new(kind, "boom").with_context(|c| {
                c.with_side(Side::Origin)
                    .with_table(TableRef::new("ks", "tbl"))
                    .with_column("c1")
                    .with_range(TokenRange::new(0, 9).unwrap())
                    .with_primary_key(pk.clone())
                    .with_config_key("spark.cdm.perfops.numParts")
            });
            let rendered = err.to_string();
            assert!(rendered.contains("origin ks.tbl"), "{rendered}");
            assert!(rendered.contains("column=c1"), "{rendered}");
            assert!(rendered.contains("range=[0, 9]"), "{rendered}");
            assert!(rendered.contains("pk=(0x01ab)"), "{rendered}");
            assert!(
                rendered.contains("key=spark.cdm.perfops.numParts"),
                "{rendered}"
            );
            assert_eq!(err.context().column.as_deref(), Some("c1"));
        }
    }

    #[test]
    fn err_001_context_renders_nothing_when_empty() {
        assert_eq!(ErrorContext::new().to_string(), "");
        assert!(ErrorContext::default().is_empty());
        assert_eq!(
            CdmError::new(ErrorKind::Read, "timeout").to_string(),
            "read error: timeout"
        );
    }

    #[test]
    fn err_001_source_is_preserved_and_exposed() {
        let cause = std::io::Error::other("underlying");
        let err = CdmError::new(ErrorKind::Tracking, "cannot write run info").with_source(cause);
        assert_eq!(
            err.source().map(ToString::to_string).as_deref(),
            Some("underlying")
        );
    }

    #[test]
    fn err_001_kinds_are_classified_for_failure_isolation() {
        for kind in ErrorKind::ALL {
            // ARCHITECTURE §13: nothing is both retried and fatal.
            assert!(
                !(kind.is_retryable() && kind.is_fatal()),
                "{kind} is both retryable and fatal"
            );
        }
        assert!(ErrorKind::Read.is_retryable());
        assert!(ErrorKind::Write.is_retryable());
        assert!(ErrorKind::RateLimited.is_retryable());
        assert!(!ErrorKind::TypeConversion.is_retryable());
        assert!(CdmError::new(ErrorKind::Write, "x").is_retryable());

        for kind in [
            ErrorKind::Config,
            ErrorKind::Connect,
            ErrorKind::Auth,
            ErrorKind::Tls,
            ErrorKind::SchemaMismatch,
            ErrorKind::Internal,
        ] {
            assert!(kind.is_fatal(), "{kind} must abort the run");
        }
        assert!(!ErrorKind::TypeConversion.is_fatal());
        assert!(!CdmError::new(ErrorKind::Cancelled, "ctrl-c").is_fatal());
    }

    #[test]
    fn err_003_every_kind_has_a_distinct_diagnostic_code() {
        let codes: BTreeSet<&str> = ErrorKind::ALL
            .iter()
            .map(ErrorKind::diagnostic_code)
            .collect();
        assert_eq!(codes.len(), ErrorKind::ALL.len());
        assert!(codes.iter().all(|c| c.starts_with("CDM-")));
        assert_eq!(
            ErrorKind::SchemaMismatch.diagnostic_code(),
            "CDM-SCHEMA-MISMATCH"
        );
    }

    #[test]
    fn err_002_errors_convert_to_diagnostics_with_code_location_and_detail() {
        let err = CdmError::new(ErrorKind::TypeConversion, "cannot convert text to int")
            .with_context(|c| {
                c.with_side(Side::Origin)
                    .with_table(TableRef::new("ks", "tbl"))
                    .with_column("age")
                    .with_config_key("spark.cdm.schema.origin.column.names.to.target")
            })
            .with_source(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "bad digit",
            ));

        let diagnostic = err.to_diagnostic();
        assert_eq!(diagnostic.code, "CDM-TYPE-CONVERSION");
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.title, "cannot convert text to int");
        assert_eq!(diagnostic.detail.as_deref(), Some("bad digit"));
        assert_eq!(
            diagnostic.location.as_deref(),
            Some("[origin ks.tbl column=age key=spark.cdm.schema.origin.column.names.to.target]")
        );
        assert_eq!(
            diagnostic.rule.as_deref(),
            Some("spark.cdm.schema.origin.column.names.to.target")
        );
    }

    #[test]
    fn err_002_a_bare_error_converts_without_optional_fields() {
        let diagnostic = CdmError::new(ErrorKind::Cancelled, "run cancelled").to_diagnostic();
        assert_eq!(diagnostic.code, "CDM-CANCELLED");
        assert!(diagnostic.detail.is_none());
        assert!(diagnostic.location.is_none());
        assert!(diagnostic.rule.is_none());
        assert!(diagnostic.docs_url.is_some());
    }

    #[test]
    fn err_004_enriching_an_error_never_panics_on_a_missing_field() {
        // `with_context` takes the existing context by value; doing it twice must not lose data
        // or panic on the temporarily-taken value.
        let err = CdmError::new(ErrorKind::Lease, "lease lost")
            .with_context(|c| c.with_side(Side::Target))
            .with_context(|c| c.with_column("k"));
        assert_eq!(err.context().side, Some(Side::Target));
        assert_eq!(err.context().column.as_deref(), Some("k"));
    }
}
