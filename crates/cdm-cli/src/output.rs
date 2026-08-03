//! Rendering command results (`CLI-005`).
//!
//! Every command produces a value that can render as prose for a terminal or as one JSON document
//! for a script. Both come from the same value, so the two can never disagree about what happened
//! — which is the failure mode of tools that build their JSON separately from their text.

use std::io::Write;

use cdm_core::{Diagnostic, Severity};
use serde::Serialize;

use crate::cli::OutputFormat;

/// Anything a command can return.
///
/// `Serialize` gives the JSON rendering for free; [`Report::render_human`] supplies the prose one.
pub trait Report: Serialize {
    /// Writes the human-readable form.
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()>;

    /// Whether the result should be reported as "completed, but look at it" (`Exit::Completed`).
    ///
    /// A validation that found mismatches succeeded as a command and failed as a comparison; the
    /// exit code has to say the second thing.
    fn has_findings(&self) -> bool {
        false
    }
}

/// Writes a report in the requested format.
pub fn emit<R: Report>(
    report: &R,
    format: OutputFormat,
    out: &mut dyn Write,
) -> std::io::Result<()> {
    match format {
        OutputFormat::Human => report.render_human(out),
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut *out, report)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            writeln!(out)
        }
    }
}

/// Renders diagnostics as an operator would want to read them.
///
/// Errors first, then warnings, then notices: the ones that stop the run should not be buried
/// under advice. Each carries its location, the offending value and a suggested fix, because a
/// diagnostic that does not say what to change is only half a diagnostic (`ERR-002`).
pub fn render_diagnostics(diagnostics: &[Diagnostic], out: &mut dyn Write) -> std::io::Result<()> {
    if diagnostics.is_empty() {
        return writeln!(out, "No problems found.");
    }

    for severity in [Severity::Error, Severity::Warning, Severity::Info] {
        let group: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .collect();
        if group.is_empty() {
            continue;
        }

        writeln!(out, "\n{}:", label(severity))?;
        for diagnostic in group {
            write!(out, "  {}", diagnostic.title)?;
            if let Some(location) = &diagnostic.location {
                write!(out, "  [{location}]")?;
            }
            writeln!(out)?;

            if let Some(detail) = &diagnostic.detail {
                writeln!(out, "      {detail}")?;
            }
            if let Some(suggestion) = &diagnostic.suggestion {
                writeln!(out, "      try: {suggestion}")?;
            }
            if let Some(rule) = &diagnostic.rule {
                writeln!(out, "      rule: {rule}")?;
            }
        }
    }
    Ok(())
}

const fn label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "Errors",
        Severity::Warning => "Warnings",
        Severity::Info => "Notices",
    }
}

/// Counts by severity, for a one-line summary.
pub fn counts(diagnostics: &[Diagnostic]) -> (usize, usize, usize) {
    let count = |s: Severity| diagnostics.iter().filter(|d| d.severity == s).count();
    (
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Info),
    )
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

    fn render(diagnostics: &[Diagnostic]) -> String {
        let mut buf = Vec::new();
        render_diagnostics(diagnostics, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn cli_005_an_empty_report_says_so_rather_than_printing_nothing() {
        assert!(render(&[]).contains("No problems found"));
    }

    #[test]
    fn cli_005_errors_are_printed_before_warnings() {
        let diagnostics = vec![
            Diagnostic::warning("CDM-CONFIG", "a warning"),
            Diagnostic::error("CDM-CONFIG", "an error"),
        ];
        let text = render(&diagnostics);
        assert!(
            text.find("an error").unwrap() < text.find("a warning").unwrap(),
            "what blocks the run must not be buried under advice:\n{text}"
        );
    }

    #[test]
    fn cli_005_a_diagnostic_shows_its_location_and_suggestion() {
        let diagnostic = Diagnostic::error("CDM-CONFIG", "the origin has no host")
            .with_location("connect.origin.host")
            .with_suggestion("set `connect.origin.host`");
        let text = render(&[diagnostic]);

        assert!(text.contains("connect.origin.host"));
        assert!(text.contains("try: set `connect.origin.host`"));
    }

    #[test]
    fn cli_005_counts_group_by_severity() {
        let diagnostics = vec![
            Diagnostic::error("C", "e1"),
            Diagnostic::error("C", "e2"),
            Diagnostic::warning("C", "w1"),
        ];
        assert_eq!(counts(&diagnostics), (2, 1, 0));
    }
}
