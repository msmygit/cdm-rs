//! Counters, instruments, progress, exporters and the run event bus.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What is here
//!
//! The thirteen counters every cdm-rs job keeps, and the two strings other people's tooling reads
//! them out of:
//!
//! * [`CounterKind`] — the counter vocabulary of `MET-001`, in Java's declaration order, which is
//!   also the rendering order;
//! * [`JobCounters`] — a lock-free registry of `AtomicU64` pairs, registered per job (`MET-002`),
//!   with Java's interim/committed two-level accounting (`MET-004`);
//! * [`Counter`] — proof that a counter is registered, resolved once at startup so that the hot
//!   path cannot fail (`MET-003`);
//! * [`JobCounters::metrics`] and [`JobCounters::final_block`] — the metrics string (`MET-005`)
//!   and the final block (`MET-006`), byte for byte as Java emits them.
//!
//! # These strings are a public contract
//!
//! `COMPAT-004` requires the metrics string and the final block to stay character-identical to
//! Java's. They are grepped by `SIT/cdm-assert.sh`, parsed by users' scripts, and stored in
//! `cdm_run_info.run_info` and `cdm_run_details.run_info`, where Java CDM reads them back when it
//! decides whether a run may be resumed (`TRK-030`). A changed separator or capital letter is a
//! silent breakage, so [`CounterKind::title_case`] is a written-down table rather than a call to
//! a case-conversion helper, and every rule has a test that a refactor would have to delete
//! rather than merely disturb.
//!
//! # Usage
//!
//! ```
//! use cdm_core::{JobKind, RunId};
//! use cdm_metrics::{CounterKind, JobCounters};
//!
//! // Once per run, and once per range.
//! let run = JobCounters::new(JobKind::Migrate);
//! let range = JobCounters::new(JobKind::Migrate);
//!
//! // Once, at startup: an unregistered counter fails here, never on a row (MET-003).
//! let read = range.counter(CounterKind::Read)?;
//! let write = range.counter(CounterKind::Write)?;
//! let passed = range.counter(CounterKind::PartitionsPassed)?;
//!
//! // The hot path: one relaxed atomic add, no error branch.
//! range.increment(read);
//! range.increment(write);
//!
//! // The range completed: fold interim into committed, record `run_info`, merge into the run.
//! range.increment(passed);
//! range.flush();
//! assert_eq!(
//!     range.run_info(),
//!     "Read: 1; Write: 1; Skipped: 0; Error: 0; Partitions Passed: 1; Partitions Failed: 0",
//! );
//! run.add(&range)?;
//!
//! // The run completed.
//! run.log_final_block(Some(RunId::from_raw(1_712_345_678_901_234)));
//! # Ok::<(), cdm_core::CdmError>(())
//! ```
//!
//! # Beyond parity
//!
//! The counters say what a run did; the rest of the crate says how fast, how far along, and what
//! happened:
//!
//! * [`Instruments`] — rows and bytes per second per side, latency percentiles per side and
//!   operation, in-flight requests, batch sizes, retries by cause and rate-limiter wait time
//!   (`MET-010`);
//! * [`ProgressTracker`] — weighted progress, ranges by state and an ETA that is honest about its
//!   error (`MET-011`);
//! * [`export::prometheus`] and [`export::otlp`] — the `GET /metrics` exposition and the OTLP
//!   payloads (`MET-020`, `MET-021`);
//! * [`EventBus`] and [`NdjsonSink`] — the structured event stream and its NDJSON transcription
//!   (`MET-030`);
//! * [`Dashboard`] and [`DashboardState`] — the terminal-agnostic view model the live display of
//!   `MET-031` draws, folded from the three above;
//! * [`logging`] — the `tracing` subscriber behind `logging.format` (`MET-032`);
//! * [`RunSummary`] — the one JSON document that says what a run did, which is the artefact a
//!   user attaches to a ticket (`MET-033`).
//!
//! The renderer of `MET-031` itself lives in `cdm-cli`, where `--tui` is parsed and where
//! `ratatui` is allowed to be a dependency; this crate supplies it with [`Dashboard`] and nothing
//! terminal-shaped.
//!
//! # Two rules this crate is built around
//!
//! **`SEC-001`.** No exported label, attribute or event field is a string somebody passed in.
//! Metric labels are the closed set of [`MetricLabels`]; every other dimension comes from a Rust
//! enum in this crate. There is no `record(name, value)` anywhere, which is what makes "a secret
//! cannot reach the metrics" a property of the types rather than of the reviewer's attention.
//!
//! **`SEC-002`.** Events carry identifiers and counts, never row payloads. The one place a row is
//! identifiable — a validate discrepancy — is redacted when the event is *constructed*
//! ([`Redaction`]), so no sink can leak what was never published.
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `MET-001` — [`CounterKind`]
//! - `MET-002` — [`registered_counters`], [`CounterKind::is_registered_for`]
//! - `MET-003` — [`Counter`], [`JobCounters::counter`]
//! - `MET-004` — [`CounterView`], [`JobCounters::flush`], [`JobCounters::add`]
//! - `MET-005` — [`JobCounters::metrics`], [`JobCounters::run_info`]
//! - `MET-006` — [`JobCounters::final_block`], [`JobCounters::log_final_block`]
//! - `MET-010` — [`Instruments`], [`RateMeter`], [`Histogram`], [`Gauge`]
//! - `MET-011` — [`ProgressTracker`], [`Progress`]
//! - `MET-020` — [`export::prometheus::render`], [`PrometheusExporter`], [`MetricLabels`]
//! - `MET-021` — [`OtlpExporter`], [`OtlpTransport`], [`SpanRecord`]
//! - `MET-030` — [`EventBus`], [`Event`], [`NdjsonSink`]
//! - `MET-031` — [`Dashboard`], [`DashboardState`], [`RangeTimings`] (the renderer is in `cdm-cli`)
//! - `MET-032` — [`logging::init`], [`logging::LogFormat`]
//! - `MET-033` — [`RunSummary`], [`DiscrepancySummary`]

mod counter;
mod registry;
mod report;
mod summary;

pub mod dashboard;
pub mod event;
pub mod export;
pub mod instrument;
pub mod label;
pub mod logging;
pub mod progress;

pub use counter::{registered_counters, CounterKind};
pub use dashboard::{Dashboard, DashboardState, ErrorLine, NodeStatus, RangeTimings};
pub use event::{
    DiscrepancyKind, Event, EventBus, EventPayload, EventRange, EventStreamError, EventSubscriber,
    KeyRef, NdjsonSink, Redaction,
};
pub use export::{
    MemoryTransport, MetricsReport, OtlpExporter, OtlpSignal, OtlpTransport, PrometheusExporter,
    SpanKind, SpanRecord,
};
pub use instrument::{
    Gauge, Histogram, HistogramSnapshot, InstrumentSnapshot, Instruments, Operation, RateMeter,
    RateSnapshot, RetryCause, SideSnapshot,
};
pub use label::MetricLabels;
pub use logging::{LogFormat, LoggingSetup};
pub use progress::{Progress, ProgressTracker, RangeEstimate};
pub use registry::{Counter, CounterView, JobCounters};
pub use report::{parse_run_info, FINAL_BLOCK_RULE, METRIC_SEPARATOR};
pub use summary::{
    DiscrepancyReportRef, DiscrepancySummary, NodeSummary, PlanSummary, RunSummary, Timings,
    SUMMARY_SCHEMA,
};

/// The version of this crate, as reported by `cdm version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    use super::*;

    #[test]
    fn version_is_populated() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn err_004_no_production_path_in_this_crate_can_panic() {
        // Clippy denies `unwrap`/`expect`/`panic!`/`todo!`/`unimplemented!` workspace-wide, but
        // only where it lints; a source-level sweep also covers code behind a `cfg` that a given
        // clippy invocation does not compile.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![root];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).unwrap() {
                    stack.push(entry.unwrap().path());
                }
                continue;
            }
            // `src/snapshots/*.snap` lives here too, and is data, not code.
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            // Everything from the test module onwards is exempt.
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (offset, line) in production.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                for construct in [
                    ".unwrap()",
                    ".expect(",
                    "panic!(",
                    "todo!(",
                    "unimplemented!(",
                ] {
                    if code.contains(construct) {
                        offenders.push(format!("{}:{}", path.display(), offset + 1));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "panicking constructs found: {offenders:?}"
        );
    }
}
