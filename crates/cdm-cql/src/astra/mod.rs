//! Astra DB secure-connect-bundles (`CON-003`, `CON-004`, `CON-005`, `CON-020`..`CON-029`).
//!
//! Astra is a first-class origin and target: Java CDM supports it directly, including downloading
//! the bundle from the DevOps API, so cdm-rs must too. The driver does not
//! (`ADR-0002`, `ADR-0009`), which makes this module the largest shim in the crate.
//!
//! | Module | Requirement |
//! |---|---|
//! | [`bundle`] | reading the zip in memory, `config.json`, `cqlshrc` (`CON-020`, `CON-021`, `CON-026`) |
//! | [`metadata`] | the metadata service and its refresh rate limit (`CON-022`, `CON-025`) |
//! | [`devops`] | the DevOps API download (`CON-004`) |
//! | [`tempdir`] | the `0700` directory a download lands in (`CON-005`) |
//! | [`strategy`] | which strategy is in force, and the credentials (`CON-022`, `CON-026`..`CON-028`) |
//!
//! Start at [`strategy`]: it documents why the SNI strategy of `CON-022` is unreachable with
//! `scylla-rust-driver` 1.7 and what cdm-rs does instead.

pub mod bundle;
pub mod devops;
pub mod metadata;
pub mod strategy;
pub mod tempdir;

pub use bundle::{BundleConfig, SecureConnectBundle};
pub use devops::{BundleLocation, BundleSelector, DevOpsClient};
pub use metadata::{MetadataResponse, MetadataService};
pub use strategy::{AstraConnection, AstraCredentials, AstraStrategy, ProxyAddressTranslator};
pub use tempdir::BundleTempDir;
