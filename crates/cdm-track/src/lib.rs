//! Run tracking, resume and rerun behind the pluggable `TrackingStore` trait.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What run tracking is for
//!
//! A migration of a large table takes hours, and something will interrupt it: a node restart, a
//! network partition, an operator's `Ctrl-C`, a Spark executor that ran out of heap. Without
//! tracking, the only recovery is to start again from the beginning. With it, every token range
//! carries a recorded status, and the next run processes exactly the ranges the last one did not
//! finish.
//!
//! The tables are Java CDM's, byte for byte (`TRK-010`, `COMPAT-003`), so a run started by either
//! tool can be resumed by the other.
//!
//! # The bias, stated once
//!
//! **A resume re-runs a range it is unsure about; it never skips one.** Re-running a completed
//! range costs time — migrate writes carry the origin's writetime, so a repeated upsert changes
//! nothing at the storage layer — while skipping an unfinished one loses rows permanently and
//! silently. Every ambiguous case in [`resume`] therefore resolves towards re-running: a
//! `STARTED` range is unfinished, an unreadable status is unfinished, an unparseable failure
//! count means "there were failures", and a previous run that cannot be found falls back to a
//! **full** plan rather than an empty one.
//!
//! The single exception is counters (`DST-015`), and it is enforced structurally rather than by a
//! guard: see [`resume::RerunPolicy::rerunnable_statuses`].
//!
//! # Interim versus committed
//!
//! Only **committed** counter values are ever persisted. [`tracker`]'s module documentation says
//! why at length; the short form is that the two Java defects cdm-rs deliberately does not
//! reproduce — `MIG-004` and `ENG-008` — are both "read a level that had not been written yet",
//! and the tracking table is where such a mistake becomes durable and is later read back into a
//! resume.
//!
//! # Layout
//!
//! * [`settings`] — is tracking on, and under which run id (`TRK-001`..`TRK-003`);
//! * [`schema`] — the two tables and every statement against them (`TRK-010`, `TRK-011`);
//! * [`compat`] — the column values Java and cdm-rs must spell identically (`TRK-013`,
//!   `TRK-014`);
//! * [`store`] — the backends behind [`TrackingStore`](cdm_core::TrackingStore): Cassandra,
//!   SQLite and in-memory (`TRK-036`);
//! * [`tracker`] — the run lifecycle and the bounded, batched writer (`TRK-020`..`TRK-022`,
//!   `TRK-035`);
//! * [`resume`] — adopting a previous run and turning it into a work list
//!   (`TRK-030`..`TRK-033`);
//! * [`manage`] — what `cdm runs` and `GET /v1/runs` are built on (`TRK-034`).
//!
//! # Two rules this crate is built around
//!
//! **`SEC-001`.** No credential, and no configuration value, reaches the tracking table, a log
//! line or a summary. The only strings written are statuses, the `run_type` name, and the metrics
//! string, which is composed entirely of counter names and integers.
//!
//! **`SEC-002`.** A range is identified by its token bounds and never by its contents. There is
//! no field on [`manage::RunDetail`], [`resume::QuarantinedRange`] or the tracking rows that a
//! row value could occupy.
//!
//! # Usage
//!
//! ```
//! use std::sync::Arc;
//!
//! use cdm_core::{JobKind, RunId, RunStatus, TableRef, TokenRange};
//! use cdm_track::store::MemoryStore;
//! use cdm_track::tracker::{new_run_record, RunTracker, TrackerConfig};
//!
//! # async fn example() -> Result<(), cdm_core::CdmError> {
//! let store = Arc::new(MemoryStore::new());
//! let table = TableRef::new("target_ks", "customers");
//! let run = new_run_record(RunId::from_raw(1), None, table, JobKind::Migrate);
//! let ranges = [TokenRange::new(0, 99)?];
//!
//! // TRK-020: the run row and every range row exist before this returns.
//! let tracker =
//!     RunTracker::start(store.clone(), &run, &ranges, TrackerConfig::default()).await?;
//!
//! // TRK-021, from a worker. Neither call blocks.
//! tracker.start_range(ranges[0]);
//! tracker.finish_range(ranges[0], RunStatus::Pass, "Read: 10; Write: 10".to_owned());
//!
//! // TRK-022.
//! tracker.finish(RunStatus::Ended, "Read: 10; Partitions Failed: 0".to_owned()).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `TRK-001`, `TRK-002` — [`settings::TrackingSettings`]
//! - `TRK-003` — [`settings::TrackingSettings::resolve_run_id`]
//! - `TRK-010`, `TRK-011` — [`schema::TrackingTables`]
//! - `TRK-012`, `TRK-013`, `TRK-014` — [`compat`]
//! - `TRK-020`, `TRK-021`, `TRK-022` — [`tracker::RunTracker`]
//! - `TRK-030` — [`resume::adopt_previous_run`]
//! - `TRK-031` — [`resume::plan_resume`]
//! - `TRK-032` — [`resume::FallbackReason`]
//! - `TRK-033` — [`resume::subdivide`]
//! - `TRK-034` — [`manage::RunManager`]
//! - `TRK-035` — [`tracker::TrackerConfig`]
//! - `TRK-036` — [`store::CassandraStore`], [`store::SqliteStore`], [`store::MemoryStore`]
//! - `DST-015` — [`resume::RerunPolicy`]

pub mod compat;
pub mod manage;
pub mod resume;
pub mod schema;
pub mod settings;
pub mod store;
pub mod tracker;

pub use compat::run_type;
pub use manage::{RunCatalog, RunDetail, RunManager, RunSummary};
pub use resume::{
    adopt_previous_run, plan_resume, subdivide, FallbackReason, QuarantineReason, QuarantinedRange,
    RangeDisposition, RerunPolicy, ResumePlan,
};
pub use schema::TrackingTables;
pub use settings::TrackingSettings;
pub use store::{CassandraStore, MemoryStore, SqliteStore};
pub use tracker::{committed_run_info, new_run_record, RunTracker, TrackerConfig};

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
