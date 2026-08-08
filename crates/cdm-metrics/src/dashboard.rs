//! The live view model behind the terminal UI (`MET-031`).
//!
//! `MET-031` asks for a terminal UI. This module is the half of it that is not a terminal: the
//! state a live display needs, folded from the sources that already exist — the weighted progress
//! and ETA of `MET-011`, the throughput meters of `MET-010`, and the structured event stream of
//! `MET-030` — into one [`Dashboard`] value a renderer can draw without knowing where any of it
//! came from.
//!
//! Keeping it here rather than in `cdm-cli` is what makes the terminal UI one renderer among
//! several: the same snapshot is what an SSE feed (`API-003`) or a web view (`UI-001`) would need,
//! and none of those may depend on a terminal library. `ratatui` is confined to `cdm-cli`, and
//! nothing in this module knows a cell from a frame.
//!
//! # Two sources, deliberately
//!
//! A live display draws from two places, and the split is not an accident:
//!
//! | Shown | Source | If a message is lost |
//! |---|---|---|
//! | progress, ETA, ranges by state, throughput | [`ProgressTracker`], [`Instruments`] — shared state | cannot happen |
//! | errors, warnings, discrepancy counts | the [`EventBus`](crate::EventBus) — a bounded broadcast | the count is still right; see below |
//!
//! `MET-030`'s bus is **bounded on purpose**: a subscriber that cannot keep up is told it lagged
//! and never gets to slow a migration down. That makes it the wrong place to read a progress bar
//! from — a dropped `RangeCompleted` would leave the bar permanently short — and the right place
//! to read a narrative from, where the worst case is a gap. So the numbers that must be exact are
//! read from shared state that cannot drop an update, and the tail is read from the bus.
//!
//! When the bus does drop, [`DashboardState::note_lag`] records how many, and
//! [`Dashboard::dropped_events`] is meant to be *displayed*. An operator who can see "412 events
//! dropped" knows the tail is incomplete; one who cannot see it believes a silent tail means a
//! quiet run.
//!
//! # `SEC-002`: what the tail may show
//!
//! An [`ErrorLine`] carries a code, a severity, a one-line title, a location and a token range.
//! It deliberately does **not** carry the diagnostic's `detail`, `value` or `suggestion`: those
//! are the fields that quote an offending input back at the operator, and a value that is safe in
//! a log file that one person reads is not obviously safe on a screen that is being shared, or
//! recorded, during an incident. The full diagnostic is in the log (`MET-032`) and in the NDJSON
//! event stream, both of which an operator opts into.
//!
//! A `Discrepancy` is counted and never quoted. `SEC-002` allows a validate finding to carry a
//! redacted key because a finding is not actionable without one, but a *screen* is not where
//! anyone acts on it — the diff log (`VAL-012`) and the discrepancy report (`VAL-013`) are — so
//! [`Dashboard`] keeps totals by kind and drops the key, the fingerprint and the column names on
//! the floor. Nothing row-derived reaches a renderer.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cdm_core::{JobKind, RunId, RunStatus, Severity, Side};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::event::{DiscrepancyKind, Event, EventPayload};
use crate::instrument::{Histogram, HistogramSnapshot, InstrumentSnapshot, Instruments};
use crate::progress::{Progress, ProgressTracker};

/// How many recent errors and warnings the tail keeps.
///
/// Sized for a screen, not for an archive: a run that produced thousands of errors has a log and
/// an event stream, and a display that tried to hold them all would grow without bound on exactly
/// the run that is already in trouble.
pub const ERROR_TAIL_CAPACITY: usize = 64;

/// How many samples the sparklines keep.
///
/// At the two-hertz refresh a terminal UI uses this is a minute of history, which is wider than
/// any terminal and therefore always enough to fill one.
pub const SPARKLINE_CAPACITY: usize = 120;

/// Nanoseconds in a millisecond: the histograms record the former, the sparkline plots the latter.
const NANOS_PER_MILLI: u64 = 1_000_000;

/// How many recent range durations [`RangeTimings`] keeps between samples.
const RECENT_TIMINGS_CAPACITY: usize = 4_096;

/// One line of the error tail (`MET-031`, `SEC-002`).
///
/// See the module documentation for why this is not simply a `Diagnostic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorLine {
    /// When it was published.
    pub at: DateTime<Utc>,
    /// How much it matters.
    pub severity: Severity,
    /// The stable diagnostic code, e.g. `CDM-CONNECT` (`ERR-002`).
    pub code: String,
    /// The one-line title. No detail, no value, no suggestion.
    pub title: String,
    /// Where the problem is, when the diagnostic said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// The token range it happened in, rendered as `min..max`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
}

/// One cluster node, as the driver reports it (`MET-031`).
///
/// # What this is, and what it is not
///
/// `MET-031` asks for "per-node status in cluster mode". There are two things that phrase can
/// mean, and only one of them is buildable today:
///
/// * the **cluster nodes cdm-rs is talking to** — origin and target Cassandra/ScyllaDB nodes,
///   their datacenter, rack and whether the driver has a connection pool to them. That is this
///   type, and it comes from the driver's own cluster metadata;
/// * the **cdm-rs nodes sharing one distributed run** — the coordinator's membership view, with
///   per-node counters and lease state (`DST-018`). That needs `cdm-cluster`, which is roadmap
///   items #50–#52 and not started; there is no membership table to read and no per-node counter
///   to display.
///
/// The second is deferred, and deliberately not faked: a display that invented a single-node
/// "cluster" would tell an operator that their distributed run was healthy when no such run
/// exists. When #50 lands, this type gains the cdm-rs nodes alongside the database ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeStatus {
    /// Which cluster the node belongs to.
    pub side: Side,
    /// The address the driver connects to it on.
    pub address: String,
    /// Its datacenter, when the driver knows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datacenter: Option<String>,
    /// Its rack, when the driver knows one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rack: Option<String>,
    /// Whether the driver currently holds a connection to it.
    pub connected: bool,
}

/// Where per-range wall-clock durations are collected, alongside the request latencies
/// (`MET-031`).
///
/// # A range duration is not a request latency, and both are worth having
///
/// `MET-010` defines request-latency percentiles per side and per operation, and
/// [`Instruments::latency`] holds them: `cdm-cql` brackets every driver request it issues and
/// records it through [`cdm_core::RequestObserver`], so those histograms are populated in a real
/// run and the display draws them. They answer "how fast is the *cluster* answering?".
///
/// A range duration answers a different question — "how long is a unit of work taking end to
/// end?" — and includes the conversion, the batching and the waiting that no request sees. The
/// scheduler already brackets it with `on_range_started` and `on_range_finished` (`ENG-002`), it
/// costs nothing per row, and a run whose requests are fast but whose ranges are slow is exactly
/// the situation the two numbers together diagnose. So both are kept, and each is labelled as
/// what it is.
///
/// Every method takes `&self`: the recorder is written by scheduler workers and read by the
/// display, on different threads.
#[derive(Debug)]
pub struct RangeTimings {
    histogram: Histogram,
    recent: Mutex<VecDeque<u64>>,
}

impl Default for RangeTimings {
    fn default() -> Self {
        Self::new()
    }
}

impl RangeTimings {
    /// An empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            histogram: Histogram::new(),
            recent: Mutex::new(VecDeque::new()),
        }
    }

    /// Records how long one range took (`ENG-002`).
    pub fn record(&self, elapsed: Duration) {
        self.histogram.record_duration(elapsed);
        let millis = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let mut recent = self.recent.lock();
        if recent.len() >= RECENT_TIMINGS_CAPACITY {
            recent.pop_front();
        }
        recent.push_back(millis);
    }

    /// The cumulative distribution, for the percentiles the panel prints.
    #[must_use]
    pub fn snapshot(&self) -> HistogramSnapshot {
        self.histogram.snapshot()
    }

    /// The mean of the durations recorded since the last call, in milliseconds.
    ///
    /// `None` when no range has completed since then, which a sparkline should draw as a
    /// continuation rather than as a drop to zero: no completion is not a zero-millisecond range.
    pub fn take_mean_millis(&self) -> Option<u64> {
        let mut recent = self.recent.lock();
        if recent.is_empty() {
            return None;
        }
        let total: u128 = recent.iter().map(|millis| u128::from(*millis)).sum();
        let count = u128::try_from(recent.len()).unwrap_or(1).max(1);
        recent.clear();
        u64::try_from(total / count).ok()
    }
}

/// The live state a terminal UI draws (`MET-031`).
///
/// One per run, owned by whatever is displaying it. It folds the lossy half of the picture — the
/// `MET-030` event stream — and borrows the exact half, which stays where the workers write it.
///
/// ```
/// use std::sync::Arc;
/// use std::time::Instant;
/// use cdm_core::{JobKind, RunId, RunStatus, TokenRange};
/// use cdm_metrics::{DashboardState, Instruments, ProgressTracker, RangeTimings};
///
/// let start = Instant::now();
/// let ranges = TokenRange::MURMUR3_FULL.split(4)?;
/// let progress = Arc::new(ProgressTracker::by_token_span(&ranges, start));
/// let state = DashboardState::new(
///     JobKind::Migrate,
///     RunId::from_raw(7),
///     "node-a",
///     Arc::clone(&progress),
///     Arc::new(Instruments::new(start)),
///     Arc::new(RangeTimings::new()),
/// );
///
/// progress.range_completed(ranges[0], RunStatus::Pass);
/// let view = state.snapshot_at(start + std::time::Duration::from_secs(30));
/// assert_eq!(view.progress.ranges_completed, 1);
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug)]
pub struct DashboardState {
    job: JobKind,
    run_id: RunId,
    node_id: String,
    keyspace: Option<String>,
    table: Option<String>,
    progress: std::sync::Arc<ProgressTracker>,
    instruments: std::sync::Arc<Instruments>,
    timings: std::sync::Arc<RangeTimings>,
    errors: VecDeque<ErrorLine>,
    errors_total: u64,
    warnings_total: u64,
    discrepancies: [u64; DiscrepancyKind::ALL.len()],
    dropped_events: u64,
    rows_history: VecDeque<u64>,
    latency_history: VecDeque<u64>,
    request_latency_history: VecDeque<u64>,
    /// The `(count, nanoseconds)` totals of every request histogram at the previous sample, so
    /// that the sparkline plots the mean over the *interval* rather than a cumulative average
    /// that flattens out an hour into a run.
    last_request_totals: (u64, u64),
    nodes: Vec<NodeStatus>,
    status: Option<RunStatus>,
    stopping: bool,
}

impl DashboardState {
    /// The state of a run that has just started.
    #[must_use]
    pub fn new(
        job: JobKind,
        run_id: RunId,
        node_id: impl Into<String>,
        progress: std::sync::Arc<ProgressTracker>,
        instruments: std::sync::Arc<Instruments>,
        timings: std::sync::Arc<RangeTimings>,
    ) -> Self {
        Self {
            job,
            run_id,
            node_id: node_id.into(),
            keyspace: None,
            table: None,
            progress,
            instruments,
            timings,
            errors: VecDeque::with_capacity(ERROR_TAIL_CAPACITY),
            errors_total: 0,
            warnings_total: 0,
            discrepancies: [0; DiscrepancyKind::ALL.len()],
            dropped_events: 0,
            rows_history: VecDeque::with_capacity(SPARKLINE_CAPACITY),
            latency_history: VecDeque::with_capacity(SPARKLINE_CAPACITY),
            request_latency_history: VecDeque::with_capacity(SPARKLINE_CAPACITY),
            last_request_totals: (0, 0),
            nodes: Vec::new(),
            status: None,
            stopping: false,
        }
    }

    /// Replaces the per-node view with what the driver currently reports (`MET-031`).
    pub fn set_nodes(&mut self, nodes: Vec<NodeStatus>) {
        self.nodes = nodes;
    }

    /// Records that the operator has asked the run to stop (`ENG-010`).
    ///
    /// Shown rather than acted on: stopping is the scheduler's business, and a display that said
    /// nothing between the keystroke and the last range draining would look like one that had
    /// ignored the keystroke.
    pub fn set_stopping(&mut self, stopping: bool) {
        self.stopping = stopping;
    }

    /// Folds one event into the state (`MET-030`).
    ///
    /// `RangeStarted` and `RangeCompleted` are deliberately ignored: progress is read from the
    /// tracker, which cannot lose an update. See the module documentation.
    pub fn apply(&mut self, event: &Event) {
        match &event.payload {
            EventPayload::RunStarted {
                keyspace, table, ..
            } => {
                self.keyspace.clone_from(keyspace);
                self.table.clone_from(table);
            }
            EventPayload::RangeStarted { .. } | EventPayload::RangeCompleted { .. } => {}
            EventPayload::Discrepancy { kind, .. } => {
                // Counted by kind. The key and the column names are not carried across.
                if let Some(slot) = self.discrepancies.get_mut(discrepancy_index(*kind)) {
                    *slot += 1;
                }
            }
            EventPayload::Warning { diagnostic } => {
                self.warnings_total += 1;
                self.push_line(ErrorLine {
                    at: event.at,
                    severity: diagnostic.severity,
                    code: diagnostic.code.clone(),
                    title: diagnostic.title.clone(),
                    location: diagnostic.location.clone(),
                    range: None,
                });
            }
            EventPayload::Error { diagnostic, range } => {
                self.errors_total += 1;
                self.push_line(ErrorLine {
                    at: event.at,
                    severity: diagnostic.severity,
                    code: diagnostic.code.clone(),
                    title: diagnostic.title.clone(),
                    location: diagnostic.location.clone(),
                    range: range
                        .as_ref()
                        .map(|range| format!("{}..{}", range.min, range.max)),
                });
            }
            EventPayload::RunCompleted { status, .. } => self.status = Some(*status),
        }
    }

    /// Records that the bus dropped `missed` events before this subscriber could read them.
    ///
    /// `MET-030`'s bus is bounded so that a slow display cannot apply backpressure to a migration.
    /// The price is that a display can miss part of the tail, and the only honest response is to
    /// say so — which is why this is a counter that gets drawn and not a log line that does not.
    pub fn note_lag(&mut self, missed: u64) {
        self.dropped_events = self.dropped_events.saturating_add(missed);
    }

    /// Takes one sparkline sample, reading the clock.
    pub fn sample(&mut self) {
        self.sample_at(Instant::now());
    }

    /// Takes one sparkline sample at an explicit instant.
    ///
    /// The throughput sample is the ten-second exponentially-weighted rate of `MET-010`, not the
    /// one-second one: at a two-hertz refresh the 1s window is mostly quantisation noise, and a
    /// sparkline of noise is a sparkline of nothing.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn sample_at(&mut self, now: Instant) {
        let rows = self.instruments.rows(Side::Origin).snapshot_at(now);
        push_bounded(
            &mut self.rows_history,
            rows.per_second_10s.max(0.0).round() as u64,
            SPARKLINE_CAPACITY,
        );

        // No completion since the last sample is not a zero-millisecond range: hold the last value
        // rather than drawing a cliff that did not happen.
        let latency = self
            .timings
            .take_mean_millis()
            .or_else(|| self.latency_history.back().copied())
            .unwrap_or(0);
        push_bounded(&mut self.latency_history, latency, SPARKLINE_CAPACITY);

        // MET-010's request latency, as a rate rather than a running average: the difference of
        // two cumulative totals is the mean of the requests that happened *between* the samples,
        // and it costs one snapshot per frame rather than any per-request state. Idle means "no
        // requests came back", which is not a zero-millisecond request, so the last value holds.
        let (count, nanos) = request_totals(&self.instruments.snapshot_at(now));
        let requests = count.saturating_sub(self.last_request_totals.0);
        let elapsed = nanos.saturating_sub(self.last_request_totals.1);
        self.last_request_totals = (count, nanos);
        let mean = elapsed
            .checked_div(requests)
            .map(|nanos| nanos / NANOS_PER_MILLI)
            .or_else(|| self.request_latency_history.back().copied())
            .unwrap_or(0);
        push_bounded(&mut self.request_latency_history, mean, SPARKLINE_CAPACITY);
    }

    /// Everything a renderer needs, at the current instant.
    #[must_use]
    pub fn snapshot(&self) -> Dashboard {
        self.snapshot_at(Instant::now())
    }

    /// Everything a renderer needs, at an explicit instant.
    #[must_use]
    pub fn snapshot_at(&self, now: Instant) -> Dashboard {
        Dashboard {
            job: self.job,
            run_id: self.run_id,
            node_id: self.node_id.clone(),
            keyspace: self.keyspace.clone(),
            table: self.table.clone(),
            progress: self.progress.snapshot_at(now),
            instruments: self.instruments.snapshot_at(now),
            range_latency: self.timings.snapshot(),
            rows_history: self.rows_history.iter().copied().collect(),
            latency_history: self.latency_history.iter().copied().collect(),
            request_latency_history: self.request_latency_history.iter().copied().collect(),
            errors: self.errors.iter().cloned().collect(),
            errors_total: self.errors_total,
            warnings_total: self.warnings_total,
            discrepancies: self.discrepancies,
            dropped_events: self.dropped_events,
            nodes: self.nodes.clone(),
            status: self.status,
            stopping: self.stopping,
        }
    }

    fn push_line(&mut self, line: ErrorLine) {
        if self.errors.len() >= ERROR_TAIL_CAPACITY {
            self.errors.pop_front();
        }
        self.errors.push_back(line);
    }
}

/// The `(requests, nanoseconds)` totals of every request-latency histogram in one snapshot.
///
/// Summed across both sides and all four operations: a sparkline plots one series, and the
/// breakdown is available separately for the panel that prints percentiles.
fn request_totals(snapshot: &InstrumentSnapshot) -> (u64, u64) {
    [Side::Origin, Side::Target]
        .into_iter()
        .flat_map(|side| snapshot.side(side).latency)
        .fold((0, 0), |(count, nanos), histogram| {
            (
                count.saturating_add(histogram.count),
                nanos.saturating_add(histogram.sum),
            )
        })
}

/// One instant of a run, as a display draws it (`MET-031`).
///
/// Serialisable so that the terminal UI is not the only thing that can consume it: the same
/// document is what a `GET /v1/runs/{id}` poller (`API-003`) or a web view (`UI-001`) needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dashboard {
    /// Which job is running.
    pub job: JobKind,
    /// The run.
    pub run_id: RunId,
    /// The node the run is on (`DST-018`).
    pub node_id: String,
    /// The keyspace, once `RunStarted` has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyspace: Option<String>,
    /// The table, once `RunStarted` has said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// Weighted progress, ranges by state and the ETA (`MET-011`).
    pub progress: Progress,
    /// Throughput and the rest of `MET-010`.
    pub instruments: InstrumentSnapshot,
    /// How long ranges have been taking. See [`RangeTimings`].
    pub range_latency: HistogramSnapshot,
    /// Origin rows per second, oldest sample first.
    pub rows_history: Vec<u64>,
    /// Mean range duration in milliseconds, oldest sample first.
    pub latency_history: Vec<u64>,
    /// Mean request latency in milliseconds over each sample interval, oldest sample first
    /// (`MET-010`).
    ///
    /// Across both sides and every operation, because a sparkline is one series. The per-side,
    /// per-operation percentiles the requirement also names are in
    /// [`instruments`](Dashboard::instruments), which is what the stats panel prints.
    pub request_latency_history: Vec<u64>,
    /// The most recent errors and warnings, oldest first (`SEC-002`).
    pub errors: Vec<ErrorLine>,
    /// How many errors the run has published, including any no longer in the tail.
    pub errors_total: u64,
    /// How many warnings the run has published.
    pub warnings_total: u64,
    /// Validate findings by kind, in [`DiscrepancyKind::ALL`] order. Counts only.
    pub discrepancies: [u64; DiscrepancyKind::ALL.len()],
    /// How many events the bounded bus dropped before this display could read them (`MET-030`).
    pub dropped_events: u64,
    /// The cluster nodes the driver reports. See [`NodeStatus`] for what is deferred.
    pub nodes: Vec<NodeStatus>,
    /// The run's terminal status, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RunStatus>,
    /// Whether a graceful stop has been requested (`ENG-010`).
    pub stopping: bool,
}

impl Dashboard {
    /// Rows per second on one side, over the ten-second window (`MET-010`).
    ///
    /// The ten-second window rather than the one-second one: at a display's refresh rate the 1s
    /// average is mostly quantisation noise, and a number that jitters by an order of magnitude
    /// between frames is one an operator learns to ignore.
    #[must_use]
    pub fn rows_per_second(&self, side: Side) -> f64 {
        self.instruments.side(side).rows.per_second_10s
    }

    /// How many validate findings of one kind have been seen.
    #[must_use]
    pub fn discrepancies_of(&self, kind: DiscrepancyKind) -> u64 {
        self.discrepancies
            .get(discrepancy_index(kind))
            .copied()
            .unwrap_or(0)
    }

    /// Every validate finding, of any kind.
    #[must_use]
    pub fn discrepancies_total(&self) -> u64 {
        self.discrepancies.iter().sum()
    }

    /// Whether the run has stopped, however it stopped.
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.status.is_some()
    }

    /// `keyspace.table`, or `—` before `RunStarted` has named them.
    #[must_use]
    pub fn table_label(&self) -> String {
        match (&self.keyspace, &self.table) {
            (Some(keyspace), Some(table)) => format!("{keyspace}.{table}"),
            (Some(keyspace), None) => keyspace.clone(),
            _ => "-".to_owned(),
        }
    }
}

/// A kind's slot in [`DiscrepancyKind::ALL`].
const fn discrepancy_index(kind: DiscrepancyKind) -> usize {
    match kind {
        DiscrepancyKind::Missing => 0,
        DiscrepancyKind::CorrectedMissing => 1,
        DiscrepancyKind::Mismatch => 2,
        DiscrepancyKind::CorrectedMismatch => 3,
    }
}

/// Pushes onto a ring buffer, dropping the oldest entry when it is full.
fn push_bounded(buffer: &mut VecDeque<u64>, value: u64, capacity: usize) {
    if buffer.len() >= capacity {
        buffer.pop_front();
    }
    buffer.push_back(value);
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

    use cdm_core::{Diagnostic, TokenRange};

    use super::*;
    use crate::event::{EventBus, EventRange, EventStreamError, Redaction};

    fn state(ranges: &[TokenRange], start: Instant) -> (DashboardState, Arc<ProgressTracker>) {
        let progress = Arc::new(ProgressTracker::by_token_span(ranges, start));
        let state = DashboardState::new(
            JobKind::Validate,
            RunId::from_raw(7),
            "node-a",
            Arc::clone(&progress),
            Arc::new(Instruments::new(start)),
            Arc::new(RangeTimings::new()),
        );
        (state, progress)
    }

    fn event(payload: EventPayload) -> Event {
        Event {
            run_id: RunId::from_raw(7),
            node_id: "node-a".to_owned(),
            at: DateTime::UNIX_EPOCH,
            payload,
        }
    }

    #[test]
    fn met_031_the_snapshot_carries_the_progress_bar_and_the_eta() {
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(4).unwrap();
        let (state, progress) = state(&ranges, start);

        progress.range_completed(ranges[0], RunStatus::Pass);
        let view = state.snapshot_at(start + Duration::from_secs(30));

        assert_eq!(view.progress.ranges_completed, 1);
        assert!((view.progress.weight_fraction - 0.25).abs() < 1e-9);
        assert_eq!(view.progress.eta, Some(Duration::from_secs(90)));
    }

    #[test]
    fn met_031_the_eta_is_withheld_until_it_means_something() {
        // `MET-011` withholds the ETA below `ETA_MIN_FRACTION`; a display must not invent one to
        // fill the space. The renderer's placeholder is tested in `cdm-cli`; what is tested here
        // is that the snapshot passes the absence through rather than defaulting it to zero.
        let start = Instant::now();
        let ranges: Vec<TokenRange> = (0..1_000)
            .map(|index| TokenRange::new(i128::from(index) * 100, i128::from(index) * 100 + 99))
            .collect::<Result<_, _>>()
            .unwrap();
        let (state, progress) = state(&ranges, start);

        assert_eq!(
            state
                .snapshot_at(start + Duration::from_secs(5))
                .progress
                .eta,
            None
        );
        progress.range_completed(ranges[0], RunStatus::Pass);
        assert_eq!(
            state
                .snapshot_at(start + Duration::from_secs(5))
                .progress
                .eta,
            None
        );

        for planned in ranges.iter().take(20).skip(1) {
            progress.range_completed(*planned, RunStatus::Pass);
        }
        assert!(state
            .snapshot_at(start + Duration::from_secs(20))
            .progress
            .eta
            .is_some());
    }

    #[test]
    fn met_031_progress_does_not_come_off_the_lossy_bus() {
        // The point of the two-source split: a `RangeCompleted` that the bus dropped must not
        // leave the bar short. Applying one changes nothing, because the tracker is the source.
        let start = Instant::now();
        let ranges = TokenRange::MURMUR3_FULL.split(4).unwrap();
        let (mut state, progress) = state(&ranges, start);

        state.apply(&event(EventPayload::RangeCompleted {
            range: EventRange::from(ranges[0]),
            status: RunStatus::Pass,
            run_info: "Read: 10".to_owned(),
        }));
        assert_eq!(state.snapshot_at(start).progress.ranges_completed, 0);

        progress.range_completed(ranges[0], RunStatus::Pass);
        assert_eq!(state.snapshot_at(start).progress.ranges_completed, 1);
    }

    #[test]
    fn met_031_the_error_tail_keeps_the_most_recent_and_counts_the_rest() {
        let start = Instant::now();
        let (mut state, _) = state(&TokenRange::MURMUR3_FULL.split(1).unwrap(), start);

        for index in 0..ERROR_TAIL_CAPACITY + 10 {
            state.apply(&event(EventPayload::Error {
                diagnostic: Diagnostic::error("CDM-INTERNAL", format!("failure {index}")),
                range: None,
            }));
        }

        let view = state.snapshot_at(start);
        assert_eq!(view.errors.len(), ERROR_TAIL_CAPACITY);
        assert_eq!(view.errors_total, ERROR_TAIL_CAPACITY as u64 + 10);
        // Oldest first, and the oldest survivor is the eleventh.
        assert_eq!(view.errors[0].title, "failure 10");
        assert_eq!(
            view.errors[ERROR_TAIL_CAPACITY - 1].title,
            format!("failure {}", ERROR_TAIL_CAPACITY + 9)
        );
    }

    #[test]
    fn met_031_sec_002_the_tail_carries_no_detail_no_value_and_no_suggestion() {
        // The three fields that quote an input back. A screen during an incident is not a private
        // log file, and `SEC-002` is not satisfied by "the value was probably fine".
        let start = Instant::now();
        let (mut state, _) = state(&TokenRange::MURMUR3_FULL.split(1).unwrap(), start);

        let diagnostic = Diagnostic::error("CDM-CONNECT", "the target refused the write")
            .with_detail("password=hunter2 was rejected")
            .with_value("hunter2")
            .with_suggestion("check connect.target.password");
        state.apply(&event(EventPayload::Error {
            diagnostic,
            range: Some(EventRange::from(TokenRange::new(-10, 10).unwrap())),
        }));

        let view = state.snapshot_at(start);
        let rendered = serde_json::to_string(&view).unwrap();
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert_eq!(view.errors[0].title, "the target refused the write");
        assert_eq!(view.errors[0].range.as_deref(), Some("-10..10"));
    }

    #[test]
    fn met_031_sec_002_a_discrepancy_is_counted_and_never_quoted() {
        let start = Instant::now();
        let (mut state, _) = state(&TokenRange::MURMUR3_FULL.split(1).unwrap(), start);

        // Built through the bus so that the redaction policy is the real one.
        let bus = EventBus::with_capacity(RunId::from_raw(7), "node-a", 16, Redaction::IncludeKeys);
        let mut events = bus.subscribe();
        bus.discrepancy(
            DateTime::UNIX_EPOCH,
            TokenRange::new(-10, 10).unwrap(),
            DiscrepancyKind::Mismatch,
            "customer-4711",
            vec!["email".to_owned()],
        );
        let published = events.try_recv().unwrap().unwrap();
        state.apply(&published);

        let view = state.snapshot_at(start);
        assert_eq!(view.discrepancies_of(DiscrepancyKind::Mismatch), 1);
        assert_eq!(view.discrepancies_total(), 1);
        // Even with keys deliberately in the clear on the bus, none of it reaches the display.
        let rendered = serde_json::to_string(&view).unwrap();
        assert!(!rendered.contains("customer-4711"), "{rendered}");
        assert!(!rendered.contains("email"), "{rendered}");
    }

    #[tokio::test]
    async fn met_031_a_display_that_falls_behind_reports_the_gap_rather_than_hiding_it() {
        // The bounded bus of `MET-030`: publishing must never block on a slow subscriber, and the
        // subscriber must be able to say how much it missed. Capacity 4, sixteen published.
        let start = Instant::now();
        let (mut state, _) = state(&TokenRange::MURMUR3_FULL.split(1).unwrap(), start);

        let bus = EventBus::with_capacity(RunId::from_raw(7), "node-a", 4, Redaction::default());
        let mut events = bus.subscribe();
        for index in 0..16 {
            bus.warning(
                DateTime::UNIX_EPOCH,
                Diagnostic::warning("CDM-INTERNAL", format!("warning {index}")),
            );
        }
        assert_eq!(bus.published(), 16, "publishing never blocked");

        let mut lagged = 0;
        loop {
            match events.try_recv() {
                Ok(Some(event)) => state.apply(&event),
                Err(EventStreamError::Lagged(missed)) => {
                    lagged += missed;
                    state.note_lag(missed);
                }
                // Nothing buffered, or the bus is gone: either way there is no more to fold.
                Ok(None) | Err(EventStreamError::Closed) => break,
            }
        }

        let view = state.snapshot_at(start);
        assert_eq!(
            lagged, 12,
            "twelve of sixteen fell out of a four-slot buffer"
        );
        assert_eq!(view.dropped_events, 12, "and the display says so");
        assert_eq!(
            view.warnings_total, 4,
            "the four that survived were folded in"
        );
        assert_eq!(view.errors.len(), 4);
    }

    #[test]
    fn met_031_sparkline_samples_are_bounded_and_hold_across_a_quiet_moment() {
        let start = Instant::now();
        let progress = Arc::new(ProgressTracker::by_token_span(
            &TokenRange::MURMUR3_FULL.split(1).unwrap(),
            start,
        ));
        let timings = Arc::new(RangeTimings::new());
        let mut state = DashboardState::new(
            JobKind::Migrate,
            RunId::from_raw(1),
            "node-a",
            progress,
            Arc::new(Instruments::new(start)),
            Arc::clone(&timings),
        );

        timings.record(Duration::from_millis(400));
        timings.record(Duration::from_millis(600));
        state.sample_at(start + Duration::from_secs(1));
        assert_eq!(state.snapshot_at(start).latency_history, vec![500]);

        // Nothing completed in this interval: hold, do not draw a cliff to zero.
        state.sample_at(start + Duration::from_secs(2));
        assert_eq!(state.snapshot_at(start).latency_history, vec![500, 500]);

        for second in 3..(SPARKLINE_CAPACITY as u64 + 20) {
            state.sample_at(start + Duration::from_secs(second));
        }
        let view = state.snapshot_at(start);
        assert_eq!(view.latency_history.len(), SPARKLINE_CAPACITY);
        assert_eq!(view.rows_history.len(), SPARKLINE_CAPACITY);
    }

    #[test]
    fn met_031_throughput_is_the_ten_second_window_of_met_010() {
        let start = Instant::now();
        let instruments = Arc::new(Instruments::new(start));
        let progress = Arc::new(ProgressTracker::by_token_span(
            &TokenRange::MURMUR3_FULL.split(1).unwrap(),
            start,
        ));
        let mut state = DashboardState::new(
            JobKind::Migrate,
            RunId::from_raw(1),
            "node-a",
            progress,
            Arc::clone(&instruments),
            Arc::new(RangeTimings::new()),
        );

        for second in 0..60 {
            instruments
                .rows(Side::Origin)
                .mark_at(1_000, start + Duration::from_secs(second));
        }
        let now = start + Duration::from_secs(60);
        state.sample_at(now);

        let view = state.snapshot_at(now);
        let rate = view.rows_per_second(Side::Origin);
        assert!((rate - 1_000.0).abs() < 100.0, "{rate}");
        assert_eq!(view.instruments.origin.rows.total, 60_000);
        assert!(*view.rows_history.last().unwrap() > 0);
    }

    #[test]
    fn met_031_run_started_names_the_table_and_run_completed_ends_the_run() {
        let start = Instant::now();
        let (mut state, _) = state(&TokenRange::MURMUR3_FULL.split(1).unwrap(), start);
        assert_eq!(state.snapshot_at(start).table_label(), "-");

        state.apply(&event(EventPayload::RunStarted {
            job: JobKind::Validate,
            keyspace: Some("ks".to_owned()),
            table: Some("tbl".to_owned()),
            ranges_planned: 1,
        }));
        let view = state.snapshot_at(start);
        assert_eq!(view.table_label(), "ks.tbl");
        assert!(!view.is_finished());

        state.apply(&event(EventPayload::RunCompleted {
            status: RunStatus::Ended,
            counters: std::collections::BTreeMap::new(),
            elapsed_secs: 1.0,
        }));
        let view = state.snapshot_at(start);
        assert!(view.is_finished());
        assert_eq!(view.status, Some(RunStatus::Ended));
    }

    #[test]
    fn met_031_the_snapshot_round_trips_through_json() {
        // `API-003` and `UI-001` are the other renderers this view model exists for.
        let start = Instant::now();
        let (mut state, _) = state(&TokenRange::MURMUR3_FULL.split(2).unwrap(), start);
        state.set_nodes(vec![NodeStatus {
            side: Side::Origin,
            address: "10.0.0.1:9042".to_owned(),
            datacenter: Some("dc1".to_owned()),
            rack: Some("rack1".to_owned()),
            connected: true,
        }]);
        state.set_stopping(true);

        let view = state.snapshot_at(start + Duration::from_secs(1));
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(serde_json::from_str::<Dashboard>(&json).unwrap(), view);
    }

    #[test]
    fn met_031_range_timings_report_a_distribution_and_drain_their_recent_window() {
        let timings = RangeTimings::new();
        assert_eq!(timings.take_mean_millis(), None);

        timings.record(Duration::from_millis(100));
        timings.record(Duration::from_millis(300));
        assert_eq!(timings.take_mean_millis(), Some(200));
        assert_eq!(timings.take_mean_millis(), None, "the window was drained");

        // The cumulative histogram keeps both, which is what the percentiles are read from.
        assert_eq!(timings.snapshot().count, 2);
    }
}
