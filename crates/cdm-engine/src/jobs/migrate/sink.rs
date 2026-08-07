//! Where a bound write goes (`MIG-005`, `MIG-020`, `MIG-032`, `MIG-041`).
//!
//! # Why the sink is a trait
//!
//! Two reasons, and only one of them is testing.
//!
//! The first is `MIG-041`. A dry run must "execute the full read + transform + bind pipeline and
//! count everything, but issue no target writes". The only way to be sure a dry run exercises the
//! same code as a real one is for it to *be* the same code, with one object swapped: [`DryRunSink`]
//! replaces [`CqlSink`] and nothing else in the job changes. A `if !dry_run { … }` scattered
//! through the row loop would leave a dry run testing a different program from the one it is
//! supposed to be rehearsing.
//!
//! The second is that the row loop, the batching, the flush threshold and the counter accounting
//! are the parts most likely to be wrong, and they are testable against a recording double with no
//! cluster at all.
//!
//! # The signatures take ownership, and that is what makes zero-copy work
//!
//! A write borrows the response frame it came from (`MIG-040`). If the sink took `&IdempotentWrite`
//! the returned future would borrow the *caller's stack slot*, and the write could not be handed to
//! an in-flight set that outlives the statement that created it. Taking the write by value moves
//! the borrow into the future, so the only thing that has to stay alive is the page — which is
//! exactly the memory bound the page loop already provides.

use cdm_core::CdmError;
use cdm_cql::exec::{BatchTemplate, TargetWriter};
use cdm_cql::statement::{CounterWrite, IdempotentWrite};
use futures::future::BoxFuture;
use futures::FutureExt as _;

/// Where the migrate job sends the statements it has bound.
pub trait WriteSink: Send + Sync {
    /// Executes one write, retrying it as `CON-011` allows.
    fn write<'w>(&'w self, write: IdempotentWrite<'w>) -> BoxFuture<'w, Result<(), CdmError>>;

    /// Executes one `UNLOGGED` batch (`MIG-020`).
    fn write_batch<'w>(
        &'w self,
        writes: Vec<IdempotentWrite<'w>>,
    ) -> BoxFuture<'w, Result<(), CdmError>>;

    /// Executes one counter update, exactly once (`MIG-032`, `CON-012`).
    fn write_counter<'w>(&'w self, write: CounterWrite<'w>) -> BoxFuture<'w, Result<(), CdmError>>;
}

/// The sink that writes to a cluster.
#[derive(Debug)]
pub struct CqlSink<'a> {
    writer: TargetWriter<'a>,
    template: &'a BatchTemplate,
}

impl<'a> CqlSink<'a> {
    /// Builds a sink over a target writer and the batch template sized from
    /// `perfops.batch_size`.
    #[must_use]
    pub const fn new(writer: TargetWriter<'a>, template: &'a BatchTemplate) -> Self {
        Self { writer, template }
    }
}

impl WriteSink for CqlSink<'_> {
    fn write<'w>(&'w self, write: IdempotentWrite<'w>) -> BoxFuture<'w, Result<(), CdmError>> {
        async move { self.writer.write(&write).await }.boxed()
    }

    fn write_batch<'w>(
        &'w self,
        writes: Vec<IdempotentWrite<'w>>,
    ) -> BoxFuture<'w, Result<(), CdmError>> {
        async move { self.writer.write_batch(self.template, &writes).await }.boxed()
    }

    fn write_counter<'w>(&'w self, write: CounterWrite<'w>) -> BoxFuture<'w, Result<(), CdmError>> {
        async move { self.writer.write_counter(&write).await }.boxed()
    }
}

/// The sink of `migrate --dry-run`: counts everything, writes nothing (`MIG-041`).
///
/// It deliberately does *not* short-circuit any earlier stage. The origin is read, the filters run,
/// the records are exploded, every value is converted and bound, and a bind failure is still an
/// error — which is the point: the counters a dry run reports are the counters the real run would
/// report, and a schema that cannot be bound is discovered before anything is written rather than
/// after the first hour.
#[derive(Debug, Clone, Copy, Default)]
pub struct DryRunSink;

impl WriteSink for DryRunSink {
    fn write<'w>(&'w self, _write: IdempotentWrite<'w>) -> BoxFuture<'w, Result<(), CdmError>> {
        std::future::ready(Ok(())).boxed()
    }

    fn write_batch<'w>(
        &'w self,
        _writes: Vec<IdempotentWrite<'w>>,
    ) -> BoxFuture<'w, Result<(), CdmError>> {
        std::future::ready(Ok(())).boxed()
    }

    fn write_counter<'w>(
        &'w self,
        _write: CounterWrite<'w>,
    ) -> BoxFuture<'w, Result<(), CdmError>> {
        std::future::ready(Ok(())).boxed()
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
pub(crate) mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use cdm_core::ErrorKind;
    use parking_lot::Mutex;

    use super::*;

    /// A sink that records what it was asked to write, for the buffer and job tests.
    ///
    /// It records the *sizes* of the batches rather than their values: `SEC-002` applies to test
    /// helpers too, and every property these tests assert — how many writes, how they were
    /// grouped, when the flush happened — is a property of the shape.
    #[derive(Debug, Default)]
    pub(crate) struct RecordingSink {
        pub singles: AtomicUsize,
        pub counters: AtomicUsize,
        pub batches: Mutex<Vec<usize>>,
        pub fail_after: Option<usize>,
        issued: AtomicUsize,
    }

    impl RecordingSink {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// A sink that fails once `after` rows have been accepted, for the range-failure tests.
        pub(crate) fn failing_after(after: usize) -> Self {
            Self {
                fail_after: Some(after),
                ..Self::default()
            }
        }

        /// Every row the sink was asked to write, however it was grouped.
        pub(crate) fn rows(&self) -> usize {
            self.singles.load(Ordering::Relaxed)
                + self.counters.load(Ordering::Relaxed)
                + self.batches.lock().iter().sum::<usize>()
        }

        fn accept(&self, rows: usize) -> Result<(), CdmError> {
            let before = self.issued.fetch_add(rows, Ordering::Relaxed);
            match self.fail_after {
                Some(limit) if before + rows > limit => Err(CdmError::new(
                    ErrorKind::Write,
                    "the recording sink refused",
                )),
                _ => Ok(()),
            }
        }
    }

    impl WriteSink for RecordingSink {
        fn write<'w>(&'w self, _write: IdempotentWrite<'w>) -> BoxFuture<'w, Result<(), CdmError>> {
            let result = self.accept(1).inspect(|()| {
                self.singles.fetch_add(1, Ordering::Relaxed);
            });
            std::future::ready(result).boxed()
        }

        fn write_batch<'w>(
            &'w self,
            writes: Vec<IdempotentWrite<'w>>,
        ) -> BoxFuture<'w, Result<(), CdmError>> {
            let result = self.accept(writes.len()).inspect(|()| {
                self.batches.lock().push(writes.len());
            });
            std::future::ready(result).boxed()
        }

        fn write_counter<'w>(
            &'w self,
            _write: CounterWrite<'w>,
        ) -> BoxFuture<'w, Result<(), CdmError>> {
            let result = self.accept(1).inspect(|()| {
                self.counters.fetch_add(1, Ordering::Relaxed);
            });
            std::future::ready(result).boxed()
        }
    }

    #[tokio::test]
    async fn mig_041_the_dry_run_sink_accepts_everything_and_writes_nothing() {
        // There is nothing to observe on the target, which is the assertion: the sink has no
        // session, no statement and no way to reach a cluster, so a `--dry-run` cannot write by
        // accident even if the row loop asks it to.
        let sink = DryRunSink;
        assert!(sink.write_batch(Vec::new()).await.is_ok());
    }
}
