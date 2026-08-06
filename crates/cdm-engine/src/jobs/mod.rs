//! The built-in jobs: migrate (`MIG`), validate (`VAL`) and guardrail (`GRD`).
//!
//! Each one is a [`RangeProcessor`](crate::scheduler::RangeProcessor) and nothing more. The
//! scheduler in [`crate::scheduler`] claims ranges, paces them, bounds their memory, catches their
//! panics and accounts for their failures; a job's whole responsibility is "given a range, read it,
//! do the work, and increment the counters". Each is reachable through the same one-method trait a
//! third-party job would implement (`PLG-004`).
//!
//! That division is deliberate and is the reason `ENG-008`'s failure accounting exists once here
//! rather than once per job. Java CDM copies the failure path into every `*JobSession`, and one of
//! the copies — `DiffJobSession`'s — reads its counters at the wrong level and therefore reports
//! `ERROR: 0` for every failed validate range. A job in cdm-rs cannot make that mistake because it
//! never writes the failure path at all.

pub mod guardrail;
pub mod migrate;
pub mod validate;

pub use guardrail::{GuardrailJob, InlineGuardrail, OriginRows, RowSizeStream};
pub use migrate::{MigrateJob, MigratePlan, MigrateSettings};
