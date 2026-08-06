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
use cdm_config::model::Autocorrect;
use cdm_core::{
    CdmError, ErrorKind, JobKind, Plugin, PrimaryKey, RawCell, Record, Row, RowSink, RowSource,
    RowStream, RunId, TokenRange,
};
use cdm_cql::schema::{ClusteringOrder, ColumnKind, ColumnMeta, TableSchema};
use cdm_cql::statement::{ColumnMapping, MappingOptions};
use cdm_feature::{ExtractJson, FeatureSchema, FilterChain, TableFacts};
use cdm_metrics::{CounterKind, CounterView, JobCounters};
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
}

impl Run {
    fn count(&self, kind: CounterKind) -> u64 {
        self.counters.count_of(kind, CounterView::Interim)
    }

    fn diff_lines(&self) -> Vec<String> {
        self.diff.captured()
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
    let diff = Arc::new(DiffLog::in_memory());
    let job = ValidateJob::new(
        origin,
        Arc::clone(&target) as Arc<dyn RowSink>,
        Arc::new(plan),
        settings,
        Arc::clone(&diff),
    )
    .with_filters(filters);
    let counters = Arc::new(JobCounters::new(JobKind::Validate));
    let ctx = context(&counters, fetch_size);
    let verdict = job.process(&ctx).await;
    Run {
        verdict,
        counters,
        diff,
        target,
    }
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
