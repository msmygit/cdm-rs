//! Paging one token range off the origin, without deserialising anything (`ENG-003`, `MIG-040`).
//!
//! # Why this is a page loop and not a row stream
//!
//! `Stream` cannot lend. `QueryPager::rows_stream` therefore demands a row type that is owned for
//! any frame lifetime, and [`RawRow`](crate::raw::RawRow) — whose whole purpose is to *borrow* the
//! frame — cannot be one. A row stream would mean decoding every cell into an owned value on the
//! read side and re-encoding it on the write side, which is exactly the cost `MIG-040` exists to
//! avoid, and which is measured in whole seconds per million rows.
//!
//! So the unit here is the page. [`Page`] owns the driver's decoded page, [`Page::rows`] lends
//! rows out of it, and the caller finishes with a page before asking for the next one. That is
//! also what bounds memory (`P6`, `NFR-003`): at most `perfops.fetch_size` rows are resident per
//! worker, whatever the size of the range.
//!
//! # Backoff lives here, not in the driver
//!
//! `CON-011` asks for exponential backoff with jitter between attempts. The driver's retry policy
//! decides *whether* to retry and *where*, but [`RetryDecision`](scylla::policies::retry::RetryDecision)
//! carries no delay and a `RetrySession` cannot sleep. So the driver covers the immediate
//! same-target/next-target retries and [`RangeScan::next_page`] covers the paced ones, which is
//! also the only level at which a paged read *can* be retried: a page request that fails is
//! re-issued with the same paging state, and re-reading a page is free of consequence.

use cdm_core::{CdmError, ErrorKind, Side, TokenRange};
use scylla::deserialize::result::TypedRowIterator;
use scylla::errors::ExecutionError;
use scylla::response::query_result::{QueryRowsResult, RowsError};
use scylla::response::PagingState;
use scylla::serialize::row::{RowSerializationContext, SerializeRow};
use scylla::serialize::writers::RowWriter;
use scylla::serialize::SerializationError;
use scylla::statement::prepared::PreparedStatement;

use crate::connect::Backoff;
use crate::errors::side_error_from;
use crate::raw::RawRow;
use crate::statement::TokenBound;

use super::DriverSession;

/// One decoded page of origin rows, owning the frame the rows borrow from.
#[derive(Debug)]
pub struct Page {
    rows: QueryRowsResult,
}

impl Page {
    /// How many rows the page holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.rows_num()
    }

    /// Whether the page is empty, which a well-behaved server only produces at the end of a range.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The serialised size of the page, for the throughput instruments of `MET-010`.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.rows.rows_bytes_size()
    }

    /// The rows, still in their wire representation (`MIG-040`).
    ///
    /// Each [`RawRow`] borrows this page, so the returned iterator cannot outlive it — which is
    /// the borrow checker enforcing the memory bound the page loop is there to provide.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Read`] if the page's column specifications cannot be type-checked.
    /// `RawRow` accepts any row shape, so in practice this is unreachable; it is surfaced rather
    /// than swallowed because a driver that started rejecting it would otherwise fail silently.
    pub fn rows(&self) -> Result<PageRows<'_>, CdmError> {
        let iter = self
            .rows
            .rows::<RawRow<'_, '_>>()
            .map_err(|error: RowsError| read_error("the origin page could not be typed", error))?;
        Ok(PageRows { iter })
    }
}

/// The rows of one [`Page`], lent out of it.
pub struct PageRows<'page> {
    iter: TypedRowIterator<'page, 'page, RawRow<'page, 'page>>,
}

impl std::fmt::Debug for PageRows<'_> {
    /// The iterator itself is not `Debug`, and its contents are row values, which `SEC-002`
    /// forbids rendering.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PageRows { .. }")
    }
}

impl<'page> Iterator for PageRows<'page> {
    type Item = Result<RawRow<'page, 'page>, CdmError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter
            .next()
            .map(|row| row.map_err(|error| read_error("a row could not be read", error)))
    }
}

/// A paged scan of one token range (`ENG-003`, `FEA-060`).
#[derive(Debug)]
pub struct RangeScan<'a> {
    session: &'a DriverSession,
    prepared: &'a PreparedStatement,
    bounds: TokenBinds,
    paging_state: PagingState,
    backoff: Backoff,
    finished: bool,
}

impl<'a> RangeScan<'a> {
    /// Starts a scan of `range`, with token bounds typed for `partitioner`.
    ///
    /// Nothing is sent until the first [`RangeScan::next_page`], so constructing a scan for a
    /// range the run then decides to skip costs nothing.
    #[must_use]
    pub fn new(
        session: &'a DriverSession,
        prepared: &'a PreparedStatement,
        min: TokenBound,
        max: TokenBound,
        backoff: Backoff,
    ) -> Self {
        Self {
            session,
            prepared,
            bounds: TokenBinds([min, max]),
            paging_state: PagingState::start(),
            backoff,
            finished: false,
        }
    }

    /// Starts a scan of a range whose tokens are typed by `partitioner`.
    ///
    /// [`TokenRange`] carries `i128` bounds because a `RandomPartitioner` token runs to
    /// `2^127 - 1`; a Murmur3 bound that does not fit an `i64` is a planner bug rather than a
    /// datum, so it saturates rather than wrapping — a wrapped bound would silently scan the
    /// wrong part of the ring.
    #[must_use]
    pub fn for_range(
        session: &'a DriverSession,
        prepared: &'a PreparedStatement,
        range: TokenRange,
        partitioner: TokenWidth,
        backoff: Backoff,
    ) -> Self {
        let (min, max) = match partitioner {
            TokenWidth::Murmur3 => (
                TokenBound::Murmur3(saturating_i64(range.min())),
                TokenBound::Murmur3(saturating_i64(range.max())),
            ),
            TokenWidth::Random => (
                TokenBound::Random(range.min()),
                TokenBound::Random(range.max()),
            ),
        };
        Self::new(session, prepared, min, max, backoff)
    }

    /// The next page, or `None` once the range is exhausted.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Read`] once the attempts allowed by `perfops.retry.max_attempts` are used up,
    /// or immediately for a failure the retry classification calls deterministic. The error fails
    /// the range and only the range (`ENG-008`).
    pub async fn next_page(&mut self) -> Result<Option<Page>, CdmError> {
        if self.finished {
            return Ok(None);
        }
        let mut attempts = 0u32;
        loop {
            attempts = attempts.saturating_add(1);
            match self.attempt().await {
                Ok(page) => return Ok(page),
                Err(error) => {
                    if !self.backoff.may_retry(attempts) || !error.is_retryable() {
                        return Err(error);
                    }
                    let delay = self.backoff.delay_for(attempts);
                    tracing::warn!(
                        target: "cdm::cql::exec",
                        attempt = attempts,
                        delay_ms = delay.as_millis(),
                        error = %error,
                        "retrying an origin page (CON-011)"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// One attempt at the current paging state.
    async fn attempt(&mut self) -> Result<Option<Page>, CdmError> {
        let (result, next) = self
            .session
            .execute_single_page(self.prepared, &self.bounds, self.paging_state.clone())
            .await
            .map_err(|error: ExecutionError| {
                read_error("the origin token-range scan failed", error)
            })?;

        match next.into_paging_control_flow() {
            std::ops::ControlFlow::Continue(state) => self.paging_state = state,
            std::ops::ControlFlow::Break(()) => self.finished = true,
        }

        let rows = result
            .into_rows_result()
            .map_err(|error| read_error("the origin page carried no rows section", error))?;
        Ok(Some(Page { rows }))
    }
}

/// The two token bounds, bound as the partitioner's own type (`TOK-001`).
///
/// A hand-written [`SerializeRow`] rather than a tuple: a Murmur3 token is a `bigint` and a
/// Random-partitioner token is a `varint` of up to sixteen bytes, and no Rust tuple types both.
/// [`TokenBound::serialized`] already produces the wire form for each, so this writes it through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TokenBinds([TokenBound; 2]);

impl SerializeRow for TokenBinds {
    fn serialize(
        &self,
        ctx: &RowSerializationContext<'_>,
        writer: &mut RowWriter<'_>,
    ) -> Result<(), SerializationError> {
        if ctx.columns().len() != 2 {
            return Err(SerializationError::new(TokenArityError(
                ctx.columns().len(),
            )));
        }
        for bound in self.0 {
            writer
                .make_cell_writer()
                .set_value(&bound.serialized())
                .map_err(SerializationError::new)?;
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        false
    }
}

/// The range scan wants exactly two bind markers, one per token bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TokenArityError(usize);

impl std::fmt::Display for TokenArityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the origin range scan has {} bind markers but a token range supplies exactly two \
             (FEA-060)",
            self.0
        )
    }
}

impl std::error::Error for TokenArityError {}

/// How wide the partitioner's tokens are (`TOK-001`).
///
/// A `bigint` for `Murmur3Partitioner`, a `varint` for `RandomPartitioner` and
/// `ByteOrderedPartitioner`. The distinction cannot be inferred from a [`TokenRange`], whose
/// bounds are always `i128`, so the caller — which introspected the partitioner at startup —
/// states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenWidth {
    /// `Murmur3Partitioner`: tokens are `bigint`.
    Murmur3,
    /// `RandomPartitioner`: tokens are `varint`.
    Random,
}

/// Narrows a planned bound to the `i64` a Murmur3 token is, without wrapping.
fn saturating_i64(token: i128) -> i64 {
    i64::try_from(token).unwrap_or(if token < 0 { i64::MIN } else { i64::MAX })
}

fn read_error(
    message: &str,
    cause: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
) -> CdmError {
    side_error_from(ErrorKind::Read, Side::Origin, message.to_owned(), cause)
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
    fn fea_060_the_token_binds_write_the_partitioners_own_wire_form() {
        let binds = TokenBinds([TokenBound::Murmur3(-1), TokenBound::Murmur3(1)]);
        let mut buffer = Vec::new();
        let mut writer = RowWriter::new(&mut buffer);
        for bound in binds.0 {
            writer
                .make_cell_writer()
                .set_value(&bound.serialized())
                .unwrap();
        }
        // Two eight-byte `bigint` values, each preceded by its four-byte length.
        assert_eq!(&buffer[0..4], &8i32.to_be_bytes());
        assert_eq!(&buffer[4..12], &(-1i64).to_be_bytes());
        assert_eq!(&buffer[12..16], &8i32.to_be_bytes());
        assert_eq!(&buffer[16..24], &1i64.to_be_bytes());
        assert!(!SerializeRow::is_empty(&binds));
    }

    #[test]
    fn fea_060_a_random_partitioner_range_binds_a_varint() {
        let binds = TokenBinds([TokenBound::Random(0), TokenBound::Random(i128::MAX)]);
        assert_eq!(binds.0[0].serialized(), vec![0x00]);
        assert_eq!(binds.0[1].serialized().len(), 16);
    }

    #[test]
    fn con_011_the_arity_error_names_the_requirement() {
        let rendered = TokenArityError(3).to_string();
        assert!(rendered.contains("exactly two"), "{rendered}");
        assert!(rendered.contains("FEA-060"), "{rendered}");
    }

    #[test]
    fn con_011_a_read_failure_is_retryable_and_carries_the_origin_side() {
        let error = read_error("boom", std::io::Error::other("network"));
        assert_eq!(error.kind(), ErrorKind::Read);
        assert!(error.is_retryable());
        assert_eq!(error.context().side, Some(Side::Origin));
    }
}
