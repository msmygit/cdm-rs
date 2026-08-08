//! Writing the target: single writes, batches, counter writes and the counter probe.
//!
//! # Three methods, because there are three different safety stories
//!
//! | Method | Accepts | Retried? | Why |
//! |---|---|---|---|
//! | [`TargetWriter::write`] | [`IdempotentWrite`] | yes, `CON-011` | re-applying an upsert with the origin's writetime is a no-op |
//! | [`TargetWriter::write_batch`] | `&[IdempotentWrite]` | yes, `CON-011` | an `UNLOGGED` batch of idempotent writes is idempotent |
//! | [`TargetWriter::write_counter`] | [`CounterWrite`] | **never**, `CON-012` | a counter `UPDATE` applied twice adds the delta twice |
//!
//! The distinction is carried by the *types*, not by a boolean. [`Idempotent`] is sealed and
//! implemented only for [`IdempotentWrite`], so `write` and `write_batch` cannot be handed a
//! counter write even by a caller that wants to: the code does not compile. Deleting a runtime
//! `if is_counter` guard is a one-line edit; deleting this one is a type error at every call site.
//!
//! # Why a counter failure has to fail the range
//!
//! When a counter write times out, the update may or may not have been applied, and nothing can
//! tell the two apart afterwards — a counter carries no writetime and no version. `CON-012`
//! therefore surfaces the failure, `ENG-008` fails the range, and `DST-015` keeps a resume from
//! quietly replaying it. The one thing that must never happen is a retry that silently
//! double-counts, and the only way to guarantee that is to never issue one.

use std::time::Duration;

use cdm_core::{CdmError, ErrorKind, Operation, Side};
use scylla::errors::ExecutionError;
use scylla::response::query_result::QueryRowsResult;
use scylla::statement::batch::{Batch, BatchStatement, BatchType};
use scylla::statement::prepared::PreparedStatement;

use crate::connect::Backoff;
use crate::errors::side_error_from;
use crate::observe::RequestMetrics;
use crate::statement::{BoundWrite, CounterWrite, IdempotentWrite};

use super::DriverSession;

/// A reusable `UNLOGGED` batch of `capacity` copies of the target upsert (`MIG-020`).
///
/// Cloning a [`PreparedStatement`] is cheap — the expensive parts are behind an `Arc` — but it is
/// not free, and a batch that is rebuilt from scratch on every flush clones `batch_size`
/// statements per flush for no reason. The template clones them once and hands out a [`Batch`]
/// sized to the number of rows actually being sent, which is what a partial flush needs.
#[derive(Clone)]
pub struct BatchTemplate {
    statements: Vec<BatchStatement>,
    consistency: scylla::statement::Consistency,
    timeout: Duration,
}

impl std::fmt::Debug for BatchTemplate {
    /// `BatchStatement` is not `Debug`, and its CQL is already logged once at startup
    /// (`FEA-062`), so the template reports only its shape.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchTemplate")
            .field("capacity", &self.statements.len())
            .field("consistency", &self.consistency)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl BatchTemplate {
    /// Builds a template holding `capacity` copies of `prepared`.
    #[must_use]
    pub fn new(
        prepared: &PreparedStatement,
        capacity: usize,
        consistency: scylla::statement::Consistency,
        timeout: Duration,
    ) -> Self {
        Self {
            statements: std::iter::repeat_n(prepared.clone(), capacity)
                .map(BatchStatement::from)
                .collect(),
            consistency,
            timeout,
        }
    }

    /// How many statements the template can supply.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.statements.len()
    }

    /// A batch of exactly `len` statements, or `None` if the template is too small — which the
    /// caller prevents by construction, since it sized the template from `perfops.batch_size`.
    fn batch(&self, len: usize) -> Option<Batch> {
        let statements = self.statements.get(..len)?;
        let mut batch = Batch::new_with_statements(BatchType::Unlogged, statements.to_vec());
        batch.set_consistency(self.consistency);
        batch.set_request_timeout(Some(self.timeout));
        // CON-011: a batch of idempotent upserts is itself idempotent. `write_batch` only accepts
        // `IdempotentWrite`s, so there is no shape of this type that carries a counter update.
        batch.set_is_idempotent(true);
        Some(batch)
    }
}

/// Executes target writes (`MIG-005`, `MIG-020`, `MIG-030`, `CON-011`, `CON-012`).
#[derive(Debug)]
pub struct TargetWriter<'a> {
    session: &'a DriverSession,
    upsert: &'a PreparedStatement,
    select_by_pk: &'a PreparedStatement,
    backoff: Backoff,
    metrics: &'a RequestMetrics,
}

impl<'a> TargetWriter<'a> {
    /// Builds a writer over the target session and its prepared statements.
    #[must_use]
    pub const fn new(
        session: &'a DriverSession,
        upsert: &'a PreparedStatement,
        select_by_pk: &'a PreparedStatement,
        backoff: Backoff,
        metrics: &'a RequestMetrics,
    ) -> Self {
        Self {
            session,
            upsert,
            select_by_pk,
            backoff,
            metrics,
        }
    }

    /// Executes one idempotent write, retrying with backoff and jitter (`CON-011`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Write`] once the attempts allowed by `perfops.retry.max_attempts` are used up.
    pub async fn write(&self, write: &IdempotentWrite<'_>) -> Result<(), CdmError> {
        let values = write.values();
        self.retrying(Operation::Write, || {
            self.session.execute_unpaged(self.upsert, values)
        })
        .await
    }

    /// Executes one `UNLOGGED` batch of idempotent writes (`MIG-020`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the template cannot supply that many statements, and
    /// [`ErrorKind::Write`] for a batch the server rejects after every allowed attempt.
    pub async fn write_batch(
        &self,
        template: &BatchTemplate,
        writes: &[IdempotentWrite<'_>],
    ) -> Result<(), CdmError> {
        let batch = template.batch(writes.len()).ok_or_else(|| {
            CdmError::new(
                ErrorKind::Internal,
                format!(
                    "a batch of {} rows was assembled from a template of {} statements (MIG-020)",
                    writes.len(),
                    template.capacity()
                ),
            )
        })?;
        let values: Vec<&BoundWrite<'_>> = writes.iter().map(IdempotentWrite::values).collect();
        // MET-010: the size actually sent, not the configured `perfops.batch_size`. A distribution
        // pinned at 1 against a configured 5 is how the coercion of `MIG-021` announces itself.
        self.metrics.batch(writes.len());
        self.retrying(Operation::Batch, || self.session.batch(&batch, &values[..]))
            .await
    }

    /// Executes one counter write. **Once.** (`CON-012`, `MIG-032`.)
    ///
    /// There is no retry loop here and there must never be one: a counter `UPDATE` that timed out
    /// may have been applied, and applying it again adds the delta a second time. The prepared
    /// statement is also marked non-idempotent (`PreparedSet::prepare`), so the driver's own retry
    /// policy declines it too — two independent guards, because this is the failure that cannot
    /// be detected afterwards.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Write`] on the first failure, which fails the range (`ENG-008`).
    pub async fn write_counter(&self, write: &CounterWrite<'_>) -> Result<(), CdmError> {
        let guard = self.metrics.begin(Side::Target, Operation::Write);
        let executed = self
            .session
            .execute_unpaged(self.upsert, write.values())
            .await;
        drop(guard);
        executed.map(|_| ()).map_err(|error| {
            write_error(
                "a counter update failed and will not be retried: re-applying it could \
                     double-count (CON-012, MIG-032)",
                error,
            )
        })
    }

    /// Reads the target row for one primary key, for the counter delta (`MIG-031`).
    ///
    /// Returns `None` when the target has no such row, which the caller treats as a current value
    /// of zero — exactly as Java's `null == targetRow ? 0L` does.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Read`] once the allowed attempts are used up. The read is idempotent, so it is
    /// retried; only the *write* it feeds is not.
    pub async fn counter_row(&self, key: &BoundWrite<'_>) -> Result<Option<CounterRow>, CdmError> {
        let mut attempts = 0u32;
        loop {
            attempts = attempts.saturating_add(1);
            let guard = self.metrics.begin(Side::Target, Operation::KeyRead);
            let executed = self.session.execute_unpaged(self.select_by_pk, key).await;
            drop(guard);
            let attempt = executed.map_err(|error: ExecutionError| {
                side_error_from(
                    ErrorKind::Read,
                    Side::Target,
                    "the target lookup for a counter delta failed (MIG-031)".to_owned(),
                    error,
                )
            });
            match attempt {
                Ok(result) => {
                    let rows = result.into_rows_result().map_err(|error| {
                        side_error_from(
                            ErrorKind::Read,
                            Side::Target,
                            "the counter lookup returned no rows section (MIG-031)".to_owned(),
                            error,
                        )
                    })?;
                    return Ok((rows.rows_num() > 0).then(|| CounterRow { rows }));
                }
                Err(error) => {
                    if !self.backoff.should_retry(&error, attempts) {
                        return Err(error);
                    }
                    self.metrics.retried(&error);
                    tokio::time::sleep(self.backoff.delay_for(attempts)).await;
                }
            }
        }
    }

    /// The retry loop of `CON-011`, shared by every idempotent target request.
    ///
    /// `operation` is the latency dimension each attempt is recorded under (`MET-010`): a single
    /// upsert and an unlogged batch are different requests with very different distributions, and
    /// averaging them would hide the one an operator is tuning.
    async fn retrying<F, Fut>(&self, operation: Operation, mut attempt: F) -> Result<(), CdmError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<
            Output = Result<scylla::response::query_result::QueryResult, ExecutionError>,
        >,
    {
        let mut attempts = 0u32;
        loop {
            attempts = attempts.saturating_add(1);
            let guard = self.metrics.begin(Side::Target, operation);
            let executed = attempt().await;
            drop(guard);
            match executed {
                Ok(_) => return Ok(()),
                Err(error) => {
                    let error = write_error("the target write failed", error);
                    if !self.backoff.should_retry(&error, attempts) {
                        return Err(error);
                    }
                    self.metrics.retried(&error);
                    let delay = self.backoff.delay_for(attempts);
                    tracing::warn!(
                        target: "cdm::cql::exec",
                        attempt = attempts,
                        delay_ms = delay.as_millis(),
                        error = %error,
                        "retrying a target write (CON-011)"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

/// The target row a counter delta is computed against (`MIG-031`).
///
/// Owns the driver's decoded page so that [`CounterRow::row`] can lend a
/// [`RawRow`](crate::raw::RawRow) out of it, for the same reason
/// [`Page`](super::Page) does: reading the current counter is a read like any other, and decoding
/// it into an owned value would be one more copy per row on the slowest path there is.
#[derive(Debug)]
pub struct CounterRow {
    rows: QueryRowsResult,
}

impl CounterRow {
    /// The single row the lookup returned, or `None` if it returned none.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Read`] if the row cannot be read off the frame.
    pub fn row(&self) -> Result<Option<crate::raw::RawRow<'_, '_>>, CdmError> {
        let mut rows = self
            .rows
            .rows::<crate::raw::RawRow<'_, '_>>()
            .map_err(|error| {
                side_error_from(
                    ErrorKind::Read,
                    Side::Target,
                    "the counter lookup's row could not be typed (MIG-031)".to_owned(),
                    error,
                )
            })?;
        rows.next().transpose().map_err(|error| {
            side_error_from(
                ErrorKind::Read,
                Side::Target,
                "the counter lookup's row could not be read (MIG-031)".to_owned(),
                error,
            )
        })
    }
}

fn write_error(
    message: &str,
    cause: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
) -> CdmError {
    side_error_from(ErrorKind::Write, Side::Target, message.to_owned(), cause)
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
    fn con_012_a_write_failure_is_retryable_but_names_the_target() {
        let error = write_error("boom", std::io::Error::other("network"));
        assert_eq!(error.kind(), ErrorKind::Write);
        assert_eq!(error.context().side, Some(Side::Target));
        assert!(error.is_retryable());
    }

    #[test]
    fn con_012_the_batch_template_only_ever_carries_idempotent_statements() {
        // `write_batch` takes `&[IdempotentWrite]`, so no shape of a batch can hold a counter
        // update. That is the structural half of CON-012: there is no runtime check to delete.
        fn accepts(_writes: &[IdempotentWrite<'_>]) {}
        accepts(&[]);
    }
}
