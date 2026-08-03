//! Schema introspection and identifiers (`SCH-001`, `SCH-002`, `SCH-010`).
//!
//! | Module | Requirement |
//! |---|---|
//! | [`identifier`] | quoting, unquoting and folding of CQL identifiers (`SCH-002`) |
//! | [`table`] | the table model: columns, key order, clustering direction, counters (`SCH-001`) |
//! | [`introspect`] | reading that model out of `system_schema` (`SCH-001`, `SCH-010`) |
//! | [`provider`] | the [`SchemaProvider`](cdm_config::SchemaProvider) Tier-3 validation needs |

pub mod identifier;
pub mod introspect;
pub mod provider;
pub mod table;

pub use provider::SchemaSnapshot;
pub use table::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
