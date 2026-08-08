//! Throughput, latency and concurrency instruments (`MET-010`).
//!
//! The counters of `MET-001` say what a run *did*; the instruments here say how fast it is doing
//! it and where the time is going. `MET-010` names them: rows per second per side over three
//! windows, bytes per second, request-latency percentiles per side and per operation, in-flight
//! requests, batch-size distribution, retry counts by cause, and rate-limiter wait time. Ranges by
//! state and the estimated time to completion are the other half of `MET-010` and live in
//! [`progress`](crate::progress), because they are derived from the plan rather than from traffic.
//!
//! # Everything is a closed set
//!
//! Sides, operations and retry causes are Rust enums with a fixed number of variants, and the
//! instrument set is a fixed-size array indexed by them. There is no `record(name, value)` taking
//! a string, which is what makes `SEC-001` structural: a configuration value cannot become a
//! metric dimension because there is no function that would accept one. It also bounds
//! cardinality by construction — `MET-020` forbids a token range or a primary key from ever
//! becoming a label, and no such value can reach an instrument in the first place.
//!
//! [`Operation`] and [`RetryCause`] are declared in
//! [`cdm_core::observe`] and re-exported here. They have to be nameable by the
//! crates that *issue* the requests — `cdm-cql` for a driver request, `cdm-engine` for a
//! rate-limiter wait — and neither of those may depend on `cdm-metrics` (`ARCHITECTURE.md` §3).
//! [`Instruments`] implements [`RequestObserver`], which is the whole of the wiring: a caller
//! holds `Arc<dyn RequestObserver>` and never names a metrics type.
//!
//! # Which accounting these reflect
//!
//! Instruments measure work as it happens — the **interim** level of `MET-004`. See [`RateMeter`]
//! for why, and for the trap this crate is deliberately not falling into.

pub mod histogram;
pub mod rate;

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cdm_core::{RequestObserver, Side};
use serde::{Deserialize, Serialize};

pub use cdm_core::observe::{Operation, RetryCause};

pub use histogram::{Histogram, HistogramSnapshot, PERCENTILES, PERCENTILE_LABELS};
pub use rate::{RateMeter, RateSnapshot};

/// A signed gauge: in-flight requests, which go up and down (`MET-010`, `ENG-007`).
///
/// Signed because a decrement that races its increment must not wrap to eighteen quintillion; a
/// gauge that briefly reads `-1` is obviously an artefact, whereas `u64::MAX` looks like a
/// catastrophe and has cost more than one on-call engineer an hour.
#[derive(Debug, Default)]
pub struct Gauge(AtomicI64);

impl Gauge {
    /// A gauge reading zero.
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicI64::new(0))
    }

    /// Adds one.
    pub fn increment(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Subtracts one.
    pub fn decrement(&self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }

    /// The current value.
    #[must_use]
    pub fn get(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// The two sides' instruments, and the run-wide ones (`MET-010`).
///
/// One per run, shared by every worker: every method takes `&self` and touches one atomic, or one
/// atomic plus a once-a-second lock for the rate meters.
///
/// ```
/// use std::time::{Duration, Instant};
/// use cdm_core::Side;
/// use cdm_metrics::{Instruments, Operation, RetryCause};
///
/// let start = Instant::now();
/// let instruments = Instruments::new(start);
///
/// instruments.rows(Side::Origin).mark_at(1_000, start);
/// instruments.bytes(Side::Origin).mark_at(2_400_000, start);
/// instruments.latency(Side::Origin, Operation::RangeRead).record_duration(Duration::from_millis(12));
/// instruments.retry(RetryCause::Overloaded);
/// instruments.inflight(Side::Target).increment();
///
/// let snapshot = instruments.snapshot_at(start + Duration::from_secs(1));
/// assert_eq!(snapshot.origin.rows.total, 1_000);
/// assert_eq!(snapshot.retries_total(), 1);
/// assert_eq!(snapshot.target.inflight, 1);
/// ```
#[derive(Debug)]
pub struct Instruments {
    sides: [SideInstruments; 2],
    batch_size: Histogram,
    retries: [AtomicU64; RetryCause::ALL.len()],
    started_at: Instant,
}

/// One side's instruments.
#[derive(Debug)]
struct SideInstruments {
    rows: RateMeter,
    bytes: RateMeter,
    latency: [Histogram; Operation::ALL.len()],
    inflight: Gauge,
    ratelimit_wait: Histogram,
}

impl SideInstruments {
    fn new(now: Instant) -> Self {
        Self {
            rows: RateMeter::new(now),
            bytes: RateMeter::new(now),
            latency: std::array::from_fn(|_| Histogram::new()),
            inflight: Gauge::new(),
            ratelimit_wait: Histogram::new(),
        }
    }
}

impl Instruments {
    /// Builds the instrument set for a run starting at `now`.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            sides: [SideInstruments::new(now), SideInstruments::new(now)],
            batch_size: Histogram::new(),
            retries: std::array::from_fn(|_| AtomicU64::new(0)),
            started_at: now,
        }
    }

    /// Rows read from, or written to, `side` (`MET-010`).
    #[must_use]
    pub fn rows(&self, side: Side) -> &RateMeter {
        &self.side(side).rows
    }

    /// Bytes read from, or written to, `side` (`MET-010`).
    ///
    /// Fed from the serialized size of the values that crossed the wire, which the zero-copy
    /// passthrough of `MIG-040` knows exactly and a deserializing path knows approximately.
    #[must_use]
    pub fn bytes(&self, side: Side) -> &RateMeter {
        &self.side(side).bytes
    }

    /// The latency distribution of one operation against one side (`MET-010`).
    //
    // SAFETY-INVARIANT: `Operation::index` returns the variant's position in `Operation::ALL`,
    // which is `0..4`, and the array has exactly four elements;
    // `met_010_operations_and_causes_are_closed_sets_with_stable_labels` proves it for every
    // variant. An `Option` here would put an impossible case on the hot path.
    #[allow(clippy::indexing_slicing)]
    #[must_use]
    pub fn latency(&self, side: Side, operation: Operation) -> &Histogram {
        &self.side(side).latency[operation.index()]
    }

    /// Requests in flight against `side` (`MET-010`, `ENG-007`).
    #[must_use]
    pub fn inflight(&self, side: Side) -> &Gauge {
        &self.side(side).inflight
    }

    /// How long the rate limiter of `side` made a caller wait (`MET-010`, `ENG-005`).
    ///
    /// This is the instrument that answers "is the run slow, or is it being held back?" — a large
    /// wait time with low latency means the configured `perfops.ratelimit` is the bottleneck.
    #[must_use]
    pub fn ratelimit_wait(&self, side: Side) -> &Histogram {
        &self.side(side).ratelimit_wait
    }

    /// The distribution of executed batch sizes (`MET-010`, `MIG-020`).
    ///
    /// Worth watching next to `MIG-021`: a configured `batch_size` of 5 that reports a
    /// distribution pinned at 1 means the coercion fired.
    #[must_use]
    pub const fn batch_size(&self) -> &Histogram {
        &self.batch_size
    }

    /// Records one retry (`MET-010`).
    pub fn retry(&self, cause: RetryCause) {
        if let Some(slot) = self.retries.get(cause.index()) {
            slot.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// How many retries have been recorded for `cause`.
    #[must_use]
    pub fn retries(&self, cause: RetryCause) -> u64 {
        self.retries
            .get(cause.index())
            .map_or(0, |slot| slot.load(Ordering::Relaxed))
    }

    /// When the run started, which the snapshot's `uptime` is measured from.
    #[must_use]
    pub const fn started_at(&self) -> Instant {
        self.started_at
    }

    /// A point-in-time reading of everything, taking the clock.
    #[must_use]
    pub fn snapshot(&self) -> InstrumentSnapshot {
        self.snapshot_at(Instant::now())
    }

    /// A point-in-time reading of everything at an explicit instant.
    #[must_use]
    pub fn snapshot_at(&self, now: Instant) -> InstrumentSnapshot {
        InstrumentSnapshot {
            uptime: now.saturating_duration_since(self.started_at),
            origin: self.side_snapshot(Side::Origin, now),
            target: self.side_snapshot(Side::Target, now),
            batch_size: self.batch_size.snapshot(),
            retries: RetryCause::ALL.map(|cause| self.retries(cause)),
        }
    }

    /// The instruments of one side.
    //
    // SAFETY-INVARIANT: `Side` has exactly two variants and `sides` has exactly two elements;
    // `met_010_each_side_has_its_own_instruments` proves the mapping. An `Option` here would put
    // an impossible case on the hot path.
    #[allow(clippy::indexing_slicing)]
    fn side(&self, side: Side) -> &SideInstruments {
        match side {
            Side::Origin => &self.sides[0],
            Side::Target => &self.sides[1],
        }
    }

    fn side_snapshot(&self, side: Side, now: Instant) -> SideSnapshot {
        let instruments = self.side(side);
        SideSnapshot {
            side,
            rows: instruments.rows.snapshot_at(now),
            bytes: instruments.bytes.snapshot_at(now),
            latency: Operation::ALL.map(|operation| self.latency(side, operation).snapshot()),
            inflight: instruments.inflight.get(),
            ratelimit_wait: instruments.ratelimit_wait.snapshot(),
        }
    }
}

/// The one implementation that matters: the instruments *are* where a request is recorded
/// (`MET-010`).
///
/// This is what lets `cdm-cql` time a driver request and `cdm-engine` time a rate-limiter wait
/// without either of them depending on this crate — they hold
/// [`Arc<dyn RequestObserver>`](cdm_core::RequestObserver), which
/// [`cdm_core::observe`] declares and this type satisfies.
///
/// # Cost
///
/// Every method is a fixed number of relaxed atomic read-modify-writes on preallocated state:
/// four for a histogram record, one for a gauge, one for a counter. Nothing allocates, nothing
/// locks and nothing awaits, so a request pays for its own measurement and for nothing else. The
/// one exception is the byte meter, whose once-a-second tick takes a lock that
/// [`RateMeter::mark`] already documents.
impl RequestObserver for Instruments {
    fn request_started(&self, side: Side) {
        self.inflight(side).increment();
    }

    fn request_finished(&self, side: Side, operation: Operation, elapsed: Duration) {
        self.inflight(side).decrement();
        self.latency(side, operation).record_duration(elapsed);
    }

    fn request_retried(&self, cause: RetryCause) {
        self.retry(cause);
    }

    fn batch_executed(&self, statements: u64) {
        self.batch_size().record(statements);
    }

    fn bytes_transferred(&self, side: Side, bytes: u64) {
        self.bytes(side).mark(bytes);
    }

    fn ratelimit_waited(&self, side: Side, waited: Duration) {
        self.ratelimit_wait(side).record_duration(waited);
    }
}

/// Everything the instruments read at one instant (`MET-010`).
///
/// Serialisable, because this is what `GET /v1/runs/{id}/metrics` returns (`API-003`), what the
/// terminal UI of `MET-031` renders, and what the exporters of `MET-020` and `MET-021` translate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentSnapshot {
    /// How long the run has been going.
    pub uptime: Duration,
    /// The origin's instruments.
    pub origin: SideSnapshot,
    /// The target's instruments.
    pub target: SideSnapshot,
    /// Executed batch sizes (`MIG-020`).
    pub batch_size: HistogramSnapshot,
    /// Retries, in [`RetryCause::ALL`] order.
    pub retries: [u64; RetryCause::ALL.len()],
}

impl InstrumentSnapshot {
    /// One side's reading.
    #[must_use]
    pub const fn side(&self, side: Side) -> &SideSnapshot {
        match side {
            Side::Origin => &self.origin,
            Side::Target => &self.target,
        }
    }

    /// Retries for one cause.
    #[must_use]
    pub fn retries_for(&self, cause: RetryCause) -> u64 {
        self.retries.get(cause.index()).copied().unwrap_or(0)
    }

    /// Retries for every cause.
    #[must_use]
    pub fn retries_total(&self) -> u64 {
        self.retries.iter().sum()
    }

    /// The retry counts paired with their label values, in [`RetryCause::ALL`] order.
    #[must_use]
    pub fn retries_labelled(&self) -> Vec<(&'static str, u64)> {
        RetryCause::ALL
            .into_iter()
            .map(|cause| (cause.as_str(), self.retries_for(cause)))
            .collect()
    }
}

/// One side's instruments at one instant (`MET-010`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideSnapshot {
    /// Which side this is.
    pub side: Side,
    /// Rows per second, and the total.
    pub rows: RateSnapshot,
    /// Bytes per second, and the total.
    pub bytes: RateSnapshot,
    /// Latency percentiles, in [`Operation::ALL`] order.
    pub latency: [HistogramSnapshot; Operation::ALL.len()],
    /// Requests currently in flight (`ENG-007`).
    pub inflight: i64,
    /// Time spent waiting on the rate limiter (`ENG-005`).
    pub ratelimit_wait: HistogramSnapshot,
}

impl SideSnapshot {
    /// One operation's latency distribution.
    #[must_use]
    pub fn latency_for(&self, operation: Operation) -> HistogramSnapshot {
        self.latency
            .get(operation.index())
            .copied()
            .unwrap_or_default()
    }

    /// The latency distributions that recorded anything, paired with their operations.
    ///
    /// An operation a job never issues contributes no series at all, which is how a guardrail run
    /// avoids exporting four empty target latency families.
    #[must_use]
    pub fn recorded_latencies(&self) -> Vec<(Operation, HistogramSnapshot)> {
        Operation::ALL
            .into_iter()
            .map(|operation| (operation, self.latency_for(operation)))
            .filter(|(_, snapshot)| !snapshot.is_empty())
            .collect()
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
    use super::*;

    #[test]
    fn met_010_each_side_has_its_own_instruments() {
        // The invariant the `indexing_slicing` allow in `Instruments::side` rests on.
        let start = Instant::now();
        let instruments = Instruments::new(start);
        instruments.rows(Side::Origin).mark_at(10, start);
        instruments.rows(Side::Target).mark_at(3, start);
        instruments.inflight(Side::Target).increment();
        instruments.inflight(Side::Target).increment();
        instruments.inflight(Side::Target).decrement();

        let snapshot = instruments.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(snapshot.origin.rows.total, 10);
        assert_eq!(snapshot.target.rows.total, 3);
        assert_eq!(snapshot.origin.inflight, 0);
        assert_eq!(snapshot.target.inflight, 1);
        assert_eq!(snapshot.side(Side::Origin).side, Side::Origin);
        assert_eq!(snapshot.side(Side::Target).side, Side::Target);
    }

    #[test]
    fn met_010_every_instrument_the_specification_names_exists() {
        // `MET-010`'s list, item by item, so that a later refactor cannot quietly drop one.
        let start = Instant::now();
        let instruments = Instruments::new(start);

        instruments.rows(Side::Origin).mark_at(100, start); // rows/sec, origin
        instruments.rows(Side::Target).mark_at(90, start); // rows/sec, target
        instruments.bytes(Side::Origin).mark_at(4_096, start); // bytes/sec
        for side in [Side::Origin, Side::Target] {
            for operation in Operation::ALL {
                instruments
                    .latency(side, operation)
                    .record_duration(Duration::from_millis(5)); // latency per side/operation
            }
            instruments.inflight(side).increment(); // in-flight requests
            instruments
                .ratelimit_wait(side)
                .record_duration(Duration::from_micros(250)); // rate-limiter wait
        }
        instruments.batch_size().record(5); // batch size distribution
        for cause in RetryCause::ALL {
            instruments.retry(cause); // retries by cause
        }

        let snapshot = instruments.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(snapshot.uptime, Duration::from_secs(1));
        assert!(snapshot.origin.rows.per_second_1s > 0.0);
        assert_eq!(snapshot.origin.bytes.total, 4_096);
        assert_eq!(snapshot.origin.recorded_latencies().len(), 4);
        assert_eq!(snapshot.target.recorded_latencies().len(), 4);
        assert_eq!(snapshot.batch_size.count, 1);
        assert_eq!(snapshot.retries_total(), 6);
        assert_eq!(snapshot.retries_for(RetryCause::Overloaded), 1);
        assert!(!snapshot.origin.ratelimit_wait.is_empty());
    }

    #[test]
    fn met_010_rates_track_interim_activity_not_committed_totals() {
        // The distinction `MET-004` draws, and the trap `MIG-004` and `ENG-008` describe: Java
        // reads the committed level where it meant the interim one, and gets a permanent zero.
        // A rate meter must see a row when the row happens, not when its range is flushed.
        let start = Instant::now();
        let instruments = Instruments::new(start);
        let counters = crate::JobCounters::new(cdm_core::JobKind::Migrate);
        let read = counters.counter(crate::CounterKind::Read).unwrap();

        // A range reads 5_000 rows and has not finished, so nothing is committed yet.
        counters.increment_by(read, 5_000);
        instruments.rows(Side::Origin).mark_at(5_000, start);

        assert_eq!(counters.count(read, crate::CounterView::Committed), 0);
        assert_eq!(counters.count(read, crate::CounterView::Interim), 5_000);
        let snapshot = instruments.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(
            snapshot.origin.rows.total, 5_000,
            "the rate meter must see rows as they are read, not as they are committed"
        );
        assert!(snapshot.origin.rows.per_second_1s > 0.0);
    }

    #[test]
    fn met_010_operations_and_causes_are_closed_sets_with_stable_labels() {
        assert_eq!(
            Operation::ALL.map(Operation::as_str),
            ["range_read", "key_read", "write", "batch"]
        );
        assert_eq!(
            RetryCause::ALL.map(RetryCause::as_str),
            [
                "read_timeout",
                "write_timeout",
                "unavailable",
                "overloaded",
                "connection_error",
                "other",
            ]
        );
        for (slot, operation) in Operation::ALL.into_iter().enumerate() {
            assert_eq!(operation.index(), slot);
        }
        for (slot, cause) in RetryCause::ALL.into_iter().enumerate() {
            assert_eq!(cause.index(), slot);
        }
    }

    #[test]
    fn met_010_the_instruments_are_the_observer_the_request_paths_record_through() {
        // The seam `cdm-cql` and `cdm-engine` see. They hold `dyn RequestObserver` and cannot name
        // this type, so if this impl ever stopped landing in the right instrument, every one of
        // their tests would still pass and every percentile would still be zero.
        let start = Instant::now();
        let instruments = Instruments::new(start);
        let observer: &dyn RequestObserver = &instruments;

        observer.request_started(Side::Origin);
        observer.request_started(Side::Origin);
        assert_eq!(instruments.inflight(Side::Origin).get(), 2);
        observer.request_finished(
            Side::Origin,
            Operation::RangeRead,
            Duration::from_millis(12),
        );
        observer.request_finished(Side::Origin, Operation::RangeRead, Duration::from_millis(8));
        observer.request_started(Side::Target);
        observer.request_finished(Side::Target, Operation::Batch, Duration::from_millis(30));
        observer.request_retried(RetryCause::Overloaded);
        observer.batch_executed(5);
        observer.bytes_transferred(Side::Origin, 4_096);
        observer.ratelimit_waited(Side::Target, Duration::from_micros(750));

        let snapshot = instruments.snapshot_at(start + Duration::from_secs(1));
        assert_eq!(snapshot.origin.inflight, 0, "a finish must balance a start");
        assert_eq!(snapshot.target.inflight, 0);
        let range_read = snapshot.origin.latency_for(Operation::RangeRead);
        assert_eq!(range_read.count, 2);
        // Recorded in nanoseconds, and the reported percentile never understates.
        assert!(range_read.percentile(0.5) >= 8_000_000, "{range_read:?}");
        assert_eq!(snapshot.target.latency_for(Operation::Batch).count, 1);
        // The operation was recorded against the side it was issued to and no other.
        assert!(snapshot.target.latency_for(Operation::RangeRead).is_empty());
        assert_eq!(snapshot.retries_for(RetryCause::Overloaded), 1);
        assert_eq!(snapshot.batch_size.count, 1);
        assert_eq!(snapshot.origin.bytes.total, 4_096);
        assert_eq!(snapshot.target.ratelimit_wait.count, 1);
        assert!(snapshot.origin.ratelimit_wait.is_empty());
    }

    #[test]
    fn met_010_an_operation_that_never_ran_contributes_no_series() {
        let start = Instant::now();
        let instruments = Instruments::new(start);
        instruments
            .latency(Side::Origin, Operation::RangeRead)
            .record_duration(Duration::from_millis(1));

        let snapshot = instruments.snapshot_at(start);
        let recorded = snapshot.origin.recorded_latencies();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, Operation::RangeRead);
        assert!(snapshot.target.recorded_latencies().is_empty());
        assert!(snapshot.origin.latency_for(Operation::Batch).is_empty());
    }

    #[test]
    fn met_010_a_gauge_may_go_negative_rather_than_wrapping() {
        // A decrement racing its increment is an artefact worth seeing as `-1`, not as
        // eighteen quintillion.
        let gauge = Gauge::new();
        gauge.decrement();
        assert_eq!(gauge.get(), -1);
        gauge.increment();
        assert_eq!(gauge.get(), 0);
    }

    #[test]
    fn met_010_a_snapshot_round_trips_through_json() {
        // `API-003` serves this snapshot; the TUI of `MET-031` and the exporters read it back.
        let start = Instant::now();
        let instruments = Instruments::new(start);
        instruments.rows(Side::Origin).mark_at(7, start);
        instruments.retry(RetryCause::Unavailable);

        let snapshot = instruments.snapshot_at(start + Duration::from_secs(2));
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: InstrumentSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, snapshot);
        assert!(json.contains("\"unavailable\"") || parsed.retries_labelled().len() == 6);
    }
}
