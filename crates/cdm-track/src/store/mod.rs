//! The tracking backends (`TRK-036`).
//!
//! [`TrackingStore`](cdm_core::TrackingStore) is declared in `cdm-core` so that a third party can
//! implement it without depending on this crate (`PLG-007`). What lives here are the built-in
//! implementations:
//!
//! | Backend | When | Requirement |
//! |---|---|---|
//! | [`CassandraStore`] | the default — Java's two tables in the target keyspace | `TRK-010` |
//! | [`SqliteStore`] | a target that cannot host extra tables; tracking goes to a local file | `TRK-036` |
//! | [`MemoryStore`] | tests, dry runs, and any run whose tracking must not outlive the process | `TRK-036` |
//!
//! Which one a deployment gets is a wiring decision, not a behavioural one: the tracker, the
//! resume and `cdm runs` name only [`TrackingStore`](cdm_core::TrackingStore), so swapping the
//! backend changes where a run is recorded and nothing about what is recorded. Only the Cassandra
//! backend is readable by Java CDM (`COMPAT-003`) and only it can back a distributed run
//! (`DST-001`), because the other two are local to one process or one machine.
//!
//! # What every backend owes the tracker
//!
//! * a run id that already exists must be **rejected**, not overwritten (`TRK-020`) — otherwise a
//!   second process silently resets the first one's range rows to `NOT_STARTED` and the two
//!   fight over the same work;
//! * a range row that cannot be interpreted must come back as *pending*, never be dropped
//!   ([`decode_status`]);
//! * nothing written may carry a row value or a credential (`SEC-001`, `SEC-002`).

pub mod cassandra;
pub mod memory;
pub mod sqlite;

pub use cassandra::CassandraStore;
pub use memory::MemoryStore;
pub use sqlite::SqliteStore;

use cdm_core::RunStatus;

/// Interprets a `status` column, biased towards re-running (`TRK-031`).
///
/// A status this build does not recognise — written by a newer cdm-rs, or by a Java build with a
/// status this one has not heard of — is reported as [`RunStatus::Started`], which `TRK-031`
/// counts as pending. The alternatives are worse: dropping the row loses the range from the
/// resume entirely, and reporting `PASS` claims work was done that may not have been.
///
/// `None` means the column was null, which Java's `INSERT` never leaves it as; it is treated the
/// same way, as pending.
pub fn decode_status(raw: Option<&str>) -> RunStatus {
    let Some(raw) = raw else {
        tracing::warn!("a cdm_run_details row has a null status; treating the range as pending");
        return RunStatus::Started;
    };
    raw.parse().unwrap_or_else(|_| {
        tracing::warn!(
            status = raw,
            "unrecognised status in the tracking table; treating the range as pending so the \
             resume re-runs it rather than skipping it"
        );
        RunStatus::Started
    })
}

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
    fn trk_031_an_unknown_or_null_status_decodes_to_a_pending_one() {
        assert_eq!(decode_status(Some("PASS")), RunStatus::Pass);
        assert_eq!(
            decode_status(Some("DIFF_CORRECTED")),
            RunStatus::DiffCorrected
        );
        // The bias: anything we cannot read is unfinished work.
        assert_eq!(decode_status(Some("QUANTUM")), RunStatus::Started);
        assert_eq!(decode_status(None), RunStatus::Started);
        assert!(decode_status(Some("QUANTUM")).is_pending());
        assert!(decode_status(None).is_pending());
    }
}
