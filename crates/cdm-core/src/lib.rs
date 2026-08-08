//! Domain vocabulary, error model, diagnostics and the plugin registry. Zero I/O.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What is here
//!
//! `cdm-core` is the crate every other crate depends on and that depends on none of them. It
//! holds three things:
//!
//! * **the vocabulary** — [`TokenRange`], [`PartitionRangeId`], [`RunId`], [`JobKind`],
//!   [`RunStatus`], [`Record`], [`PrimaryKey`] and the small types around them;
//! * **the error model** — one [`CdmError`] enum with the stable [`ErrorKind`] codes of
//!   `ERR-001`, and the [`Diagnostic`] that renders it for a human (`ERR-002`);
//! * **the plugin surface** — the nine traits of `PLG-001`..`PLG-007` and the single
//!   [`Registry`] every implementation, built-in or third-party, registers through (`PLG-010`).
//!
//! # Zero I/O
//!
//! This crate performs no I/O, opens no sockets, reads no clock and starts no runtime, so a
//! plugin author can implement its traits without pulling in axum, the CQL driver, or Tokio
//! (`ARCHITECTURE.md` §3.2). Where a trait needs to name something a later crate owns — a
//! configuration, a schema, a counter snapshot — `cdm-core` defines a minimal placeholder in
//! [`registry::context`] and documents which crate supersedes it.
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `ERR-001` — [`CdmError`], [`ErrorKind`], [`ErrorContext`]
//! - `ERR-002` — [`Diagnostic`], [`Severity`]
//! - `ERR-003` — [`Diagnostic::docs_url_for`] (the pages themselves land with the CLI)
//! - `ERR-004` — no panicking path anywhere in this crate
//! - `PLG-001` — [`CodecPlugin`]
//! - `PLG-002` — [`FeaturePlugin`]
//! - `PLG-003` — [`FilterPlugin`], [`GuardrailPlugin`]
//! - `PLG-004` — [`JobPlugin`], [`JobRunner`]
//! - `PLG-005` — [`RowSource`], [`RowStream`], [`RowSink`]
//! - `PLG-006` — [`MetricsExporter`]
//! - `PLG-007` — [`TrackingStore`]
//! - `MET-010` — [`RequestObserver`], [`Operation`], [`RetryCause`] (the seam; the instruments
//!   they feed live in `cdm-metrics`)
//! - `PLG-010` — [`Registry`], [`RegistryBuilder`]
//! - `PLG-013` — [`Plugin::config_schema`]

pub mod domain;
pub mod error;
pub mod observe;
pub mod registry;

pub use domain::{
    ColumnRef, ExplodedEntry, JobKind, PartitionRangeId, PrimaryKey, RawCell, Record, Row, RunId,
    RunIdGenerator, RunStatus, Side, TableRef, TokenRange,
};
pub use error::{CdmError, Diagnostic, ErrorContext, ErrorKind, Severity};
pub use observe::{Operation, RequestObserver, RetryCause};
pub use registry::{
    BindingBuilder, CodecPlugin, CompareHook, EffectiveConfig, FeaturePlugin, FilterPlugin,
    GuardrailPlugin, JobPlugin, JobRunner, LeaseOutcome, LeaseRecord, LeaseStore, MetricsExporter,
    MetricsSnapshot, Plugin, ProjectionBuilder, RangeOutcome, RangeRecord, RecordSink, Registry,
    RegistryBuilder, RowSink, RowSource, RowStream, RunClaim, RunRecord, SchemaPair, TableView,
    TrackingStore, TypePair,
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
