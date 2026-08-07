//! Small commands: version, codecs, completions, and honest placeholders.

use std::io::Write;

use cdm_core::{CdmError, ErrorKind};
use serde::Serialize;

use crate::output::Report;

/// Build information (`CLI-001`).
#[derive(Debug, Serialize)]
pub struct VersionReport {
    /// The crate version.
    pub version: &'static str,
    /// The CQL driver in use, which determines cluster compatibility (`CON-000`).
    pub driver: &'static str,
    /// Compile-time features.
    pub features: Vec<&'static str>,
}

impl Report for VersionReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "cdm {}", self.version)?;
        writeln!(out, "driver: {}", self.driver)?;
        if !self.features.is_empty() {
            writeln!(out, "features: {}", self.features.join(", "))?;
        }
        Ok(())
    }
}

/// Reports the build (`CLI-001`).
pub fn version() -> VersionReport {
    VersionReport {
        version: env!("CARGO_PKG_VERSION"),
        driver: "scylla-rust-driver",
        features: Vec::new(),
    }
}

/// An operation that is specified but not yet built.
///
/// `missing` names **what is absent and where it will come from**, not a pull-request number. The
/// number was the first attempt and it aged badly: an evaluator reading "delivered by PR #12" has
/// to go and find PR #12 to learn whether the gap is a missing crate, a missing driver feature or
/// an afternoon's wiring, and a renumbered roadmap makes the message a lie. Saying "the control
/// plane lives in `cdm-service`, which is not built" answers the question in the message.
pub fn not_yet(what: &str, missing: &str) -> CdmError {
    CdmError::new(
        ErrorKind::Internal,
        format!("`{what}` is specified but not yet available: {missing}. See docs/ROADMAP.md."),
    )
}
