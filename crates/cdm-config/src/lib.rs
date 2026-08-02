//! The single typed configuration model, its loaders, and three-tier validation.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `CFG-001`
//! - `CFG-010`
//! - `CFG-020`
//! - `CFG-100`
//! - `CFG-200`
//!
//! # Status
//!
//! Scaffolding only. Implementation lands in the pull requests listed in
//! [`docs/ROADMAP.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/ROADMAP.md).

/// The version of this crate, as reported by `cdm version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_populated() {
        assert!(!VERSION.is_empty());
    }
}
