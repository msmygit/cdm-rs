//! The guardrail job, driven end to end through the real scheduler over a scripted origin.
//!
//! # Why a scripted reader rather than a cluster
//!
//! Everything `GRD-001`..`GRD-004` asserts is about *which counter moves* and *what is reported*
//! for a given row, and a row here is a list of lengths. A scripted [`OriginRows`] states those
//! directly, which makes each case one line of intent rather than a table of seeded CQL — and
//! removes the only thing that could make these tests flaky. The claim that a real cluster's rows
//! reduce to the same lengths is a different claim, and `tests/guardrail_it.rs` makes it against
//! Cassandra.

use std::sync::atomic::{AtomicUsize, Ordering};

use cdm_core::{ColumnRef, PrimaryKey, RawCell, Row, RunId, TableRef, TableView};
use cdm_feature::{Guardrail, TableFacts};
use cdm_metrics::CounterView;
use parking_lot::Mutex;

use crate::planner::{Partitioner, Planner, PlannerSettings};
use crate::scheduler::{NoopObserver, Scheduler, SchedulerSettings};

use super::*;

// =================================================================================================
// Fixtures
// =================================================================================================

/// The table `SIT/features/05_guardrail` uses.
fn origin_facts() -> TableFacts {
    TableFacts::from_view(
        &TableView::new(
            TableRef::new("origin", "feature_guardrail"),
            vec![
                ColumnRef::new("key", "text"),
                ColumnRef::new("value", "text"),
                ColumnRef::new("fruits", "map<text, text>"),
            ],
        ),
        &["key"],
    )
    .unwrap()
}

fn guardrail(kb: f64) -> ColumnSizeGuardrail {
    let config: cdm_core::EffectiveConfig = [("feature.guardrail.column_size_kb", kb.to_string())]
        .into_iter()
        .collect();
    Guardrail::load(&config)
        .unwrap()
        .resolve(&origin_facts())
        .unwrap()
}

fn key(name: &str) -> PrimaryKey {
    PrimaryKey::new(vec![RawCell::new(name.as_bytes().to_vec())])
}

/// A row of the SIT table, given its three column lengths.
fn row(name: &str, lengths: [usize; 3]) -> RowSizes {
    RowSizes::new(key(name), lengths)
}

/// An origin that hands back the same scripted rows for every range, and counts its scans.
///
/// It has no way to write anything, which is the point: [`OriginRows`] is the entire surface the
/// job is given (`GRD-001`).
struct ScriptedOrigin {
    rows: Vec<RowSizes>,
    scans: AtomicUsize,
    fetch_sizes: Mutex<Vec<u32>>,
    fail_after: Option<usize>,
}

impl ScriptedOrigin {
    fn new(rows: Vec<RowSizes>) -> Arc<Self> {
        Arc::new(Self {
            rows,
            scans: AtomicUsize::new(0),
            fetch_sizes: Mutex::new(Vec::new()),
            fail_after: None,
        })
    }

    /// An origin whose scan fails after `rows` rows, for the `ENG-008` case.
    fn failing_after(rows: Vec<RowSizes>, fail_after: usize) -> Arc<Self> {
        Arc::new(Self {
            rows,
            scans: AtomicUsize::new(0),
            fetch_sizes: Mutex::new(Vec::new()),
            fail_after: Some(fail_after),
        })
    }
}

#[async_trait]
impl OriginRows for ScriptedOrigin {
    async fn scan(
        &self,
        _range: TokenRange,
        fetch_size: u32,
    ) -> Result<Box<dyn RowSizeStream>, CdmError> {
        self.scans.fetch_add(1, Ordering::SeqCst);
        self.fetch_sizes.lock().push(fetch_size);
        Ok(Box::new(ScriptedStream {
            remaining: self.rows.clone().into_iter().collect(),
            served: 0,
            fail_after: self.fail_after,
        }))
    }
}

struct ScriptedStream {
    remaining: std::collections::VecDeque<RowSizes>,
    served: usize,
    fail_after: Option<usize>,
}

#[async_trait]
impl RowSizeStream for ScriptedStream {
    async fn next_row(&mut self) -> Result<Option<RowSizes>, CdmError> {
        if self.fail_after == Some(self.served) {
            return Err(CdmError::new(ErrorKind::Read, "injected read timeout"));
        }
        self.served += 1;
        Ok(self.remaining.pop_front())
    }
}

/// A plan of one range, so a case's counters are the job's and not the scheduler's arithmetic.
fn one_range_plan() -> crate::planner::TokenPlan {
    Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(1))
        .plan(RunId::from_raw(1_712_345_678_901_234), None)
        .unwrap()
}

fn settings() -> SchedulerSettings {
    SchedulerSettings::default()
        .with_workers(1)
        .with_ratelimits(0, 0)
        .with_node_id("node-under-test")
}

async fn run(job: GuardrailJob) -> RunReport {
    Scheduler::new(settings())
        .unwrap()
        .run(&one_range_plan(), Arc::new(job), Arc::new(NoopObserver))
        .await
        .unwrap()
}

fn total(report: &RunReport, kind: CounterKind) -> u64 {
    report.counters().count_of(kind, CounterView::Committed)
}

// =================================================================================================
// GRD-001 — the origin, and only the origin
// =================================================================================================

#[test]
fn grd_001_the_job_holds_nothing_that_could_write() {
    // Structural, and asserted here so that a future field which *could* write has to change this
    // test to land: the job's own Debug rendering names its guardrail and nothing else, and the
    // guardrail is a table name, three column names and a threshold.
    let job = GuardrailJob::new(ScriptedOrigin::new(Vec::new()), guardrail(1.0)).unwrap();
    let rendered = format!("{job:?}");
    assert!(
        rendered.starts_with("GuardrailJob { guardrail: "),
        "{rendered}"
    );
    for forbidden in ["session", "upsert", "insert", "target", "sink"] {
        assert!(
            !rendered.to_lowercase().contains(forbidden),
            "a guardrail job must not reach a target: {rendered}"
        );
    }
    assert_eq!(job.job(), JobKind::Guardrail);
    assert_eq!(job.guardrail().table().keyspace(), "origin");
}

#[test]
fn grd_001_a_disabled_guardrail_refuses_to_build_a_job() {
    // Java logs an error and runs anyway, reporting the whole table clean. See
    // docs/MIGRATION_FROM_JAVA.md.
    let disabled = Guardrail::default().resolve(&origin_facts()).unwrap();
    let error = GuardrailJob::new(ScriptedOrigin::new(Vec::new()), disabled).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Config);
    assert_eq!(
        error.context().config_key.as_deref(),
        Some("feature.guardrail.column_size_kb")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn grd_001_the_run_reads_the_origin_once_per_range_at_the_configured_page_size() {
    let origin = ScriptedOrigin::new(vec![row("clean", [8, 6, 74])]);
    let job =
        GuardrailJob::new(Arc::clone(&origin) as Arc<dyn OriginRows>, guardrail(1.0)).unwrap();
    let report = run(job).await;

    assert_eq!(origin.scans.load(Ordering::SeqCst), 1);
    assert_eq!(
        origin.fetch_sizes.lock().as_slice(),
        [SchedulerSettings::default().fetch_size()],
        "ENG-003: the reader is told the configured page size"
    );
    assert_eq!(report.job(), JobKind::Guardrail);
    assert_eq!(report.ranges_passed(), 1);
}

// =================================================================================================
// GRD-002, GRD-003 — the counters
// =================================================================================================

/// The exact fixture of `SIT/features/05_guardrail`: four rows, one clean and three oversized,
/// against a 1 kB threshold. Java's assertion file expects READ 4, VALID 1, LARGE 3.
fn sit_rows() -> Vec<RowSizes> {
    vec![
        row("clean", [5, 6, 74]),
        row("badValue", [8, 1474, 74]),
        row("badMapKey", [9, 6, 1482]),
        row("badMapValue", [11, 6, 1482]),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn grd_003_the_java_sit_fixture_produces_the_java_sit_counts() {
    let origin = ScriptedOrigin::new(sit_rows());
    let job = GuardrailJob::new(origin as Arc<dyn OriginRows>, guardrail(1.0)).unwrap();
    let report = run(job).await;

    assert_eq!(total(&report, CounterKind::Read), 4);
    assert_eq!(total(&report, CounterKind::Valid), 1);
    assert_eq!(total(&report, CounterKind::Large), 3);
    assert_eq!(total(&report, CounterKind::Skipped), 0);
    assert_eq!(total(&report, CounterKind::PartitionsPassed), 1);
    assert_eq!(total(&report, CounterKind::PartitionsFailed), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn grd_002_read_always_equals_large_plus_valid() {
    for threshold in [0.001_f64, 0.5, 1.0, 2.0, 1000.0] {
        let origin = ScriptedOrigin::new(sit_rows());
        let job = GuardrailJob::new(origin as Arc<dyn OriginRows>, guardrail(threshold)).unwrap();
        let report = run(job).await;
        assert_eq!(
            total(&report, CounterKind::Read),
            total(&report, CounterKind::Large) + total(&report, CounterKind::Valid),
            "at {threshold} kB every row is counted exactly once"
        );
        assert_eq!(total(&report, CounterKind::Read), 4);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn grd_002_a_row_just_under_the_threshold_is_valid_and_one_just_over_is_large() {
    let origin = ScriptedOrigin::new(vec![row("under", [4, 1000, 4]), row("over", [4, 1001, 4])]);
    let job = GuardrailJob::new(origin as Arc<dyn OriginRows>, guardrail(1.0)).unwrap();
    let report = run(job).await;

    assert_eq!(total(&report, CounterKind::Valid), 1);
    assert_eq!(total(&report, CounterKind::Large), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn grd_003_a_run_that_found_nothing_ends_and_one_that_found_something_differs() {
    // CLI-004: `Exit::for_run_status` maps ENDED to 0 and DIFF to 1. A guardrail that found
    // oversized columns worked; it is the data that has a problem.
    let clean = ScriptedOrigin::new(vec![row("clean", [5, 6, 74])]);
    let report =
        run(GuardrailJob::new(clean as Arc<dyn OriginRows>, guardrail(1.0)).unwrap()).await;
    assert_eq!(report.status(), RunStatus::Ended);
    assert_eq!(run_status(&report), RunStatus::Ended);

    let dirty = ScriptedOrigin::new(sit_rows());
    let report =
        run(GuardrailJob::new(dirty as Arc<dyn OriginRows>, guardrail(1.0)).unwrap()).await;
    assert_eq!(
        report.status(),
        RunStatus::Ended,
        "the per-range tracking status stays byte-compatible with Java's (TRK-012)"
    );
    assert_eq!(run_status(&report), RunStatus::Diff);
}

#[tokio::test(flavor = "multi_thread")]
async fn grd_003_an_interrupted_run_keeps_its_own_status_over_a_finding() {
    let origin = ScriptedOrigin::new(sit_rows());
    let job = GuardrailJob::new(origin as Arc<dyn OriginRows>, guardrail(1.0)).unwrap();
    let scheduler = Scheduler::new(settings()).unwrap();
    let control = scheduler.control();
    control.stop(crate::scheduler::StopReason::Signal);
    let report = scheduler
        .run(&one_range_plan(), Arc::new(job), Arc::new(NoopObserver))
        .await
        .unwrap();
    assert_eq!(
        run_status(&report),
        RunStatus::Interrupted,
        "an incomplete run's findings are a floor, not an answer"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn grd_001_a_read_failure_fails_the_range_and_costs_no_error_rows() {
    // MET-002 registers no ERROR counter for guardrail, so ENG-008's lost-rows term is zero and a
    // failed range costs the run exactly one PARTITIONS_FAILED.
    let origin = ScriptedOrigin::failing_after(sit_rows(), 2);
    let job = GuardrailJob::new(origin as Arc<dyn OriginRows>, guardrail(1.0)).unwrap();
    let report = run(job).await;

    assert_eq!(report.ranges_failed(), 1);
    assert_eq!(total(&report, CounterKind::PartitionsFailed), 1);
    assert_eq!(total(&report, CounterKind::PartitionsPassed), 0);
    assert_eq!(
        total(&report, CounterKind::Read),
        2,
        "the rows read before the failure are still accounted for"
    );
}

// =================================================================================================
// GRD-004 — the inline guardrail
// =================================================================================================

fn record(name: &str, payload: usize) -> Record {
    Record::new(
        key(name),
        Row::new(vec![
            RawCell::new(name.as_bytes().to_vec()),
            RawCell::new(vec![0_u8; payload]),
            RawCell::NULL,
        ]),
    )
}

#[test]
fn grd_004_only_block_withholds_a_row_and_the_other_modes_only_report() {
    for (mode, blocks) in [("check", false), ("warn", false), ("block", true)] {
        let config: cdm_core::EffectiveConfig = [
            ("feature.guardrail.column_size_kb", "1"),
            ("feature.guardrail.mode", mode),
        ]
        .into_iter()
        .collect();
        let inline = InlineGuardrail::new(
            Guardrail::load(&config)
                .unwrap()
                .resolve(&origin_facts())
                .unwrap(),
        );
        assert!(inline.is_enabled());
        assert_eq!(inline.inspect(&record("big", 4000)), blocks, "mode {mode}");
        assert!(
            !inline.inspect(&record("small", 4)),
            "a clean row is never withheld, whatever the mode"
        );
        assert_eq!(inline.guardrail().mode().as_str(), mode);
    }
}

#[test]
fn grd_004_a_disabled_inline_guardrail_never_blocks_anything() {
    let inline = InlineGuardrail::new(Guardrail::default().resolve(&origin_facts()).unwrap());
    assert!(!inline.is_enabled());
    assert!(!inline.inspect(&record("enormous", 10_000_000)));
}

#[test]
fn grd_004_a_blocked_row_is_counted_skipped_in_every_job_that_can_have_one() {
    // GRD-004 says LARGE; MET-002 does not register LARGE for migrate or validate and MET-003
    // makes reaching for it a startup error. SKIPPED is what both jobs do register, and is already
    // what MIG-002 means by "rejected before the write". See the SPEC correction under GRD-004.
    for job in [JobKind::Migrate, JobKind::Validate] {
        let counters = JobCounters::new(job);
        record_blocked_row(&counters).unwrap();
        record_blocked_row(&counters).unwrap();
        assert_eq!(
            counters.count_of(CounterKind::Skipped, CounterView::Interim),
            2,
            "{job:?}"
        );
        assert!(
            counters.counter(CounterKind::Large).is_err(),
            "{job:?} must not register LARGE, or MET-005's metrics block changes shape"
        );
    }
    // And guardrail's own counters, where LARGE is the registered one, still accept the call.
    let counters = JobCounters::new(JobKind::Guardrail);
    record_blocked_row(&counters).unwrap();
    assert_eq!(
        counters.count_of(CounterKind::Skipped, CounterView::Interim),
        1
    );
}
