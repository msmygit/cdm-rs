//! Test fixtures: containers, schema/data generators, counter assertions, mock sessions.
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
//! - `TST-100`
//! - `TST-101`
//! - `TST-102`
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
