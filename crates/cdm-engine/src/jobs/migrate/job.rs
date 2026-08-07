//! The migrate row loop (`MIG-001`..`MIG-005`, `MIG-041`, `SCH-009`).
//!
//! # The loop, in the order `MIG-001` gives it
//!
//! ```text
//!   page ──► row ──► READ++ ──► filters ──► explode ──► bind ──► write ──► UNFLUSHED++
//!                       │           │           │          │                    │
//!                       │        reject     no entries  no statement        threshold?
//!                       │           ▼           ▼          ▼                    ▼
//!                       │       SKIPPED++   SKIPPED++  SKIPPED++          flush: WRITE += n
//!                       └──► record failure ──► ERROR++, carry on
//! ```
//!
//! # Two kinds of failure, and never one mistaken for the other
//!
//! `ARCHITECTURE.md` §13 has three levels of isolation, and only the innermost belongs to this
//! file: a bad *row* costs one `ERROR` and the loop continues, a bad *range* is the `Err` this
//! function returns, and the scheduler does the rest (`ENG-008`, `ENG-009`).
//!
//! Telling the two apart cannot be done by [`ErrorKind`] alone, because a bind failure and a
//! target timeout are both `Write`. So the private `RowError` says which it is at the point where
//! it is known, and the row loop matches on that rather than guessing from a kind. Guessing is
//! not a theoretical risk: a target that is refusing every write would otherwise look like a run
//! with a great many bad rows, which is exactly the shape of report that gets a migration signed
//! off.
//!
//! # Passthrough is preserved, and proved
//!
//! On the fast path there is no owned row between
//! [`Page::rows`](cdm_cql::exec::Page::rows) and [`Binder::bind`](cdm_cql::statement::Binder::bind):
//! every bound value is a borrow of the response frame (`MIG-040`, `MIG-041`).
//! `mig_041_the_bound_write_is_the_read_frames_own_bytes` asserts it by pointer identity, so the
//! fast path cannot be lost to a refactor without a test going red.
//!
//! A run that switches on a filter, the TTL/writetime feature, the explode map or extract-JSON
//! *does* pay one materialisation per row: those features are defined over owned types.
//! [`MigratePlan::needs_record`] decides that once, so the loop is a branch on a `bool`.
//!
//! # Counter rows take a different path, deliberately
//!
//! `MIG-031` needs the target's current value before it can know what to add, so a counter row is:
//! rate-limited target lookup, delta, bind, write, await. Not pipelined, not batched, not retried
//! (`MIG-032`, `CON-012`). See [`super::counter`] for why each of those is load-bearing.

use std::sync::Arc;

use async_trait::async_trait;
use cdm_core::{CdmError, ErrorKind, JobKind, PrimaryKey, RawCell, Record, Row, Side};
use cdm_cql::exec::Page;
use cdm_cql::statement::{BindInputs, Bound, SourceRow};
use cdm_feature::ExplodedEntry;

use crate::scheduler::{RangeContext, RangeProcessor, RangeVerdict};

use super::buffer::WriteBuffer;
use super::counter::CounterDeltas;
use super::counters::MigrateCounters;
use super::plan::{MigrateFeatures, MigratePlan};
use super::sink::{CqlSink, DryRunSink, WriteSink};

/// Which level of isolation a failure belongs to (`ARCHITECTURE.md` §13).
#[derive(Debug)]
enum RowError {
    /// This row could not be migrated. `ERROR` is incremented and the range carries on.
    Record(CdmError),
    /// The range cannot continue: the cluster, the network or the schema has failed.
    Range(CdmError),
}

impl RowError {
    /// The underlying error, whichever level it belongs to.
    fn inner(&self) -> &CdmError {
        match self {
            Self::Record(error) | Self::Range(error) => error,
        }
    }
}

/// The migrate job (`MIG-001`).
#[derive(Debug)]
pub struct MigrateJob {
    plan: Arc<MigratePlan>,
}

impl MigrateJob {
    /// Builds the job from a resolved plan.
    ///
    /// The plan is shared by every worker: it is immutable and it holds the prepared statements,
    /// so building one per range would re-prepare `perfops.num_parts` times.
    #[must_use]
    pub const fn new(plan: Arc<MigratePlan>) -> Self {
        Self { plan }
    }

    /// The plan this job executes.
    #[must_use]
    pub fn plan(&self) -> &Arc<MigratePlan> {
        &self.plan
    }
}

#[async_trait]
impl RangeProcessor for MigrateJob {
    fn job(&self) -> JobKind {
        JobKind::Migrate
    }

    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
        // SCH-009: checked before the range's first read, so a schema change is caught at the
        // granularity everything else in cdm-rs is accounted at (`P5`) rather than at the end of
        // the run, by which point the damage would be done and unattributable.
        self.plan.executor().check_schema().await?;

        let counters = MigrateCounters::resolve(ctx.counters())?;
        let cql_sink = CqlSink::new(
            self.plan.executor().writer(),
            self.plan.executor().batch_template(),
        );
        let dry_sink = DryRunSink;
        // MIG-041: one object swapped, and nothing else in the loop differs, so a dry run
        // rehearses the program a real run executes rather than a similar one.
        let sink: &dyn WriteSink = if self.plan.settings().is_dry_run() {
            &dry_sink
        } else {
            &cql_sink
        };

        let mut scan = self.plan.executor().scan(ctx.range());
        loop {
            // ENG-010: a stopping run winds down at a page boundary, which is the largest unit
            // that can be abandoned without leaving a flush half-done.
            if ctx.is_cancelled() {
                return Err(CdmError::new(
                    ErrorKind::Cancelled,
                    "the range was abandoned before its next page: the run is shutting down \
                     (ENG-010)",
                )
                .with_context(|c| c.with_range(ctx.range())));
            }

            let page = {
                // ENG-007: the in-flight slot covers the request, not the processing of its rows.
                let _slot = ctx.read_slot().await?;
                scan.next_page().await?
            };
            let Some(page) = page else { break };
            if page.is_empty() {
                continue;
            }
            self.process_page(ctx, sink, counters, &page).await?;
        }
        Ok(RangeVerdict::Pass)
    }
}

impl MigrateJob {
    /// One page: bind and write every row, then flush what is left (`MIG-004`, `MIG-005`).
    async fn process_page<'page>(
        &self,
        ctx: &'page RangeContext,
        sink: &'page (dyn WriteSink + 'page),
        counters: MigrateCounters,
        page: &'page Page,
    ) -> Result<(), CdmError> {
        let mut buffer = WriteBuffer::new(
            sink,
            ctx,
            counters,
            self.plan.settings(),
            self.plan.partition_positions(),
        );

        for row in page.rows()? {
            let row = row?;
            // MIG-001: one origin permit per row, exactly where Java acquires it.
            ctx.acquire_read_rows(1).await;
            ctx.counters().increment(counters.read());

            match self.process_row(ctx, counters, &mut buffer, &row).await {
                Ok(()) => {}
                Err(RowError::Range(error)) => return Err(error),
                Err(failure @ RowError::Record(_)) => {
                    ctx.counters().increment(counters.error());
                    tracing::error!(
                        target: "cdm::engine::migrate",
                        error = %failure.inner(),
                        "a record could not be migrated; the range continues (ERR-005)"
                    );
                }
            }
        }

        // MIG-004: the final flush of the page. `flush_threshold <= fetch_size`, so a full page
        // has already flushed at least once; this covers the remainder.
        buffer.flush().await
    }

    /// One row: filter, explode, bind, write (`MIG-001`..`MIG-003`).
    ///
    /// Two shapes, chosen once at startup by [`MigratePlan::needs_record`]:
    ///
    /// * **no features** — the row goes straight from the response frame into the bind, and the
    ///   bound values *are* the frame's bytes (`MIG-040`, `MIG-041`);
    /// * **features on** — the row is materialised once, the features run against the owned form,
    ///   and the resulting write owns its values because it borrows a per-row buffer that the
    ///   page's write buffer would otherwise outlive.
    ///
    /// The split is here rather than inside the bind because it is the *only* difference between
    /// the two, and burying it would make it impossible to prove the fast path is still taken.
    async fn process_row<'page, R>(
        &self,
        ctx: &RangeContext,
        counters: MigrateCounters,
        buffer: &mut WriteBuffer<'page, dyn WriteSink + 'page>,
        row: &R,
    ) -> Result<(), RowError>
    where
        R: SourceRow<'page>,
    {
        if !self.plan.needs_record() {
            // MIG-040: nothing between the frame and the bind.
            return self
                .write_borrowed(buffer, row, BindInputs::default())
                .await;
        }

        let record = materialise(
            row,
            self.plan.projection_width(),
            self.plan.origin_key_indices(),
        );
        let inputs = match classify(self.plan.features(), &record, ctx.counters(), counters)
            .map_err(RowError::Record)?
        {
            RowDecision::Skipped => return Ok(()),
            RowDecision::Write(inputs) => inputs,
        };

        let base = BindInputs {
            ttl: inputs.ttl,
            writetime: inputs.writetime,
            extracted_json: inputs
                .extracted
                .as_ref()
                .and_then(|cell| cell.bytes())
                .map(|b| &**b),
            key: Some(record.key()),
            ..BindInputs::default()
        };

        match &inputs.entries {
            Some(entries) => {
                for entry in entries {
                    let bound = BindInputs {
                        explode_key: entry.key.bytes().map(|bytes| &**bytes),
                        explode_value: entry.value.bytes().map(|bytes| &**bytes),
                        ..base
                    };
                    self.write_materialised(buffer, &record, bound).await?;
                }
                Ok(())
            }
            None => self.write_materialised(buffer, &record, base).await,
        }
    }

    /// The fast path: bind straight off the response frame and hand the borrow to the buffer.
    async fn write_borrowed<'page, R>(
        &self,
        buffer: &mut WriteBuffer<'page, dyn WriteSink + 'page>,
        row: &R,
        inputs: BindInputs<'page>,
    ) -> Result<(), RowError>
    where
        R: SourceRow<'page>,
    {
        if self.plan.is_counter_run() {
            return self.write_counter_row(buffer, row, inputs).await;
        }
        let bound = self.plan.binder().bind(row, inputs).map_err(|failure| {
            failure.log();
            RowError::Record(CdmError::from(failure))
        })?;
        match bound {
            Bound::Idempotent(write) => buffer.push(write).await.map_err(RowError::Range),
            // Unreachable: `is_counter_run` is false, so the statement is an `INSERT`. Returning
            // an error rather than asserting keeps `ERR-004` true of this file.
            Bound::Counter(_) => Err(RowError::Range(counter_shape_mismatch())),
        }
    }

    /// The feature path: bind from the materialised row and own the result.
    ///
    /// The bound values here borrow the exploded entry and the extracted property, both of which
    /// live for this row only, while the write buffer lives for the page. `into_owned` is what
    /// reconciles the two, and it is called *only* here — the fast path above must never reach it,
    /// which is what `mig_041_the_bound_write_is_the_read_frames_own_bytes` checks.
    async fn write_materialised<'page>(
        &self,
        buffer: &mut WriteBuffer<'page, dyn WriteSink + 'page>,
        record: &Record,
        inputs: BindInputs<'_>,
    ) -> Result<(), RowError> {
        let source: &Row = record.origin();
        if self.plan.is_counter_run() {
            return self.write_counter_row(buffer, &source, inputs).await;
        }
        let bound = self
            .plan
            .binder()
            .bind(&source, inputs)
            .map_err(|failure| {
                failure.log();
                RowError::Record(CdmError::from(failure))
            })?;
        match bound {
            Bound::Idempotent(write) => buffer
                .push(write.into_owned())
                .await
                .map_err(RowError::Range),
            Bound::Counter(_) => Err(RowError::Range(counter_shape_mismatch())),
        }
    }

    /// The counter path: read, subtract, bind, write, once (`MIG-030`..`MIG-032`).
    ///
    /// This is the one place the zero-copy path of `MIG-040` is deliberately given up. The delta
    /// for a counter column is computed per row and has to outlive the bind that carries it, and
    /// the response frame outlives everything — so making the two agree would mean keeping a
    /// per-row buffer alive for the whole page. Since a counter row already costs a full target
    /// round trip before it can be written at all, one row-sized copy alongside it is not the
    /// expensive part, and this way the borrow rules and the at-most-once rule want the same
    /// thing: the write completes before anything it borrows goes away.
    async fn write_counter_row<'page, 'r, R>(
        &self,
        buffer: &mut WriteBuffer<'page, dyn WriteSink + 'page>,
        row: &R,
        inputs: BindInputs<'r>,
    ) -> Result<(), RowError>
    where
        R: SourceRow<'r>,
    {
        let writer = self.plan.executor().writer();
        let owned = materialise_row(row, self.plan.projection_width());
        let source: &Row = &owned;

        let key = self
            .plan
            .binder()
            .bind_key(self.plan.key_binding(), &source, inputs)
            .map_err(|failure| {
                failure.log();
                RowError::Record(CdmError::from(failure))
            })?;

        // MIG-031: the lookup is target load, so it takes a target permit, exactly as Java's
        // `rateLimiterTarget.acquire(1)` does before `targetSelectByPKStatement.getRecord`.
        buffer.acquire_counter_permit().await;
        let current = writer.counter_row(&key).await.map_err(RowError::Range)?;

        let deltas = self.counter_deltas(&source, current.as_ref())?;
        let delta_row = CounterDeltas::new(&source, &deltas);
        let bound = self
            .plan
            .binder()
            .bind(&delta_row, inputs)
            .map_err(|failure| {
                failure.log();
                RowError::Record(CdmError::from(failure))
            })?;
        let Bound::Counter(write) = bound else {
            return Err(RowError::Range(counter_shape_mismatch()));
        };

        buffer.acquire_counter_permit().await;
        // MIG-032, CON-012: one attempt. A failure fails the range rather than being retried.
        // The sink reference is taken out of the buffer first: holding a shared borrow of the
        // buffer across an `await` would require it to be `Sync`, which its in-flight set is not.
        let sink = buffer.sink();
        sink.write_counter(write).await.map_err(RowError::Range)?;
        buffer.push_counter_written().await.map_err(RowError::Range)
    }

    /// `origin - current` for every counter column (`MIG-031`).
    fn counter_deltas<'r, R>(
        &self,
        row: &R,
        current: Option<&cdm_cql::exec::CounterRow>,
    ) -> Result<Vec<(usize, [u8; 8])>, RowError>
    where
        R: SourceRow<'r>,
    {
        let plan = self.plan.counter_plan();
        match current {
            Some(rows) => {
                let target = rows.row().map_err(RowError::Range)?;
                match target {
                    Some(target) => plan.deltas(row, Some(&target)),
                    None => plan.deltas::<_, &Row>(row, None),
                }
            }
            None => plan.deltas::<_, &Row>(row, None),
        }
        .map_err(RowError::Record)
    }
}

/// The per-row values the enabled features produce.
struct RowInputs {
    ttl: Option<i32>,
    writetime: Option<i64>,
    extracted: Option<RawCell>,
    entries: Option<Vec<ExplodedEntry>>,
}

/// What the enabled features say happens to one row (`MIG-002`, `MIG-003`).
enum RowDecision {
    /// Nothing is written, and `SKIPPED` has already been incremented.
    Skipped,
    /// The row is written, with these inputs.
    Write(RowInputs),
}

/// Runs the filters and the row-shaping features, counting `SKIPPED` where they say to
/// (`MIG-002`, `MIG-003`).
///
/// A free function rather than a method so that the two `SKIPPED` rules — the one a filter
/// triggers and the one an empty exploded map triggers — are testable without a cluster. They are
/// the two places a migration can silently *not* write a row, which makes them exactly the two
/// worth testing directly.
///
/// # Errors
///
/// Whatever a filter or a feature reports, which the caller counts as a record-level `ERROR`.
fn classify(
    features: &MigrateFeatures,
    record: &Record,
    registry: &cdm_metrics::JobCounters,
    counters: MigrateCounters,
) -> Result<RowDecision, CdmError> {
    // MIG-002: a rejected row is SKIPPED and is not written.
    if !features.filters.accepts(record)? {
        registry.increment(counters.skipped());
        return Ok(RowDecision::Skipped);
    }

    let ttl = features.writetime.ttl(record.origin())?;
    let writetime = features.writetime.writetime(record.origin())?;
    let extracted = match &features.extract_json {
        Some(plan) => plan.extract_record(record)?,
        None => None,
    };
    let entries = match &features.explode {
        Some(plan) => Some(plan.explode_record(record)?),
        None => None,
    };

    // FEA-020, MIG-003: a null or empty map produces no target row at all, so there is no
    // statement to execute and the record is SKIPPED rather than counted as written.
    if entries.as_ref().is_some_and(Vec::is_empty) {
        registry.increment(counters.skipped());
        return Ok(RowDecision::Skipped);
    }

    Ok(RowDecision::Write(RowInputs {
        ttl,
        writetime,
        extracted,
        entries,
    }))
}

/// Copies a row into the owned form the features are defined over, and derives its primary key
/// (`MIG-001`).
///
/// This is the one allocation the fast path does not do, and it exists because
/// [`FilterChain::accepts`](cdm_feature::FilterChain::accepts),
/// [`WritetimeTtlPlan::writetime`](cdm_feature::WritetimeTtlPlan::writetime),
/// [`ExplodePlan::explode_record`](cdm_feature::ExplodePlan::explode_record) and
/// [`ExtractJsonPlan::extract_record`](cdm_feature::ExtractJsonPlan::extract_record) all take
/// owned types. A run with none of them switched on never calls this.
///
/// The row is padded to the projection's width first. That matters: a row narrower than the
/// projection would otherwise shift every later column's index, which is the class of bug that
/// writes the right data into the wrong column.
fn materialise<'r, R>(row: &R, projection_width: usize, key_indices: &[usize]) -> Record
where
    R: SourceRow<'r>,
{
    let cells = materialise_row(row, projection_width);
    let key = PrimaryKey::new(
        key_indices
            .iter()
            .map(|&index| cells.get(index).cloned().unwrap_or(RawCell::NULL))
            .collect(),
    );
    Record::new(key, cells)
}

/// Copies a row into an owned [`Row`], padded to the projection's width.
fn materialise_row<'r, R>(row: &R, projection_width: usize) -> Row
where
    R: SourceRow<'r>,
{
    let width = projection_width.max(row.width());
    Row::new(
        (0..width)
            .map(|index| match row.cell(index) {
                Some(Some(bytes)) => RawCell::new(bytes.to_vec()),
                _ => RawCell::NULL,
            })
            .collect(),
    )
}

fn counter_shape_mismatch() -> CdmError {
    CdmError::new(
        ErrorKind::Internal,
        "the binder produced a write of the wrong shape for this table: the statement and the \
         counter plan disagree about whether the target is a counter table (SCH-005)",
    )
    .with_context(|c| c.with_side(Side::Target))
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

    use cdm_core::{EffectiveConfig, JobKind, TableRef};
    use cdm_feature::{table_view, ColumnValueFilter, ExplodeMap, FilterChain, TableFacts};
    use cdm_metrics::{CounterKind, CounterView, JobCounters};

    use crate::jobs::migrate::testfixtures::planner;

    use super::*;

    fn registry() -> (JobCounters, MigrateCounters) {
        let registry = JobCounters::new(JobKind::Migrate);
        let counters = MigrateCounters::resolve(&registry).unwrap();
        (registry, counters)
    }

    fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
        pairs.iter().copied().collect()
    }

    fn plain_record(status: &str) -> Record {
        Record::new(
            PrimaryKey::new(vec![RawCell::new(1i32.to_be_bytes().to_vec())]),
            Row::new(vec![
                RawCell::new(1i32.to_be_bytes().to_vec()),
                RawCell::new(status.as_bytes().to_vec()),
            ]),
        )
    }

    #[test]
    fn mig_001_the_target_primary_key_is_built_from_the_projection() {
        let row = Row::new(vec![
            RawCell::new(7i32.to_be_bytes().to_vec()),
            RawCell::new(b"cc".to_vec()),
            RawCell::new(b"payload".to_vec()),
        ]);
        let source: &Row = &row;
        let record = materialise(&source, 3, &[0, 1]);

        assert_eq!(record.key().len(), 2);
        assert_eq!(
            record.key().values()[0].bytes().map(|b| b.to_vec()),
            Some(7i32.to_be_bytes().to_vec())
        );
        assert_eq!(record.origin().len(), 3);
    }

    #[test]
    fn mig_001_a_row_narrower_than_the_projection_is_padded_rather_than_shifted() {
        // A short row must not slide later columns down an index: that writes the right data into
        // the wrong column, and nothing downstream would notice.
        let row = Row::new(vec![RawCell::new(1i32.to_be_bytes().to_vec())]);
        let source: &Row = &row;
        let record = materialise(&source, 4, &[0]);
        assert_eq!(record.origin().len(), 4);
        assert!(record.origin().get(3).unwrap().is_null());
    }

    #[test]
    fn mig_002_a_row_a_filter_rejects_is_skipped_and_never_written() {
        let origin = TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "src"),
                &[("id", "int"), ("status", "text")],
            ),
            &["id"],
        )
        .unwrap();
        let filter = ColumnValueFilter::load(
            &config(&[
                ("filter.column.name", "status"),
                ("filter.column.value", "drop"),
            ]),
            &origin,
        );
        assert!(filter.is_enabled());
        let features = MigrateFeatures {
            filters: FilterChain::new().with(Arc::new(filter)),
            ..MigrateFeatures::default()
        };

        let (registry, counters) = registry();
        let decision = classify(&features, &plain_record("drop"), &registry, counters).unwrap();
        assert!(matches!(decision, RowDecision::Skipped));
        assert_eq!(
            registry.count_of(CounterKind::Skipped, CounterView::Interim),
            1
        );
        assert_eq!(
            registry.count_of(CounterKind::Write, CounterView::Interim),
            0,
            "MIG-002: a rejected row must not be written"
        );

        // A row the filter accepts is not skipped.
        let decision = classify(&features, &plain_record("keep"), &registry, counters).unwrap();
        assert!(matches!(decision, RowDecision::Write(_)));
        assert_eq!(
            registry.count_of(CounterKind::Skipped, CounterView::Interim),
            1
        );
    }

    #[test]
    fn mig_003_a_record_that_produces_no_statement_is_skipped() {
        // An explode map with no entries yields no target row at all, so there is no statement to
        // bind and the record is SKIPPED rather than counted as an error or as written.
        let schema = cdm_feature::FeatureSchema::new(
            TableFacts::from_view(
                &table_view(
                    TableRef::new("ks", "src"),
                    &[("id", "int"), ("m", "map<text, int>")],
                ),
                &["id"],
            )
            .unwrap(),
            TableFacts::from_view(
                &table_view(
                    TableRef::new("ks", "dst"),
                    &[("id", "int"), ("k", "text"), ("v", "int")],
                ),
                &["id", "k"],
            )
            .unwrap(),
        );
        let explode = ExplodeMap::load(&config(&[
            ("feature.explode_map.origin_column", "m"),
            ("feature.explode_map.target_key_column", "k"),
            ("feature.explode_map.target_value_column", "v"),
        ]))
        .resolve(&schema, &planner())
        .unwrap();
        let features = MigrateFeatures {
            explode: Some(explode),
            ..MigrateFeatures::default()
        };

        let (registry, counters) = registry();
        let empty = Record::new(
            PrimaryKey::new(vec![RawCell::new(1i32.to_be_bytes().to_vec())]),
            Row::new(vec![
                RawCell::new(1i32.to_be_bytes().to_vec()),
                RawCell::NULL,
            ]),
        );
        assert!(matches!(
            classify(&features, &empty, &registry, counters).unwrap(),
            RowDecision::Skipped
        ));
        assert_eq!(
            registry.count_of(CounterKind::Skipped, CounterView::Interim),
            1
        );

        // A populated map produces one record per entry instead.
        let mut serialised = 2i32.to_be_bytes().to_vec();
        for (key, value) in [("a", 1i32), ("b", 2)] {
            serialised.extend_from_slice(&i32::try_from(key.len()).unwrap().to_be_bytes());
            serialised.extend_from_slice(key.as_bytes());
            serialised.extend_from_slice(&4i32.to_be_bytes());
            serialised.extend_from_slice(&value.to_be_bytes());
        }
        let populated = Record::new(
            PrimaryKey::new(vec![RawCell::new(1i32.to_be_bytes().to_vec())]),
            Row::new(vec![
                RawCell::new(1i32.to_be_bytes().to_vec()),
                RawCell::new(serialised),
            ]),
        );
        let RowDecision::Write(inputs) =
            classify(&features, &populated, &registry, counters).unwrap()
        else {
            panic!("a populated map produces target rows")
        };
        assert_eq!(inputs.entries.as_ref().map(Vec::len), Some(2));
        assert_eq!(
            registry.count_of(CounterKind::Skipped, CounterView::Interim),
            1,
            "only the empty map was skipped"
        );
    }

    #[test]
    fn mig_032_a_counter_write_is_never_batched_and_never_retried() {
        use crate::jobs::migrate::settings::BatchCoercion;
        use cdm_config::types::BatchGrouping;

        // Never batched: the coercion is unconditional for a counter target.
        let settings = super::super::MigrateSettings::new(
            1_000,
            1_000,
            BatchGrouping::Strict,
            true,
            false,
            false,
        );
        assert_eq!(settings.batch_size(), 1);
        assert!(!settings.is_batching());
        assert_eq!(settings.coercion(), Some(BatchCoercion::CounterTable));

        // Never retried: `TargetWriter::write_counter` has no retry loop, and a source sweep is
        // the check that survives somebody adding one back "for symmetry".
        let write_source = include_str!("../../../../cdm-cql/src/exec/write.rs");
        let production = write_source.split("#[cfg(test)]").next().unwrap();
        let counter_fn = production
            .split("pub async fn write_counter")
            .nth(1)
            .unwrap()
            .split("/// Reads the target row")
            .next()
            .unwrap();
        for forbidden in ["retrying(", "may_retry", "should_retry", "loop {"] {
            assert!(
                !counter_fn.contains(forbidden),
                "write_counter must issue exactly one attempt; found `{forbidden}` (CON-012)"
            );
        }
    }

    #[test]
    fn eng_008_a_record_failure_and_a_range_failure_are_different_values() {
        let record = RowError::Record(CdmError::new(ErrorKind::Write, "a bad bind"));
        let range = RowError::Range(CdmError::new(ErrorKind::Write, "the target is down"));
        // Both are `Write`, which is exactly why the level cannot be inferred from the kind.
        assert_eq!(record.inner().kind(), ErrorKind::Write);
        assert_eq!(range.inner().kind(), ErrorKind::Write);
        assert!(matches!(record, RowError::Record(_)));
        assert!(matches!(range, RowError::Range(_)));
    }

    #[test]
    fn mig_041_the_fast_path_never_copies_the_values_it_binds() {
        // `into_owned` is the one call that can turn a passthrough write into a copy. It belongs
        // in exactly one place — the feature path, which has no frame borrow to keep — and a
        // source sweep is the only check that survives a refactor moving code between methods.
        let source = include_str!("job.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let occurrences = production.matches("into_owned()").count();
        assert_eq!(
            occurrences, 1,
            "into_owned() must appear once, in write_materialised; found {occurrences}"
        );

        let fast_path = production
            .split("async fn write_borrowed")
            .nth(1)
            .unwrap()
            .split("/// The feature path:")
            .next()
            .unwrap();
        assert!(
            !fast_path.contains("into_owned"),
            "the fast path must hand the buffer the frame\'s own bytes (MIG-040, MIG-041)"
        );
    }

    #[test]
    fn sch_005_a_shape_mismatch_is_internal_and_names_the_target() {
        let error = counter_shape_mismatch();
        assert_eq!(error.kind(), ErrorKind::Internal);
        assert_eq!(error.context().side, Some(Side::Target));
    }
}
