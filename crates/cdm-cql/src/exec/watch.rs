//! Aborting a run whose schema moved underneath it (`SCH-009`).
//!
//! # What can go wrong, concretely
//!
//! `ARCHITECTURE.md` §5.5 resolves everything once: the origin projection, the target column
//! order, one [`ConversionPlan`](cdm_codec::ConversionPlan) per column, the bind-slot vector, the
//! prepared statements. All of it is a function of the schema as it was at startup.
//!
//! Add a column to the target table mid-run and the server re-prepares the statement with a wider
//! result metadata; drop one and the bind indices shift; change a column's type and every
//! conversion plan for it is now wrong. None of those produce an obvious failure. They produce
//! rows written into the wrong columns, or silently truncated, in the middle of a job that will
//! report `Partitions Passed` for every range it touched.
//!
//! So `SCH-009` requires the run to **stop**, with an error kind of its own
//! ([`ErrorKind::SchemaChanged`]) so an operator can tell it apart from the `SchemaMismatch` that
//! is decided before any data moves.
//!
//! # How it is detected
//!
//! Cassandra gives every node a `schema_version` UUID and the driver aggregates them:
//! `Session::check_schema_agreement` returns `Some(version)` when every reachable node agrees and
//! `None` when they do not. [`SchemaWatch::baseline`] records the agreed version of both sides at
//! startup; [`SchemaWatch::check`] compares.
//!
//! Two cases are deliberately *not* failures:
//!
//! * **disagreement** (`None`) — a schema change is propagating, or a node is catching up after a
//!   restart. The version cdm-rs planned against may still be the one that wins, so a transient
//!   disagreement is logged and tolerated. The change is caught on the next check, once the
//!   cluster settles on a version;
//! * **an unavailable check** — the agreement query itself failing is a connectivity problem, and
//!   `ENG-008` already fails ranges for those. Turning it into a schema abort would stop a whole
//!   run for a blip.
//!
//! # It is checked per range, not per row
//!
//! `check_schema_agreement` is a query against `system.local`/`system.peers` on every connection.
//! Per row it would dominate the workload; per range it costs one round trip per range and bounds
//! the damage to the range that was in flight when the change landed, which is the granularity
//! everything else in cdm-rs is accounted at anyway (`P5`).

use std::sync::atomic::{AtomicBool, Ordering};

use cdm_core::{CdmError, ErrorKind, Side};
use uuid::Uuid;

use super::DriverSession;

/// The schema versions a run planned against, and the check that they have not moved (`SCH-009`).
#[derive(Debug)]
pub struct SchemaWatch {
    origin: Option<Uuid>,
    target: Option<Uuid>,
    /// Set once a version has been reported as changed, so a run with many workers logs the
    /// change once rather than once per range.
    reported: AtomicBool,
}

impl SchemaWatch {
    /// Records the agreed schema version of both sides.
    ///
    /// A side whose agreement cannot be established right now records `None`, which makes every
    /// later check for that side a no-op: cdm-rs cannot say a version changed if it never knew
    /// what the version was, and inventing a baseline would abort runs at random.
    pub async fn baseline(origin: &DriverSession, target: &DriverSession) -> Self {
        Self {
            origin: agreed_version(origin, Side::Origin).await,
            target: agreed_version(target, Side::Target).await,
            reported: AtomicBool::new(false),
        }
    }

    /// A watch that never fires, for a run that has opted out and for unit tests.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            origin: None,
            target: None,
            reported: AtomicBool::new(false),
        }
    }

    /// The origin's baseline version, if one was established.
    #[must_use]
    pub const fn origin_version(&self) -> Option<Uuid> {
        self.origin
    }

    /// The target's baseline version, if one was established.
    #[must_use]
    pub const fn target_version(&self) -> Option<Uuid> {
        self.target
    }

    /// Whether either side's schema has changed since the baseline (`SCH-009`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaChanged`] naming the side, the version planned against and the version
    /// now in force. The kind is fatal, so the caller's range failure is not something a resume
    /// should quietly retry: the plan itself is stale.
    pub async fn check(
        &self,
        origin: &DriverSession,
        target: &DriverSession,
    ) -> Result<(), CdmError> {
        self.check_side(origin, Side::Origin, self.origin).await?;
        self.check_side(target, Side::Target, self.target).await
    }

    async fn check_side(
        &self,
        session: &DriverSession,
        side: Side,
        baseline: Option<Uuid>,
    ) -> Result<(), CdmError> {
        let Some(baseline) = baseline else {
            return Ok(());
        };
        let Some(current) = agreed_version(session, side).await else {
            // Disagreement, or a failed check. Both are transient by nature; see the module docs.
            return Ok(());
        };
        if current == baseline {
            return Ok(());
        }
        self.verdict(side, baseline, current)
    }

    /// Builds the abort, logging it once per run however many workers reach it.
    fn verdict(&self, side: Side, baseline: Uuid, current: Uuid) -> Result<(), CdmError> {
        if !self.reported.swap(true, Ordering::Relaxed) {
            tracing::error!(
                target: "cdm::cql::exec",
                side = side.as_str(),
                planned_version = %baseline,
                current_version = %current,
                "the {side} schema changed while the run was in progress; aborting (SCH-009)"
            );
        }
        Err(CdmError::new(
            ErrorKind::SchemaChanged,
            format!(
                "the {side} schema changed while the run was in progress: it was planned against \
                 schema version {baseline} and the cluster now agrees on {current}. Every \
                 statement, conversion plan and bind position was resolved from the old schema, \
                 so continuing could write data that cannot be reconciled. Re-run once the schema \
                 has settled (SCH-009)."
            ),
        )
        .with_context(|c| c.with_side(side)))
    }
}

/// The agreed schema version, or `None` when the nodes disagree or the check itself fails.
async fn agreed_version(session: &DriverSession, side: Side) -> Option<Uuid> {
    match session.check_schema_agreement().await {
        Ok(version) => version,
        Err(error) => {
            tracing::debug!(
                target: "cdm::cql::exec",
                side = side.as_str(),
                error = %error,
                "the schema-agreement check could not be completed (SCH-009)"
            );
            None
        }
    }
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

    fn watch(origin: Option<Uuid>, target: Option<Uuid>) -> SchemaWatch {
        SchemaWatch {
            origin,
            target,
            reported: AtomicBool::new(false),
        }
    }

    #[test]
    fn sch_009_a_changed_version_aborts_with_its_own_error_kind() {
        let planned = Uuid::from_u128(1);
        let current = Uuid::from_u128(2);
        let error = watch(Some(planned), None)
            .verdict(Side::Origin, planned, current)
            .unwrap_err();

        assert_eq!(error.kind(), ErrorKind::SchemaChanged);
        assert!(error.kind().is_fatal(), "a stale plan must not be retried");
        assert_eq!(error.context().side, Some(Side::Origin));
        let rendered = error.to_string();
        assert!(rendered.contains(&planned.to_string()), "{rendered}");
        assert!(rendered.contains(&current.to_string()), "{rendered}");
        assert!(rendered.contains("SCH-009"), "{rendered}");
    }

    #[test]
    fn sch_009_the_change_is_logged_once_however_many_ranges_notice_it() {
        let watch = watch(Some(Uuid::from_u128(1)), None);
        assert!(!watch.reported.load(Ordering::Relaxed));
        let _ = watch.verdict(Side::Target, Uuid::from_u128(1), Uuid::from_u128(2));
        assert!(watch.reported.load(Ordering::Relaxed));
        // A second range still gets the error; only the log line is suppressed.
        assert!(watch
            .verdict(Side::Target, Uuid::from_u128(1), Uuid::from_u128(2))
            .is_err());
    }

    #[test]
    fn sch_009_a_watch_with_no_baseline_never_fires() {
        let watch = SchemaWatch::disabled();
        assert_eq!(watch.origin_version(), None);
        assert_eq!(watch.target_version(), None);
    }
}
