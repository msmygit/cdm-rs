//! Optional behaviours as plugins: constant columns, explode map, extract JSON, TTL/writetime, filters, guardrails.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What is here
//!
//! Five features, each a plugin (`PLG-002`, `PLG-003`) and each independently unit-testable without
//! a cluster:
//!
//! * [`ConstantColumns`] — target columns written with a fixed literal (`FEA-010`..`FEA-014`);
//! * [`ExplodeMap`] — one target row per entry of an origin `map` (`FEA-020`..`FEA-023`);
//! * [`ExtractJson`] — one property of a JSON document promoted to a column (`FEA-030`..`FEA-035`);
//! * [`WritetimeTtl`] — the origin's cell metadata carried to the target (`FEA-040`..`FEA-046`);
//! * [`FilterChain`] and its built-in filters — everything that decides a row is not this run's
//!   business (`FEA-050`..`FEA-054`).
//!
//! # The two-phase shape every feature has
//!
//! Each feature is **loaded** from configuration, **validated** against the schema, and then
//! **resolved** into a plan that holds column positions and conversion plans. That third step is the
//! one that matters for throughput: `ARCHITECTURE.md` §5.5 requires work to be resolved once at
//! startup rather than per row, so the types named `…Plan` here own the hot path and the types named
//! after the feature own only configuration.
//!
//! ```
//! use cdm_core::EffectiveConfig;
//! use cdm_feature::{ConstantColumns, FeatureSchema, TableFacts, table_view};
//! use cdm_core::TableRef;
//!
//! let config: EffectiveConfig = [
//!     ("feature.constant_columns.names", "tenant"),
//!     ("feature.constant_columns.values", "'acme'"),
//! ]
//! .into_iter()
//! .collect();
//!
//! let target = TableFacts::from_view(
//!     &table_view(TableRef::new("ks", "dst"), &[("id", "int"), ("tenant", "text")]),
//!     &["id", "tenant"],
//! )?;
//!
//! let feature = ConstantColumns::load(&config)?;
//! assert!(feature.validate(&target).is_empty());
//! assert_eq!(feature.where_clause_terms(&target)?, ["tenant='acme'"]);
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
//! - `FEA-010`..`FEA-014` — [`ConstantColumns`], [`ResolvedConstant`], [`ColumnSource`]
//! - `FEA-020`..`FEA-023` — [`ExplodeMap`], [`ExplodePlan`], [`ExplodedEntry`]
//! - `FEA-030`..`FEA-035` — [`ExtractJson`], [`ExtractJsonPlan`], [`JsonPath`]
//! - `FEA-040`..`FEA-046` — [`WritetimeTtl`], [`WritetimeTtlPlan`], [`UsingClause`]
//! - `FEA-050`..`FEA-054` — [`CqlWhereFilter`], [`WritetimeFilter`], [`ColumnValueFilter`],
//!   [`TokenBounds`], [`FilterChain`]
//!
//! Guardrails (`GRD-001`..`GRD-005`) land in this crate too, with PR #24.

mod constant;
mod diagnostic;
mod explode;
mod extract_json;
mod filter;
mod literal;
pub mod properties;
mod schema;
mod wire;
mod writetime;

pub use constant::{dropped_origin_columns, ColumnSource, ConstantColumns, ResolvedConstant};
pub use explode::{ExplodeMap, ExplodePlan, ExplodedEntry};
pub use extract_json::{ExtractJson, ExtractJsonPlan, JsonPath};
pub use filter::{ColumnValueFilter, CqlWhereFilter, FilterChain, TokenBounds, WritetimeFilter};
pub use literal::{encode_json, parse_literal};
pub use properties::{registry, PropertyKey};
pub use schema::{table_view, ColumnFacts, FeatureSchema, TableFacts};
pub use writetime::{UsingClause, WritetimeTtl, WritetimeTtlPlan};

/// The provider name every built-in feature reports (`PLG-010`).
pub const PROVIDER: &str = "cdm-feature";

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
        assert_eq!(PROVIDER, "cdm-feature");
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
