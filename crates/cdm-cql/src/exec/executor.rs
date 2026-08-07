//! One value that owns both sessions and everything prepared against them.
//!
//! # Why this exists
//!
//! `ARCHITECTURE.md` §3 says `cdm-cql` is the only crate that may name `scylla`, and §3.1 says
//! `cdm-engine` owns the jobs. Between those two sentences there has to be a type that a job can
//! hold without naming a driver type, and this is it: [`RunExecutor`] carries the origin session,
//! the target session, the prepared statements, the batch template, the retry backoff and the
//! schema baseline, and hands out [`RangeScan`]s and [`TargetWriter`]s.
//!
//! A job therefore reads as `executor.scan(range)` and `executor.writer()`, and `cdm-engine`'s
//! `Cargo.toml` has no `scylla` line to justify. The seam is one struct wide, which is also what
//! makes it cheap to move if the driver is ever swapped (`ADR-0002`).
//!
//! # It is built once, per run
//!
//! Preparation is a round trip per statement and the schema baseline is one more; doing either per
//! range would multiply them by `perfops.num_parts`. `ARCHITECTURE.md` §5.5's "plan once, execute
//! many" is not only about the per-row path.

use std::sync::Arc;
use std::time::Duration;

use cdm_core::{CdmError, TokenRange};
use scylla::client::session::Session;

use crate::connect::{Backoff, ClusterSession};
use crate::statement::StatementSet;

use super::scan::TokenWidth;
use super::statements::{PreparedSet, PreparedSetOptions};
use super::write::BatchTemplate;
use super::{RangeScan, SchemaWatch, TargetWriter};

/// Everything a run needs in order to execute its statements.
#[derive(Debug)]
pub struct RunExecutor {
    origin: Arc<Session>,
    target: Arc<Session>,
    prepared: PreparedSet,
    batch: BatchTemplate,
    backoff: Backoff,
    token_width: TokenWidth,
    watch: SchemaWatch,
    statements: StatementSet,
}

impl RunExecutor {
    /// Prepares every statement and records the schema baseline (`FEA-062`, `SCH-009`).
    ///
    /// `batch_capacity` is the resolved `perfops.batch_size` **after** the coercion of `MIG-021`:
    /// a template sized from the configured value would let a caller assemble a batch the
    /// coercion was supposed to forbid.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`](cdm_core::ErrorKind::SchemaMismatch) if the server rejects a
    /// generated statement, which nearly always means the live schema has already moved away from
    /// the one introspection reported.
    pub async fn prepare(
        origin: &ClusterSession,
        target: &ClusterSession,
        statements: &StatementSet,
        options: PreparedSetOptions,
        batch_capacity: u32,
        token_width: TokenWidth,
    ) -> Result<Self, CdmError> {
        let origin_session = Arc::clone(origin.session());
        let target_session = Arc::clone(target.session());
        let prepared =
            PreparedSet::prepare(&origin_session, &target_session, statements, options).await?;
        let batch = BatchTemplate::new(
            prepared.target_upsert(),
            batch_capacity.max(1) as usize,
            prepared.write_consistency(),
            prepared.request_timeout(),
        );
        let watch = SchemaWatch::baseline(&origin_session, &target_session).await;
        Ok(Self {
            origin: origin_session,
            target: target_session,
            prepared,
            batch,
            // CON-011: the backoff the target side negotiated. The origin's differs only if the
            // operator configured it to, and a read that is retried on the target's schedule is
            // never wrong, only occasionally patient.
            backoff: target.backoff(),
            token_width,
            watch,
            statements: statements.clone(),
        })
    }

    /// The statements this run executes, for the startup log and `GET /v1/runs/{id}/statements`
    /// (`FEA-062`).
    #[must_use]
    pub const fn statements(&self) -> &StatementSet {
        &self.statements
    }

    /// Whether the target is a counter table (`SCH-005`).
    #[must_use]
    pub const fn is_counter_target(&self) -> bool {
        self.prepared.is_counter_target()
    }

    /// The batch template sized from the resolved `perfops.batch_size` (`MIG-020`).
    #[must_use]
    pub const fn batch_template(&self) -> &BatchTemplate {
        &self.batch
    }

    /// The retry backoff in force (`CON-011`).
    #[must_use]
    pub const fn backoff(&self) -> Backoff {
        self.backoff
    }

    /// The per-request timeout, for a caller that reports it.
    #[must_use]
    pub const fn request_timeout(&self) -> Duration {
        self.prepared.request_timeout()
    }

    /// A paged scan of one token range (`ENG-003`, `FEA-060`).
    #[must_use]
    pub fn scan(&self, range: TokenRange) -> RangeScan<'_> {
        RangeScan::for_range(
            &self.origin,
            self.prepared.origin_range_select(),
            range,
            self.token_width,
            self.backoff,
        )
    }

    /// A writer over the target session (`MIG-005`, `CON-011`, `CON-012`).
    #[must_use]
    pub fn writer(&self) -> TargetWriter<'_> {
        TargetWriter::new(
            &self.target,
            self.prepared.target_upsert(),
            self.prepared.target_select_by_pk(),
            self.backoff,
        )
    }

    /// Whether either schema has moved since the baseline (`SCH-009`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaChanged`](cdm_core::ErrorKind::SchemaChanged) when it has.
    pub async fn check_schema(&self) -> Result<(), CdmError> {
        self.watch.check(&self.origin, &self.target).await
    }

    /// The schema baseline, for a caller that reports or asserts on it.
    #[must_use]
    pub const fn schema_watch(&self) -> &SchemaWatch {
        &self.watch
    }
}
