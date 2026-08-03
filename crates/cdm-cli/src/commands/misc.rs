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
/// Naming the pull request is deliberate. A bare "not implemented" leaves someone evaluating
/// cdm-rs unable to tell a gap from an oversight, and the roadmap is public.
pub fn not_yet(what: &str, pr: &str) -> CdmError {
    CdmError::new(
        ErrorKind::Internal,
        format!("`{what}` is delivered by PR {pr}; see docs/ROADMAP.md"),
    )
}
