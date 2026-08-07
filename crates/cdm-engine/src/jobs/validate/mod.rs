//! The validate job — Java's `DiffJobSession` (`VAL-001`..`VAL-012`, `VAL-016`, `VAL-017`).
//!
//! Validate answers one question per origin row: *does the target agree?* It reads a token range
//! from the origin, derives each row's target primary key, looks that key up on the target, and
//! compares column by column. Every row lands in exactly one of four buckets — `VALID`, `MISSING`,
//! `MISMATCH` or `SKIPPED` — and, when autocorrect is on, a `MISSING` or `MISMATCH` row is repaired
//! and counted again as `CORRECTED_MISSING` or `CORRECTED_MISMATCH`.
//!
//! ```text
//!   origin range scan ──► READ ──► filters ──► SKIPPED
//!         │                            │
//!         │                            ▼  build the target PK (VAL-001)
//!         │                    issue target SELECT, asynchronously
//!         ▼                            │
//!   buffer of fetch_size ──────────────┘
//!         │
//!         ▼  compare the batch
//!    ┌────┴────┬──────────────┬──────────────┐
//!  VALID    MISSING        MISMATCH      (per-column error → part of the mismatch detail)
//!             │                │
//!      autocorrect.missing   autocorrect.mismatch
//!             │                │
//!    CORRECTED_MISSING   CORRECTED_MISMATCH
//! ```
//!
//! # Five things that are easy to get wrong, and are not
//!
//! **Validate never deletes** (`VAL-010`). There is no delete path in this module. A target row the
//! origin does not have is invisible to a validate run — Java has the same blind spot, and it is
//! deliberate: the origin is not necessarily authoritative about *absence*, and a job that removed
//! target rows on that assumption would be a data-loss tool wearing a validation tool's name.
//!
//! **Autocorrect writes are writes.** They go through the same [`Binder`](cdm_cql::statement::Binder)
//! path as migrate, with the same `UNSET`-not-`NULL` rule (`MIG-012`), and a counter correction
//! carries the same hazard every counter write does: a counter row that was deleted and re-inserted
//! double-counts. `VAL-004` therefore requires the separate `autocorrect.missing_counter` opt-in,
//! and `CON-012` forbids retrying a counter write — which this job structurally cannot do, because
//! it issues each correction exactly once and hands a failure straight back to the caller.
//!
//! **A range failure's `ERROR` count is not this module's to compute** (`ENG-008`). Returning `Err`
//! is the whole of the contract; [`scheduler::failure`](crate::scheduler::failure) reads the
//! **interim** counters and increments `ERROR` by `READ − VALID − MISSING − MISMATCH − SKIPPED`.
//! Java computes the same expression inside `DiffJobSession`, from the *committed* counters, on a
//! path where `flush()` has not run — so every term is `0`, and a failed validate range in Java
//! always records `ERROR: 0`. cdm-rs does not reproduce that, and `--compat-java` does not restore
//! it.
//!
//! **Row values are never logged** (`SEC-002`, `VAL-017`). See [`compare`] and [`difflog`].
//!
//! **A corrected row carries the origin's TTL and writetime** (`VAL-018`). It is the origin row
//! that is authoritative about *when* a value was written, not the clock of the coordinator that
//! happens to repair it: a correction stamped with wall-clock time shadows every later origin write
//! whose timestamp is earlier, so the run after this one cannot put it right. This job does not
//! compute the two values itself — `FEA-040`..`FEA-046` resolve them from `TTL(…)`/`WRITETIME(…)`
//! cells of the origin projection, exactly as they do for migrate — but it is the job whose
//! guarantee they belong to, and the three places they are supplied from are outside this module:
//! the validate builder resolves the plan and extends the projection, the target upsert is
//! generated with the `USING` clause the plan implies, and the row sink binds the per-row values
//! into it. `FEA-045` disables all of that for a counter table on either side, which is why a
//! counter correction is unaffected.
//!
//! # Specification
//!
//! - `VAL-001` — [`ValidateJob::process`], the read/filter/fetch/buffer loop
//! - `VAL-002`, `VAL-008` — [`ComparisonPlan::compare`]
//! - `VAL-003`, `VAL-004`, `VAL-007` — autocorrect, inside [`ValidateJob::process`]
//! - `VAL-005`, `VAL-006`, `VAL-009`, `VAL-011` — [`compare`]
//! - `VAL-010` — nothing in this module deletes
//! - `VAL-012`, `VAL-017` — [`difflog`]
//! - `VAL-013` — [`report`]
//! - `VAL-015` — [`ComparisonPlan::with_keys_only`], [`sample_percent`]
//! - `VAL-016` — [`status::verdict`]
//! - `VAL-018` — supplied to [`ValidateJob`]'s sink by the harness; see the note above

pub mod compare;
pub mod difflog;
pub mod report;
pub mod status;

use std::collections::VecDeque;
use std::sync::Arc;

use cdm_config::model::{Autocorrect, CdmConfig};
use cdm_core::{CdmError, ErrorKind, JobKind, PrimaryKey, Record, RowSink, RowSource, TokenRange};
use cdm_feature::FilterChain;
use cdm_metrics::{Counter, CounterKind, CounterView, DiscrepancyKind, EventBus, JobCounters};
use futures::stream::{FuturesOrdered, StreamExt};

use crate::scheduler::{java_thread_label, RangeContext, RangeProcessor, RangeVerdict};

pub use compare::{Comparison, ComparisonPlan, Mismatch, REDACTED};
pub use difflog::{DiffLog, DEFAULT_DIFF_FILE};
pub use report::{
    ColumnRecord, DiscrepancyRecord, DiscrepancyReport, DEFAULT_REPORT_FILE, NULL_VALUE,
    REDACTED_PREFIX,
};

/// Applies `validate --sample <percent>` (`VAL-015`).
///
/// The flag is sugar and nothing else: it sets `filter.token_coverage_percent`, so the sampling a
/// `--sample 5` run performs is `TOK-005`'s, deterministic seeding and all, and there is no second
/// implementation of range shrinking that could disagree with the first. A CLI that wants the flag
/// calls this and then plans as usual.
///
/// # Errors
///
/// [`ErrorKind::Config`] when `percent` is outside 1–100, which is Tier-1's rule for the property
/// itself (`CFG-020`). Rejecting it here as well means the flag cannot smuggle in a value the
/// property would have refused — `--sample 0` in particular, which would plan a run that reads
/// nothing and reports everything it did not look at as fine.
pub fn sample_percent(config: &mut CdmConfig, percent: u8) -> Result<(), CdmError> {
    if !(1..=100).contains(&percent) {
        return Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "`--sample {percent}` is out of range: a sample is a percentage of each token \
                 range, between 1 and 100 (VAL-015, TOK-005)"
            ),
        ));
    }
    config.filter.token_coverage_percent = percent;
    Ok(())
}

/// Everything about a validate run that is not the two clusters (`CFG-140`, `VAL-004`).
#[derive(Debug, Clone)]
pub struct ValidateSettings {
    /// What to repair, from `spark.cdm.autocorrect.*` (`CFG-140`).
    pub autocorrect: Autocorrect,
    /// Whether the target is a counter table, which gates missing-row correction (`VAL-004`).
    ///
    /// Taken from the schema — [`ColumnMapping::target_is_counter`](cdm_cql::statement::ColumnMapping::target_is_counter)
    /// — rather than from configuration, because it is a fact about the table and an operator
    /// cannot be asked to restate it correctly.
    pub target_is_counter: bool,
}

impl ValidateSettings {
    /// Settings that compare and report but repair nothing, which is the default validate run.
    #[must_use]
    pub fn read_only() -> Self {
        Self {
            autocorrect: Autocorrect::default(),
            target_is_counter: false,
        }
    }

    /// Whether this run will write to the target at all.
    ///
    /// `TRK-030` asks the same question when it decides whether a resumed counter range may be
    /// re-processed: a validate run that corrects nothing is a pure reader and is always safe to
    /// re-run, and one that corrects anything is not.
    #[must_use]
    pub const fn writes_to_target(&self) -> bool {
        self.autocorrect.missing || self.autocorrect.mismatch || self.autocorrect.missing_counter
    }
}

/// The counters validate touches, resolved once so the hot path cannot fail (`MET-003`).
#[derive(Debug, Clone, Copy)]
struct ValidateCounters {
    read: Counter,
    valid: Counter,
    missing: Counter,
    mismatch: Counter,
    corrected_missing: Counter,
    corrected_mismatch: Counter,
    skipped: Counter,
}

impl ValidateCounters {
    /// Resolves every counter against a range's registry.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`](cdm_core::ErrorKind::Internal) if a counter is not registered for
    /// [`JobKind::Validate`], which `MET-002` guarantees it is. Resolving here rather than per row
    /// is what makes the increment itself infallible.
    fn resolve(counters: &JobCounters) -> Result<Self, CdmError> {
        Ok(Self {
            read: counters.counter(CounterKind::Read)?,
            valid: counters.counter(CounterKind::Valid)?,
            missing: counters.counter(CounterKind::Missing)?,
            mismatch: counters.counter(CounterKind::Mismatch)?,
            corrected_missing: counters.counter(CounterKind::CorrectedMissing)?,
            corrected_mismatch: counters.counter(CounterKind::CorrectedMismatch)?,
            skipped: counters.counter(CounterKind::Skipped)?,
        })
    }
}

/// The validate job (`VAL-001`).
///
/// One per run, shared by every worker. It owns no mutable state: the origin source, the target
/// sink, the comparison plan and the difference log are all shared immutably, and everything a
/// range accumulates lives in the [`RangeContext`] the scheduler hands to [`RangeProcessor::process`].
pub struct ValidateJob {
    origin: Arc<dyn RowSource>,
    target: Arc<dyn RowSink>,
    plan: Arc<ComparisonPlan>,
    filters: FilterChain,
    settings: ValidateSettings,
    diff_log: Arc<DiffLog>,
    report: Arc<DiscrepancyReport>,
    events: Option<Arc<EventBus>>,
}

impl std::fmt::Debug for ValidateJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidateJob")
            .field("origin", &self.origin.name())
            .field("target", &self.target.name())
            .field("columns", &self.plan.len())
            .field("filters", &self.filters.names())
            .field("settings", &self.settings)
            .field("report", &self.report.format())
            .finish_non_exhaustive()
    }
}

impl ValidateJob {
    /// Assembles the job.
    ///
    /// Everything is resolved before this returns: the comparison plan holds one conversion per
    /// column and the difference log holds an open file handle, so a misconfiguration is a startup
    /// failure rather than something discovered on the first differing row of a six-hour run
    /// (`ARCHITECTURE.md` §5.5).
    #[must_use]
    pub fn new(
        origin: Arc<dyn RowSource>,
        target: Arc<dyn RowSink>,
        plan: Arc<ComparisonPlan>,
        settings: ValidateSettings,
        diff_log: Arc<DiffLog>,
    ) -> Self {
        Self {
            origin,
            target,
            plan,
            filters: FilterChain::new(),
            settings,
            diff_log,
            report: Arc::new(DiscrepancyReport::disabled()),
            events: None,
        }
    }

    /// Installs the machine-readable discrepancy report of `VAL-013`.
    ///
    /// Optional, and off unless `validate.report.format` says otherwise: a report is an export, and
    /// an export happens when somebody asks for it. A job without one holds a disabled report
    /// rather than an `Option`, so the discrepancy path has one shape either way.
    #[must_use]
    pub fn with_report(mut self, report: Arc<DiscrepancyReport>) -> Self {
        self.report = report;
        self
    }

    /// Publishes each finding to the run event bus (`MET-030`).
    ///
    /// The bus applies its own redaction to the key when the event is constructed, and an event
    /// never carries a value in either mode; the report is where values live, under its own switch.
    /// The two are populated from the same comparison, so a discrepancy that appears in one appears
    /// in the other.
    #[must_use]
    pub fn with_events(mut self, events: Arc<EventBus>) -> Self {
        self.events = Some(events);
        self
    }

    /// The discrepancy report this run writes (`VAL-013`).
    ///
    /// The caller needs it back in order to [`finish`](DiscrepancyReport::finish) it and to attach
    /// its [`reference`](DiscrepancyReport::reference) to the run summary of `MET-033`.
    #[must_use]
    pub fn report(&self) -> &Arc<DiscrepancyReport> {
        &self.report
    }

    /// Installs the row-level filters of `FEA-050`..`FEA-054`.
    ///
    /// A rejected record is `SKIPPED`, never an error, and — unlike in migrate, where the saving is
    /// a write — it costs the target nothing at all, because `VAL-001` applies the filters before
    /// the target lookup is issued.
    #[must_use]
    pub fn with_filters(mut self, filters: FilterChain) -> Self {
        self.filters = filters;
        self
    }

    /// The settings this run was built with.
    #[must_use]
    pub const fn settings(&self) -> &ValidateSettings {
        &self.settings
    }

    /// The difference log this run writes to (`VAL-012`).
    #[must_use]
    pub fn diff_log(&self) -> &Arc<DiffLog> {
        &self.diff_log
    }

    /// Compares one buffered batch, applying `VAL-002`..`VAL-009` to each record.
    ///
    /// The target lookups were issued as each record was buffered and are awaited here, in the
    /// order they were issued, so the batch's latency is one round trip rather than one per row.
    async fn compare_batch(
        &self,
        label: &str,
        range: TokenRange,
        counters: &JobCounters,
        handles: &ValidateCounters,
        batch: &mut Batch,
    ) -> Result<bool, CdmError> {
        let mut had_discrepancy = false;
        while let Some(fetched) = batch.inflight.next().await {
            let Some(record) = batch.records.pop_front() else {
                // The two queues are pushed to together and popped together; a divergence is a bug
                // in this function, and `ERR-004` forbids expressing it as a panic.
                return Err(CdmError::new(
                    cdm_core::ErrorKind::Internal,
                    "the validate batch lost a record between issuing its target lookup and \
                     comparing the result",
                ));
            };
            // ENG-013: a lookup task that panicked is a range failure like any other. It cannot
            // simply be dropped: the record it belonged to would then be neither validated nor
            // counted, and `ENG-008` would report it as lost without saying why.
            let target = fetched.map_err(|join| {
                CdmError::new(
                    cdm_core::ErrorKind::Internal,
                    format!("a target lookup task ended abnormally: {join}"),
                )
            })??;
            let comparison = self
                .plan
                .compare(&record, target.as_ref().map(Record::origin));
            had_discrepancy |= comparison.is_discrepancy();
            match comparison {
                // VAL-008.
                Comparison::Valid => counters.increment(handles.valid),
                // VAL-002.
                Comparison::Missing => {
                    counters.increment(handles.missing);
                    self.diff_log.missing(label, record.key());
                    // The report and the event carry the *outcome*, so a row that was repaired
                    // says so in one record rather than appearing twice with a correction implied
                    // by the second. That is why the correction runs first.
                    let corrected = self
                        .autocorrect_missing(label, counters, handles, &record)
                        .await?;
                    self.report.missing(range, record.key(), corrected);
                    self.publish(range, record.key(), missing_kind(corrected), Vec::new());
                }
                // VAL-006.
                Comparison::Mismatch(mismatch) => {
                    counters.increment(handles.mismatch);
                    self.diff_log
                        .mismatch(label, record.key(), &mismatch.detail());
                    let key = record.key().clone();
                    let corrected = self
                        .autocorrect_mismatch(label, counters, handles, record, target)
                        .await?;
                    self.report.mismatch(range, &key, &mismatch, corrected);
                    self.publish(range, &key, mismatch_kind(corrected), mismatch.columns());
                }
            }
        }
        batch.records.clear();
        Ok(had_discrepancy)
    }

    /// `VAL-003` and `VAL-004`: repair a missing row, unless it is a counter row nobody opted in to.
    ///
    /// # Why the counter guard is not a warning
    ///
    /// Re-inserting a counter row that was deleted does not restore it — it *adds* the origin's
    /// value to whatever the target's tombstoned counter shard still resolves to, which is how a
    /// counter comes back doubled. Cassandra offers no way to set a counter, only to add to it, so
    /// there is no correct repair available; the honest options are "do it anyway, knowing" and
    /// "do not". `autocorrect.missing_counter` is that choice, made explicitly, and Tier-2
    /// validation already warns about it at startup (`CFG-040`).
    /// Returns whether the row was repaired, which is what the report and the event record.
    async fn autocorrect_missing(
        &self,
        label: &str,
        counters: &JobCounters,
        handles: &ValidateCounters,
        record: &Record,
    ) -> Result<bool, CdmError> {
        if !self.settings.autocorrect.missing {
            return Ok(false);
        }
        // VAL-004: counted as MISSING, logged, and left alone. Not counted as CORRECTED_MISSING,
        // which is what makes `VAL-016` report the range `DIFF` rather than `DIFF_CORRECTED`.
        if self.settings.target_is_counter && !self.settings.autocorrect.missing_counter {
            self.diff_log
                .counter_correction_skipped(label, record.key());
            return Ok(false);
        }
        // CON-012: issued once. A failure fails the range rather than being retried, because a
        // counter update that may or may not have landed must not be sent twice.
        self.target.write(record).await?;
        self.target.flush().await?;
        counters.increment(handles.corrected_missing);
        self.diff_log.inserted_missing(label, record.key());
        Ok(true)
    }

    /// `VAL-007`: rewrite a mismatched row.
    ///
    /// The fetched target row is attached to the record before the write, because a counter upsert
    /// binds `origin − current_target` (`MIG-031`) and would otherwise double the delta. There is
    /// no counter guard here, matching Java: correcting a counter *mismatch* converges the counter
    /// on the origin's value rather than re-adding it, so it is safe in the way a re-insert is not.
    /// Returns whether the row was rewritten, which is what the report and the event record.
    async fn autocorrect_mismatch(
        &self,
        label: &str,
        counters: &JobCounters,
        handles: &ValidateCounters,
        record: Record,
        target: Option<Record>,
    ) -> Result<bool, CdmError> {
        if !self.settings.autocorrect.mismatch {
            return Ok(false);
        }
        let key = record.key().clone();
        let record = match target {
            Some(target) => record.with_target(target.origin().clone()),
            None => record,
        };
        self.target.write(&record).await?;
        self.target.flush().await?;
        counters.increment(handles.corrected_mismatch);
        self.diff_log.corrected_mismatch(label, &key);
        Ok(true)
    }

    /// Publishes one finding to the event bus, when a run has one (`MET-030`).
    ///
    /// The key is handed over in the clear and redacted by [`EventBus::discrepancy`] before the
    /// event exists; the column *names* travel, their values never do.
    fn publish(
        &self,
        range: TokenRange,
        key: &PrimaryKey,
        kind: DiscrepancyKind,
        columns: Vec<String>,
    ) {
        if let Some(events) = self.events.as_ref() {
            events.discrepancy(chrono::Utc::now(), range, kind, &key.to_string(), columns);
        }
    }

    /// Issues one record's target lookup and buffers it (`VAL-001`).
    ///
    /// The lookup is **spawned**, not merely constructed: `VAL-001` says "issue an asynchronous
    /// target SELECT", and a future that is only polled when the batch is drained would be neither
    /// asynchronous nor issued — the batch would cost one round trip per row instead of one per
    /// batch, which is the whole reason the buffer exists.
    ///
    /// The in-flight permit and the rate-limit reservation are taken *before* the task is spawned
    /// and the permit moves into it, so `ENG-007`'s bound is on outstanding requests rather than on
    /// requests this function happens to be inside of, and `ENG-004`'s pacing applies to the
    /// spawning rather than to the awaiting.
    async fn enqueue(
        &self,
        ctx: &RangeContext,
        batch: &mut Batch,
        record: Record,
    ) -> Result<(), CdmError> {
        // The target's rate limiter, not the origin's: the lookup is a request against the target
        // cluster, and Java paces it with `rateLimiterTarget` for the same reason.
        ctx.acquire_write_rows(1).await;
        let permit = ctx.write_slot().await?;
        let target = Arc::clone(&self.target);
        let key: PrimaryKey = record.key().clone();
        batch.inflight.push_back(tokio::spawn(async move {
            let fetched = target.fetch(&key).await;
            drop(permit);
            fetched
        }));
        batch.records.push_back(record);
        Ok(())
    }
}

/// Which kind a missing row is reported as, once autocorrect has had its turn.
const fn missing_kind(corrected: bool) -> DiscrepancyKind {
    if corrected {
        DiscrepancyKind::CorrectedMissing
    } else {
        DiscrepancyKind::Missing
    }
}

/// Which kind a differing row is reported as, once autocorrect has had its turn.
const fn mismatch_kind(corrected: bool) -> DiscrepancyKind {
    if corrected {
        DiscrepancyKind::CorrectedMismatch
    } else {
        DiscrepancyKind::Mismatch
    }
}

/// One buffered batch: the records, and the target lookups outstanding for them.
///
/// The two queues are index-aligned and drained together. `FuturesOrdered` rather than
/// `FuturesUnordered` because the comparison must pair each result with the record that asked for
/// it, and pairing by completion order would pair them wrongly.
struct Batch {
    records: VecDeque<Record>,
    inflight: FuturesOrdered<tokio::task::JoinHandle<Result<Option<Record>, CdmError>>>,
}

impl Batch {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            records: VecDeque::with_capacity(capacity),
            inflight: FuturesOrdered::new(),
        }
    }

    fn len(&self) -> usize {
        self.records.len()
    }
}

#[async_trait::async_trait]
impl RangeProcessor for ValidateJob {
    fn job(&self) -> JobKind {
        JobKind::Validate
    }

    /// Validates one token range (`VAL-001`, `VAL-016`).
    ///
    /// # Errors
    ///
    /// Any error fails the range and only the range. The scheduler applies `ENG-008`: it marks the
    /// range `FAIL`, counts the rows this range read but did not classify, logs the error with the
    /// range bounds, and carries on. Nothing here tries to contain a range failure itself, which is
    /// exactly the discipline Java's copy-pasted failure path lacks.
    async fn process(&self, ctx: &RangeContext) -> Result<RangeVerdict, CdmError> {
        let counters = ctx.counters();
        let handles = ValidateCounters::resolve(counters)?;
        let label = java_thread_label(ctx.range());
        let fetch_size = usize::try_from(ctx.fetch_size())
            .unwrap_or(usize::MAX)
            .max(1);

        let mut stream = {
            let _slot = ctx.read_slot().await?;
            self.origin.open(ctx.range()).await?
        };
        let mut batch = Batch::with_capacity(fetch_size);
        let mut had_discrepancy = false;

        loop {
            // ENG-010: a cancelled range winds down rather than being dropped mid-batch, so the
            // lookups already issued are accounted for before the range gives up.
            if ctx.is_cancelled() {
                // The verdict is discarded — a cancelled range has none — but the comparison is
                // not: the lookups already issued are drained and counted, so `ENG-008`'s lost-row
                // arithmetic is computed against what the range actually did.
                let _drained = self
                    .compare_batch(&label, ctx.range(), counters, &handles, &mut batch)
                    .await?;
                return Err(CdmError::new(
                    cdm_core::ErrorKind::Cancelled,
                    "the validate range was cancelled while reading the origin",
                ));
            }
            ctx.acquire_read_rows(1).await;
            let Some(record) = stream.next_record().await? else {
                break;
            };
            // Java increments READ once per origin row, before filtering, and so do we: the
            // counter answers "how many rows did this range read", which a filter does not change.
            counters.increment(handles.read);

            // VAL-001: filters are applied before the target is asked anything, because a filtered
            // row must not cost a target read.
            if !self.filters.accepts(&record)? {
                counters.increment(handles.skipped);
                continue;
            }

            self.enqueue(ctx, &mut batch, record).await?;
            if batch.len() >= fetch_size {
                had_discrepancy |= self
                    .compare_batch(&label, ctx.range(), counters, &handles, &mut batch)
                    .await?;
            }
        }
        had_discrepancy |= self
            .compare_batch(&label, ctx.range(), counters, &handles, &mut batch)
            .await?;

        tracing::debug!(
            target: "cdm::validate",
            read = counters.count_of(CounterKind::Read, CounterView::Interim),
            valid = counters.count_of(CounterKind::Valid, CounterView::Interim),
            missing = counters.count_of(CounterKind::Missing, CounterView::Interim),
            mismatch = counters.count_of(CounterKind::Mismatch, CounterView::Interim),
            "validated a range"
        );
        // VAL-016, from the interim counters — see `status`.
        Ok(status::verdict(counters, had_discrepancy))
    }
}

#[cfg(test)]
mod tests;
