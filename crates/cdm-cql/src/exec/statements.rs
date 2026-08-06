//! Preparing the statements a run executes, once, at startup.
//!
//! `ARCHITECTURE.md` §5.5 requires per-row work to be lookup rather than decision, and preparation
//! is the largest single decision there is: a `PreparedStatement` carries the server's column
//! specifications, which is what lets a bind be a memcpy instead of a type negotiation. Preparing
//! per range — let alone per row — would also re-do the round trip on every worker.
//!
//! # Idempotence is set here, deliberately
//!
//! The driver's retry policy (`CON-011`) only ever retries a request the caller has *marked*
//! idempotent. [`PreparedSet::prepare`] marks the origin scan and the target lookup idempotent —
//! reads always are — and marks the target write idempotent **only** when the target is not a
//! counter table. That one line is where `CON-012` becomes true of the driver as well as of the
//! engine, and it is why the flag is derived from the statement rather than passed in.

use std::time::Duration;

use cdm_core::{CdmError, ErrorKind, Side};
use scylla::statement::prepared::PreparedStatement;
use scylla::statement::{Consistency, SerialConsistency};

use crate::errors::side_error_from;
use crate::statement::StatementSet;

use super::DriverSession;

/// What preparation needs beyond the statement text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedSetOptions {
    /// Origin page size, in rows (`ENG-003`, `perfops.fetch_size`).
    pub fetch_size: u32,
    /// The consistency level origin reads run at.
    pub read_consistency: Consistency,
    /// The consistency level target writes run at.
    pub write_consistency: Consistency,
    /// Per-request timeout (`perfops.request_timeout`).
    pub request_timeout: Duration,
    /// Whether the target is a counter table, which decides idempotence (`CON-012`, `SCH-005`).
    pub counter_target: bool,
}

impl Default for PreparedSetOptions {
    /// The `CFG-160` defaults, so a test does not have to restate them.
    fn default() -> Self {
        Self {
            fetch_size: 1_000,
            read_consistency: Consistency::LocalQuorum,
            write_consistency: Consistency::LocalQuorum,
            request_timeout: Duration::from_secs(30),
            counter_target: false,
        }
    }
}

/// Every statement a run executes, prepared against its own side's session (`FEA-062`).
#[derive(Debug)]
pub struct PreparedSet {
    origin_range_select: PreparedStatement,
    target_select_by_pk: PreparedStatement,
    target_upsert: PreparedStatement,
    counter_target: bool,
    write_consistency: Consistency,
    request_timeout: Duration,
}

impl PreparedSet {
    /// Prepares the origin scan on the origin session and both target statements on the target
    /// session.
    ///
    /// The origin lookup by primary key is deliberately *not* prepared: nothing in the migrate
    /// job reads a single origin row, and preparing a statement no job executes costs a round
    /// trip per run and gives an operator a statement in the server's prepared-statement cache
    /// that never fires.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] if the server rejects a statement. That is nearly always the
    /// generated CQL disagreeing with the live schema — a column dropped between introspection
    /// and preparation, say — and the message carries the offending statement so it is diagnosable
    /// without turning on driver tracing.
    pub async fn prepare(
        origin: &DriverSession,
        target: &DriverSession,
        statements: &StatementSet,
        options: PreparedSetOptions,
    ) -> Result<Self, CdmError> {
        let mut origin_range_select =
            prepare_one(origin, Side::Origin, &statements.origin_range_select).await?;
        // ENG-003: the page size is a property of the statement, so it is set once here rather
        // than per execution, where it would be one more thing a call site could forget.
        origin_range_select.set_page_size(page_size(options.fetch_size));
        origin_range_select.set_consistency(options.read_consistency);
        origin_range_select.set_request_timeout(Some(options.request_timeout));
        // CON-011: a `SELECT` has no side effect, so it is always safe to retry.
        origin_range_select.set_is_idempotent(true);

        let mut target_select_by_pk =
            prepare_one(target, Side::Target, &statements.target_select_by_pk).await?;
        target_select_by_pk.set_consistency(options.read_consistency);
        target_select_by_pk.set_request_timeout(Some(options.request_timeout));
        target_select_by_pk.set_is_idempotent(true);
        // MIG-031: the counter delta is read one primary key at a time, and a serial read would
        // be both slower and unnecessary — the value only has to be the one the coordinator has.
        target_select_by_pk.set_serial_consistency(None::<SerialConsistency>);

        let mut target_upsert =
            prepare_one(target, Side::Target, &statements.target_upsert).await?;
        target_upsert.set_consistency(options.write_consistency);
        target_upsert.set_request_timeout(Some(options.request_timeout));
        // CON-012: the whole of the driver-level counter guard. A counter `UPDATE` applied twice
        // adds the delta twice, so it must never be marked retryable.
        target_upsert.set_is_idempotent(!options.counter_target);

        Ok(Self {
            origin_range_select,
            target_select_by_pk,
            target_upsert,
            counter_target: options.counter_target,
            write_consistency: options.write_consistency,
            request_timeout: options.request_timeout,
        })
    }

    /// The prepared origin token-range scan (`FEA-060`).
    #[must_use]
    pub const fn origin_range_select(&self) -> &PreparedStatement {
        &self.origin_range_select
    }

    /// The prepared target lookup by primary key, used for the counter delta (`MIG-031`).
    #[must_use]
    pub const fn target_select_by_pk(&self) -> &PreparedStatement {
        &self.target_select_by_pk
    }

    /// The prepared target write (`MIG-010`, `MIG-030`).
    #[must_use]
    pub const fn target_upsert(&self) -> &PreparedStatement {
        &self.target_upsert
    }

    /// Whether the target is a counter table (`SCH-005`).
    #[must_use]
    pub const fn is_counter_target(&self) -> bool {
        self.counter_target
    }

    /// The consistency level target writes run at, which a batch must be given explicitly.
    #[must_use]
    pub const fn write_consistency(&self) -> Consistency {
        self.write_consistency
    }

    /// The per-request timeout, which a batch must also be given explicitly.
    #[must_use]
    pub const fn request_timeout(&self) -> Duration {
        self.request_timeout
    }
}

/// The driver takes a page size as `i32`; a configured value beyond that is clamped rather than
/// wrapped, and a zero page size is refused by the driver, so it becomes one row.
fn page_size(fetch_size: u32) -> i32 {
    i32::try_from(fetch_size).unwrap_or(i32::MAX).max(1)
}

async fn prepare_one(
    session: &DriverSession,
    side: Side,
    cql: &str,
) -> Result<PreparedStatement, CdmError> {
    session.prepare(cql).await.map_err(|error| {
        side_error_from(
            ErrorKind::SchemaMismatch,
            side,
            format!("the server rejected the generated statement `{cql}`"),
            error,
        )
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
    fn eng_003_the_page_size_is_clamped_into_the_drivers_range() {
        assert_eq!(page_size(1_000), 1_000);
        assert_eq!(page_size(0), 1, "the driver refuses a page size of zero");
        assert_eq!(page_size(u32::MAX), i32::MAX);
    }

    #[test]
    fn cfg_160_the_defaults_are_the_configured_ones() {
        let options = PreparedSetOptions::default();
        assert_eq!(options.fetch_size, 1_000);
        assert_eq!(options.read_consistency, Consistency::LocalQuorum);
        assert_eq!(options.write_consistency, Consistency::LocalQuorum);
        assert_eq!(options.request_timeout, Duration::from_secs(30));
        assert!(!options.counter_target);
    }
}
