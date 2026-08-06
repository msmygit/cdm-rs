//! The Prometheus exposition endpoint (`MET-020`).
//!
//! Renders a [`MetricsReport`] as Prometheus text format 0.0.4 — the body of `GET /metrics`
//! (`API-003`). Metric names are prefixed [`METRIC_PREFIX`] and the identity labels are exactly
//! the closed set of [`MetricLabels`].
//!
//! # Why this is written out rather than delegated
//!
//! There are good Prometheus client crates, and this module does not use one. The reason is
//! `SEC-001`. Every such crate's API is `counter!("name", "label" => value)` with string labels,
//! which is precisely the "serialise whatever you have" path a metrics exporter must not offer:
//! the closed label set of `MET-020` would become a convention enforced by review rather than a
//! type enforced by the compiler. The exposition format itself is a dozen lines of `write!`, the
//! cardinality is fixed by construction, and the output is snapshot-tested. That trade — a page
//! of formatting code in exchange for a label surface that cannot be widened by accident — is
//! worth making in a tool that runs against production data.
//!
//! # Cardinality
//!
//! Every series in this module is one of a fixed number: the counters `MET-002` registers for the
//! job, two sides, four operations, six retry causes, four quantiles, and the range states. There
//! is no path by which a token range or a primary key becomes a label, because no function here
//! accepts one.

use std::fmt::Write as _;

use cdm_core::{CdmError, MetricsSnapshot, Side};
use parking_lot::Mutex;

use crate::instrument::Instruments;
use crate::label::MetricLabels;
use crate::progress::{Progress, ProgressTracker};
use crate::CounterKind;

use super::{counter_help, MetricsReport};

/// The prefix every cdm-rs metric name carries (`MET-020`).
pub const METRIC_PREFIX: &str = "cdm_";

/// The three rate windows, as the `window` label spells them.
const WINDOW_LABELS: [&str; 3] = ["1s", "10s", "60s"];

/// Renders a report as Prometheus exposition text (`MET-020`).
///
/// The output is deterministic: families appear in a fixed order, series within a family follow
/// the declaration order of the enum that produced them, and no map iteration order is observable.
/// A scrape is therefore diffable, which is what makes the snapshot test in this module useful.
///
/// ```
/// use std::time::Instant;
/// use chrono::{DateTime, Utc};
/// use cdm_core::{JobKind, RunId};
/// use cdm_metrics::{export::prometheus, Instruments, JobCounters, MetricLabels, MetricsReport};
///
/// let now = Instant::now();
/// let report = MetricsReport::collect(
///     MetricLabels::new(RunId::from_raw(7), JobKind::Migrate, "node-a"),
///     &JobCounters::new(JobKind::Migrate),
///     &Instruments::new(now),
///     None,
///     now,
///     DateTime::UNIX_EPOCH,
/// );
///
/// let text = prometheus::render(&report);
/// assert!(text.contains("# TYPE cdm_read_total counter"));
/// assert!(text.contains("cdm_read_total{run_id=\"7\",job=\"migrate\",node_id=\"node-a\"} 0"));
/// ```
#[must_use]
pub fn render(report: &MetricsReport) -> String {
    let mut out = String::with_capacity(4_096);
    render_counters(&mut out, report);
    render_throughput(&mut out, report);
    render_latency(&mut out, report);
    render_concurrency(&mut out, report);
    render_retries(&mut out, report);
    if let Some(progress) = &report.progress {
        render_progress(&mut out, report, progress);
    }
    out
}

/// The counters of `MET-001`, one metric family each.
///
/// `UNFLUSHED` is omitted. It is bookkeeping for the flush threshold of `MIG-004` and lives only
/// at the interim level, so its committed value — the level everything here exports — is
/// permanently zero. `MET-005` and `MET-006` omit it from their committed renderings for the same
/// reason, and a `cdm_unflushed_total 0` that never moved would be read as "nothing is buffered",
/// which is not what it would mean.
fn render_counters(out: &mut String, report: &MetricsReport) {
    for (kind, value) in report.typed_counters() {
        if kind == CounterKind::Unflushed {
            continue;
        }
        let name = counter_metric_name(kind);
        family(out, &name, "counter", counter_help(kind));
        series(
            out,
            &name,
            &report.labels.render_prometheus(None, &[]),
            value,
        );
    }
}

/// Rows and bytes per side: cumulative totals plus the three windowed rates of `MET-010`.
fn render_throughput(out: &mut String, report: &MetricsReport) {
    family(
        out,
        "cdm_rows_total",
        "counter",
        "Rows that crossed the wire, per side.",
    );
    for side in [Side::Origin, Side::Target] {
        series(
            out,
            "cdm_rows_total",
            &report.labels.render_prometheus(Some(side), &[]),
            report.instruments.side(side).rows.total,
        );
    }

    family(
        out,
        "cdm_bytes_total",
        "counter",
        "Bytes that crossed the wire, per side.",
    );
    for side in [Side::Origin, Side::Target] {
        series(
            out,
            "cdm_bytes_total",
            &report.labels.render_prometheus(Some(side), &[]),
            report.instruments.side(side).bytes.total,
        );
    }

    // The exponentially-weighted rates are exported alongside the totals rather than instead of
    // them: `rate()` over the total is what a Prometheus query should use, but the TUI
    // (`MET-031`), the API and a single-scrape debugging session all want the run's own view.
    family(
        out,
        "cdm_rows_per_second",
        "gauge",
        "Exponentially-weighted rows per second, per side and averaging window.",
    );
    for side in [Side::Origin, Side::Target] {
        let rates = report.instruments.side(side).rows;
        for (window, value) in WINDOW_LABELS.into_iter().zip([
            rates.per_second_1s,
            rates.per_second_10s,
            rates.per_second_60s,
        ]) {
            float_series(
                out,
                "cdm_rows_per_second",
                &report
                    .labels
                    .render_prometheus(Some(side), &[("window", window)]),
                value,
            );
        }
    }

    family(
        out,
        "cdm_bytes_per_second",
        "gauge",
        "Exponentially-weighted bytes per second, per side and averaging window.",
    );
    for side in [Side::Origin, Side::Target] {
        let rates = report.instruments.side(side).bytes;
        for (window, value) in WINDOW_LABELS.into_iter().zip([
            rates.per_second_1s,
            rates.per_second_10s,
            rates.per_second_60s,
        ]) {
            float_series(
                out,
                "cdm_bytes_per_second",
                &report
                    .labels
                    .render_prometheus(Some(side), &[("window", window)]),
                value,
            );
        }
    }
}

/// Request latency and rate-limiter wait time, as Prometheus summaries.
///
/// A summary rather than a histogram: the recorder keeps 976 buckets (see
/// [`histogram`](crate::instrument::histogram)) and exporting all of them per side and operation
/// would be four thousand series per scrape. Four quantiles, a sum and a count is what a summary
/// is for, and the quantile error is documented where the buckets are.
fn render_latency(out: &mut String, report: &MetricsReport) {
    family(
        out,
        "cdm_request_duration_seconds",
        "summary",
        "Request latency, per side and operation.",
    );
    for side in [Side::Origin, Side::Target] {
        for (operation, snapshot) in report.instruments.side(side).recorded_latencies() {
            let operation = operation.as_str();
            for (quantile, nanos) in snapshot.labelled() {
                float_series(
                    out,
                    "cdm_request_duration_seconds",
                    &report.labels.render_prometheus(
                        Some(side),
                        &[("operation", operation), ("quantile", quantile)],
                    ),
                    seconds(nanos),
                );
            }
            let labels = report
                .labels
                .render_prometheus(Some(side), &[("operation", operation)]);
            float_series(
                out,
                "cdm_request_duration_seconds_sum",
                &labels,
                seconds(snapshot.sum),
            );
            series(
                out,
                "cdm_request_duration_seconds_count",
                &labels,
                snapshot.count,
            );
        }
    }

    family(
        out,
        "cdm_ratelimit_wait_seconds",
        "summary",
        "Time spent waiting for a rate-limit permit, per side.",
    );
    for side in [Side::Origin, Side::Target] {
        let snapshot = report.instruments.side(side).ratelimit_wait;
        if snapshot.is_empty() {
            continue;
        }
        for (quantile, nanos) in snapshot.labelled() {
            float_series(
                out,
                "cdm_ratelimit_wait_seconds",
                &report
                    .labels
                    .render_prometheus(Some(side), &[("quantile", quantile)]),
                seconds(nanos),
            );
        }
        let labels = report.labels.render_prometheus(Some(side), &[]);
        float_series(
            out,
            "cdm_ratelimit_wait_seconds_sum",
            &labels,
            seconds(snapshot.sum),
        );
        series(
            out,
            "cdm_ratelimit_wait_seconds_count",
            &labels,
            snapshot.count,
        );
    }
}

/// In-flight requests and the batch-size distribution.
fn render_concurrency(out: &mut String, report: &MetricsReport) {
    family(
        out,
        "cdm_inflight_requests",
        "gauge",
        "Requests issued and not yet answered, per side.",
    );
    for side in [Side::Origin, Side::Target] {
        signed_series(
            out,
            "cdm_inflight_requests",
            &report.labels.render_prometheus(Some(side), &[]),
            report.instruments.side(side).inflight,
        );
    }

    let batch = report.instruments.batch_size;
    if batch.is_empty() {
        return;
    }
    family(
        out,
        "cdm_batch_size",
        "summary",
        "Rows per executed batch. Pinned at one when the coercion of MIG-021 fires.",
    );
    for (quantile, size) in batch.labelled() {
        series(
            out,
            "cdm_batch_size",
            &report
                .labels
                .render_prometheus(None, &[("quantile", quantile)]),
            size,
        );
    }
    let labels = report.labels.render_prometheus(None, &[]);
    series(out, "cdm_batch_size_sum", &labels, batch.sum);
    series(out, "cdm_batch_size_count", &labels, batch.count);
}

/// Retries, by cause.
fn render_retries(out: &mut String, report: &MetricsReport) {
    family(
        out,
        "cdm_retries_total",
        "counter",
        "Requests retried, by cause.",
    );
    for (cause, count) in report.instruments.retries_labelled() {
        series(
            out,
            "cdm_retries_total",
            &report.labels.render_prometheus(None, &[("cause", cause)]),
            count,
        );
    }
}

/// Progress, ranges by state, and the ETA (`MET-010`, `MET-011`).
fn render_progress(out: &mut String, report: &MetricsReport, progress: &Progress) {
    let labels = report.labels.render_prometheus(None, &[]);

    family(
        out,
        "cdm_ranges_planned",
        "gauge",
        "Token ranges in the plan.",
    );
    series(out, "cdm_ranges_planned", &labels, progress.ranges_total);

    family(
        out,
        "cdm_ranges",
        "gauge",
        "Token ranges in each state. Terminal states use the TRK-012 spellings.",
    );
    for (state, count) in [
        ("pending", progress.ranges_pending),
        ("in_flight", progress.ranges_in_flight),
    ] {
        series(
            out,
            "cdm_ranges",
            &report.labels.render_prometheus(None, &[("state", state)]),
            count,
        );
    }
    for (state, count) in &progress.ranges_by_status {
        series(
            out,
            "cdm_ranges",
            &report
                .labels
                .render_prometheus(None, &[("state", state.as_str())]),
            *count,
        );
    }

    family(
        out,
        "cdm_progress_ratio",
        "gauge",
        "Completed weight over planned weight, weighted by token span or size estimate.",
    );
    float_series(out, "cdm_progress_ratio", &labels, progress.weight_fraction);

    family(
        out,
        "cdm_progress_ranges_ratio",
        "gauge",
        "Completed ranges over planned ranges, unweighted.",
    );
    float_series(
        out,
        "cdm_progress_ranges_ratio",
        &labels,
        progress.ranges_fraction,
    );

    family(
        out,
        "cdm_run_elapsed_seconds",
        "gauge",
        "Wall-clock seconds since the run started.",
    );
    float_series(
        out,
        "cdm_run_elapsed_seconds",
        &labels,
        progress.elapsed.as_secs_f64(),
    );

    // Absent rather than zero when there is not enough of the run to extrapolate from: a zero ETA
    // means "about to finish", and a run that has just started is not.
    if let Some(eta) = progress.eta {
        family(
            out,
            "cdm_eta_seconds",
            "gauge",
            "Estimated seconds to completion. Absent until the estimate is worth having.",
        );
        float_series(out, "cdm_eta_seconds", &labels, eta.as_secs_f64());
    }
}

/// Writes a family's `# HELP` and `# TYPE` lines.
fn family(out: &mut String, name: &str, kind: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

/// Writes one unsigned series line.
fn series(out: &mut String, name: &str, labels: &str, value: u64) {
    let _ = writeln!(out, "{name}{labels} {value}");
}

/// Writes one signed series line, for a gauge that may go negative.
fn signed_series(out: &mut String, name: &str, labels: &str, value: i64) {
    let _ = writeln!(out, "{name}{labels} {value}");
}

/// Writes one floating-point series line, to three decimals.
///
/// Three decimals is a millisecond for a duration and a thousandth of a row per second for a rate,
/// both of which are far below the noise floor of what is being measured, and it keeps the output
/// byte-stable across platforms whose `exp` differs in the last bit.
fn float_series(out: &mut String, name: &str, labels: &str, value: f64) {
    let _ = writeln!(out, "{name}{labels} {value:.3}");
}

/// Nanoseconds as seconds, the unit Prometheus wants for a duration.
#[allow(clippy::cast_precision_loss)]
fn seconds(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000_000.0
}

/// The metric name of a counter: `cdm_` plus its lower-cased `MET-001` name plus `_total`.
fn counter_metric_name(kind: CounterKind) -> String {
    format!("{METRIC_PREFIX}{}_total", kind.as_str().to_lowercase())
}

/// The `GET /metrics` endpoint's backing state (`MET-020`, `PLG-006`).
///
/// Registers as a [`MetricsExporter`](cdm_core::MetricsExporter) like any other plugin: the engine
/// hands it a counter snapshot periodically, it renders the exposition together with the
/// instruments and progress it holds, and the HTTP handler serves whatever
/// [`PrometheusExporter::text`] last produced. Rendering on export rather than on scrape means a
/// scrape costs a string clone and cannot be made to walk the histograms by an attacker with a
/// `curl` loop.
#[derive(Debug)]
pub struct PrometheusExporter {
    labels: MetricLabels,
    instruments: std::sync::Arc<Instruments>,
    progress: Option<std::sync::Arc<ProgressTracker>>,
    text: Mutex<String>,
}

impl PrometheusExporter {
    /// Builds an exporter over a run's instruments and, when the plan is known, its progress.
    #[must_use]
    pub fn new(
        labels: MetricLabels,
        instruments: std::sync::Arc<Instruments>,
        progress: Option<std::sync::Arc<ProgressTracker>>,
    ) -> Self {
        Self {
            labels,
            instruments,
            progress,
            text: Mutex::new(String::new()),
        }
    }

    /// The exposition text as of the last export. Empty until the first one.
    #[must_use]
    pub fn text(&self) -> String {
        self.text.lock().clone()
    }

    /// The content type `GET /metrics` must respond with.
    pub const CONTENT_TYPE: &'static str = "text/plain; version=0.0.4; charset=utf-8";

    /// Renders and stores the exposition for a counter snapshot.
    ///
    /// Separated from the async trait method so that the rendering is testable without a runtime.
    pub fn refresh(&self, counters: &MetricsSnapshot) {
        let now = std::time::Instant::now();
        let report = MetricsReport::from_parts(
            self.labels.clone(),
            counters,
            self.instruments.snapshot_at(now),
            self.progress
                .as_ref()
                .map(|tracker| tracker.snapshot_at(now)),
        );
        *self.text.lock() = render(&report);
    }
}

impl cdm_core::Plugin for PrometheusExporter {
    fn name(&self) -> &'static str {
        "prometheus"
    }

    fn provider(&self) -> &'static str {
        "cdm-metrics"
    }
}

#[async_trait::async_trait]
impl cdm_core::MetricsExporter for PrometheusExporter {
    /// Re-renders the exposition. Cannot fail: there is nothing to fail, which is the point of
    /// rendering rather than transmitting.
    async fn export(&self, snapshot: &MetricsSnapshot) -> Result<(), CdmError> {
        self.refresh(snapshot);
        Ok(())
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
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use cdm_core::{JobKind, MetricsExporter, RunId, TableRef, TokenRange};

    use crate::export::tests::sample_report;
    use crate::JobCounters;

    use super::*;

    #[test]
    fn met_020_the_exposition_is_rendered_in_full() {
        insta::assert_snapshot!("prometheus_exposition", render(&sample_report()));
    }

    #[test]
    fn met_020_every_metric_name_is_prefixed_and_every_family_is_declared() {
        let text = render(&sample_report());
        let mut declared = Vec::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                declared.push(rest.split(' ').next().unwrap_or_default().to_owned());
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            let name = line.split('{').next().unwrap_or_default();
            assert!(
                name.starts_with(METRIC_PREFIX),
                "`{name}` is not prefixed `{METRIC_PREFIX}`"
            );
            // A summary's `_sum` and `_count` belong to the family declared without them.
            let family = name
                .trim_end_matches("_sum")
                .trim_end_matches("_count")
                .to_owned();
            assert!(
                declared.contains(&family) || declared.contains(&name.to_owned()),
                "`{name}` has no # TYPE line"
            );
        }
        assert!(declared.contains(&"cdm_read_total".to_owned()));
    }

    #[test]
    fn met_020_no_series_is_labelled_by_a_token_range_or_a_key() {
        // The cardinality rule of `MET-020`, checked against a report whose plan and counters are
        // both non-trivial: the only label names that appear are the six identity labels and the
        // five closed intrinsic ones.
        let text = render(&sample_report());
        let allowed = [
            "run_id",
            "job",
            "side",
            "node_id",
            "keyspace",
            "table", // MET-020 identity
            "operation",
            "quantile",
            "cause",
            "state",
            "window", // closed enums
        ];
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            let Some(labels) = line.split_once('{') else {
                continue;
            };
            let labels = labels.1.split('}').next().unwrap_or_default();
            for pair in labels.split(',') {
                let name = pair.split('=').next().unwrap_or_default();
                assert!(
                    allowed.contains(&name),
                    "unexpected label `{name}` in {line}"
                );
            }
        }
        // And no *series* mentions a range bound. (The `# HELP` prose does say "token span",
        // which is documentation rather than cardinality.)
        for line in text.lines().filter(|line| !line.starts_with('#')) {
            assert!(!line.contains("range_min"), "{line}");
            assert!(!line.contains("range_max"), "{line}");
        }
    }

    #[test]
    fn sec_001_no_configuration_value_reaches_the_exposition_except_the_closed_set() {
        // The label values are the run id, the job, the node id, the keyspace and the table, and
        // nothing else — in particular nothing that a password could be hiding in.
        let text = render(&sample_report());
        for forbidden in ["password", "token=", "secret", "AstraCS", "username"] {
            assert!(!text.contains(forbidden), "`{forbidden}` reached /metrics");
        }
        assert!(text.contains("keyspace=\"target_ks\""));
        assert!(text.contains("table=\"orders\""));
    }

    #[test]
    fn met_020_unflushed_is_not_exported_because_its_committed_value_is_always_zero() {
        let text = render(&sample_report());
        assert!(text.contains("cdm_read_total"));
        assert!(text.contains("cdm_partitions_passed_total"));
        assert!(!text.contains("unflushed"), "{text}");
    }

    #[test]
    fn met_020_an_operation_that_never_ran_exports_no_latency_series() {
        let text = render(&sample_report());
        assert!(text.contains("operation=\"range_read\""));
        assert!(text.contains("operation=\"write\""));
        assert!(!text.contains("operation=\"key_read\""), "{text}");
        assert!(!text.contains("operation=\"batch\""), "{text}");
    }

    #[test]
    fn met_011_the_eta_is_absent_rather_than_zero_before_it_is_meaningful() {
        let start = Instant::now();
        let labels = MetricLabels::new(RunId::from_raw(1), JobKind::Migrate, "n");
        let instruments = Arc::new(Instruments::new(start));
        let ranges = TokenRange::MURMUR3_FULL.split(100).unwrap();
        let progress = Arc::new(ProgressTracker::by_token_span(&ranges, start));

        let counters = JobCounters::new(JobKind::Migrate);
        let snapshot = counters.snapshot(RunId::from_raw(1), chrono::DateTime::UNIX_EPOCH);

        let exporter = PrometheusExporter::new(labels, instruments, Some(Arc::clone(&progress)));
        exporter.refresh(&snapshot);
        let text = exporter.text();
        assert!(text.contains("cdm_ranges{"), "{text}");
        assert!(text.contains("state=\"pending\"} 100"), "{text}");
        assert!(!text.contains("cdm_eta_seconds"), "{text}");

        for planned in ranges.iter().take(50) {
            progress.range_completed(*planned, cdm_core::RunStatus::Pass);
        }
        exporter.refresh(&snapshot);
        let text = exporter.text();
        assert!(text.contains("cdm_eta_seconds"), "{text}");
        assert!(text.contains("state=\"PASS\"} 50"), "{text}");
    }

    #[tokio::test]
    async fn plg_006_the_exporter_registers_as_a_metrics_exporter_plugin() {
        use cdm_core::Plugin;

        let start = Instant::now();
        let labels = MetricLabels::new(RunId::from_raw(9), JobKind::Guardrail, "node-b")
            .with_table(&TableRef::new("ks", "t"));
        let exporter = PrometheusExporter::new(labels, Arc::new(Instruments::new(start)), None);
        assert_eq!(exporter.name(), "prometheus");
        assert_eq!(exporter.provider(), "cdm-metrics");
        assert!(exporter.text().is_empty(), "nothing until the first export");

        let counters = JobCounters::new(JobKind::Guardrail);
        counters.increment_by(counters.counter(CounterKind::Large).unwrap(), 3);
        counters.flush();
        exporter
            .export(&counters.snapshot(RunId::from_raw(9), chrono::DateTime::UNIX_EPOCH))
            .await
            .unwrap();

        let text = exporter.text();
        assert!(text.contains("cdm_large_total"), "{text}");
        assert!(text.contains("} 3"), "{text}");
        assert_eq!(
            PrometheusExporter::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8"
        );
    }
}
