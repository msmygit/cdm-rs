//! The single typed configuration model, its loaders, and three-tier validation.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # The one model
//!
//! [`CdmConfig`] is written once, in [`model`], using the [`cdm_properties!`] macro. Every other
//! artefact is a projection of it: the [`PropertyRegistry`] that the Java `.properties` loader
//! resolves aliases against, the JSON Schema at `schema/cdm-config.schema.json`, the property
//! table in `docs/generated/PROPERTIES.md`, and the config-builder UI form. No parallel list of
//! property names exists anywhere in the repository (`CFG-001`, `ADR-0005`).
//!
//! # The pipeline
//!
//! ```text
//! defaults → file → CDM__env → --set/--conf → typed flags → API body
//!     └─ layered merge ─→ secret resolution ─→ tier 1 ─→ tier 2 ─→ tier 3 ─→ EffectiveConfig
//! ```
//!
//! [`ConfigLoader`] performs the merge and the secret resolution; [`Validator`] performs the
//! three tiers, reporting **every** violation rather than the first (`CFG-021`); and
//! [`EffectiveConfig`] is the immutable, hashed result the engine plans from.
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `CFG-001`..`CFG-003` — [`CdmConfig`], [`meta::PropertyMeta`], [`json_schema`]
//! - `CFG-010`..`CFG-013` — [`ConfigLoader`], [`Secret`]
//! - `CFG-020`..`CFG-040`, `CFG-161` — [`Validator`], [`SchemaProvider`]
//! - `CFG-100`..`CFG-200` — the property registry in [`model`]
//! - `UI-004` — [`BestPractices`]

#[macro_use]
pub mod macros;

pub mod best_practices;
pub mod effective;
pub mod explain;
pub mod generate;
pub mod loader;
pub mod meta;
pub mod model;
pub mod registry;
pub mod secret;
pub mod types;
pub mod validate;

pub use best_practices::{
    BestPracticeInputs, BestPracticeReport, BestPractices, Recommendation, TableProfile,
};
pub use effective::EffectiveConfig;
pub use explain::{diff, explain, ConfigDiff, Explanation, PropertyChange};
pub use generate::{json_schema, json_schema_document, properties_markdown};
pub use loader::{ConfigLoader, LoadOutcome, Source};
pub use meta::{PropertyKind, PropertyMeta, Stability};
pub use model::CdmConfig;
pub use registry::PropertyRegistry;
pub use secret::{Secret, SecretSource, SystemSecrets};
pub use validate::{
    ColumnDescription, SchemaProvider, TableDescription, Tier, ValidationOptions, ValidationReport,
    Validator,
};

/// The version of this crate, as reported by `cdm version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    fn version_is_populated() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn err_004_no_production_path_in_this_crate_can_panic() {
        // Clippy denies `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!` workspace-wide, but
        // only where it lints; a source-level sweep also covers code behind a `cfg` that a given
        // clippy invocation does not compile.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).unwrap() {
                    stack.push(entry.unwrap().path());
                }
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Everything from the test module onwards is exempt.
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (offset, line) in production.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for construct in [
                    ".unwrap()",
                    ".expect(",
                    "panic!(",
                    "todo!(",
                    "unimplemented!(",
                ] {
                    if code.contains(construct) {
                        offenders.push(format!("{}:{}", path.display(), offset + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "panicking constructs found: {offenders:?}"
        );
    }
}
