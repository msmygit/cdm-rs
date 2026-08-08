//! Driver layer: connections, TLS/Astra, schema introspection, statements. The only crate that depends on `scylla`.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What is here
//!
//! * [`connect`] — building an origin and a target session independently, in each of the four
//!   connection modes, with the load-balancing, speculative-execution and retry policies cdm-rs
//!   requires, and the start-up capability probe.
//! * [`tls`] — the trust and key store readers (`PEM`, `PKCS12` and a pure-Rust `JKS`), the
//!   cipher-suite gate and the certificate verifier.
//! * [`astra`] — the secure-connect-bundle: reading the zip in memory, the metadata service, the
//!   DevOps API download and the strategy that follows from what the driver can actually do.
//! * [`schema`] — `system_schema` introspection, identifier quoting, and the
//!   [`SchemaProvider`](cdm_config::SchemaProvider) implementation that lets Tier-3 configuration
//!   validation run.
//! * [`raw`] — undeserialized access to result rows, the primitive zero-copy passthrough is built
//!   on.
//! * [`statement`] — the CQL a run executes: the column mapping, the origin projection, the read
//!   and write statements, and binding a row into one without ever writing a tombstone.
//! * [`exec`] — running those statements: paging a token range off the origin without decoding a
//!   cell, writing the target with the retry policy each kind of write is entitled to, and
//!   noticing a schema that moved underneath the run.
//! * [`observe`] — the per-request timing `MET-010`'s latency percentiles are computed from,
//!   recorded where the requests are actually issued.
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `CON-000` — the driver, and only in this crate
//! - `CON-001`..`CON-013` — connection building, TLS, load balancing, retries, the capability probe
//! - `CON-020`..`CON-029` — Astra secure-connect-bundles
//! - `SCH-001`, `SCH-002`, `SCH-010` — schema introspection, identifiers, materialized views
//! - `SCH-003`..`SCH-007` — [`statement::ColumnMapping`], [`statement::OriginProjection`],
//!   [`statement::TargetUpsert`]
//! - `FEA-060`..`FEA-062` — [`statement::OriginRangeSelect`], [`statement::StatementSet`]
//! - `MIG-010`..`MIG-014` — [`statement::TargetUpsert`], [`statement::Binder`]
//! - `ERR-005` — [`statement::BindFailure`]
//! - `ENG-003`, `CON-011`, `CON-012` — [`exec::RangeScan`], [`exec::TargetWriter`]
//! - `MET-010` — [`observe::RequestMetrics`], which times every request the four executors issue
//! - `GRD-001` — [`exec::OriginReader`], the origin-only reader a guardrail run is built on
//! - `SCH-009` — [`exec::SchemaWatch`]
//!
//! # A driver limitation you will meet
//!
//! `scylla-rust-driver` 1.7 cannot set a per-connection TLS `server_name`, which is the mechanism
//! Astra's SNI proxy routes on. `CON-022`'s primary strategy is therefore not reachable today and
//! cdm-rs uses the documented single-endpoint fallback (`CON-026`) with a loud warning. The
//! analysis, and everything that *is* implemented behind it, is in [`astra::strategy`].

pub mod astra;
pub mod connect;
pub mod exec;
pub mod observe;
pub mod raw;
pub mod rows;
pub mod schema;
pub mod statement;
pub mod tls;

mod errors;
mod http;

#[cfg(test)]
mod testfixtures;

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
}
