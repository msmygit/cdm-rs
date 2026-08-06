//! Buffering, batching and flushing target writes (`MIG-004`, `MIG-005`, `MIG-020`, `MIG-022`).
//!
//! # The flush threshold that actually fires
//!
//! `MIG-004` requires a flush when `UNFLUSHED >= min(fetch_size, max(batch_size * 10, 100))`.
//! Java compares the **committed** `UNFLUSHED`, which is permanently zero, so the flush never
//! happens and every write for a whole token range stays resident. [`WriteBuffer::push`] compares
//! [`MigrateCounters::unflushed_count`], which is the **interim** value — the one that is actually
//! incremented — so the threshold works as written.
//!
//! `mig_004_the_threshold_flushes_mid_range_where_javas_never_would` proves it by counting flushes:
//! a range of a thousand rows at the default threshold of a hundred flushes ten times, where Java
//! flushes once. That is the difference between a bounded buffer and `--driver-memory 25G`.
//!
//! # One buffer per page, and why that is the whole memory story
//!
//! A bound write borrows the response frame it was read from (`MIG-040`), so it cannot outlive its
//! page. The buffer is therefore created per page and drained before the next page is fetched.
//! Since `flush_threshold <= fetch_size` by construction, a full page always crosses the threshold
//! at least once anyway, so the per-page boundary costs nothing and gives `P6` its guarantee for
//! free: at most `fetch_size` rows are resident per worker, whatever the size of the range.
//!
//! # Batch grouping (`MIG-022`)
//!
//! An `UNLOGGED` batch that spans partitions is slower than the writes it replaces: the
//! coordinator fans it out and waits for the slowest replica set. Under
//! `perfops.batch_grouping = strict` — the default — the buffer closes the current batch whenever
//! the next row belongs to a different partition. Because a token-range scan returns rows in
//! partition order, that single comparison is both sufficient and optimal; there is no grouping map
//! and no reordering. `legacy` appends in read order, exactly as Java does.

use cdm_config::types::BatchGrouping;
use cdm_core::CdmError;
use cdm_cql::statement::{BoundValue, IdempotentWrite};
use futures::stream::{FuturesUnordered, StreamExt as _};

use crate::scheduler::limits::InflightPermit;
use crate::scheduler::RangeContext;

use super::counters::MigrateCounters;
use super::settings::MigrateSettings;
use super::sink::WriteSink;

/// One page's worth of buffered target writes.
pub struct WriteBuffer<'page, S: WriteSink + ?Sized> {
    sink: &'page S,
    ctx: &'page RangeContext,
    counters: MigrateCounters,
    settings: MigrateSettings,
    /// The target column positions, within a bound write, that make up the partition key
    /// (`MIG-022`). Empty under `legacy` grouping, which is what turns the comparison off.
    partition_positions: &'page [usize],
    batch: Vec<IdempotentWrite<'page>>,
    inflight: FuturesUnordered<futures::future::BoxFuture<'page, Result<(), CdmError>>>,
    flushes: u64,
}

impl<S: WriteSink + ?Sized> std::fmt::Debug for WriteBuffer<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteBuffer")
            .field("batched", &self.batch.len())
            .field("inflight", &self.inflight.len())
            .field("flushes", &self.flushes)
            .finish_non_exhaustive()
    }
}

impl<'page, S: WriteSink + ?Sized> WriteBuffer<'page, S> {
    /// A buffer for one page.
    pub fn new(
        sink: &'page S,
        ctx: &'page RangeContext,
        counters: MigrateCounters,
        settings: MigrateSettings,
        partition_positions: &'page [usize],
    ) -> Self {
        Self {
            sink,
            ctx,
            counters,
            settings,
            partition_positions: match settings.grouping() {
                BatchGrouping::Strict => partition_positions,
                BatchGrouping::Legacy => &[],
            },
            batch: Vec::with_capacity(settings.batch_size() as usize),
            inflight: FuturesUnordered::new(),
            flushes: 0,
        }
    }

    /// The sink writes go to, so the counter path can issue its own write through the same
    /// object a dry run replaces (`MIG-041`).
    #[must_use]
    pub const fn sink(&self) -> &'page S {
        self.sink
    }

    /// How many times this buffer has flushed, which is what `MIG-004` is measured by.
    #[must_use]
    pub const fn flushes(&self) -> u64 {
        self.flushes
    }

    /// Accepts one ordinary write (`MIG-004`, `MIG-020`, `MIG-022`).
    ///
    /// # Errors
    ///
    /// Any target failure, which fails the range (`ENG-008`, `MIG-005`).
    pub async fn push(&mut self, write: IdempotentWrite<'page>) -> Result<(), CdmError> {
        // ENG-004: one target permit per row, as Java's `rateLimiterTarget.acquire(1)` does. A dry
        // run issues nothing, so it must not consume the target's budget either (`MIG-041`).
        if !self.settings.is_dry_run() {
            self.ctx.acquire_write_rows(1).await;
        }

        if self.settings.is_batching() {
            if !self.batch.is_empty() && !self.same_partition(&write) {
                self.send_batch().await?;
            }
            self.batch.push(write);
            if self.batch.len() >= self.settings.batch_size() as usize {
                self.send_batch().await?;
            }
        } else {
            let permit = self.ctx.write_slot().await?;
            let future = self.sink.write(write);
            self.inflight
                .push(Box::pin(hold(permit, future)) as futures::future::BoxFuture<'page, _>);
        }

        self.count_issued().await
    }

    /// Accepts one counter write, which has already been executed (`MIG-032`).
    ///
    /// Counter updates are neither batched nor pipelined here. The target lookup that computes the
    /// delta (`MIG-031`) already serialises the path row by row, and the value being written
    /// depends on that lookup, so there is nothing to overlap; what pipelining *would* buy is a
    /// second in-flight update whose failure cannot be attributed. The caller therefore awaits the
    /// write and reports it here, which keeps the accounting identical to the ordinary path.
    ///
    /// # Errors
    ///
    /// As [`WriteBuffer::push`].
    pub async fn push_counter_written(&mut self) -> Result<(), CdmError> {
        self.count_issued().await
    }

    /// The target rate permit a counter write needs before it is issued (`ENG-004`).
    ///
    /// Takes `&mut self` although it mutates nothing: an `&self` held across an `await` would make
    /// the enclosing future require `WriteBuffer: Sync`, and the in-flight set is `Send` but not
    /// `Sync`. Asking for the exclusive borrow costs the caller nothing and keeps
    /// `RangeProcessor::process` — which must be `Send` — compiling.
    pub async fn acquire_counter_permit(&mut self) {
        if !self.settings.is_dry_run() {
            self.ctx.acquire_write_rows(1).await;
        }
    }

    /// `UNFLUSHED += 1`, then the threshold test of `MIG-004`.
    async fn count_issued(&mut self) -> Result<(), CdmError> {
        self.ctx.counters().increment(self.counters.unflushed());
        // MIG-004: the *interim* count. Reading the committed one here is the Java defect.
        let unflushed = self.counters.unflushed_count(self.ctx.counters());
        if self.settings.should_flush(unflushed) {
            self.flush().await?;
        }
        Ok(())
    }

    /// Waits for every issued write, then credits `WRITE` (`MIG-004`, `MIG-005`).
    ///
    /// # Errors
    ///
    /// The first failure any in-flight write reported. `MIG-005` makes a flush failure a range
    /// failure, so nothing is credited: the rows this flush covered are accounted by `ENG-008`'s
    /// `READ - WRITE - SKIPPED` arithmetic instead.
    pub async fn flush(&mut self) -> Result<(), CdmError> {
        if !self.batch.is_empty() {
            self.send_batch().await?;
        }

        let mut failure = None;
        while let Some(result) = self.inflight.next().await {
            // Every future is drained even after a failure: a dropped in-flight request is a
            // request whose outcome nobody knows, and on a shutdown path that is exactly the
            // ambiguity `ENG-010` and `DST-015` are trying to avoid.
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }

        let unflushed = self.counters.unflushed_count(self.ctx.counters());
        if unflushed > 0 {
            // MIG-005: WRITE is credited only once the writes have actually completed.
            self.ctx
                .counters()
                .increment_by(self.counters.write(), unflushed);
            self.ctx.counters().reset(self.counters.unflushed());
        }
        self.flushes = self.flushes.saturating_add(1);
        Ok(())
    }

    /// Sends the accumulated batch (`MIG-020`).
    async fn send_batch(&mut self) -> Result<(), CdmError> {
        if self.batch.is_empty() {
            return Ok(());
        }
        let writes = std::mem::replace(
            &mut self.batch,
            Vec::with_capacity(self.settings.batch_size() as usize),
        );
        let permit = self.ctx.write_slot().await?;
        let future = self.sink.write_batch(writes);
        self.inflight
            .push(Box::pin(hold(permit, future)) as futures::future::BoxFuture<'page, _>);
        Ok(())
    }

    /// Whether `write` belongs to the same partition as the batch already being accumulated
    /// (`MIG-022`).
    ///
    /// Compares the bound values at the partition-key positions against the last row in the batch.
    /// No allocation, no hashing, and exact: two rows are in the same partition precisely when
    /// their partition-key bytes are equal, which is what these values are.
    fn same_partition(&self, write: &IdempotentWrite<'page>) -> bool {
        if self.partition_positions.is_empty() {
            return true;
        }
        let Some(last) = self.batch.last() else {
            return true;
        };
        let previous = last.values().values();
        let current = write.values().values();
        self.partition_positions.iter().all(|&position| {
            match (previous.get(position), current.get(position)) {
                (Some(a), Some(b)) => same_value(a, b),
                // A position the bound write does not have cannot distinguish two rows, and
                // guessing that they differ would disable batching entirely.
                _ => true,
            }
        })
    }
}

/// Whether two bound values are the same partition-key component.
fn same_value(left: &BoundValue<'_>, right: &BoundValue<'_>) -> bool {
    left.bytes() == right.bytes()
}

/// Runs `future` while holding `permit`, so the in-flight slot is released when the request ends
/// rather than when the buffer is dropped (`ENG-007`).
async fn hold(
    permit: InflightPermit,
    future: futures::future::BoxFuture<'_, Result<(), CdmError>>,
) -> Result<(), CdmError> {
    let result = future.await;
    drop(permit);
    result
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
    use std::sync::Arc;

    use cdm_core::{JobKind, RunId, TokenRange};
    use cdm_metrics::{CounterKind, CounterView, JobCounters};
    use tokio_util::sync::CancellationToken;

    use cdm_cql::statement::BindInputs;

    use crate::migrate::sink::tests::RecordingSink;
    use crate::migrate::testfixtures::{binder, row_of};
    use crate::scheduler::limits::RuntimeLimits;
    use crate::scheduler::SchedulerSettings;

    use super::*;

    fn context() -> RangeContext {
        let settings = SchedulerSettings::default().with_ratelimits(0, 0);
        RangeContext::new(
            RunId::from_raw(1),
            Arc::from("node"),
            TokenRange::new(-10, 10).unwrap(),
            1_000,
            Arc::new(JobCounters::new(JobKind::Migrate)),
            Arc::new(RuntimeLimits::new(&settings).unwrap()),
            CancellationToken::new(),
        )
    }

    #[tokio::test]
    async fn mig_004_the_threshold_flushes_mid_range_where_javas_never_would() {
        let sink = RecordingSink::new();
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(1, 1_000, BatchGrouping::Strict, false, false, false);
        assert_eq!(settings.flush_threshold(), 100);

        let binder = binder();
        let rows: Vec<_> = (0..1_000).map(|i| row_of(i, "v")).collect();
        let mut buffer = WriteBuffer::new(&sink, &ctx, counters, settings, &[]);
        for row in &rows {
            let bound = binder.bind(&row, BindInputs::default()).unwrap();
            match bound {
                cdm_cql::statement::Bound::Idempotent(write) => buffer.push(write).await.unwrap(),
                cdm_cql::statement::Bound::Counter(_) => panic!("not a counter table"),
            }
        }
        assert_eq!(
            buffer.flushes(),
            10,
            "a thousand rows at a threshold of a hundred flushes ten times; Java flushes once"
        );
        assert_eq!(sink.rows(), 1_000);
        assert_eq!(
            ctx.counters()
                .count_of(CounterKind::Write, CounterView::Interim),
            1_000
        );
        assert_eq!(
            ctx.counters()
                .count_of(CounterKind::Unflushed, CounterView::Interim),
            0,
            "MIG-004: UNFLUSHED is reset once its rows are credited"
        );
    }

    #[tokio::test]
    async fn mig_005_write_is_credited_only_once_the_flush_succeeds() {
        let sink = RecordingSink::new();
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(1, 1_000, BatchGrouping::Strict, false, false, false);
        let binder = binder();
        let rows: Vec<_> = (0..5).map(|i| row_of(i, "v")).collect();
        let mut buffer = WriteBuffer::new(&sink, &ctx, counters, settings, &[]);
        for row in &rows {
            let cdm_cql::statement::Bound::Idempotent(write) =
                binder.bind(&row, BindInputs::default()).unwrap()
            else {
                panic!("not a counter table")
            };
            buffer.push(write).await.unwrap();
        }

        assert_eq!(
            ctx.counters()
                .count_of(CounterKind::Write, CounterView::Interim),
            0,
            "nothing is credited before the flush"
        );
        assert_eq!(
            ctx.counters()
                .count_of(CounterKind::Unflushed, CounterView::Interim),
            5
        );

        buffer.flush().await.unwrap();
        assert_eq!(
            ctx.counters()
                .count_of(CounterKind::Write, CounterView::Interim),
            5
        );
    }

    #[tokio::test]
    async fn mig_005_a_flush_failure_credits_nothing_and_fails_the_range() {
        let sink = RecordingSink::failing_after(2);
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(1, 1_000, BatchGrouping::Strict, false, false, false);
        let binder = binder();
        let rows: Vec<_> = (0..4).map(|i| row_of(i, "v")).collect();
        let mut buffer = WriteBuffer::new(&sink, &ctx, counters, settings, &[]);
        for row in &rows {
            let cdm_cql::statement::Bound::Idempotent(write) =
                binder.bind(&row, BindInputs::default()).unwrap()
            else {
                panic!("not a counter table")
            };
            buffer.push(write).await.unwrap();
        }
        let error = buffer.flush().await.unwrap_err();
        assert_eq!(error.kind(), cdm_core::ErrorKind::Write);
        assert_eq!(
            ctx.counters()
                .count_of(CounterKind::Write, CounterView::Interim),
            0,
            "MIG-005 credits successfully written rows only"
        );
    }

    #[tokio::test]
    async fn mig_020_writes_accumulate_into_batches_of_the_configured_size() {
        let sink = RecordingSink::new();
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(3, 1_000, BatchGrouping::Legacy, false, false, false);
        let binder = binder();
        let rows: Vec<_> = (0..7).map(|i| row_of(i, "v")).collect();
        let mut buffer = WriteBuffer::new(&sink, &ctx, counters, settings, &[]);
        for row in &rows {
            let cdm_cql::statement::Bound::Idempotent(write) =
                binder.bind(&row, BindInputs::default()).unwrap()
            else {
                panic!("not a counter table")
            };
            buffer.push(write).await.unwrap();
        }
        buffer.flush().await.unwrap();

        assert_eq!(
            *sink.batches.lock(),
            vec![3, 3, 1],
            "two full batches and the remainder"
        );
        assert_eq!(sink.singles.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn mig_022_strict_grouping_never_lets_a_batch_span_two_partitions() {
        let sink = RecordingSink::new();
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(10, 1_000, BatchGrouping::Strict, false, false, false);
        let binder = binder();
        // Partition 1 three times, then partition 2 twice: the partition change closes the batch
        // well before `batch_size` is reached.
        let rows: Vec<_> = [1, 1, 1, 2, 2].iter().map(|i| row_of(*i, "v")).collect();
        let mut buffer = WriteBuffer::new(&sink, &ctx, counters, settings, &[0]);
        for row in &rows {
            let cdm_cql::statement::Bound::Idempotent(write) =
                binder.bind(&row, BindInputs::default()).unwrap()
            else {
                panic!("not a counter table")
            };
            buffer.push(write).await.unwrap();
        }
        buffer.flush().await.unwrap();
        assert_eq!(*sink.batches.lock(), vec![3, 2]);
    }

    #[tokio::test]
    async fn mig_022_legacy_grouping_reproduces_javas_index_order_batching() {
        let sink = RecordingSink::new();
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(10, 1_000, BatchGrouping::Legacy, false, false, false);
        let binder = binder();
        let rows: Vec<_> = [1, 1, 1, 2, 2].iter().map(|i| row_of(*i, "v")).collect();
        // The partition positions are supplied, and `legacy` ignores them.
        let mut buffer = WriteBuffer::new(&sink, &ctx, counters, settings, &[0]);
        for row in &rows {
            let cdm_cql::statement::Bound::Idempotent(write) =
                binder.bind(&row, BindInputs::default()).unwrap()
            else {
                panic!("not a counter table")
            };
            buffer.push(write).await.unwrap();
        }
        buffer.flush().await.unwrap();
        assert_eq!(
            *sink.batches.lock(),
            vec![5],
            "Java batches by index order, whatever partition a row belongs to"
        );
    }

    /// A sink that records the address of the bytes it was handed, for the passthrough proof.
    ///
    /// The address is kept as a `usize` rather than a `*const u8`: nothing here dereferences it,
    /// `#![forbid(unsafe_code)]` rules out the `unsafe impl Send` a raw pointer would need, and
    /// comparing addresses is the whole of the assertion.
    #[derive(Debug, Default)]
    struct PointerSink {
        seen: parking_lot::Mutex<Vec<usize>>,
    }

    impl WriteSink for PointerSink {
        fn write<'w>(
            &'w self,
            write: cdm_cql::statement::IdempotentWrite<'w>,
        ) -> futures::future::BoxFuture<'w, Result<(), CdmError>> {
            if let Some(bytes) = write.values().values().get(1).and_then(|v| v.bytes()) {
                self.seen.lock().push(bytes.as_ptr() as usize);
            }
            Box::pin(std::future::ready(Ok(())))
        }

        fn write_batch<'w>(
            &'w self,
            writes: Vec<cdm_cql::statement::IdempotentWrite<'w>>,
        ) -> futures::future::BoxFuture<'w, Result<(), CdmError>> {
            for write in &writes {
                if let Some(bytes) = write.values().values().get(1).and_then(|v| v.bytes()) {
                    self.seen.lock().push(bytes.as_ptr() as usize);
                }
            }
            Box::pin(std::future::ready(Ok(())))
        }

        fn write_counter<'w>(
            &'w self,
            _write: cdm_cql::statement::CounterWrite<'w>,
        ) -> futures::future::BoxFuture<'w, Result<(), CdmError>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }

    #[tokio::test]
    async fn mig_041_a_buffered_write_still_points_at_the_bytes_it_was_read_from() {
        // The passthrough claim of MIG-040/MIG-041 is not "the bytes are equal" — a copy would
        // pass that. It is "the bytes are the same bytes", so this compares addresses all the way
        // through the buffer and out the other side.
        let sink = PointerSink::default();
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(1, 1_000, BatchGrouping::Strict, false, false, false);
        let binder = binder();
        let source = row_of(1, "the quick brown fox");
        let expected = source.get(1).unwrap().bytes().unwrap().as_ptr() as usize;

        let mut buffer = WriteBuffer::new(&sink, &ctx, counters, settings, &[]);
        let cdm_cql::statement::Bound::Idempotent(write) =
            binder.bind(&&source, BindInputs::default()).unwrap()
        else {
            panic!("not a counter table")
        };
        buffer.push(write).await.unwrap();
        buffer.flush().await.unwrap();

        assert_eq!(
            *sink.seen.lock(),
            vec![expected],
            "the buffer must hand the target the row\'s own bytes, not a copy of them"
        );
    }

    #[tokio::test]
    async fn mig_041_a_batched_write_also_keeps_the_borrow() {
        let sink = PointerSink::default();
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(4, 1_000, BatchGrouping::Legacy, false, false, false);
        let binder = binder();
        let rows: Vec<_> = (0..3).map(|i| row_of(i, "payload")).collect();
        let expected: Vec<_> = rows
            .iter()
            .map(|r| r.get(1).unwrap().bytes().unwrap().as_ptr() as usize)
            .collect();

        let mut buffer = WriteBuffer::new(&sink, &ctx, counters, settings, &[]);
        for row in &rows {
            let cdm_cql::statement::Bound::Idempotent(write) =
                binder.bind(&row, BindInputs::default()).unwrap()
            else {
                panic!("not a counter table")
            };
            buffer.push(write).await.unwrap();
        }
        buffer.flush().await.unwrap();
        assert_eq!(*sink.seen.lock(), expected);
    }

    #[tokio::test]
    async fn mig_041_a_dry_run_does_not_spend_the_targets_rate_budget() {
        let sink = RecordingSink::new();
        let ctx = context();
        let counters = MigrateCounters::resolve(ctx.counters()).unwrap();
        let settings = MigrateSettings::new(1, 1_000, BatchGrouping::Strict, false, false, true);
        let mut buffer: WriteBuffer<'_, RecordingSink> =
            WriteBuffer::new(&sink, &ctx, counters, settings, &[]);
        // Nothing to await: a dry run's counter permit is a no-op, which is what makes
        // `--dry-run` finish at read speed rather than at the target's configured write speed.
        buffer.acquire_counter_permit().await;
    }
}
