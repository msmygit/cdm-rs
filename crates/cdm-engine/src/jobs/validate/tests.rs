//! The validate job, driven by in-memory doubles for both clusters.
//!
//! Everything `VAL-001`..`VAL-012`, `VAL-016` and `VAL-017` require is observable without a node:
//! the origin is a vector of records, the target is a map from primary key to row, and the two
//! record every operation they were asked to perform — which is how `VAL-010`'s "validation never
//! deletes" is asserted as a property of the run rather than of a code review.
//!
//! What a node is needed for — that the generated CQL prepares, that a seeded difference is really
//! found, that autocorrect really repairs it — is in `tests/validate_it.rs`.

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::HashMap;
use std::sync::Arc;

use cdm_codec::{CodecRegistry, Codecset, Planner, PlannerOptions};
use cdm_config::model::{Autocorrect, CdmConfig};
use cdm_config::types::ReportFormat;
use cdm_core::{
    CdmError, ErrorKind, JobKind, Plugin, PrimaryKey, RawCell, Record, Row, RowSink, RowSource,
    RowStream, RunId, TokenRange,
};
use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
use cdm_cql::statement::{ColumnMapping, MappingOptions};
use cdm_feature::{ExtractJson, FeatureSchema, FilterChain, TableFacts};
use cdm_metrics::{CounterKind, CounterView, EventBus, EventPayload, JobCounters, Redaction};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::scheduler::{RangeContext, RuntimeLimits, SchedulerSettings};

use super::*;

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn column(name: &str, cql_type: &str, kind: ColumnKind, position: i32) -> ColumnMeta {
    ColumnMeta {
        name: name.to_owned(),
        cql_type: cql_type.to_owned(),
        kind,
        position,
        clustering_order: if kind == ColumnKind::Clustering {
            ClusteringOrder::Asc
        } else {
            ClusteringOrder::None
        },
    }
}

fn table(name: &str, columns: Vec<ColumnMeta>) -> TableSchema {
    TableSchema {
        keyspace: "ks".to_owned(),
        table: name.to_owned(),
        columns,
        is_materialized_view: false,
    }
}

/// `id int PRIMARY KEY, data text` on both sides.
fn simple() -> (TableSchema, TableSchema) {
    let columns = || {
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("data", "text", ColumnKind::Regular, -1),
        ]
    };
    (table("src", columns()), table("dst", columns()))
}

fn planner(codecs: &[Codecset]) -> Planner {
    Planner::new(
        CodecRegistry::with_builtins(codecs, None).unwrap(),
        PlannerOptions::default(),
    )
}

fn plan_for(
    origin: &TableSchema,
    target: &TableSchema,
    options: &MappingOptions,
) -> ComparisonPlan {
    let mapping = ColumnMapping::resolve(origin, target, options).unwrap();
    ComparisonPlan::resolve(&mapping, &planner(&[]), None, false).unwrap()
}

fn int(value: i32) -> RawCell {
    RawCell::new(value.to_be_bytes().to_vec())
}

fn text(value: &str) -> RawCell {
    RawCell::new(value.as_bytes().to_vec())
}

fn key(value: i32) -> PrimaryKey {
    PrimaryKey::new(vec![int(value)])
}

fn record(id: i32, data: &str) -> Record {
    Record::new(key(id), Row::new(vec![int(id), text(data)]))
}

// ---------------------------------------------------------------------------------------------
// Doubles
// ---------------------------------------------------------------------------------------------

/// An origin that yields a fixed list of records, or fails after `fail_after` of them.
#[derive(Debug, Default)]
struct FakeOrigin {
    records: Vec<Record>,
    fail_after: Option<usize>,
}

impl FakeOrigin {
    fn of(records: Vec<Record>) -> Arc<Self> {
        Arc::new(Self {
            records,
            fail_after: None,
        })
    }

    fn failing_after(records: Vec<Record>, after: usize) -> Arc<Self> {
        Arc::new(Self {
            records,
            fail_after: Some(after),
        })
    }
}

impl Plugin for FakeOrigin {
    fn name(&self) -> &'static str {
        "fake-origin"
    }
    fn provider(&self) -> &'static str {
        "cdm-engine-tests"
    }
}

#[async_trait::async_trait]
impl RowSource for FakeOrigin {
    async fn open(&self, _range: TokenRange) -> Result<Box<dyn RowStream>, CdmError> {
        Ok(Box::new(FakeStream {
            records: self.records.clone().into(),
            fail_after: self.fail_after,
            served: 0,
        }))
    }
}

struct FakeStream {
    records: std::collections::VecDeque<Record>,
    fail_after: Option<usize>,
    served: usize,
}

#[async_trait::async_trait]
impl RowStream for FakeStream {
    async fn next_record(&mut self) -> Result<Option<Record>, CdmError> {
        if self.fail_after == Some(self.served) {
            return Err(CdmError::new(ErrorKind::Read, "the origin page failed"));
        }
        self.served += 1;
        Ok(self.records.pop_front())
    }
}

/// A target that answers lookups from a map and records every operation it was asked to perform.
#[derive(Debug, Default)]
struct FakeTarget {
    rows: Mutex<HashMap<PrimaryKey, Row>>,
    ops: Mutex<Vec<String>>,
}

impl FakeTarget {
    fn with(rows: Vec<(PrimaryKey, Row)>) -> Arc<Self> {
        Arc::new(Self {
            rows: Mutex::new(rows.into_iter().collect()),
            ops: Mutex::new(Vec::new()),
        })
    }

    fn ops(&self) -> Vec<String> {
        self.ops.lock().clone()
    }

    fn writes(&self) -> Vec<String> {
        self.ops()
            .into_iter()
            .filter(|op| op.starts_with("write"))
            .collect()
    }
}

impl Plugin for FakeTarget {
    fn name(&self) -> &'static str {
        "fake-target"
    }
    fn provider(&self) -> &'static str {
        "cdm-engine-tests"
    }
}

#[async_trait::async_trait]
impl RowSink for FakeTarget {
    async fn write(&self, record: &Record) -> Result<(), CdmError> {
        self.ops.lock().push(format!("write:{}", record.key()));
        self.rows
            .lock()
            .insert(record.key().clone(), record.origin().clone());
        Ok(())
    }

    async fn flush(&self) -> Result<(), CdmError> {
        Ok(())
    }

    async fn fetch(&self, key: &PrimaryKey) -> Result<Option<Record>, CdmError> {
        self.ops.lock().push(format!("fetch:{key}"));
        Ok(self
            .rows
            .lock()
            .get(key)
            .map(|row| Record::new(key.clone(), row.clone())))
    }
}

// ---------------------------------------------------------------------------------------------
// Running a range
// ---------------------------------------------------------------------------------------------

fn context(counters: &Arc<JobCounters>, fetch_size: u32) -> RangeContext {
    let settings = SchedulerSettings::default().with_ratelimits(0, 0);
    RangeContext::new(
        RunId::from_raw(1),
        Arc::from("node-a"),
        TokenRange::new(-100, 100).unwrap(),
        fetch_size,
        Arc::clone(counters),
        Arc::new(RuntimeLimits::new(&settings).unwrap()),
        CancellationToken::new(),
    )
}

struct Run {
    verdict: Result<RangeVerdict, CdmError>,
    counters: Arc<JobCounters>,
    diff: Arc<DiffLog>,
    target: Arc<FakeTarget>,
    report: Arc<DiscrepancyReport>,
}

impl Run {
    fn count(&self, kind: CounterKind) -> u64 {
        self.counters.count_of(kind, CounterView::Interim)
    }

    fn diff_lines(&self) -> Vec<String> {
        self.diff.captured()
    }

    /// The discrepancy report's text, closed as a real run closes it (`VAL-013`).
    fn report_text(&self) -> String {
        self.report.finish().unwrap();
        self.report.captured()
    }
}

async fn run_with(
    origin: Arc<FakeOrigin>,
    target: Arc<FakeTarget>,
    plan: ComparisonPlan,
    settings: ValidateSettings,
    fetch_size: u32,
) -> Run {
    run_full(
        origin,
        target,
        plan,
        settings,
        fetch_size,
        FilterChain::new(),
    )
    .await
}

async fn run_full(
    origin: Arc<FakeOrigin>,
    target: Arc<FakeTarget>,
    plan: ComparisonPlan,
    settings: ValidateSettings,
    fetch_size: u32,
    filters: FilterChain,
) -> Run {
    run_reporting(
        origin,
        target,
        plan,
        settings,
        fetch_size,
        filters,
        Arc::new(DiscrepancyReport::disabled()),
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)] // A test harness, and every argument is one run's setting.
async fn run_reporting(
    origin: Arc<FakeOrigin>,
    target: Arc<FakeTarget>,
    plan: ComparisonPlan,
    settings: ValidateSettings,
    fetch_size: u32,
    filters: FilterChain,
    report: Arc<DiscrepancyReport>,
    events: Option<Arc<EventBus>>,
) -> Run {
    let diff = Arc::new(DiffLog::in_memory());
    let mut job = ValidateJob::new(
        origin,
        Arc::clone(&target) as Arc<dyn RowSink>,
        Arc::new(plan),
        settings,
        Arc::clone(&diff),
    )
    .with_filters(filters)
    .with_report(Arc::clone(&report));
    if let Some(events) = events {
        job = job.with_events(events);
    }
    let counters = Arc::new(JobCounters::new(JobKind::Validate));
    let ctx = context(&counters, fetch_size);
    let verdict = job.process(&ctx).await;
    Run {
        verdict,
        counters,
        diff,
        target,
        report,
    }
}

/// A run that writes a report of the given shape, over the `id int, data text` fixture.
async fn run_reported(
    origin: Vec<Record>,
    target: Vec<(PrimaryKey, Row)>,
    format: ReportFormat,
    redact_values: bool,
) -> Run {
    let (src, dst) = simple();
    run_reporting(
        FakeOrigin::of(origin),
        FakeTarget::with(target),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings::read_only(),
        1000,
        FilterChain::new(),
        Arc::new(DiscrepancyReport::in_memory(
            RunId::from_raw(7),
            format,
            redact_values,
        )),
        None,
    )
    .await
}

/// The common case: `id int PRIMARY KEY, data text`, no features, no autocorrect.
async fn run_simple(origin: Vec<Record>, target: Vec<(PrimaryKey, Row)>) -> Run {
    let (src, dst) = simple();
    run_with(
        FakeOrigin::of(origin),
        FakeTarget::with(target),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings::read_only(),
        1000,
    )
    .await
}

// ---------------------------------------------------------------------------------------------
// VAL-001 — read, filter, look up, buffer
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn val_001_every_origin_row_is_read_and_looked_up_by_its_target_key() {
    let run = run_simple(
        vec![record(1, "a"), record(2, "b")],
        vec![
            (key(1), Row::new(vec![int(1), text("a")])),
            (key(2), Row::new(vec![int(2), text("b")])),
        ],
    )
    .await;

    assert_eq!(run.count(CounterKind::Read), 2);
    assert_eq!(run.count(CounterKind::Valid), 2);
    let mut fetched = run.target.ops();
    fetched.sort();
    assert_eq!(fetched, vec!["fetch:(0x00000001)", "fetch:(0x00000002)"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn val_001_records_are_buffered_and_compared_in_batches_of_fetch_size() {
    // Autocorrect turns each comparison into an observable write, so the operation log shows where
    // one batch ended and the next began: with `fetch_size = 2` and five missing rows the lookups
    // arrive in groups of two, each group followed by its corrections.
    let (src, dst) = simple();
    let run = run_with(
        FakeOrigin::of((1..=5).map(|id| record(id, "a")).collect()),
        FakeTarget::with(Vec::new()),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings {
            autocorrect: Autocorrect {
                missing: true,
                ..Autocorrect::default()
            },
            target_is_counter: false,
        },
        2,
    )
    .await;

    let ops = run.target.ops();
    let at = |op: &str| {
        ops.iter()
            .position(|seen| seen == op)
            .unwrap_or_else(|| panic!("`{op}` never happened: {ops:?}"))
    };
    let hex = |id: u32| format!("(0x{id:08x})");

    // The batch boundary is what is asserted, not the order within a batch: two lookups issued
    // together may complete in either order, and pinning that down would be testing the runtime.
    // What `VAL-001` promises is that a batch of `fetch_size` records is compared — and therefore
    // corrected — before the next record's lookup is issued.
    for corrected in 1..=2 {
        assert!(
            at(&format!("write:{}", hex(corrected))) < at(&format!("fetch:{}", hex(3))),
            "the first batch must be compared before the second is read: {ops:?}"
        );
    }
    for corrected in 3..=4 {
        assert!(
            at(&format!("write:{}", hex(corrected))) < at(&format!("fetch:{}", hex(5))),
            "the second batch must be compared before the third is read: {ops:?}"
        );
    }
    // Five records, five lookups, five corrections, and nothing else.
    assert_eq!(ops.len(), 10, "{ops:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn val_001_a_filtered_row_is_skipped_and_costs_the_target_nothing() {
    /// Rejects every record, which is what `FEA-054` calls a filter.
    #[derive(Debug)]
    struct RejectAll;
    impl Plugin for RejectAll {
        fn name(&self) -> &'static str {
            "reject-all"
        }
        fn provider(&self) -> &'static str {
            "cdm-engine-tests"
        }
    }
    impl cdm_core::FilterPlugin for RejectAll {
        fn accepts(&self, _record: &Record) -> Result<bool, CdmError> {
            Ok(false)
        }
    }

    let (src, dst) = simple();
    let run = run_full(
        FakeOrigin::of(vec![record(1, "a")]),
        FakeTarget::with(Vec::new()),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings::read_only(),
        1000,
        FilterChain::new().with(Arc::new(RejectAll)),
    )
    .await;

    assert_eq!(run.count(CounterKind::Read), 1);
    assert_eq!(run.count(CounterKind::Skipped), 1);
    assert_eq!(run.count(CounterKind::Missing), 0);
    assert!(
        run.target.ops().is_empty(),
        "a filtered row must not reach the target: {:?}",
        run.target.ops()
    );
    assert_eq!(*run.verdict.as_ref().unwrap(), RangeVerdict::Pass);
}

// ---------------------------------------------------------------------------------------------
// VAL-002, VAL-008 — missing and valid
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn val_002_a_missing_target_row_is_counted_and_logged_by_key() {
    let run = run_simple(vec![record(7, "a")], Vec::new()).await;

    assert_eq!(run.count(CounterKind::Missing), 1);
    assert_eq!(run.count(CounterKind::Valid), 0);
    assert_eq!(run.count(CounterKind::CorrectedMissing), 0);
    assert_eq!(*run.verdict.as_ref().unwrap(), RangeVerdict::Diff);

    let lines = run.diff_lines();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].ends_with("Missing target row found for key: (0x00000007)"),
        "{}",
        lines[0]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn val_008_a_fully_matching_record_is_valid_and_says_nothing() {
    let run = run_simple(
        vec![record(1, "a")],
        vec![(key(1), Row::new(vec![int(1), text("a")]))],
    )
    .await;

    assert_eq!(run.count(CounterKind::Valid), 1);
    assert_eq!(run.count(CounterKind::Mismatch), 0);
    assert!(run.diff_lines().is_empty());
    assert_eq!(*run.verdict.as_ref().unwrap(), RangeVerdict::Pass);
}

// ---------------------------------------------------------------------------------------------
// VAL-003, VAL-004 — correcting a missing row
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn val_003_autocorrect_missing_upserts_the_record_and_counts_it() {
    let (src, dst) = simple();
    let run = run_with(
        FakeOrigin::of(vec![record(7, "a")]),
        FakeTarget::with(Vec::new()),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings {
            autocorrect: Autocorrect {
                missing: true,
                ..Autocorrect::default()
            },
            target_is_counter: false,
        },
        1000,
    )
    .await;

    assert_eq!(run.count(CounterKind::Missing), 1);
    assert_eq!(run.count(CounterKind::CorrectedMissing), 1);
    assert_eq!(run.target.writes(), vec!["write:(0x00000007)"]);
    // VAL-016: every discrepancy was corrected.
    assert_eq!(*run.verdict.as_ref().unwrap(), RangeVerdict::DiffCorrected);

    let lines = run.diff_lines();
    assert!(
        lines[1].ends_with("Inserted missing row in target: (0x00000007)"),
        "{lines:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn val_004_a_counter_row_is_not_reinserted_without_the_explicit_opt_in() {
    let (src, dst) = simple();
    let settings = |missing_counter: bool| ValidateSettings {
        autocorrect: Autocorrect {
            missing: true,
            missing_counter,
            ..Autocorrect::default()
        },
        target_is_counter: true,
    };

    let refused = run_with(
        FakeOrigin::of(vec![record(7, "a")]),
        FakeTarget::with(Vec::new()),
        plan_for(&src, &dst, &MappingOptions::default()),
        settings(false),
        1000,
    )
    .await;
    assert_eq!(refused.count(CounterKind::Missing), 1);
    assert_eq!(refused.count(CounterKind::CorrectedMissing), 0);
    assert!(
        refused.target.writes().is_empty(),
        "a counter row was re-inserted"
    );
    // Nothing was corrected, so the range is DIFF and a resume re-plans it (TRK-031).
    assert_eq!(*refused.verdict.as_ref().unwrap(), RangeVerdict::Diff);
    assert!(
        refused.diff_lines()[1].contains(
            "autocorrect.missing is true, but not Inserting as autocorrect.missing_counter is \
             not enabled; key : (0x00000007)"
        ),
        "{:?}",
        refused.diff_lines()
    );

    let allowed = run_with(
        FakeOrigin::of(vec![record(7, "a")]),
        FakeTarget::with(Vec::new()),
        plan_for(&src, &dst, &MappingOptions::default()),
        settings(true),
        1000,
    )
    .await;
    assert_eq!(allowed.count(CounterKind::CorrectedMissing), 1);
    assert_eq!(allowed.target.writes(), vec!["write:(0x00000007)"]);
}

// ---------------------------------------------------------------------------------------------
// VAL-005 — the comparison itself
// ---------------------------------------------------------------------------------------------

#[test]
fn val_005_the_target_is_converted_into_the_origins_type_space() {
    // Origin `int`, target `text`: the migration wrote "10", and validation must parse it back to
    // `10` rather than compare four bytes against two characters.
    let origin = table(
        "src",
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("n", "int", ColumnKind::Regular, -1),
        ],
    );
    let target = table(
        "dst",
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("n", "text", ColumnKind::Regular, -1),
        ],
    );
    let mapping = ColumnMapping::resolve(&origin, &target, &MappingOptions::default()).unwrap();
    let plan =
        ComparisonPlan::resolve(&mapping, &planner(&[Codecset::IntString]), None, false).unwrap();

    let record = Record::new(key(1), Row::new(vec![int(1), int(10)]));
    let agreeing = Row::new(vec![int(1), text("10")]);
    assert_eq!(plan.compare(&record, Some(&agreeing)), Comparison::Valid);

    let differing = Row::new(vec![int(1), text("11")]);
    assert!(matches!(
        plan.compare(&record, Some(&differing)),
        Comparison::Mismatch(_)
    ));
}

#[test]
fn val_005_two_nulls_agree_and_an_origin_null_with_a_target_value_does_not() {
    let (src, dst) = simple();
    let plan = plan_for(&src, &dst, &MappingOptions::default());

    let record = Record::new(key(1), Row::new(vec![int(1), RawCell::NULL]));
    let both_null = Row::new(vec![int(1), RawCell::NULL]);
    assert_eq!(plan.compare(&record, Some(&both_null)), Comparison::Valid);

    let target_populated = Row::new(vec![int(1), text("x")]);
    let Comparison::Mismatch(mismatch) = plan.compare(&record, Some(&target_populated)) else {
        panic!("an origin null against a target value is a mismatch");
    };
    assert_eq!(mismatch.columns(), vec!["data"]);
    assert_eq!(
        mismatch.detail(),
        format!("Target column:data origin is null, target is {REDACTED}; ")
    );
}

#[test]
fn val_005_an_origin_value_against_a_target_null_is_a_mismatch() {
    let (src, dst) = simple();
    let plan = plan_for(&src, &dst, &MappingOptions::default());
    let record = record(1, "a");
    let Comparison::Mismatch(mismatch) =
        plan.compare(&record, Some(&Row::new(vec![int(1), RawCell::NULL])))
    else {
        panic!("expected a mismatch");
    };
    assert_eq!(
        mismatch.detail(),
        format!("Target column:data-origin[{REDACTED}]-target[null]; ")
    );
}

#[test]
fn val_005_a_constant_column_is_excluded_from_comparison() {
    let (src, mut dst) = simple();
    dst.columns
        .push(column("tenant", "text", ColumnKind::Regular, -1));
    let options = MappingOptions {
        constants: vec![("tenant".to_owned(), "'acme'".to_owned())],
        ..MappingOptions::default()
    };
    let plan = plan_for(&src, &dst, &options);

    // The target's constant column holds something else entirely; a constant is configuration, not
    // data, so there is nothing on the origin for it to disagree with.
    let target = Row::new(vec![int(1), text("a"), text("something-else")]);
    assert_eq!(
        plan.compare(&record(1, "a"), Some(&target)),
        Comparison::Valid
    );
}

#[test]
fn val_005_a_target_only_column_with_a_value_is_reported() {
    let (src, mut dst) = simple();
    dst.columns
        .push(column("extra", "text", ColumnKind::Regular, -1));
    let plan = plan_for(&src, &dst, &MappingOptions::default());

    let quiet = Row::new(vec![int(1), text("a"), RawCell::NULL]);
    assert_eq!(
        plan.compare(&record(1, "a"), Some(&quiet)),
        Comparison::Valid
    );

    let populated = Row::new(vec![int(1), text("a"), text("x")]);
    let Comparison::Mismatch(mismatch) = plan.compare(&record(1, "a"), Some(&populated)) else {
        panic!("expected a mismatch");
    };
    assert_eq!(mismatch.columns(), vec!["extra"]);
}

// ---------------------------------------------------------------------------------------------
// VAL-006, VAL-007 — mismatch and its correction
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn val_006_a_mismatch_is_counted_and_logged_with_the_key_and_the_detail() {
    let run = run_simple(
        vec![record(3, "a")],
        vec![(key(3), Row::new(vec![int(3), text("b")]))],
    )
    .await;

    assert_eq!(run.count(CounterKind::Mismatch), 1);
    assert_eq!(run.count(CounterKind::Valid), 0);
    let lines = run.diff_lines();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].ends_with(&format!(
            "Mismatch row found for key: (0x00000003) Mismatch: Target \
             column:data-origin[{REDACTED}]-target[{REDACTED}]; "
        )),
        "{}",
        lines[0]
    );
}

#[test]
fn val_006_the_detail_lists_every_differing_column_in_target_order() {
    let columns = || {
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("a", "text", ColumnKind::Regular, -1),
            column("b", "text", ColumnKind::Regular, -1),
        ]
    };
    let plan = plan_for(
        &table("src", columns()),
        &table("dst", columns()),
        &MappingOptions::default(),
    );
    let record = Record::new(key(1), Row::new(vec![int(1), text("x"), text("y")]));
    let Comparison::Mismatch(mismatch) =
        plan.compare(&record, Some(&Row::new(vec![int(1), text("p"), text("q")])))
    else {
        panic!("expected a mismatch");
    };
    // Deterministic order, unlike Java's parallel stream over a shared StringBuffer.
    assert_eq!(mismatch.columns(), vec!["a", "b"]);
    assert_eq!(mismatch.len(), 2);
    assert!(!mismatch.is_empty());
    assert!(mismatch.detail().starts_with("Target column:a-origin["));
}

#[tokio::test(flavor = "multi_thread")]
async fn val_007_autocorrect_mismatch_rewrites_the_row_and_counts_it() {
    let (src, dst) = simple();
    let run = run_with(
        FakeOrigin::of(vec![record(3, "a")]),
        FakeTarget::with(vec![(key(3), Row::new(vec![int(3), text("b")]))]),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings {
            autocorrect: Autocorrect {
                mismatch: true,
                ..Autocorrect::default()
            },
            target_is_counter: false,
        },
        1000,
    )
    .await;

    assert_eq!(run.count(CounterKind::Mismatch), 1);
    assert_eq!(run.count(CounterKind::CorrectedMismatch), 1);
    assert_eq!(run.target.writes(), vec!["write:(0x00000003)"]);
    assert_eq!(*run.verdict.as_ref().unwrap(), RangeVerdict::DiffCorrected);
    assert!(
        run.diff_lines()[1].ends_with("Corrected mismatch row in target: (0x00000003)"),
        "{:?}",
        run.diff_lines()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn val_007_the_corrected_record_carries_the_target_row_it_disagreed_with() {
    // `MIG-031` binds a counter as `origin − current_target`, so a correction that dropped the
    // fetched target row would double the delta. The record handed to the sink must carry it.
    #[derive(Debug, Default)]
    struct TargetRowSpy {
        saw_target: Mutex<Option<bool>>,
    }
    impl Plugin for TargetRowSpy {
        fn name(&self) -> &'static str {
            "target-row-spy"
        }
        fn provider(&self) -> &'static str {
            "cdm-engine-tests"
        }
    }
    #[async_trait::async_trait]
    impl RowSink for TargetRowSpy {
        async fn write(&self, record: &Record) -> Result<(), CdmError> {
            *self.saw_target.lock() = Some(record.target().is_some());
            Ok(())
        }
        async fn flush(&self) -> Result<(), CdmError> {
            Ok(())
        }
        async fn fetch(&self, key: &PrimaryKey) -> Result<Option<Record>, CdmError> {
            Ok(Some(Record::new(
                key.clone(),
                Row::new(vec![int(3), text("b")]),
            )))
        }
    }

    let (src, dst) = simple();
    let spy = Arc::new(TargetRowSpy::default());
    let job = ValidateJob::new(
        FakeOrigin::of(vec![record(3, "a")]),
        Arc::clone(&spy) as Arc<dyn RowSink>,
        Arc::new(plan_for(&src, &dst, &MappingOptions::default())),
        ValidateSettings {
            autocorrect: Autocorrect {
                mismatch: true,
                ..Autocorrect::default()
            },
            target_is_counter: false,
        },
        Arc::new(DiffLog::in_memory()),
    );
    let counters = Arc::new(JobCounters::new(JobKind::Validate));
    job.process(&context(&counters, 1000)).await.unwrap();
    assert_eq!(*spy.saw_target.lock(), Some(true));
}

// ---------------------------------------------------------------------------------------------
// VAL-009 — a column that cannot be compared
// ---------------------------------------------------------------------------------------------

#[test]
fn val_009_a_column_that_cannot_be_compared_becomes_detail_not_a_failed_range() {
    // An exploded map's key column has no origin cell on the record, which is precisely the case
    // Java reaches by throwing `IndexOutOfBoundsException` out of its message builder and catching
    // it into the detail. The outcome is the same — a mismatch, with an explanation — without the
    // stack trace.
    let origin = table(
        "src",
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("m", "map<text, text>", ColumnKind::Regular, -1),
        ],
    );
    let target = table(
        "dst",
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("k", "text", ColumnKind::Clustering, 0),
            column("v", "text", ColumnKind::Regular, -1),
        ],
    );
    let options = MappingOptions {
        explode_map: Some(("m".to_owned(), "k".to_owned(), "v".to_owned())),
        ..MappingOptions::default()
    };
    let mapping = ColumnMapping::resolve(&origin, &target, &options).unwrap();
    let plan = ComparisonPlan::resolve(&mapping, &planner(&[]), None, false).unwrap();

    let record = Record::new(key(1), Row::new(vec![int(1), RawCell::NULL]));
    let Comparison::Mismatch(mismatch) =
        plan.compare(&record, Some(&Row::new(vec![int(1), text("a"), text("b")])))
    else {
        panic!("expected the uncomparable columns to be reported");
    };
    assert_eq!(mismatch.columns(), vec!["k", "v"]);
    let detail = mismatch.detail();
    assert!(detail.contains("Target column:k Exception "), "{detail}");
    assert!(
        detail.contains("targetIndex:1 originIndex:-1; "),
        "{detail}"
    );
}

// ---------------------------------------------------------------------------------------------
// VAL-010 — validation never deletes
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn val_010_no_run_ever_asks_the_target_to_delete_anything() {
    // A target row the origin does not have is invisible: it is never fetched, never compared and
    // never removed. The op log is the proof, and the `RowSink` trait offers no delete at all —
    // there is nothing to call even by accident.
    let (src, dst) = simple();
    let run = run_with(
        FakeOrigin::of(vec![record(1, "a")]),
        FakeTarget::with(vec![
            (key(1), Row::new(vec![int(1), text("b")])),
            (key(99), Row::new(vec![int(99), text("orphan")])),
        ]),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings {
            autocorrect: Autocorrect {
                missing: true,
                mismatch: true,
                missing_counter: true,
            },
            target_is_counter: false,
        },
        1000,
    )
    .await;

    for op in run.target.ops() {
        assert!(
            op.starts_with("fetch:") || op.starts_with("write:"),
            "validate performed a `{op}`"
        );
    }
    assert!(run.target.rows.lock().contains_key(&key(99)));
}

#[test]
fn val_010_no_source_file_in_the_job_contains_a_delete() {
    // A sweep rather than a review comment: `VAL-010` is a promise about the whole module, and the
    // cheapest way to keep it is to make adding a `DELETE` fail a test.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/jobs");
    let mut stack = vec![root];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).unwrap() {
                stack.push(entry.unwrap().path());
            }
            continue;
        }
        if path.file_stem().is_some_and(|stem| stem == "tests") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.to_uppercase().contains("DELETE FROM"),
            "{} contains a DELETE (VAL-010)",
            path.display()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// VAL-011 — extract JSON with overwrite = false
// ---------------------------------------------------------------------------------------------

fn extract_json_plan(overwrite: bool) -> ComparisonPlan {
    let origin = table(
        "src",
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("doc", "text", ColumnKind::Regular, -1),
        ],
    );
    let target = table(
        "dst",
        vec![
            column("id", "int", ColumnKind::PartitionKey, 0),
            column("city", "text", ColumnKind::Regular, -1),
        ],
    );
    let config: cdm_core::EffectiveConfig = [
        ("feature.extract_json.origin_column", "doc"),
        ("feature.extract_json.property_mapping", "city:city"),
        (
            "feature.extract_json.overwrite",
            if overwrite { "true" } else { "false" },
        ),
    ]
    .into_iter()
    .collect();
    let feature = ExtractJson::load(&config);
    let schema = FeatureSchema::new(
        TableFacts::from_view(
            &cdm_feature::table_view(
                cdm_core::TableRef::new("ks", "src"),
                &[("id", "int"), ("doc", "text")],
            ),
            &["id"],
        )
        .unwrap(),
        TableFacts::from_view(
            &cdm_feature::table_view(
                cdm_core::TableRef::new("ks", "dst"),
                &[("id", "int"), ("city", "text")],
            ),
            &["id"],
        )
        .unwrap(),
    );
    let extract = feature.resolve(&schema).unwrap();
    let options = MappingOptions {
        extract_json: Some(("doc".to_owned(), "city".to_owned())),
        ..MappingOptions::default()
    };
    let mapping = ColumnMapping::resolve(&origin, &target, &options).unwrap();
    ComparisonPlan::resolve(&mapping, &planner(&[]), Some(extract), overwrite).unwrap()
}

#[test]
fn val_011_a_populated_extract_column_is_skipped_when_overwrite_is_false() {
    let plan = extract_json_plan(false);
    let record = Record::new(key(1), Row::new(vec![int(1), text(r#"{"city":"Paris"}"#)]));
    // The target says something else entirely. With `overwrite = false` the migration deliberately
    // left it alone, so comparing it would report a difference the run itself chose to create.
    let target = Row::new(vec![int(1), text("Berlin")]);
    assert_eq!(plan.compare(&record, Some(&target)), Comparison::Valid);
}

#[test]
fn val_011_an_empty_extract_column_is_compared_even_when_overwrite_is_false() {
    let plan = extract_json_plan(false);
    let record = Record::new(key(1), Row::new(vec![int(1), text(r#"{"city":"Paris"}"#)]));
    let target = Row::new(vec![int(1), RawCell::NULL]);
    assert!(matches!(
        plan.compare(&record, Some(&target)),
        Comparison::Mismatch(_)
    ));
}

#[test]
fn val_011_with_overwrite_the_populated_column_is_compared_normally() {
    let plan = extract_json_plan(true);
    let record = Record::new(key(1), Row::new(vec![int(1), text(r#"{"city":"Paris"}"#)]));
    assert_eq!(
        plan.compare(&record, Some(&Row::new(vec![int(1), text("Paris")]))),
        Comparison::Valid
    );
    assert!(matches!(
        plan.compare(&record, Some(&Row::new(vec![int(1), text("Berlin")]))),
        Comparison::Mismatch(_)
    ));
}

// ---------------------------------------------------------------------------------------------
// VAL-012 — the dedicated sink
// ---------------------------------------------------------------------------------------------

#[test]
fn val_012_the_diff_log_defaults_to_javas_path_and_is_written_as_a_separate_file() {
    assert_eq!(DEFAULT_DIFF_FILE, "cdm_logs/cdm_diff.log");
    assert_eq!(
        cdm_config::CdmConfig::default().logging.diff_file,
        std::path::PathBuf::from(DEFAULT_DIFF_FILE),
        "the configured default and the appender Java ships must agree"
    );

    let dir = tempfile::tempdir().unwrap();
    // A nested path the operator has not created: Java's appender creates it, and so must this.
    let path = dir.path().join("cdm_logs").join("cdm_diff.log");
    let log = DiffLog::open(&path).unwrap();
    assert_eq!(log.path(), path);
    log.missing("-9:9", &key(4));
    log.mismatch("-9:9", &key(5), "Target column:data-origin[x]-target[y]; ");
    drop(log);

    let written = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines.len(), 2, "{written}");
    assert!(lines[0]
        .contains(" ERROR [-9:9] validate - Missing target row found for key: (0x00000004)"));
    assert!(lines[1].contains("Mismatch row found for key: (0x00000005)"));
}

#[test]
fn val_012_a_diff_log_that_cannot_be_opened_is_a_startup_error() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file should be: opening it for append fails, and it must fail now
    // rather than at the end of a six-hour run.
    let path = dir.path().join("occupied");
    std::fs::create_dir(&path).unwrap();
    let error = DiffLog::open(&path).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config);
    assert!(error.to_string().contains("logging.diff_file"), "{error}");
}

// ---------------------------------------------------------------------------------------------
// VAL-016 — the verdict
// ---------------------------------------------------------------------------------------------

#[test]
fn val_016_the_verdict_reads_the_level_that_has_the_counts_in_it() {
    let counters = JobCounters::new(JobKind::Validate);
    let bump = |kind, by| {
        let counter = counters.counter(kind).unwrap();
        counters.increment_by(counter, by);
    };
    bump(CounterKind::Missing, 3);
    bump(CounterKind::CorrectedMissing, 1);

    // Interim holds the truth: one of three missing rows was corrected, so the range is DIFF.
    assert_eq!(status::verdict(&counters, true), RangeVerdict::Diff);

    // The committed level is still empty, because the scheduler has not flushed. Reading it here
    // would compare 0 == 0 for both pairs and call this range DIFF_CORRECTED — the same shape of
    // defect as ENG-008, on a counter that decides whether a resume ever revisits the range.
    assert_eq!(
        counters.count_of(CounterKind::Missing, CounterView::Committed),
        0
    );
    assert_eq!(
        counters.count_of(CounterKind::Missing, CounterView::Interim),
        3
    );
}

#[test]
fn val_016_resolves_pass_diff_and_diff_corrected() {
    let counters = JobCounters::new(JobKind::Validate);
    let bump = |kind, by| {
        let counter = counters.counter(kind).unwrap();
        counters.increment_by(counter, by);
    };

    // No discrepancy at all.
    assert_eq!(status::verdict(&counters, false), RangeVerdict::Pass);

    bump(CounterKind::Mismatch, 2);
    bump(CounterKind::CorrectedMismatch, 2);
    // Every discrepancy corrected. `had_discrepancy` is still true: the range *was* wrong, and
    // saying PASS would erase that.
    assert_eq!(
        status::verdict(&counters, true),
        RangeVerdict::DiffCorrected
    );

    bump(CounterKind::Missing, 1);
    assert_eq!(status::verdict(&counters, true), RangeVerdict::Diff);
}

// ---------------------------------------------------------------------------------------------
// VAL-017 — no row value is ever rendered
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn val_017_no_row_value_reaches_the_diff_log() {
    let secret = "4111111111111111";
    let run = run_simple(
        vec![record(1, secret)],
        vec![(key(1), Row::new(vec![int(1), text("9876543210")]))],
    )
    .await;

    let lines = run.diff_lines().join("\n");
    assert!(!lines.contains(secret), "the origin value leaked: {lines}");
    assert!(
        !lines.contains("9876543210"),
        "the target value leaked: {lines}"
    );
    assert!(lines.contains(REDACTED));
    // The key is what identifies the row — the same resolution ERR-005 reached.
    assert!(lines.contains("(0x00000001)"));
}

#[test]
fn val_017_the_redaction_marker_is_the_only_thing_a_value_position_can_render_as() {
    let (src, dst) = simple();
    let plan = plan_for(&src, &dst, &MappingOptions::default());
    let Comparison::Mismatch(mismatch) = plan.compare(
        &record(1, "sensitive"),
        Some(&Row::new(vec![int(1), text("also-sensitive")])),
    ) else {
        panic!("expected a mismatch");
    };
    let detail = mismatch.detail();
    assert_eq!(
        detail,
        format!("Target column:data-origin[{REDACTED}]-target[{REDACTED}]; ")
    );
    assert!(!detail.contains("sensitive"));
}

// ---------------------------------------------------------------------------------------------
// ENG-008 — a failed validate range counts what it lost
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn eng_008_a_failing_validate_range_reports_an_error_count_java_would_report_as_zero() {
    use crate::planner::{Partitioner, Planner as TokenPlanner, PlannerSettings};
    use crate::scheduler::{NoopObserver, Scheduler};

    // Four rows are read and classified before the origin page fails. Java's `DiffJobSession`
    // computes `READ − VALID − MISSING − MISMATCH − SKIPPED` from *committed* counters on this
    // path, where every term is still zero, and therefore always adds `ERROR: 0`.
    let (src, dst) = simple();
    let origin = FakeOrigin::failing_after((1..=10).map(|id| record(id, "a")).collect(), 4);
    // The target has nothing, so every row read is a MISSING — a *classified* row. The rows the
    // range loses are the ones it read into its buffer and never got to compare.
    let job = ValidateJob::new(
        origin,
        FakeTarget::with(
            (1..=10)
                .map(|id| (key(id), Row::new(vec![int(id), text("a")])))
                .collect(),
        ),
        Arc::new(plan_for(&src, &dst, &MappingOptions::default())),
        ValidateSettings::read_only(),
        Arc::new(DiffLog::in_memory()),
    );

    let plan = TokenPlanner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(1))
        .plan(RunId::from_raw(1), None)
        .unwrap();
    let scheduler = Scheduler::new(SchedulerSettings::default().with_workers(1)).unwrap();
    let report = scheduler
        .run(&plan, Arc::new(job), Arc::new(NoopObserver))
        .await
        .unwrap();

    assert_eq!(report.ranges_failed(), 1);
    let errors = report
        .counters()
        .count_of(CounterKind::Error, CounterView::Committed);
    assert_eq!(
        errors, 4,
        "the four rows the range read and never classified are the rows it lost"
    );
    // The run did not abort: ENG-008 requires a failed range to be isolated, not fatal.
    assert!(report.is_complete());
}

// ---------------------------------------------------------------------------------------------
// VAL-013 — the machine-readable discrepancy report, and what it is allowed to contain
// ---------------------------------------------------------------------------------------------

/// The origin, the target and the one thing that differs between them: a text column holding an
/// address, which is the sort of value `SEC-002` exists for.
const SECRET: &str = "alice@example.com";

/// Its hex rendering, which is what a report with redaction off would contain.
fn secret_hex() -> String {
    RawCell::new(SECRET.as_bytes().to_vec()).to_string()
}

/// Row 1 differs (origin holds the secret, target holds something else) and row 2 is missing.
async fn seeded(format: ReportFormat, redact_values: bool) -> Run {
    run_reported(
        vec![record(1, SECRET), record(2, "gone")],
        vec![(key(1), Row::new(vec![int(1), text("tampered")]))],
        format,
        redact_values,
    )
    .await
}

fn parse_ndjson(text: &str) -> Vec<DiscrepancyRecord> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{line}: {e}")))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn val_013_a_record_names_the_run_the_range_the_key_the_kind_and_the_columns() {
    let run = seeded(ReportFormat::Ndjson, true).await;
    let mut records = parse_ndjson(&run.report_text());
    records.sort_by(|a, b| a.key.cmp(&b.key));
    assert_eq!(records.len(), 2, "one record per discrepancy");

    let mismatch = &records[0];
    assert_eq!(mismatch.run_id, RunId::from_raw(7));
    // The range the context was built with, as decimal strings (TOK-002).
    assert_eq!(mismatch.range.min, "-100");
    assert_eq!(mismatch.range.max, "100");
    assert_eq!(mismatch.key, key(1).to_string());
    assert_eq!(mismatch.kind, DiscrepancyKind::Mismatch);
    assert_eq!(mismatch.columns.len(), 1);
    assert_eq!(mismatch.columns[0].column, "data");
    assert!(mismatch.columns[0].error.is_none());

    // VAL-002: a missing row has no columns, because nothing was compared.
    let missing = &records[1];
    assert_eq!(missing.kind, DiscrepancyKind::Missing);
    assert!(missing.columns.is_empty());
    assert_eq!(run.count(CounterKind::Valid), 0);
    assert_eq!(run.report.records(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn sec_002_a_report_redacts_row_values_by_default() {
    // The property the whole feature turns on: a report written with the shipped defaults contains
    // no row value, in any format. Asserted over the file's *bytes*, not over the record type, so
    // a future field that carried a value would fail this too.
    for format in [ReportFormat::Json, ReportFormat::Ndjson, ReportFormat::Csv] {
        let run = seeded(format, true).await;
        let text = run.report_text();
        assert!(run.report.redacts_values());
        assert!(!text.contains(SECRET), "{format}: a value leaked: {text}");
        assert!(
            !text.contains(&secret_hex()),
            "{format}: a value leaked as hex: {text}"
        );
        assert!(
            !text.contains(&RawCell::new(b"tampered".to_vec()).to_string()),
            "{format}: the target's value leaked: {text}"
        );
        // What it does contain: the key, the column name, and a self-describing digest.
        assert!(text.contains("data"), "{format}: {text}");
        assert!(text.contains(REDACTED_PREFIX), "{format}: {text}");
        assert!(text.contains(&key(1).to_string()), "{format}: {text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sec_002_the_default_is_redaction_and_nothing_has_to_be_set_to_get_it() {
    // The default lives in one place — `validate.report.redact_values` — and this is the assertion
    // that it is the safe one. A future change to it fails here rather than in a customer's ticket.
    let defaults = CdmConfig::default();
    assert!(defaults.validate.report.redact_values);
    assert_eq!(defaults.validate.report.format, ReportFormat::None);
    assert!(!defaults.validate.keys_only);

    let report = DiscrepancyReport::in_memory(
        RunId::from_raw(1),
        ReportFormat::Ndjson,
        defaults.validate.report.redact_values,
    );
    assert!(report.redacts_values());
}

#[tokio::test(flavor = "multi_thread")]
async fn sec_002_values_appear_only_when_redaction_is_explicitly_turned_off() {
    let run = seeded(ReportFormat::Ndjson, false).await;
    let text = run.report_text();
    assert!(text.contains(&secret_hex()), "{text}");
    assert!(!text.contains(REDACTED_PREFIX), "{text}");

    let records = parse_ndjson(&text);
    let mismatch = records
        .iter()
        .find(|record| record.kind == DiscrepancyKind::Mismatch)
        .unwrap();
    assert!(!mismatch.values_redacted);
    assert_eq!(mismatch.columns[0].origin, secret_hex());
    assert_eq!(
        mismatch.columns[0].target,
        RawCell::new(b"tampered".to_vec()).to_string()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sec_002_a_redacted_value_is_a_stable_digest_rather_than_a_placeholder() {
    // Why a digest and not `<redacted>`: two rows wrong in the same way must still look the same,
    // which is what makes a redacted report worth reading at all.
    let run = run_reported(
        vec![record(1, SECRET), record(2, SECRET), record(3, "other")],
        vec![
            (key(1), Row::new(vec![int(1), text("t")])),
            (key(2), Row::new(vec![int(2), text("t")])),
            (key(3), Row::new(vec![int(3), text("t")])),
        ],
        ReportFormat::Ndjson,
        true,
    )
    .await;
    let text = run.report_text();
    let mut origins: Vec<String> = parse_ndjson(&text)
        .into_iter()
        .map(|record| record.columns[0].origin.clone())
        .collect();
    origins.sort();
    origins.dedup();
    assert_eq!(origins.len(), 2, "two distinct values, two digests: {text}");
    assert!(origins.iter().all(|o| o.starts_with(REDACTED_PREFIX)));
}

#[tokio::test(flavor = "multi_thread")]
async fn val_013_a_null_is_reported_as_null_in_both_modes() {
    // Null-ness is metadata, not content — the same judgement `VAL-017` makes for the diff log —
    // because "the target is empty" and "the target holds something else" are different problems.
    for redact in [true, false] {
        let run = run_reported(
            vec![record(1, "value")],
            vec![(key(1), Row::new(vec![int(1), RawCell::NULL]))],
            ReportFormat::Ndjson,
            redact,
        )
        .await;
        let text = run.report_text();
        let records = parse_ndjson(&text);
        assert_eq!(records[0].columns[0].target, NULL_VALUE, "{text}");
        assert_ne!(records[0].columns[0].origin, NULL_VALUE, "{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn val_013_the_json_report_is_one_closed_array() {
    let run = seeded(ReportFormat::Json, true).await;
    let text = run.report_text();
    let parsed: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{text}: {e}"));
    assert_eq!(parsed.as_array().map(Vec::len), Some(2), "{text}");
    assert!(text.starts_with("[\n  {"), "{text}");
    assert!(text.ends_with("\n]\n"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn val_013_a_run_that_finds_nothing_still_writes_a_readable_report() {
    // An empty report and a missing report are different statements, and only one of them says
    // "this run looked".
    let run = run_reported(
        vec![record(1, "a")],
        vec![(key(1), Row::new(vec![int(1), text("a")]))],
        ReportFormat::Json,
        true,
    )
    .await;
    assert_eq!(run.count(CounterKind::Valid), 1);
    let text = run.report_text();
    assert_eq!(text, "[]\n");
    assert_eq!(
        serde_json::from_str::<Vec<DiscrepancyRecord>>(&text)
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn val_013_the_csv_report_has_a_header_and_one_row_per_differing_column() {
    let run = seeded(ReportFormat::Csv, true).await;
    let text = run.report_text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "run_id,range_min,range_max,key,kind,column,origin,target,error"
    );
    assert_eq!(lines.len(), 3, "a header, a mismatch and a missing: {text}");
    assert!(
        lines.iter().any(|line| line.contains(",mismatch,data,")),
        "{text}"
    );
    // The missing row has no columns and still gets a row: a report in which a missing row simply
    // did not appear would be worse than no report.
    assert!(
        lines.iter().any(|line| line.ends_with(",missing,,,,")),
        "{text}"
    );
    assert_eq!(
        run.report.records(),
        2,
        "records count discrepancies, not rows"
    );
}

#[test]
fn val_013_a_csv_field_is_quoted_when_it_has_to_be() {
    // The key of a text partition key can contain anything, and a comma in an unquoted field
    // silently shifts every column after it.
    assert_eq!(super::report::csv_field("plain"), "plain");
    assert_eq!(super::report::csv_field("a,b"), "\"a,b\"");
    assert_eq!(super::report::csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(super::report::csv_field("two\nlines"), "\"two\nlines\"");
}

#[tokio::test(flavor = "multi_thread")]
async fn val_013_no_report_is_written_when_the_format_is_none() {
    // The default. A report nobody asked for must not appear, and must not cost the run anything.
    let run = seeded(ReportFormat::None, true).await;
    assert!(!run.report.is_enabled());
    assert_eq!(run.report.records(), 0);
    assert!(run.report_text().is_empty());
    assert!(run.report.reference().is_none());
    // The findings still reached the diff log, which is not optional.
    assert_eq!(run.diff_lines().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn val_013_a_corrected_row_is_recorded_as_corrected_rather_than_twice() {
    let (src, dst) = simple();
    let run = run_reporting(
        FakeOrigin::of(vec![record(1, SECRET), record(2, "b")]),
        FakeTarget::with(vec![(key(1), Row::new(vec![int(1), text("tampered")]))]),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings {
            autocorrect: Autocorrect {
                missing: true,
                mismatch: true,
                missing_counter: false,
            },
            target_is_counter: false,
        },
        1000,
        FilterChain::new(),
        Arc::new(DiscrepancyReport::in_memory(
            RunId::from_raw(7),
            ReportFormat::Ndjson,
            true,
        )),
        None,
    )
    .await;

    let text = run.report_text();
    let mut kinds: Vec<DiscrepancyKind> = parse_ndjson(&text)
        .into_iter()
        .map(|record| record.kind)
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        vec![
            DiscrepancyKind::CorrectedMissing,
            DiscrepancyKind::CorrectedMismatch
        ],
        "a repaired row appears once, saying it was repaired: {text}"
    );
    assert_eq!(run.count(CounterKind::CorrectedMissing), 1);
    assert_eq!(run.count(CounterKind::CorrectedMismatch), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn val_013_the_report_and_the_diff_log_agree_about_which_rows_are_wrong() {
    // The two sinks are fed from one comparison; a discrepancy in one is a discrepancy in the
    // other, identified by the same rendering of the same key.
    let run = seeded(ReportFormat::Ndjson, true).await;
    let report_keys: Vec<String> = parse_ndjson(&run.report_text())
        .into_iter()
        .map(|record| record.key)
        .collect();
    let lines = run.diff_lines().join("\n");
    for key in &report_keys {
        assert!(
            lines.contains(key.as_str()),
            "{key} is missing from {lines}"
        );
    }
    assert_eq!(report_keys.len(), run.diff_lines().len());
}

#[tokio::test(flavor = "multi_thread")]
async fn met_030_each_finding_is_published_with_the_key_fingerprinted() {
    // The third sink. The bus redacts the key at construction, so the event stream carries a
    // correlation token and never a value — in either report mode.
    let bus = Arc::new(EventBus::new(RunId::from_raw(7), "node-a"));
    let mut events = bus.subscribe();
    let (src, dst) = simple();
    let run = run_reporting(
        FakeOrigin::of(vec![record(1, SECRET), record(2, "gone")]),
        FakeTarget::with(vec![(key(1), Row::new(vec![int(1), text("tampered")]))]),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings::read_only(),
        1000,
        FilterChain::new(),
        // Values in the file, so the test can show they still do not reach the bus.
        Arc::new(DiscrepancyReport::in_memory(
            RunId::from_raw(7),
            ReportFormat::Ndjson,
            false,
        )),
        Some(Arc::clone(&bus)),
    )
    .await;

    assert_eq!(bus.redaction(), Redaction::Fingerprint);
    let mut published = Vec::new();
    while let Ok(Some(event)) = events.try_recv() {
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains(SECRET), "{json}");
        assert!(!json.contains(&secret_hex()), "{json}");
        assert!(json.contains("fingerprint"), "{json}");
        if let EventPayload::Discrepancy { kind, columns, .. } = event.payload {
            published.push((kind, columns));
        }
    }
    published.sort();
    assert_eq!(
        published,
        vec![
            (DiscrepancyKind::Missing, Vec::new()),
            (DiscrepancyKind::Mismatch, vec!["data".to_owned()]),
        ]
    );
    // And the values the bus withheld are in the report, which is the sanctioned route.
    assert!(run.report_text().contains(&secret_hex()));
}

#[test]
fn met_033_the_report_hands_the_summary_a_pointer_to_itself() {
    let report = DiscrepancyReport::in_memory(RunId::from_raw(7), ReportFormat::Ndjson, true);
    let reference = report.reference().unwrap();
    assert_eq!(reference.format, "ndjson");
    assert_eq!(reference.records, 0);
    assert!(reference.values_redacted, "SEC-002");
    assert_eq!(reference.path, report.path());
}

#[test]
fn val_013_an_unwritable_report_is_a_startup_error_not_a_surprise_at_the_end() {
    // A path whose *parent* is a regular file, which no platform will create a directory beneath.
    // `/dev/null/nested` was the obvious spelling and is Unix-only: Windows has no `/dev/null`, so
    // the path reads as an ordinary relative directory and the open succeeds.
    let file = tempfile::NamedTempFile::new().unwrap();
    let unwritable = file.path().join("nested").join("report.json");

    let error = DiscrepancyReport::open(RunId::from_raw(1), ReportFormat::Json, &unwritable, true)
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config);
    assert!(error.to_string().contains("VAL-013"), "{error}");
    assert!(
        error.to_string().contains("validate.report.path"),
        "{error}"
    );
}

#[test]
fn val_013_a_disabled_report_touches_the_filesystem_not_at_all() {
    // `format = none` must not create — let alone truncate — the file at the configured path.
    // The path is one `open` would fail on, so a report that quietly opened it anyway could not
    // pass this test on any platform.
    let file = tempfile::NamedTempFile::new().unwrap();
    let unwritable = file.path().join("nested").join("report.json");

    let report =
        DiscrepancyReport::open(RunId::from_raw(1), ReportFormat::None, &unwritable, true).unwrap();
    assert!(!report.is_enabled());
    report.finish().unwrap();
    assert!(
        !unwritable.exists(),
        "a disabled report must not create {}",
        unwritable.display()
    );
}

// ---------------------------------------------------------------------------------------------
// VAL-015 — sampling and keys-only
// ---------------------------------------------------------------------------------------------

#[test]
fn val_015_sample_is_sugar_for_the_token_coverage_percent_property() {
    let mut config = CdmConfig::default();
    assert_eq!(config.filter.token_coverage_percent, 100);

    sample_percent(&mut config, 5).unwrap();
    assert_eq!(config.filter.token_coverage_percent, 5);
    // No second setting is disturbed: the flag is one property, so `TOK-005` does the sampling.
    assert_eq!(config, {
        let mut expected = CdmConfig::default();
        expected.filter.token_coverage_percent = 5;
        expected
    });
}

#[test]
fn val_015_a_sample_outside_one_to_a_hundred_is_refused() {
    let mut config = CdmConfig::default();
    for percent in [0, 101, 255] {
        let error = sample_percent(&mut config, percent).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("VAL-015"), "{error}");
    }
    // And nothing was changed on the way out.
    assert_eq!(config.filter.token_coverage_percent, 100);
    sample_percent(&mut config, 100).unwrap();
    sample_percent(&mut config, 1).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn val_015_a_keys_only_run_compares_existence_and_nothing_else() {
    let (src, dst) = simple();
    let run = run_with(
        FakeOrigin::of(vec![record(1, SECRET), record(2, "gone")]),
        // Row 1 is present and completely different; row 2 is absent.
        FakeTarget::with(vec![(key(1), Row::new(vec![int(1), text("tampered")]))]),
        plan_for(&src, &dst, &MappingOptions::default()).with_keys_only(true),
        ValidateSettings::read_only(),
        1000,
    )
    .await;

    assert_eq!(run.count(CounterKind::Read), 2);
    assert_eq!(
        run.count(CounterKind::Valid),
        1,
        "the row is there, which is all a keys-only run claims"
    );
    assert_eq!(run.count(CounterKind::Missing), 1);
    assert_eq!(
        run.count(CounterKind::Mismatch),
        0,
        "a keys-only run structurally cannot report a mismatch"
    );
    // The lookups still happen: existence is a question only the target can answer.
    assert_eq!(run.target.ops().len(), 2);
    assert_eq!(*run.verdict.as_ref().unwrap(), RangeVerdict::Diff);
}

#[tokio::test(flavor = "multi_thread")]
async fn val_015_a_full_run_over_the_same_data_finds_what_keys_only_missed() {
    // The pair that makes the trade-off explicit: same rows, same fixture, one comparison mode
    // apart. A keys-only pass is a reason to run a full validation, not a substitute for one.
    let (src, dst) = simple();
    let full = run_with(
        FakeOrigin::of(vec![record(1, SECRET)]),
        FakeTarget::with(vec![(key(1), Row::new(vec![int(1), text("tampered")]))]),
        plan_for(&src, &dst, &MappingOptions::default()),
        ValidateSettings::read_only(),
        1000,
    )
    .await;
    assert_eq!(full.count(CounterKind::Mismatch), 1);
    assert_eq!(full.count(CounterKind::Valid), 0);

    let keys_only = run_with(
        FakeOrigin::of(vec![record(1, SECRET)]),
        FakeTarget::with(vec![(key(1), Row::new(vec![int(1), text("tampered")]))]),
        plan_for(&src, &dst, &MappingOptions::default()).with_keys_only(true),
        ValidateSettings::read_only(),
        1000,
    )
    .await;
    assert_eq!(keys_only.count(CounterKind::Mismatch), 0);
    assert_eq!(keys_only.count(CounterKind::Valid), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn val_015_keys_only_still_repairs_a_missing_row_when_asked_to() {
    let (src, dst) = simple();
    let run = run_with(
        FakeOrigin::of(vec![record(1, "a")]),
        FakeTarget::with(Vec::new()),
        plan_for(&src, &dst, &MappingOptions::default()).with_keys_only(true),
        ValidateSettings {
            autocorrect: Autocorrect {
                missing: true,
                mismatch: true,
                missing_counter: false,
            },
            target_is_counter: false,
        },
        1000,
    )
    .await;
    assert_eq!(run.count(CounterKind::CorrectedMissing), 1);
    assert_eq!(run.target.writes(), vec!["write:(0x00000001)"]);
    assert_eq!(*run.verdict.as_ref().unwrap(), RangeVerdict::DiffCorrected);
}

// ---------------------------------------------------------------------------------------------
// FEA-020..FEA-023 — an exploded map is validated one entry at a time
// ---------------------------------------------------------------------------------------------

/// SIT `features/02_explode_map`, in miniature: `key text PRIMARY KEY, value text,
/// fruits map<text,int>` exploded into `((key), fruit)` with the quantity in `fruit_qty`.
fn explode_tables() -> (TableSchema, TableSchema) {
    (
        table(
            "src",
            vec![
                column("key", "text", ColumnKind::PartitionKey, 0),
                column("value", "text", ColumnKind::Regular, -1),
                column("fruits", "map<text, int>", ColumnKind::Regular, -1),
            ],
        ),
        table(
            "dst",
            vec![
                column("key", "text", ColumnKind::PartitionKey, 0),
                column("fruit", "text", ColumnKind::Clustering, 0),
                column("value", "text", ColumnKind::Regular, -1),
                column("fruit_qty", "int", ColumnKind::Regular, -1),
            ],
        ),
    )
}

fn explode_options() -> MappingOptions {
    MappingOptions {
        explode_map: Some((
            "fruits".to_owned(),
            "fruit".to_owned(),
            "fruit_qty".to_owned(),
        )),
        ..MappingOptions::default()
    }
}

/// The explode map and the key plan a validate run is built with, resolved from the same tables.
fn explode_feature() -> ValidateExplode {
    let (src, dst) = explode_tables();
    let mapping = ColumnMapping::resolve(&src, &dst, &explode_options()).unwrap();
    let select = cdm_cql::statement::TargetSelectByPk::new(&mapping).unwrap();
    let keys = TargetKeyPlan::resolve(
        &mapping,
        &select,
        cdm_cql::statement::MissingKeyPolicy::default(),
    )
    .unwrap();
    let config: cdm_core::EffectiveConfig = [
        ("feature.explode_map.origin_column", "fruits"),
        ("feature.explode_map.target_key_column", "fruit"),
        ("feature.explode_map.target_value_column", "fruit_qty"),
    ]
    .into_iter()
    .collect();
    let schema = FeatureSchema::new(
        TableFacts::from_view(
            &cdm_feature::table_view(
                cdm_core::TableRef::new("ks", "src"),
                &[
                    ("key", "text"),
                    ("value", "text"),
                    ("fruits", "map<text, int>"),
                ],
            ),
            &["key"],
        )
        .unwrap(),
        TableFacts::from_view(
            &cdm_feature::table_view(
                cdm_core::TableRef::new("ks", "dst"),
                &[
                    ("key", "text"),
                    ("fruit", "text"),
                    ("value", "text"),
                    ("fruit_qty", "int"),
                ],
            ),
            &["key", "fruit"],
        )
        .unwrap(),
    );
    let plan = cdm_feature::ExplodeMap::load(&config)
        .resolve(&schema, &planner(&[]))
        .unwrap();
    ValidateExplode::new(plan, keys)
}

/// A `map<text,int>` cell, in wire order.
fn fruit_map(entries: &[(&str, i32)]) -> RawCell {
    let mut out = i32::try_from(entries.len()).unwrap().to_be_bytes().to_vec();
    for (name, qty) in entries {
        out.extend_from_slice(&i32::try_from(name.len()).unwrap().to_be_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&4_i32.to_be_bytes());
        out.extend_from_slice(&qty.to_be_bytes());
    }
    RawCell::new(out)
}

/// One origin record, keyed as `CqlRowSource` keys it: the exploded component is left null.
fn explode_record(key_value: &str, value: &str, fruits: &[(&str, i32)]) -> Record {
    Record::new(
        PrimaryKey::new(vec![text(key_value), RawCell::NULL]),
        Row::new(vec![text(key_value), text(value), fruit_map(fruits)]),
    )
}

/// The target row the migration wrote for one entry, and the key it wrote it under.
fn exploded_target(key_value: &str, fruit: &str, value: &str, qty: i32) -> (PrimaryKey, Row) {
    (
        PrimaryKey::new(vec![text(key_value), text(fruit)]),
        Row::new(vec![text(key_value), text(fruit), text(value), int(qty)]),
    )
}

async fn run_exploding(
    origin: Vec<Record>,
    target: Vec<(PrimaryKey, Row)>,
    settings: ValidateSettings,
) -> Run {
    let (src, dst) = explode_tables();
    let diff = Arc::new(DiffLog::in_memory());
    let target = FakeTarget::with(target);
    let job = ValidateJob::new(
        FakeOrigin::of(origin),
        Arc::clone(&target) as Arc<dyn RowSink>,
        Arc::new(plan_for(&src, &dst, &explode_options())),
        settings,
        Arc::clone(&diff),
    )
    .with_explode(explode_feature());
    let counters = Arc::new(JobCounters::new(JobKind::Validate));
    let verdict = job.process(&context(&counters, 1000)).await;
    Run {
        verdict,
        counters,
        diff,
        target,
        report: Arc::new(DiscrepancyReport::disabled()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fea_020_validate_looks_up_one_target_row_per_map_entry_not_per_record() {
    // The defect this test was written for: one origin record with a four-entry map produced one
    // target lookup, under a key whose clustering component was null — which `CqlRowSink::fetch`
    // answers as absent without querying, so the row reported MISSING however healthy the target
    // was. READ still counts origin *rows*, exactly as it does on the migrate side.
    let run = run_exploding(
        vec![explode_record(
            "key1",
            "valueA",
            &[("apples", 3), ("bananas", 2)],
        )],
        vec![
            exploded_target("key1", "apples", "valueA", 3),
            exploded_target("key1", "bananas", "valueA", 2),
        ],
        ValidateSettings::read_only(),
    )
    .await;

    assert_eq!(run.count(CounterKind::Read), 1);
    assert_eq!(run.count(CounterKind::Valid), 2);
    assert_eq!(run.count(CounterKind::Missing), 0);
    assert_eq!(run.count(CounterKind::Mismatch), 0);
    // Sorted: the lookups are issued concurrently, so which of them the target records first is
    // not part of the contract. What is, is that both keys are looked up — `apples` and `bananas`,
    // each under its own entry's clustering component, rather than one lookup under a null one.
    let mut fetched = run.target.ops();
    fetched.sort();
    assert_eq!(
        fetched,
        vec![
            "fetch:(0x6b657931, 0x6170706c6573)",
            "fetch:(0x6b657931, 0x62616e616e6173)"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn fea_020_the_exploded_value_column_is_compared_against_the_entry_it_came_from() {
    // SIT `features/02_explode_map`'s `fix` step breaks exactly this: `fruit_qty=999` on one
    // exploded row, which it expects to be counted as a MISMATCH. A comparison that skipped the
    // exploded columns because it "cannot obtain" them would call that row VALID.
    let run = run_exploding(
        vec![explode_record(
            "key1",
            "valueA",
            &[("apples", 3), ("bananas", 2)],
        )],
        vec![
            exploded_target("key1", "apples", "valueA", 999),
            exploded_target("key1", "bananas", "valueA", 2),
        ],
        ValidateSettings::read_only(),
    )
    .await;

    assert_eq!(run.count(CounterKind::Valid), 1);
    assert_eq!(run.count(CounterKind::Mismatch), 1);
    let detail = run.diff_lines().join("\n");
    assert!(detail.contains("fruit_qty"), "{detail}");
}

#[tokio::test(flavor = "multi_thread")]
async fn fea_023_a_null_or_empty_map_is_one_skipped_record_and_no_lookup_at_all() {
    // FEA-023 on the read side. The migration wrote no target row for these records, so there is
    // none to find; reporting them MISSING would invent a discrepancy the migration was right
    // about.
    let run = run_exploding(
        vec![
            Record::new(
                PrimaryKey::new(vec![text("key1"), RawCell::NULL]),
                Row::new(vec![text("key1"), text("valueA"), RawCell::NULL]),
            ),
            explode_record("key2", "valueB", &[]),
        ],
        Vec::new(),
        ValidateSettings::read_only(),
    )
    .await;

    assert_eq!(run.count(CounterKind::Read), 2);
    assert_eq!(run.count(CounterKind::Skipped), 2);
    assert_eq!(run.count(CounterKind::Missing), 0);
    assert!(run.target.ops().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn fea_022_autocorrect_repairs_a_missing_entry_under_that_entrys_own_key() {
    // `VAL-003` on an exploded run: the record written is the *entry's*, so the repair lands on
    // the row that was missing rather than on a row keyed by a null clustering column. The entry
    // travels on the record, which is what `CqlRowSink::write` binds the two exploded columns from.
    let run = run_exploding(
        vec![explode_record(
            "key1",
            "valueA",
            &[("apples", 3), ("bananas", 2)],
        )],
        vec![exploded_target("key1", "apples", "valueA", 3)],
        ValidateSettings {
            autocorrect: Autocorrect {
                missing: true,
                ..Autocorrect::default()
            },
            target_is_counter: false,
        },
    )
    .await;

    assert_eq!(run.count(CounterKind::Missing), 1);
    assert_eq!(run.count(CounterKind::CorrectedMissing), 1);
    assert_eq!(
        run.target.writes(),
        vec!["write:(0x6b657931, 0x62616e616e6173)"]
    );
    let written = run.target.rows.lock();
    let repaired = written
        .get(&PrimaryKey::new(vec![text("key1"), text("bananas")]))
        .expect("the missing entry was written under its own key");
    assert_eq!(repaired.get(0), Some(&text("key1")));
    assert_eq!(
        run.count(CounterKind::Valid),
        1,
        "the entry the target already had is still VALID"
    );
}
