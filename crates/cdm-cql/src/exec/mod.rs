//! Executing the statements: paging the origin, writing the target, watching the schema.
//!
//! # Why this lives here and not in `cdm-engine`
//!
//! `ARCHITECTURE.md` §3 makes `cdm-cql` the only crate that may name `scylla`. A job, however,
//! has to *run* the statements [`crate::statement`] builds, and the natural place to put a read
//! loop is next to the job that consumes it. This module is the compromise the dependency graph
//! forces and the one it rewards: everything that touches a [`Session`] is here, expressed over
//! cdm-rs's own types, so `cdm-engine` describes *what a migrate does* without ever mentioning a
//! driver type.
//!
//! # Zero-copy survives the seam (`MIG-040`, `MIG-041`)
//!
//! The obvious shape for a read loop is a `Stream` of rows. The driver cannot give one for rows
//! that borrow the response frame — `QueryPager::rows_stream` requires an *owned* row type, and
//! `RawRow` is the opposite of owned — so a stream would force a deserialize-and-allocate per row
//! and forfeit passthrough entirely.
//!
//! [`RangeScan`] therefore hands back a [`Page`] that **owns** the driver's decoded page, and
//! `Page::rows` lends [`RawRow`](crate::raw::RawRow)s out of it. The frame lives exactly as long
//! as the loop that reads it, every cell is a borrow into that frame, and the bound write that
//! goes to the target is the very same slice. `mig_041_the_write_path_binds_the_read_frame_itself`
//! in `cdm-engine` proves it by pointer identity.
//!
//! # Retry, and the one thing that is never retried
//!
//! [`TargetWriter::write`] takes an [`IdempotentWrite`](crate::statement::IdempotentWrite) and
//! retries it with exponential backoff and jitter (`CON-011`). [`TargetWriter::write_counter`]
//! takes a [`CounterWrite`](crate::statement::CounterWrite) and does not — one attempt, and a
//! failure is the caller's problem (`CON-012`, `MIG-032`). The two are different methods over
//! different types rather than one method with a flag, because a flag can be passed wrongly and a
//! type cannot: [`Idempotent`](crate::statement::Idempotent) is sealed and a counter write does
//! not implement it, so no generic retry helper can ever accept one.
//!
//! # Specification
//!
//! - `ENG-003` — [`RangeScan`] pages the origin at `perfops.fetch_size`
//! - `CON-011` — [`TargetWriter::write`], [`RangeScan::next_page`]
//! - `CON-012`, `MIG-032` — [`TargetWriter::write_counter`]
//! - `MIG-020` — [`TargetWriter::write_batch`]
//! - `MIG-031` — [`TargetWriter::counter_row`]
//! - `SCH-009` — [`SchemaWatch`]

mod executor;
mod scan;
mod statements;
mod watch;
mod write;

pub use executor::RunExecutor;
pub use scan::{Page, PageRows, RangeScan, TokenWidth};
pub use statements::{PreparedSet, PreparedSetOptions};
pub use watch::SchemaWatch;
pub use write::{BatchTemplate, CounterRow, TargetWriter};

use scylla::client::session::Session;

/// Everything in this module is expressed over a driver session; the alias keeps the signatures
/// readable and marks every place the seam is crossed.
pub(crate) type DriverSession = Session;
