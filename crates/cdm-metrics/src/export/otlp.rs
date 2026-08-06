//! OpenTelemetry OTLP export of metrics and traces (`MET-021`).
//!
//! # What this module does and does not do
//!
//! It builds OTLP payloads — `ExportMetricsServiceRequest` and `ExportTraceServiceRequest`, in the
//! JSON encoding the specification defines for `OTLP/HTTP` — and hands the bytes to an
//! [`OtlpTransport`]. It does not open a connection. The reason is in the
//! [module documentation of the parent](super): `cdm-metrics` may not depend on an HTTP client,
//! and OTLP is a wire protocol, so the wire is somebody else's.
//!
//! JSON rather than protobuf for the same reason. The OTLP specification defines both encodings
//! for `OTLP/HTTP` and requires collectors to accept `application/json`; protobuf would mean
//! `prost` and a build-time `protoc`, and neither buys anything a collector notices at the volume
//! cdm-rs exports — one payload per scrape interval, not one per row.
//!
//! # Which counter accounting is exported
//!
//! **Committed** (`MET-004`), the same as Prometheus, and for the same reason. Metrics are emitted
//! as cumulative monotonic sums with `aggregationTemporality: 2` (`CUMULATIVE`), which is what a
//! committed run total is; exporting interim counts as cumulative would produce a series that goes
//! backwards, and every OTLP backend treats that as a counter reset.
//!
//! # Traces
//!
//! `MET-021` requires traces as well as metrics. The spans cdm-rs produces are the ones `ENG-011`
//! defines — one per token range, carrying `run_id`, `range_min`, `range_max` and `node_id` — plus
//! a span for the run itself. Note that a *span* may carry a token range where a *metric* may not
//! (`MET-020`): a span is a single event, not a time series, so a range on a span costs one
//! attribute rather than an unbounded number of series. That distinction is not drawn in
//! `SPEC.md`, and `met_021_a_range_span_carries_the_bounds_a_metric_may_not` is where it is
//! written down.

use std::sync::Arc;

use cdm_core::{CdmError, ErrorKind, RunId, Side, TokenRange};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::instrument::Operation;
use crate::label::MetricLabels;

use super::{counter_help, MetricsReport};

/// The `service.name` every payload carries.
pub const SERVICE_NAME: &str = "cdm-rs";

/// The instrumentation scope every metric and span is attributed to.
pub const SCOPE_NAME: &str = "cdm-metrics";

/// OTLP cumulative aggregation temporality, as the enum's wire value.
const AGGREGATION_TEMPORALITY_CUMULATIVE: i32 = 2;

/// Which OTLP signal a payload belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OtlpSignal {
    /// Metrics, posted to `/v1/metrics`.
    Metrics,
    /// Traces, posted to `/v1/traces`.
    Traces,
}

impl OtlpSignal {
    /// The path this signal is posted to, appended to `metrics.otlp.endpoint`.
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Metrics => "/v1/metrics",
            Self::Traces => "/v1/traces",
        }
    }
}

/// Where an OTLP payload goes (`MET-021`).
///
/// Implemented by the crate that is allowed to make network calls; `cdm-api` installs one over its
/// HTTP client, and [`MemoryTransport`] is the one the tests use. A transport must not block the
/// caller for long and must not fail a run: `PLG-006` already requires an exporter's failure to be
/// logged and ignored, and this trait inherits that contract.
#[async_trait::async_trait]
pub trait OtlpTransport: Send + Sync + std::fmt::Debug {
    /// Delivers one payload.
    ///
    /// # Errors
    ///
    /// Whatever delivery failure the implementation encountered. The caller logs it and carries
    /// on; observability is never worth failing a migration for.
    async fn send(&self, signal: OtlpSignal, endpoint: &str, body: &[u8]) -> Result<(), CdmError>;
}

/// A transport that keeps payloads in memory, for tests and for `--dry-run`.
#[derive(Debug, Default)]
pub struct MemoryTransport {
    sent: Mutex<Vec<(OtlpSignal, String, String)>>,
}

impl MemoryTransport {
    /// An empty transport.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything sent so far, as `(signal, endpoint, body)`.
    #[must_use]
    pub fn sent(&self) -> Vec<(OtlpSignal, String, String)> {
        self.sent.lock().clone()
    }

    /// The most recent body for a signal, if any.
    #[must_use]
    pub fn last_body(&self, signal: OtlpSignal) -> Option<String> {
        self.sent
            .lock()
            .iter()
            .rev()
            .find(|(sent, _, _)| *sent == signal)
            .map(|(_, _, body)| body.clone())
    }
}

#[async_trait::async_trait]
impl OtlpTransport for MemoryTransport {
    async fn send(&self, signal: OtlpSignal, endpoint: &str, body: &[u8]) -> Result<(), CdmError> {
        self.sent.lock().push((
            signal,
            endpoint.to_owned(),
            String::from_utf8_lossy(body).into_owned(),
        ));
        Ok(())
    }
}

/// Exports metrics and traces over OTLP (`MET-021`).
///
/// Configured by `metrics.otlp.endpoint`; export is off when that is unset, which is why
/// construction takes the endpoint rather than an `Option`.
#[derive(Debug)]
pub struct OtlpExporter {
    endpoint: String,
    labels: MetricLabels,
    transport: Arc<dyn OtlpTransport>,
}

impl OtlpExporter {
    /// Builds an exporter posting to `endpoint`.
    ///
    /// The endpoint is the collector's base URL — `http://localhost:4318` — and the signal path is
    /// appended, as the OTLP/HTTP specification requires.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        labels: MetricLabels,
        transport: Arc<dyn OtlpTransport>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            labels,
            transport,
        }
    }

    /// The full URL a signal is posted to.
    #[must_use]
    pub fn url(&self, signal: OtlpSignal) -> String {
        format!("{}{}", self.endpoint.trim_end_matches('/'), signal.path())
    }

    /// Builds the metrics payload for a report.
    #[must_use]
    pub fn metrics_payload(&self, report: &MetricsReport) -> OtlpMetricsPayload {
        OtlpMetricsPayload::build(report)
    }

    /// Builds the traces payload for a batch of spans.
    #[must_use]
    pub fn traces_payload(&self, spans: &[SpanRecord]) -> OtlpTracesPayload {
        OtlpTracesPayload::build(&self.labels, spans)
    }

    /// Serialises and sends a metrics report.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the payload cannot be serialised, or whatever the transport
    /// reports.
    pub async fn export_metrics(&self, report: &MetricsReport) -> Result<(), CdmError> {
        let body = to_json(&self.metrics_payload(report))?;
        self.transport
            .send(
                OtlpSignal::Metrics,
                &self.url(OtlpSignal::Metrics),
                body.as_bytes(),
            )
            .await
    }

    /// Serialises and sends a batch of spans (`ENG-011`).
    ///
    /// An empty batch sends nothing: a collector should not be woken up to be told that nothing
    /// happened.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the payload cannot be serialised, or whatever the transport
    /// reports.
    pub async fn export_spans(&self, spans: &[SpanRecord]) -> Result<(), CdmError> {
        if spans.is_empty() {
            return Ok(());
        }
        let body = to_json(&self.traces_payload(spans))?;
        self.transport
            .send(
                OtlpSignal::Traces,
                &self.url(OtlpSignal::Traces),
                body.as_bytes(),
            )
            .await
    }
}

/// Serialises a payload, turning a serialisation failure into a `CdmError` rather than a panic.
fn to_json<T: Serialize>(payload: &T) -> Result<String, CdmError> {
    serde_json::to_string(payload).map_err(|error| {
        CdmError::new(
            ErrorKind::Internal,
            format!("cannot serialise the OTLP payload: {error}"),
        )
    })
}

/// An OTLP attribute: a key and a typed value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    /// The attribute name. Always from a closed set in this crate (`SEC-001`).
    pub key: String,
    /// The attribute value.
    pub value: AnyValue,
}

impl KeyValue {
    /// A string-valued attribute.
    #[must_use]
    pub fn string(key: &str, value: impl Into<String>) -> Self {
        Self {
            key: key.to_owned(),
            value: AnyValue::String {
                string_value: value.into(),
            },
        }
    }

    /// An integer-valued attribute. OTLP/JSON encodes a 64-bit integer as a string.
    #[must_use]
    pub fn int(key: &str, value: i64) -> Self {
        Self {
            key: key.to_owned(),
            value: AnyValue::Int {
                int_value: value.to_string(),
            },
        }
    }
}

/// An OTLP `AnyValue`, restricted to the two shapes cdm-rs emits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnyValue {
    /// A string.
    String {
        /// The value.
        #[serde(rename = "stringValue")]
        string_value: String,
    },
    /// An integer, encoded as a string per the OTLP/JSON mapping.
    Int {
        /// The value.
        #[serde(rename = "intValue")]
        int_value: String,
    },
}

/// The OTLP resource every payload carries: the service, and the run's identity labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    /// The resource attributes.
    pub attributes: Vec<KeyValue>,
}

impl Resource {
    /// Builds the resource from the closed label set of `MET-020`.
    ///
    /// Nothing else is put here. A resource is attached to every metric and every span a process
    /// emits and is the most attractive place in the whole protocol to dump the configuration,
    /// which is exactly why `SEC-001` says not to.
    #[must_use]
    pub fn from_labels(labels: &MetricLabels) -> Self {
        let mut attributes = vec![KeyValue::string("service.name", SERVICE_NAME)];
        for (key, value) in labels.pairs(None) {
            attributes.push(KeyValue::string(&format!("cdm.{key}"), value));
        }
        Self { attributes }
    }
}

/// The instrumentation scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    /// The scope name.
    pub name: String,
    /// The scope version — this crate's.
    pub version: String,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            name: SCOPE_NAME.to_owned(),
            version: crate::VERSION.to_owned(),
        }
    }
}

/// An OTLP `ExportMetricsServiceRequest` (`MET-021`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpMetricsPayload {
    /// One entry: this process.
    pub resource_metrics: Vec<ResourceMetrics>,
}

/// One resource's metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceMetrics {
    /// The resource.
    pub resource: Resource,
    /// One entry: this crate.
    pub scope_metrics: Vec<ScopeMetrics>,
}

/// One scope's metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeMetrics {
    /// The scope.
    pub scope: Scope,
    /// The metrics.
    pub metrics: Vec<Metric>,
}

/// One OTLP metric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metric {
    /// The metric name, in OTLP's dotted convention (`cdm.rows.read`).
    pub name: String,
    /// A one-line description, the same text the Prometheus `# HELP` line carries.
    pub description: String,
    /// The unit, as UCUM: `1` for a count, `s` for a duration, `By` for bytes.
    pub unit: String,
    /// A monotonic cumulative sum, for a counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sum: Option<Sum>,
    /// A gauge, for a rate, a ratio or an in-flight count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gauge: Option<Gauge>,
}

/// An OTLP cumulative sum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sum {
    /// The points.
    pub data_points: Vec<NumberDataPoint>,
    /// Always [`AGGREGATION_TEMPORALITY_CUMULATIVE`]: an exported counter is a run total.
    pub aggregation_temporality: i32,
    /// Always true: cdm-rs counters never decrease.
    pub is_monotonic: bool,
}

/// An OTLP gauge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gauge {
    /// The points.
    pub data_points: Vec<NumberDataPoint>,
}

/// One OTLP data point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NumberDataPoint {
    /// When the point was taken, in nanoseconds since the Unix epoch, encoded as a string.
    pub time_unix_nano: String,
    /// The point's dimensions: the closed intrinsic labels of this crate, never the identity
    /// labels, which live on the resource.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<KeyValue>,
    /// An integer value, for a counter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_int: Option<String>,
    /// A floating-point value, for a rate or a ratio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_double: Option<f64>,
}

impl NumberDataPoint {
    /// An integer point.
    #[must_use]
    pub fn int(taken_at: DateTime<Utc>, attributes: Vec<KeyValue>, value: u64) -> Self {
        Self {
            time_unix_nano: unix_nanos(taken_at),
            attributes,
            as_int: Some(value.to_string()),
            as_double: None,
        }
    }

    /// A floating-point point.
    #[must_use]
    pub fn double(taken_at: DateTime<Utc>, attributes: Vec<KeyValue>, value: f64) -> Self {
        Self {
            time_unix_nano: unix_nanos(taken_at),
            attributes,
            as_int: None,
            as_double: Some(value),
        }
    }
}

impl OtlpMetricsPayload {
    /// Translates a report into OTLP.
    ///
    /// The families mirror the Prometheus ones exactly — same values, same closed dimensions,
    /// OTLP's dotted naming instead of Prometheus's underscored — so that the two exports never
    /// disagree about what a run did.
    //
    // One long function rather than eight short ones: it is a straight-line translation table
    // from a report to a payload, and splitting it would hide the fact that the list of families
    // here must match the list in the Prometheus renderer.
    #[allow(clippy::too_many_lines)]
    #[must_use]
    pub fn build(report: &MetricsReport) -> Self {
        let at = report.taken_at;
        let mut metrics = Vec::new();

        for (kind, value) in report.typed_counters() {
            if kind == crate::CounterKind::Unflushed {
                continue; // always zero at the committed level; see the Prometheus renderer
            }
            metrics.push(Metric {
                name: format!("cdm.{}", kind.as_str().to_lowercase().replace('_', ".")),
                description: counter_help(kind).to_owned(),
                unit: "1".to_owned(),
                sum: Some(Sum {
                    data_points: vec![NumberDataPoint::int(at, Vec::new(), value)],
                    aggregation_temporality: AGGREGATION_TEMPORALITY_CUMULATIVE,
                    is_monotonic: true,
                }),
                gauge: None,
            });
        }

        let sides = [Side::Origin, Side::Target];
        metrics.push(Metric {
            name: "cdm.rows".to_owned(),
            description: "Rows that crossed the wire, per side.".to_owned(),
            unit: "1".to_owned(),
            sum: Some(Sum {
                data_points: sides
                    .into_iter()
                    .map(|side| {
                        NumberDataPoint::int(
                            at,
                            vec![KeyValue::string("side", side.as_str())],
                            report.instruments.side(side).rows.total,
                        )
                    })
                    .collect(),
                aggregation_temporality: AGGREGATION_TEMPORALITY_CUMULATIVE,
                is_monotonic: true,
            }),
            gauge: None,
        });
        metrics.push(Metric {
            name: "cdm.bytes".to_owned(),
            description: "Bytes that crossed the wire, per side.".to_owned(),
            unit: "By".to_owned(),
            sum: Some(Sum {
                data_points: sides
                    .into_iter()
                    .map(|side| {
                        NumberDataPoint::int(
                            at,
                            vec![KeyValue::string("side", side.as_str())],
                            report.instruments.side(side).bytes.total,
                        )
                    })
                    .collect(),
                aggregation_temporality: AGGREGATION_TEMPORALITY_CUMULATIVE,
                is_monotonic: true,
            }),
            gauge: None,
        });
        metrics.push(Metric {
            name: "cdm.rows.rate".to_owned(),
            description: "Exponentially-weighted rows per second, per side.".to_owned(),
            unit: "1/s".to_owned(),
            sum: None,
            gauge: Some(Gauge {
                data_points: sides
                    .into_iter()
                    .map(|side| {
                        NumberDataPoint::double(
                            at,
                            vec![
                                KeyValue::string("side", side.as_str()),
                                KeyValue::string("window", "10s"),
                            ],
                            report.instruments.side(side).rows.per_second_10s,
                        )
                    })
                    .collect(),
            }),
        });

        let mut latency = Vec::new();
        for side in sides {
            for (operation, snapshot) in report.instruments.side(side).recorded_latencies() {
                for (quantile, nanos) in snapshot.labelled() {
                    latency.push(NumberDataPoint::double(
                        at,
                        vec![
                            KeyValue::string("side", side.as_str()),
                            KeyValue::string("operation", operation.as_str()),
                            KeyValue::string("quantile", quantile),
                        ],
                        nanos_to_seconds(nanos),
                    ));
                }
            }
        }
        if !latency.is_empty() {
            metrics.push(Metric {
                name: "cdm.request.duration".to_owned(),
                description: "Request latency, per side and operation.".to_owned(),
                unit: "s".to_owned(),
                sum: None,
                gauge: Some(Gauge {
                    data_points: latency,
                }),
            });
        }

        metrics.push(Metric {
            name: "cdm.requests.inflight".to_owned(),
            description: "Requests issued and not yet answered, per side.".to_owned(),
            unit: "1".to_owned(),
            sum: None,
            gauge: Some(Gauge {
                data_points: sides
                    .into_iter()
                    .map(|side| {
                        NumberDataPoint::double(
                            at,
                            vec![KeyValue::string("side", side.as_str())],
                            f64::from(
                                i32::try_from(report.instruments.side(side).inflight)
                                    .unwrap_or(i32::MAX),
                            ),
                        )
                    })
                    .collect(),
            }),
        });

        metrics.push(Metric {
            name: "cdm.retries".to_owned(),
            description: "Requests retried, by cause.".to_owned(),
            unit: "1".to_owned(),
            sum: Some(Sum {
                data_points: report
                    .instruments
                    .retries_labelled()
                    .into_iter()
                    .map(|(cause, count)| {
                        NumberDataPoint::int(at, vec![KeyValue::string("cause", cause)], count)
                    })
                    .collect(),
                aggregation_temporality: AGGREGATION_TEMPORALITY_CUMULATIVE,
                is_monotonic: true,
            }),
            gauge: None,
        });

        if let Some(progress) = &report.progress {
            metrics.push(Metric {
                name: "cdm.progress.ratio".to_owned(),
                description: "Completed weight over planned weight.".to_owned(),
                unit: "1".to_owned(),
                sum: None,
                gauge: Some(Gauge {
                    data_points: vec![NumberDataPoint::double(
                        at,
                        Vec::new(),
                        progress.weight_fraction,
                    )],
                }),
            });
            if let Some(eta) = progress.eta {
                metrics.push(Metric {
                    name: "cdm.eta".to_owned(),
                    description: "Estimated seconds to completion.".to_owned(),
                    unit: "s".to_owned(),
                    sum: None,
                    gauge: Some(Gauge {
                        data_points: vec![NumberDataPoint::double(
                            at,
                            Vec::new(),
                            eta.as_secs_f64(),
                        )],
                    }),
                });
            }
        }

        Self {
            resource_metrics: vec![ResourceMetrics {
                resource: Resource::from_labels(&report.labels),
                scope_metrics: vec![ScopeMetrics {
                    scope: Scope::default(),
                    metrics,
                }],
            }],
        }
    }
}

/// What a span describes (`ENG-011`, `MET-021`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// The run as a whole.
    Run,
    /// One token range (`ENG-011`).
    Range,
    /// One request against one side.
    Request,
}

impl SpanKind {
    /// The OTLP span name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "cdm.run",
            Self::Range => "cdm.range",
            Self::Request => "cdm.request",
        }
    }
}

/// One span, ready to be exported (`ENG-011`, `MET-021`).
///
/// The attribute set is closed, exactly as the metric label set is: a span may carry the token
/// range a metric may not, and it may carry nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanRecord {
    /// What the span describes.
    pub kind: SpanKind,
    /// The run, which is also what the trace id is derived from.
    pub run_id: RunId,
    /// The token range, for a range span (`ENG-011`).
    pub range: Option<TokenRange>,
    /// The side, for a request span.
    pub side: Option<Side>,
    /// The operation, for a request span.
    pub operation: Option<Operation>,
    /// When it started.
    pub started_at: DateTime<Utc>,
    /// When it ended.
    pub ended_at: DateTime<Utc>,
    /// Whether it ended in an error. The message is not carried: `ERR-002` diagnostics go to the
    /// event stream and the log, where their redaction rules are already established.
    pub failed: bool,
}

impl SpanRecord {
    /// A span for one token range (`ENG-011`).
    #[must_use]
    pub const fn range(
        run_id: RunId,
        range: TokenRange,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        failed: bool,
    ) -> Self {
        Self {
            kind: SpanKind::Range,
            run_id,
            range: Some(range),
            side: None,
            operation: None,
            started_at,
            ended_at,
            failed,
        }
    }

    /// A span for the run itself.
    #[must_use]
    pub const fn run(run_id: RunId, started_at: DateTime<Utc>, ended_at: DateTime<Utc>) -> Self {
        Self {
            kind: SpanKind::Run,
            run_id,
            range: None,
            side: None,
            operation: None,
            started_at,
            ended_at,
            failed: false,
        }
    }

    /// The trace id: one trace per run, derived from the run id so that every node in a
    /// distributed run (`DST-001`) joins the same trace without coordinating.
    #[must_use]
    pub fn trace_id(&self) -> String {
        let raw = self.run_id.as_i64().to_be_bytes();
        let mut id = [0_u8; 16];
        id[..8].copy_from_slice(&raw);
        id[8..].copy_from_slice(&raw);
        hex(&id)
    }

    /// The span id, derived from the run, the kind and the range so that a re-export of the same
    /// span does not create a second one.
    #[must_use]
    pub fn span_id(&self) -> String {
        let mut hash = FNV_OFFSET;
        for byte in self.run_id.as_i64().to_be_bytes() {
            hash = fnv(hash, byte);
        }
        hash = fnv(hash, self.kind as u8);
        if let Some(range) = self.range {
            for byte in range.min().to_be_bytes() {
                hash = fnv(hash, byte);
            }
            for byte in range.max().to_be_bytes() {
                hash = fnv(hash, byte);
            }
        }
        hex(&hash.to_be_bytes())
    }

    /// The span's attributes: the closed set of `ENG-011`.
    #[must_use]
    pub fn attributes(&self) -> Vec<KeyValue> {
        let mut attributes = vec![KeyValue::int("cdm.run_id", self.run_id.as_i64())];
        if let Some(range) = self.range {
            // A token range on a *span* is one attribute on one event, not an unbounded family of
            // time series; `MET-020`'s cardinality rule is about metrics and does not apply here.
            // `ENG-011` requires these two by name.
            attributes.push(KeyValue::string("cdm.range_min", range.min().to_string()));
            attributes.push(KeyValue::string("cdm.range_max", range.max().to_string()));
        }
        if let Some(side) = self.side {
            attributes.push(KeyValue::string("cdm.side", side.as_str()));
        }
        if let Some(operation) = self.operation {
            attributes.push(KeyValue::string("cdm.operation", operation.as_str()));
        }
        attributes
    }
}

/// An OTLP `ExportTraceServiceRequest` (`MET-021`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OtlpTracesPayload {
    /// One entry: this process.
    pub resource_spans: Vec<ResourceSpans>,
}

/// One resource's spans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSpans {
    /// The resource.
    pub resource: Resource,
    /// One entry: this crate.
    pub scope_spans: Vec<ScopeSpans>,
}

/// One scope's spans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopeSpans {
    /// The scope.
    pub scope: Scope,
    /// The spans.
    pub spans: Vec<Span>,
}

/// One OTLP span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    /// The trace, as 32 hex characters.
    pub trace_id: String,
    /// The span, as 16 hex characters.
    pub span_id: String,
    /// The span name.
    pub name: String,
    /// Start time, in nanoseconds since the Unix epoch, encoded as a string.
    pub start_time_unix_nano: String,
    /// End time, in nanoseconds since the Unix epoch, encoded as a string.
    pub end_time_unix_nano: String,
    /// The closed attribute set.
    pub attributes: Vec<KeyValue>,
    /// `1` for OK, `2` for ERROR, as the OTLP status enum spells them.
    pub status: SpanStatus,
}

/// An OTLP span status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanStatus {
    /// `1` = OK, `2` = ERROR.
    pub code: i32,
}

impl OtlpTracesPayload {
    /// Translates a batch of spans into OTLP.
    #[must_use]
    pub fn build(labels: &MetricLabels, spans: &[SpanRecord]) -> Self {
        Self {
            resource_spans: vec![ResourceSpans {
                resource: Resource::from_labels(labels),
                scope_spans: vec![ScopeSpans {
                    scope: Scope::default(),
                    spans: spans
                        .iter()
                        .map(|record| Span {
                            trace_id: record.trace_id(),
                            span_id: record.span_id(),
                            name: record.kind.as_str().to_owned(),
                            start_time_unix_nano: unix_nanos(record.started_at),
                            end_time_unix_nano: unix_nanos(record.ended_at),
                            attributes: record.attributes(),
                            status: SpanStatus {
                                code: if record.failed { 2 } else { 1 },
                            },
                        })
                        .collect(),
                }],
            }],
        }
    }
}

/// A timestamp as OTLP wants it: nanoseconds since the Unix epoch, as a decimal string.
///
/// A timestamp outside the nanosecond range — before 1677 or after 2262 — is reported as zero,
/// which OTLP reads as "unset". No cdm-rs run has one.
fn unix_nanos(at: DateTime<Utc>) -> String {
    at.timestamp_nanos_opt().unwrap_or(0).to_string()
}

/// Nanoseconds as seconds.
#[allow(clippy::cast_precision_loss)]
fn nanos_to_seconds(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000_000.0
}

/// The FNV-1a 64-bit offset basis.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// One FNV-1a round. Used only to derive a stable span id; nothing depends on it being hard to
/// invert.
fn fnv(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
}

/// Lower-case hexadecimal, as OTLP/JSON encodes a trace or span id.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            // Writing into a `String` is infallible; the result is discarded rather than
            // unwrapped so that no panicking path exists (`ERR-004`).
            let _ = write!(out, "{byte:02x}");
            out
        })
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
    use cdm_core::{JobKind, TableRef};

    use crate::export::tests::sample_report;

    use super::*;

    fn labels() -> MetricLabels {
        MetricLabels::new(RunId::from_raw(1_712_345_678), JobKind::Migrate, "node-a")
            .with_table(&TableRef::new("ks", "orders"))
    }

    fn span() -> SpanRecord {
        SpanRecord::range(
            RunId::from_raw(1_712_345_678),
            TokenRange::new(-100, 99).unwrap(),
            DateTime::UNIX_EPOCH,
            DateTime::UNIX_EPOCH + chrono::Duration::seconds(3),
            false,
        )
    }

    #[test]
    fn met_021_the_metrics_payload_is_a_cumulative_otlp_request() {
        let payload = OtlpMetricsPayload::build(&sample_report());
        let json = serde_json::to_value(&payload).unwrap();

        let metrics = &json["resourceMetrics"][0]["scopeMetrics"][0]["metrics"];
        assert!(metrics.is_array());
        let read = metrics
            .as_array()
            .unwrap()
            .iter()
            .find(|metric| metric["name"] == "cdm.read")
            .expect("the READ counter must be exported");
        assert_eq!(read["unit"], "1");
        assert_eq!(read["sum"]["isMonotonic"], true);
        assert_eq!(read["sum"]["aggregationTemporality"], 2);
        // OTLP/JSON encodes a 64-bit integer as a string.
        assert_eq!(read["sum"]["dataPoints"][0]["asInt"], "1000000");
        assert_eq!(
            json["resourceMetrics"][0]["scopeMetrics"][0]["scope"]["name"],
            SCOPE_NAME
        );
    }

    #[test]
    fn met_021_counter_names_are_dotted_and_partitions_keep_their_shape() {
        let payload = OtlpMetricsPayload::build(&sample_report());
        let names: Vec<String> = payload.resource_metrics[0].scope_metrics[0]
            .metrics
            .iter()
            .map(|metric| metric.name.clone())
            .collect();
        assert!(names.contains(&"cdm.read".to_owned()));
        assert!(names.contains(&"cdm.partitions.passed".to_owned()));
        assert!(names.contains(&"cdm.rows".to_owned()));
        assert!(names.contains(&"cdm.progress.ratio".to_owned()));
        assert!(
            !names.contains(&"cdm.unflushed".to_owned()),
            "UNFLUSHED is always zero at the committed level"
        );
    }

    #[test]
    fn sec_001_the_otlp_resource_carries_only_the_closed_label_set() {
        let resource = Resource::from_labels(&labels());
        let keys: Vec<&str> = resource
            .attributes
            .iter()
            .map(|attribute| attribute.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "service.name",
                "cdm.run_id",
                "cdm.job",
                "cdm.node_id",
                "cdm.keyspace",
                "cdm.table",
            ]
        );
    }

    #[test]
    fn met_021_a_range_span_carries_the_bounds_a_metric_may_not() {
        // `ENG-011` requires `run_id`, `range_min`, `range_max` and `node_id` on a span;
        // `MET-020` forbids a token range as a metric label. Both are satisfied: the range is a
        // span attribute, the node id is a resource attribute, and no metric carries either.
        let payload = OtlpTracesPayload::build(&labels(), &[span()]);
        let json = serde_json::to_value(&payload).unwrap();
        let attributes = &json["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["attributes"];
        let keys: Vec<String> = attributes
            .as_array()
            .unwrap()
            .iter()
            .map(|attribute| attribute["key"].as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(keys, vec!["cdm.run_id", "cdm.range_min", "cdm.range_max"]);
        assert_eq!(attributes[1]["value"]["stringValue"], "-100");

        let metrics_json =
            serde_json::to_string(&OtlpMetricsPayload::build(&sample_report())).unwrap();
        assert!(!metrics_json.contains("range_min"), "{metrics_json}");
    }

    #[test]
    fn met_021_span_identity_is_derived_and_stable() {
        let first = span();
        let second = span();
        assert_eq!(first.trace_id(), second.trace_id());
        assert_eq!(first.span_id(), second.span_id());
        assert_eq!(first.trace_id().len(), 32);
        assert_eq!(first.span_id().len(), 16);
        assert!(first.trace_id().chars().all(|c| c.is_ascii_hexdigit()));

        // Every span of a run shares its trace; different ranges are different spans.
        let run = SpanRecord::run(
            RunId::from_raw(1_712_345_678),
            DateTime::UNIX_EPOCH,
            DateTime::UNIX_EPOCH,
        );
        assert_eq!(run.trace_id(), first.trace_id());
        assert_ne!(run.span_id(), first.span_id());

        let other_range = SpanRecord::range(
            RunId::from_raw(1_712_345_678),
            TokenRange::new(100, 199).unwrap(),
            DateTime::UNIX_EPOCH,
            DateTime::UNIX_EPOCH,
            false,
        );
        assert_ne!(other_range.span_id(), first.span_id());

        // A different run is a different trace.
        let other_run = SpanRecord::run(
            RunId::from_raw(7),
            DateTime::UNIX_EPOCH,
            DateTime::UNIX_EPOCH,
        );
        assert_ne!(other_run.trace_id(), run.trace_id());
    }

    #[test]
    fn met_021_a_failed_range_span_reports_the_error_status_without_the_message() {
        let mut failed = span();
        failed.failed = true;
        let payload = OtlpTracesPayload::build(&labels(), &[failed]);
        assert_eq!(
            payload.resource_spans[0].scope_spans[0].spans[0]
                .status
                .code,
            2
        );
        assert_eq!(
            OtlpTracesPayload::build(&labels(), &[span()]).resource_spans[0].scope_spans[0].spans
                [0]
            .status
            .code,
            1
        );
    }

    #[tokio::test]
    async fn met_021_the_exporter_posts_both_signals_to_the_configured_endpoint() {
        let transport = Arc::new(MemoryTransport::new());
        let exporter = OtlpExporter::new(
            "http://collector:4318/",
            labels(),
            Arc::clone(&transport) as Arc<dyn OtlpTransport>,
        );

        assert_eq!(
            exporter.url(OtlpSignal::Metrics),
            "http://collector:4318/v1/metrics"
        );
        assert_eq!(
            exporter.url(OtlpSignal::Traces),
            "http://collector:4318/v1/traces"
        );

        exporter.export_metrics(&sample_report()).await.unwrap();
        exporter.export_spans(&[span()]).await.unwrap();
        // An empty batch is not worth a request.
        exporter.export_spans(&[]).await.unwrap();

        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].0, OtlpSignal::Metrics);
        assert_eq!(sent[1].0, OtlpSignal::Traces);
        assert!(transport
            .last_body(OtlpSignal::Metrics)
            .unwrap()
            .contains("resourceMetrics"));
        assert!(transport
            .last_body(OtlpSignal::Traces)
            .unwrap()
            .contains("resourceSpans"));
    }

    #[test]
    fn met_021_payloads_round_trip_through_the_otlp_json_encoding() {
        let metrics = OtlpMetricsPayload::build(&sample_report());
        let json = serde_json::to_string(&metrics).unwrap();
        assert_eq!(
            serde_json::from_str::<OtlpMetricsPayload>(&json).unwrap(),
            metrics
        );

        let traces = OtlpTracesPayload::build(&labels(), &[span()]);
        let json = serde_json::to_string(&traces).unwrap();
        assert_eq!(
            serde_json::from_str::<OtlpTracesPayload>(&json).unwrap(),
            traces
        );
    }
}
