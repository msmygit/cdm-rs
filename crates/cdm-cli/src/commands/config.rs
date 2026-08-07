//! `cdm config …` — the configuration commands.
//!
//! These are the commands that work before a cluster exists, which makes them the ones an operator
//! reaches for first. Everything here delegates to `cdm-config`; the CLI only loads, dispatches and
//! renders, so the rules the API and the web UI enforce are the same rules by construction
//! (`ADR-0005`).

use std::io::Write;
use std::path::Path;

use cdm_config::{
    diff as diff_configs, explain as explain_key, json_schema_document, CdmConfig, ConfigDiff,
    Explanation, LoadOutcome, Validator,
};
use cdm_core::{CdmError, Diagnostic, ErrorKind};
use serde::Serialize;

use crate::cli::{ConfigArgs, Tier};
use crate::loader::load;
use crate::output::{counts, render_diagnostics, Report};

/// The result of `cdm config validate`.
#[derive(Debug, Serialize)]
pub struct ValidateReport {
    /// Whether the run may proceed.
    pub valid: bool,
    /// The highest tier that actually ran.
    pub tier: &'static str,
    /// Everything found, never only the first (`CFG-021`).
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for ValidateReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        render_diagnostics(&self.diagnostics, out)?;
        let (errors, warnings, notices) = counts(&self.diagnostics);
        writeln!(
            out,
            "\n{} through tier {}: {errors} error(s), {warnings} warning(s), {notices} notice(s).",
            if self.valid { "Valid" } else { "Invalid" },
            self.tier
        )
    }

    fn has_findings(&self) -> bool {
        !self.valid
    }
}

/// Validates a configuration to the requested tier (`CFG-020`, `CFG-021`).
///
/// Tier 3 is not reachable from here yet. `cdm-cql`'s `SchemaProvider` exists, but nothing in the
/// CLI opens a session to feed it: that is the shared job harness (roadmap #21a), which the job
/// commands are waiting on too. Asking for tier 3 is an error rather than a silent downgrade —
/// quietly running fewer checks than requested is how a configuration reaches production
/// unvalidated.
pub fn validate(args: &ConfigArgs, tier: Tier) -> Result<ValidateReport, CdmError> {
    if tier == Tier::Schema {
        return Err(CdmError::new(
            ErrorKind::Config,
            "tier 3 validation needs a live cluster session, which the CLI cannot yet open; \
             it lands with the shared job harness (roadmap #21a)",
        )
        .with_context(|c| c.with_config_key("--tier")));
    }

    let outcome = load(args)?;
    let label = match tier {
        Tier::Syntactic => "syntactic",
        Tier::Semantic | Tier::Schema => "semantic",
    };

    // A configuration that could not be assembled has nothing to check, but the loader's
    // diagnostics are exactly what the operator needs — reporting only "could not load" would
    // throw away the reason (`CFG-021`).
    let Some(config) = outcome.config.as_ref() else {
        return Ok(ValidateReport {
            valid: false,
            tier: label,
            diagnostics: outcome.diagnostics,
        });
    };

    let validator = Validator::new();
    let mut checks = validator.tier1(config);
    if tier != Tier::Syntactic {
        checks.extend(validator.tier2(config));
    }

    // The loader's own findings — unknown keys, close-match suggestions, secret-resolution
    // failures — belong in the same report. They are the ones most likely to be the actual
    // problem, and splitting them across two outputs would hide that.
    let mut diagnostics = outcome.diagnostics;
    diagnostics.extend(checks);
    let valid = !diagnostics
        .iter()
        .any(|d| d.severity == cdm_core::Severity::Error);

    Ok(ValidateReport {
        valid,
        tier: label,
        diagnostics,
    })
}

/// The result of `cdm config explain` (`CFG-028`).
#[derive(Debug, Serialize)]
pub struct ExplainReport {
    /// What was found, if the key exists.
    pub explanation: Option<Explanation>,
    /// The key that was asked about.
    pub key: String,
}

impl Report for ExplainReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        match &self.explanation {
            None => writeln!(
                out,
                "No property named `{}`. Try `cdm config schema` for the full list.",
                self.key
            ),
            Some(explanation) => {
                let json = serde_json::to_value(explanation).unwrap_or(serde_json::Value::Null);
                writeln!(out, "{}", self.key)?;
                for (field, value) in json.as_object().into_iter().flatten() {
                    writeln!(out, "  {field}: {value}")?;
                }
                Ok(())
            }
        }
    }
}

/// Explains one property: its value, and where that value came from (`CFG-028`).
///
/// Provenance records *where*, never *what* — printing the value a `--set` carried would leak a
/// password into a terminal and a shell history (`SEC-001`).
pub fn explain(key: &str, args: &ConfigArgs) -> Result<ExplainReport, CdmError> {
    let outcome = load(args)?;
    Ok(ExplainReport {
        explanation: explain_key(key, &outcome),
        key: key.to_owned(),
    })
}

/// The result of `cdm config diff` (`CFG-029`).
#[derive(Debug, Serialize)]
pub struct DiffReport {
    /// The differences, ignoring ordering and defaults.
    pub diff: ConfigDiff,
}

impl Report for DiffReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        if self.diff.changes.is_empty() {
            return writeln!(out, "The two configurations are equivalent.");
        }
        writeln!(out, "{} difference(s):\n", self.diff.changes.len())?;
        for change in &self.diff.changes {
            writeln!(out, "  {}", change.canonical)?;
            writeln!(out, "    left:  {}", change.left.as_deref().unwrap_or("—"))?;
            writeln!(out, "    right: {}", change.right.as_deref().unwrap_or("—"))?;
        }
        Ok(())
    }

    fn has_findings(&self) -> bool {
        !self.diff.changes.is_empty()
    }
}

/// Compares two configuration files (`CFG-029`).
pub fn diff(left: &Path, right: &Path) -> Result<DiffReport, CdmError> {
    let load_one = |path: &Path| -> Result<CdmConfig, CdmError> {
        let args = ConfigArgs {
            config: Some(path.to_path_buf()),
            ..ConfigArgs::default()
        };
        load(&args)?.config.ok_or_else(|| {
            CdmError::new(
                ErrorKind::Config,
                format!("{} could not be loaded as a configuration", path.display()),
            )
        })
    };

    Ok(DiffReport {
        diff: diff_configs(&load_one(left)?, &load_one(right)?),
    })
}

/// The result of `cdm config schema` (`CFG-003`).
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct SchemaReport {
    /// The JSON Schema document.
    pub schema: serde_json::Value,
}

impl Report for SchemaReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        writeln!(out, "{}", json_schema_document())
    }
}

/// Prints the JSON Schema of the configuration model (`CFG-003`).
pub fn schema() -> Result<SchemaReport, CdmError> {
    let schema = serde_json::from_str(&json_schema_document()).map_err(|e| {
        CdmError::new(
            ErrorKind::Internal,
            format!("the generated configuration schema is not valid JSON: {e}"),
        )
    })?;
    Ok(SchemaReport { schema })
}

/// The result of `cdm config convert` (`CLI-003`).
#[derive(Debug, Serialize)]
pub struct ConvertReport {
    /// The converted configuration, as TOML.
    pub config: String,
    /// Anything noticed while reading the Java file — unknown keys, deprecations, defaults.
    pub diagnostics: Vec<Diagnostic>,
}

impl Report for ConvertReport {
    fn render_human(&self, out: &mut dyn Write) -> std::io::Result<()> {
        render_diagnostics(&self.diagnostics, out)?;
        writeln!(out, "\n{}", self.config)
    }
}

/// Converts a Java properties file to canonical TOML (`CLI-003`, `CFG-011`).
pub fn convert(from: &Path, to: Option<&Path>) -> Result<ConvertReport, CdmError> {
    let args = ConfigArgs {
        properties_file: Some(from.to_path_buf()),
        ..ConfigArgs::default()
    };
    let outcome: LoadOutcome = load(&args)?;

    let converted = outcome.config.as_ref().ok_or_else(|| {
        CdmError::new(
            ErrorKind::Config,
            format!(
                "{} could not be read as a Java properties file",
                from.display()
            ),
        )
    })?;
    let config = toml::to_string_pretty(converted).map_err(|e| {
        CdmError::new(
            ErrorKind::Internal,
            format!("cannot render the converted configuration as TOML: {e}"),
        )
    })?;

    if let Some(path) = to {
        std::fs::write(path, &config).map_err(|e| {
            CdmError::new(
                ErrorKind::Config,
                format!("cannot write {}: {e}", path.display()),
            )
        })?;
    }

    Ok(ConvertReport {
        config,
        diagnostics: outcome.diagnostics,
    })
}
