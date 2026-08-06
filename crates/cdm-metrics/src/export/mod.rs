//! Metric exporters: Prometheus (`MET-020`) and OpenTelemetry OTLP (`MET-021`).
//!
//! # One report, two renderings
//!
//! Both exporters render the same value, [`MetricsReport`]: the counters of `MET-001`, the
//! instruments of `MET-010` and the progress of `MET-011`, plus the closed identity label set of
//! [`MetricLabels`]. Collecting once and rendering twice keeps the two exports consistent with
//! each other — and with `GET /v1/runs/{id}/metrics`, which serves the same report as JSON — so
//! that an operator comparing a Prometheus graph against an OTLP dashboard is not debugging two
//! different collectors.
//!
//! # These exporters render; they do not transmit
//!
//! Neither exporter opens a socket. [`prometheus::render`] returns the exposition text that
//! `GET /metrics` serves, and [`otlp::OtlpExporter`] produces an OTLP payload and hands it to an
//! [`OtlpTransport`] that somebody else implements.
//!
//! That is not squeamishness, it is the dependency graph. `ARCHITECTURE.md` §3 places the
//! exporters in `cdm-metrics`, and `AGENTS.md` says only `cdm-api` may depend on HTTP crates —
//! and OTLP is HTTP or gRPC, whichever encoding is chosen. `SPEC.md` does not notice the
//! contradiction. Splitting at the transport resolves it the way the rest of the workspace
//! resolves the same shape of problem: `cdm-config` takes a `SchemaProvider` rather than a driver
//! session, `cdm-engine` takes a `ClusterTopology` rather than a cluster. The payloads are
//! testable without a collector, and the crate that is allowed to make network calls makes them.
//!
//! # Which counter accounting is exported
//!
//! **Committed** (`MET-004`), always. An exported counter is a run total, and a run total that
//! included the interim work of ranges still in flight would double-count them the moment those
//! ranges were flushed and merged — and would count the work of a range that later failed and was
//! re-planned. Rates are the opposite (see [`RateMeter`](crate::RateMeter)), and the two must not
//! be confused: `met_020_exported_counters_are_the_committed_totals` pins this choice, because
//! reading the wrong level is exactly the mistake Java makes in `MIG-004` and `ENG-008`.

pub mod otlp;
pub mod prometheus;

use std::collections::BTreeMap;
use std::time::Instant;

use cdm_core::{JobKind, MetricsSnapshot, RunId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::instrument::{InstrumentSnapshot, Instruments};
use crate::label::MetricLabels;
use crate::progress::{Progress, ProgressTracker};
use crate::registry::{CounterView, JobCounters};
use crate::CounterKind;

pub use otlp::{MemoryTransport, OtlpExporter, OtlpSignal, OtlpTransport, SpanKind, SpanRecord};
pub use prometheus::{PrometheusExporter, METRIC_PREFIX};

/// Everything an exporter publishes at one instant.
///
/// Serialisable as it stands: this is also the body of `GET /v1/runs/{id}/metrics` (`API-003`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsReport {
    /// The run, job and node these values belong to, and the table if one is resolved.
    ///
    /// The closed set of `MET-020`, and the only place a configured string appears in a report.
    pub labels: MetricLabels,
    /// When the report was collected. RFC 3339 UTC on the wire (`NFR-007`).
    pub taken_at: DateTime<Utc>,
    /// The counters of `MET-001`, under their `SCREAMING_SNAKE_CASE` names, at the
    /// [`CounterView::Committed`] level.
    pub counters: BTreeMap<String, u64>,
    /// The instruments of `MET-010`.
    pub instruments: InstrumentSnapshot,
    /// The progress of `MET-011`, when the plan is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<Progress>,
}

impl MetricsReport {
    /// The counter level every exporter publishes. See the module documentation.
    pub const VIEW: CounterView = CounterView::Committed;

    /// Collects a report from the live registry, instruments and progress tracker.
    #[must_use]
    pub fn collect(
        labels: MetricLabels,
        counters: &JobCounters,
        instruments: &Instruments,
        progress: Option<&ProgressTracker>,
        now: Instant,
        taken_at: DateTime<Utc>,
    ) -> Self {
        let snapshot = counters.snapshot(labels.run_id(), taken_at);
        Self::from_parts(
            labels,
            &snapshot,
            instruments.snapshot_at(now),
            progress.map(|tracker| tracker.snapshot_at(now)),
        )
    }

    /// Assembles a report from parts already snapshotted.
    ///
    /// The shape a [`MetricsExporter`](cdm_core::MetricsExporter) plugin sees: `PLG-006` hands it
    /// a [`MetricsSnapshot`], and the instruments and progress come from the exporter's own
    /// handles on them.
    #[must_use]
    pub fn from_parts(
        labels: MetricLabels,
        counters: &MetricsSnapshot,
        instruments: InstrumentSnapshot,
        progress: Option<Progress>,
    ) -> Self {
        Self {
            taken_at: counters.taken_at,
            counters: counters.counters.clone(),
            labels,
            instruments,
            progress,
        }
    }

    /// The run these values belong to, as the labels record it.
    ///
    /// [`MetricsReport::from_parts`] takes both a label set and a [`MetricsSnapshot`], which each
    /// carry a run id and a job. The labels win: they are what the series are published under, and
    /// a report whose labels said one thing and whose body said another would be worse than either.
    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.labels.run_id()
    }

    /// The job that produced them, as the labels record it.
    #[must_use]
    pub const fn job(&self) -> JobKind {
        self.labels.job()
    }

    /// The counters, resolved to their [`CounterKind`] and in `MET-001` rendering order.
    ///
    /// A name the counter vocabulary does not know is skipped rather than exported under a made-up
    /// metric name: the counter set is closed (`MET-001`), and a snapshot carrying something else
    /// came from somewhere that had no business inventing one.
    #[must_use]
    pub fn typed_counters(&self) -> Vec<(CounterKind, u64)> {
        CounterKind::ALL
            .into_iter()
            .filter_map(|kind| self.counters.get(kind.as_str()).map(|value| (kind, *value)))
            .collect()
    }
}

/// One line of documentation per counter, for the `# HELP` line and the OTLP metric description.
///
/// Written here rather than derived from the rustdoc on [`CounterKind`], because an exported
/// description is read by people who have never seen the source and should not contain a
/// requirement id.
#[must_use]
pub const fn counter_help(kind: CounterKind) -> &'static str {
    match kind {
        CounterKind::Read => "Rows read from the origin.",
        CounterKind::Write => "Rows written to the target and flushed.",
        CounterKind::Mismatch => "Rows present on both sides whose values differ.",
        CounterKind::CorrectedMismatch => "Mismatched rows that autocorrect rewrote.",
        CounterKind::Missing => "Rows present on the origin and absent from the target.",
        CounterKind::CorrectedMissing => "Missing rows that autocorrect inserted.",
        CounterKind::Valid => "Rows that compared equal, or that passed the guardrail.",
        CounterKind::Skipped => "Rows a filter rejected, or that produced no statement.",
        CounterKind::Large => "Rows exceeding the configured guardrail threshold.",
        CounterKind::Error => "Rows a failed token range could not account for.",
        CounterKind::Unflushed => "Writes issued but not yet flushed.",
        CounterKind::PartitionsPassed => "Token ranges that completed successfully.",
        CounterKind::PartitionsFailed => "Token ranges that failed.",
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
    use std::time::Duration;

    use cdm_core::{RunStatus, Side, TableRef, TokenRange};

    use crate::instrument::{Operation, RetryCause};

    use super::*;

    /// A report with something recorded in every family, for the exporter snapshot tests.
    pub(crate) fn sample_report() -> MetricsReport {
        let start = Instant::now();
        let labels = MetricLabels::new(
            RunId::from_raw(1_712_345_678_901_234),
            JobKind::Migrate,
            "node-a",
        )
        .with_table(&TableRef::new("target_ks", "orders"));

        let counters = JobCounters::new(JobKind::Migrate);
        counters.increment_by(counters.counter(CounterKind::Read).unwrap(), 1_000_000);
        counters.increment_by(counters.counter(CounterKind::Write).unwrap(), 999_998);
        counters.increment_by(counters.counter(CounterKind::Skipped).unwrap(), 2);
        counters.increment_by(counters.counter(CounterKind::PartitionsPassed).unwrap(), 3);
        counters.flush();

        let instruments = Instruments::new(start);
        instruments.rows(Side::Origin).mark_at(1_000_000, start);
        instruments.rows(Side::Target).mark_at(999_998, start);
        instruments.bytes(Side::Origin).mark_at(64_000_000, start);
        instruments
            .latency(Side::Origin, Operation::RangeRead)
            .record_duration(Duration::from_millis(8));
        instruments
            .latency(Side::Target, Operation::Write)
            .record_duration(Duration::from_millis(3));
        instruments.inflight(Side::Target).increment();
        instruments.batch_size().record(5);
        instruments.retry(RetryCause::Overloaded);
        instruments
            .ratelimit_wait(Side::Origin)
            .record_duration(Duration::from_micros(500));

        let ranges = TokenRange::MURMUR3_FULL.split(4).unwrap();
        let progress = ProgressTracker::by_token_span(&ranges, start);
        for planned in ranges.iter().take(3) {
            progress.range_completed(*planned, RunStatus::Pass);
        }
        progress.range_started(ranges[3]);

        MetricsReport::collect(
            labels,
            &counters,
            &instruments,
            Some(&progress),
            start + Duration::from_secs(30),
            DateTime::UNIX_EPOCH + Duration::from_secs(1_712_345_678),
        )
    }

    #[test]
    fn met_020_exported_counters_are_the_committed_totals() {
        // The choice the module documentation defends. A range that has read rows but has not
        // completed contributes nothing to an exported total; the rate meters are what report it
        // as it happens.
        let start = Instant::now();
        let labels = MetricLabels::new(RunId::from_raw(1), JobKind::Migrate, "node-a");
        let counters = JobCounters::new(JobKind::Migrate);
        let read = counters.counter(CounterKind::Read).unwrap();
        counters.increment_by(read, 500);

        let instruments = Instruments::new(start);
        let in_flight = MetricsReport::collect(
            labels.clone(),
            &counters,
            &instruments,
            None,
            start,
            DateTime::UNIX_EPOCH,
        );
        assert_eq!(MetricsReport::VIEW, CounterView::Committed);
        assert_eq!(in_flight.counters.get("READ"), Some(&0));

        counters.flush();
        let committed = MetricsReport::collect(
            labels,
            &counters,
            &instruments,
            None,
            start,
            DateTime::UNIX_EPOCH,
        );
        assert_eq!(committed.counters.get("READ"), Some(&500));
    }

    #[test]
    fn met_020_only_the_counters_the_job_registered_are_exported() {
        let report = sample_report();
        let names: Vec<&str> = report
            .typed_counters()
            .into_iter()
            .map(|(kind, _)| kind.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                "READ",
                "WRITE",
                "SKIPPED",
                "ERROR",
                "UNFLUSHED",
                "PARTITIONS_PASSED",
                "PARTITIONS_FAILED",
            ],
            "a migrate run exports migrate's counters, in MET-001 order"
        );
        assert!(!report.counters.contains_key("MISMATCH"));
    }

    #[test]
    fn met_020_every_counter_has_a_description_that_names_no_requirement() {
        for kind in CounterKind::ALL {
            let help = counter_help(kind);
            assert!(help.ends_with('.'), "{kind}: {help}");
            assert!(!help.contains("MET-"), "{kind}: {help}");
            assert!(!help.contains('\n'), "{kind}: {help}");
        }
    }

    #[test]
    fn met_020_a_report_round_trips_through_json() {
        // `API-003` serves this as the body of `GET /v1/runs/{id}/metrics`.
        let report = sample_report();
        let json = serde_json::to_string(&report).unwrap();
        let parsed: MetricsReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.counters, report.counters);
        assert_eq!(parsed.labels, report.labels);
        assert_eq!(parsed.run_id(), report.run_id());
        assert_eq!(parsed.progress, report.progress);
        // Totals and percentiles are integers and must survive exactly; the exponentially-weighted
        // rates are `f64`, and a JSON round trip is allowed to disturb their last bit.
        assert_eq!(
            parsed.instruments.origin.rows.total,
            report.instruments.origin.rows.total
        );
        assert_eq!(
            parsed.instruments.origin.latency,
            report.instruments.origin.latency
        );
        assert_eq!(parsed.instruments.retries, report.instruments.retries);
        assert_eq!(parsed.instruments.uptime, report.instruments.uptime);
    }
}
