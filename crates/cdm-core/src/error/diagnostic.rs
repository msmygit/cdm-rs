//! The user-facing diagnostic (`ERR-002`) and its CLI rendering.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Base URL of the per-code documentation pages required by `ERR-003`.
///
/// `docs_url` must point at `docs/errors/<CODE>.md`; CI fails if a code has no page.
const DOCS_BASE_URL: &str = "https://github.com/msmygit/cdm-rs/blob/main/docs/errors/";

/// How much a diagnostic matters.
///
/// The three tiers of `CFG-020`..`CFG-040` map onto this: hard validation failures are
/// [`Severity::Error`], best-practice findings are [`Severity::Warning`], and informational notes
/// (for example "column will be defaulted") are [`Severity::Info`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The run cannot proceed.
    Error,
    /// The run can proceed, but the operator almost certainly wants to know.
    Warning,
    /// Context that helps the operator confirm the run does what they intended.
    Info,
}

impl Severity {
    /// The stable lowercase string form, as it appears in CLI output and `problem+json`.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        }
    }

    /// Whether a diagnostic of this severity must stop the run.
    pub const fn is_blocking(&self) -> bool {
        matches!(self, Self::Error)
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One user-visible finding (`ERR-002`).
///
/// Every user-visible error is a `Diagnostic`, and the same value renders three ways: as CLI text
/// through [`Display`](fmt::Display) here, as RFC 9457 `problem+json` and as an SSE event through
/// its [`Serialize`] implementation in `cdm-api`. Keeping one value and three renderings is what
/// makes `TST-050` — identical behaviour across CLI, REST, MCP and A2A — structural.
///
/// The fields are exactly those named by `ERR-002`, in that order.
///
/// ```
/// use cdm_core::Diagnostic;
///
/// let d = Diagnostic::error("CDM-CONFIG", "unknown property")
///     .with_location("cdm.properties:12")
///     .with_value("spark.cdm.orgin.host")
///     .with_rule("CFG-021")
///     .with_suggestion("did you mean `spark.cdm.origin.host`?");
///
/// assert!(d.to_string().starts_with("error[CDM-CONFIG]: unknown property"));
/// ```
///
/// # Security
///
/// `value` is rendered verbatim. `SEC-001` forbids secrets in any user-visible output, so callers
/// must never place a resolved credential in it; `SEC-002` forbids row values outside the validate
/// diff path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The stable machine-readable code, e.g. `CDM-CONFIG`. Has a page in `docs/errors/`.
    pub code: String,
    /// How much it matters.
    pub severity: Severity,
    /// One line, no trailing punctuation, describing what is wrong.
    pub title: String,
    /// Optional prose expanding on the title, including any underlying cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Where the problem is: a config file and line, a property key, or a table and column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The offending value, if quoting it helps and it is not a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The requirement or best-practice rule that was violated, e.g. `CFG-031`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// What the operator should do about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// Link to the page documenting `code` (`ERR-003`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
}

impl Diagnostic {
    /// Creates a diagnostic, defaulting `docs_url` to the page for `code` (`ERR-003`).
    pub fn new(code: impl Into<String>, severity: Severity, title: impl Into<String>) -> Self {
        let code = code.into();
        let docs_url = Some(Self::docs_url_for(&code));
        Self {
            code,
            severity,
            title: title.into(),
            detail: None,
            location: None,
            value: None,
            rule: None,
            suggestion: None,
            docs_url,
        }
    }

    /// A blocking diagnostic.
    pub fn error(code: impl Into<String>, title: impl Into<String>) -> Self {
        Self::new(code, Severity::Error, title)
    }

    /// A non-blocking diagnostic the operator should read.
    pub fn warning(code: impl Into<String>, title: impl Into<String>) -> Self {
        Self::new(code, Severity::Warning, title)
    }

    /// An informational diagnostic.
    pub fn info(code: impl Into<String>, title: impl Into<String>) -> Self {
        Self::new(code, Severity::Info, title)
    }

    /// The canonical documentation URL for a code (`ERR-003`).
    pub fn docs_url_for(code: &str) -> String {
        format!("{DOCS_BASE_URL}{code}.md")
    }

    /// Sets [`Diagnostic::detail`].
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Sets [`Diagnostic::location`].
    #[must_use]
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Sets [`Diagnostic::value`]. Never pass a secret (`SEC-001`).
    #[must_use]
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Sets [`Diagnostic::rule`].
    #[must_use]
    pub fn with_rule(mut self, rule: impl Into<String>) -> Self {
        self.rule = Some(rule.into());
        self
    }

    /// Sets [`Diagnostic::suggestion`].
    #[must_use]
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Overrides [`Diagnostic::docs_url`], which otherwise follows `code`.
    #[must_use]
    pub fn with_docs_url(mut self, docs_url: impl Into<String>) -> Self {
        self.docs_url = Some(docs_url.into());
        self
    }

    /// Whether this diagnostic must stop the run.
    pub const fn is_blocking(&self) -> bool {
        self.severity.is_blocking()
    }
}

impl fmt::Display for Diagnostic {
    /// The CLI rendering of `ERR-002`, modelled on rustc so it is immediately legible:
    ///
    /// ```text
    /// error[CDM-CONFIG]: unknown property
    ///   --> cdm.properties:12
    ///    = detail: no such key in the property registry
    ///    = value: spark.cdm.orgin.host
    ///    = rule: CFG-021
    ///    = help: did you mean `spark.cdm.origin.host`?
    ///    = docs: https://github.com/msmygit/cdm-rs/blob/main/docs/errors/CDM-CONFIG.md
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]: {}", self.severity, self.code, self.title)?;
        if let Some(location) = &self.location {
            write!(f, "\n  --> {location}")?;
        }
        for (label, value) in [
            ("detail", self.detail.as_ref()),
            ("value", self.value.as_ref()),
            ("rule", self.rule.as_ref()),
            ("help", self.suggestion.as_ref()),
            ("docs", self.docs_url.as_ref()),
        ] {
            if let Some(value) = value {
                write!(f, "\n   = {label}: {value}")?;
            }
        }
        Ok(())
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

    #[test]
    fn err_002_diagnostic_has_exactly_the_specified_fields() {
        let json = serde_json::to_value(
            Diagnostic::error("CDM-CONFIG", "t")
                .with_detail("d")
                .with_location("l")
                .with_value("v")
                .with_rule("r")
                .with_suggestion("s"),
        )
        .unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "code",
                "detail",
                "docs_url",
                "location",
                "rule",
                "severity",
                "suggestion",
                "title",
                "value",
            ]
        );
    }

    #[test]
    fn err_002_diagnostic_renders_location_and_suggestion() {
        let rendered = Diagnostic::error("CDM-CONFIG", "unknown property")
            .with_location("cdm.properties:12")
            .with_detail("no such key in the property registry")
            .with_value("spark.cdm.orgin.host")
            .with_rule("CFG-021")
            .with_suggestion("did you mean `spark.cdm.origin.host`?")
            .to_string();

        assert_eq!(
            rendered,
            concat!(
                "error[CDM-CONFIG]: unknown property\n",
                "  --> cdm.properties:12\n",
                "   = detail: no such key in the property registry\n",
                "   = value: spark.cdm.orgin.host\n",
                "   = rule: CFG-021\n",
                "   = help: did you mean `spark.cdm.origin.host`?\n",
                "   = docs: https://github.com/msmygit/cdm-rs/blob/main/docs/errors/CDM-CONFIG.md",
            )
        );
    }

    #[test]
    fn err_002_a_minimal_diagnostic_renders_one_line_plus_its_docs_link() {
        let rendered = Diagnostic::info("CDM-READ", "nothing to do")
            .with_docs_url("https://example.invalid/x")
            .to_string();
        assert_eq!(
            rendered,
            "info[CDM-READ]: nothing to do\n   = docs: https://example.invalid/x"
        );
    }

    #[test]
    fn err_002_severity_controls_blocking_and_serialises_lowercase() {
        assert!(Diagnostic::error("C", "t").is_blocking());
        assert!(!Diagnostic::warning("C", "t").is_blocking());
        assert!(!Diagnostic::info("C", "t").is_blocking());
        assert_eq!(Severity::Warning.to_string(), "warning");
        assert_eq!(serde_json::to_string(&Severity::Info).unwrap(), "\"info\"");
        assert_eq!(
            serde_json::from_str::<Severity>("\"error\"").unwrap(),
            Severity::Error
        );
        assert!(
            Severity::Error < Severity::Warning,
            "severities order by urgency"
        );
    }

    #[test]
    fn err_002_diagnostic_round_trips_through_json() {
        let original = Diagnostic::warning("CDM-WRITE", "slow target").with_detail("p99 > 1s");
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<Diagnostic>(&json).unwrap(), original);
        // Absent fields are omitted rather than serialised as null, which problem+json wants.
        assert!(!json.contains("\"value\""));
    }

    #[test]
    fn err_003_docs_url_defaults_to_the_page_for_the_code() {
        let diagnostic = Diagnostic::error("CDM-LEASE", "lease lost");
        assert_eq!(
            diagnostic.docs_url.as_deref(),
            Some("https://github.com/msmygit/cdm-rs/blob/main/docs/errors/CDM-LEASE.md")
        );
        assert_eq!(
            Diagnostic::docs_url_for("CDM-AUTH"),
            "https://github.com/msmygit/cdm-rs/blob/main/docs/errors/CDM-AUTH.md"
        );
    }
}
