//! CQL type taxonomy, conversion planner and the codec registry.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What is here
//!
//! * **[`CqlTypeInfo`]** — a driver-independent CQL type tree covering every primitive, the
//!   collections, tuples, UDTs, `vector<T, N>` and the DSE custom types (`CDC-001`..`CDC-004`).
//! * **[`Planner`] / [`ConversionPlan`]** — the conversion plan, resolved once per column pair at
//!   startup and applied per row (`CDC-010`..`CDC-016`).
//! * **[`CodecRegistry`] / [`Converter`]** — the pluggable codec registry (`CDC-030`,
//!   `CDC-031`), populated from `cdm-core`'s [`Registry`](cdm_core::Registry) by ordinary
//!   [`CodecPlugin`](cdm_core::CodecPlugin)s.
//! * **[`Codecset`]** — the built-in codecs, with Java-identical semantics (`CDC-020`,
//!   `CDC-021`), and [`JavaDateFormat`] for the Java date patterns they accept (`CDC-022`).
//!
//! # No driver here
//!
//! This crate does not depend on `scylla` or on `cdm-cql` (`ARCHITECTURE.md` §3.2). Everything
//! operates on [`CqlTypeInfo`] and raw byte buffers, so codecs are unit-testable with no session
//! and stay valid if the driver is ever swapped. The corollary is the point of `MIG-040`: a
//! [`ConversionPlan::Passthrough`] never decodes anything at all.
//!
//! ```
//! use cdm_codec::{CodecRegistry, Codecset, CqlTypeInfo, Planner, PlannerOptions};
//! use cdm_core::RawCell;
//!
//! // Resolve the plan once, at startup.
//! let registry = CodecRegistry::with_builtins(&[Codecset::IntString], None)?;
//! let planner = Planner::new(registry, PlannerOptions::default());
//! let plan = planner.plan_column("n", &CqlTypeInfo::Int, &CqlTypeInfo::Text);
//!
//! // Apply it per row.
//! let converted = plan.plan().apply(&RawCell::new(10_i32.to_be_bytes().to_vec()))?;
//! assert_eq!(converted, RawCell::new(b"10".to_vec()));
//! # Ok::<(), cdm_core::CdmError>(())
//! ```
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `CDC-001`, `CDC-002` — [`CqlTypeInfo`]
//! - `CDC-003` — [`Geometry`], [`DateRange`]
//! - `CDC-004` — `CqlTypeInfo::Vector`
//! - `CDC-010`..`CDC-016` — [`Planner`], [`ConversionPlan`]
//! - `CDC-020`, `CDC-021` — [`Codecset`], [`TimestampFormat`]
//! - `CDC-022` — [`JavaDateFormat`]
//! - `CDC-030`, `CDC-031` — [`CodecRegistry`], [`Converter`], [`CodecDescription`]
//! - `CDC-032` — the round-trip property tests in `tests/`

mod builtin;
mod codec;
mod format;
mod geo;
mod plan;
mod types;
mod wire;

pub use builtin::{Codecset, TimestampFormat, BUILTIN_PROVIDER};
pub use codec::{CodecDescription, CodecEntry, CodecRegistry, Converter};
pub use format::{format_double_java, parse_double_java, JavaDateFormat, DOUBLE_FORMAT};
pub use geo::{DateRange, DateRangeBound, Geometry, Precision};
pub use plan::{ColumnPlan, ConversionPlan, Planner, PlannerOptions, UdtFieldPlan};
pub use types::{CqlTypeInfo, UdtField, UdtResolver};

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
                if code.starts_with("//") || code.starts_with("///") {
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
