//! Timing the requests this crate issues (`MET-010`).
//!
//! # Why the measurement belongs here
//!
//! `MET-010` asks for request-latency percentiles *per side and per operation*. A request is
//! issued in exactly four places, all of them in this crate: [`exec::RangeScan`] and
//! [`rows::CqlRowSource`] page the origin, and [`exec::TargetWriter`] and [`rows::CqlRowSink`]
//! write and look up the target. Nothing above the seam can see a request at all — `cdm-engine`
//! sees a page of rows and a bound write — so a histogram fed from up there would be measuring
//! something else, which is precisely how `MET-010`'s histograms came to be permanently empty
//! while the requirement was marked delivered.
//!
//! [`exec::RangeScan`]: crate::exec::RangeScan
//! [`exec::TargetWriter`]: crate::exec::TargetWriter
//! [`rows::CqlRowSource`]: crate::rows::CqlRowSource
//! [`rows::CqlRowSink`]: crate::rows::CqlRowSink
//!
//! # Cost, which is the whole design constraint
//!
//! A run issues one of these per page and one per written row. The recording therefore has to be
//! cheap enough to disappear into the request it measures:
//!
//! * **Nothing is observing** — [`RequestMetrics::default`] holds `None`, so
//!   `RequestMetrics::begin` is one null check on a fat pointer and returns immediately. No
//!   clock is read. That is what a `--tui`-less run pays, and it is why the harness only builds
//!   an observer when something is watching.
//! * **Something is observing** — two [`Instant::now`] calls (one per bracket), one relaxed
//!   atomic increment and one decrement for the in-flight gauge, and four relaxed atomic
//!   read-modify-writes for the histogram. No allocation, no mutex, no `await`, and no branch on
//!   any value that came off the wire.
//!
//! The bracket is an RAII guard rather than a pair of calls because the paths it wraps are
//! full of `?`: a guard that had to be closed by hand would leak an in-flight count on exactly the
//! failing request whose latency an operator most wants to see.
//!
//! # Cardinality
//!
//! Nothing here can name a token range, a primary key or a row value — [`RequestObserver`] has no
//! method that accepts one (`MET-020`, `SEC-002`).

use std::sync::Arc;
use std::time::Instant;

use cdm_core::{CdmError, ErrorKind, Operation, RequestObserver, RetryCause, Side};
use scylla::errors::{DbError, ExecutionError, RequestAttemptError};

/// Where this crate reports the requests it issues, or nowhere (`MET-010`).
///
/// Cheap to clone (an `Option<Arc<_>>`) and cheaper still to ignore: the default observes nothing
/// and costs a null check per request. Every executor in this crate holds one, and a caller that
/// wants the numbers hands in `cdm_metrics::Instruments`, which implements [`RequestObserver`].
#[derive(Debug, Clone, Default)]
pub struct RequestMetrics(Option<Arc<dyn RequestObserver>>);

impl RequestMetrics {
    /// Records nothing. The same as [`RequestMetrics::default`], nameable in a `const` context.
    #[must_use]
    pub const fn unobserved() -> Self {
        Self(None)
    }

    /// Records every request against `observer`.
    #[must_use]
    pub fn new(observer: Arc<dyn RequestObserver>) -> Self {
        Self(Some(observer))
    }

    /// Records against `observer` when there is one, and nothing when there is not.
    #[must_use]
    pub fn from_option(observer: Option<Arc<dyn RequestObserver>>) -> Self {
        Self(observer)
    }

    /// Whether anything is listening, which is the only branch on the hot path.
    #[must_use]
    pub const fn is_observed(&self) -> bool {
        self.0.is_some()
    }

    /// Brackets one attempt at `operation` against `side`.
    ///
    /// The returned guard records the latency and balances the in-flight gauge when it is
    /// dropped — on the success path, the `?` path and the cancellation path alike. `None` when
    /// nothing is observing, in which case no clock has been read.
    #[must_use]
    pub(crate) fn begin(&self, side: Side, operation: Operation) -> Option<RequestGuard<'_>> {
        let observer = self.0.as_deref()?;
        observer.request_started(side);
        Some(RequestGuard {
            observer,
            side,
            operation,
            started: Instant::now(),
        })
    }

    /// Records one paced re-issue, classified from the failure that provoked it (`CON-011`).
    pub(crate) fn retried(&self, error: &CdmError) {
        if let Some(observer) = self.0.as_deref() {
            observer.request_retried(retry_cause(error));
        }
    }

    /// Records the size of one executed batch (`MIG-020`).
    pub(crate) fn batch(&self, statements: usize) {
        if let Some(observer) = self.0.as_deref() {
            observer.batch_executed(statements as u64);
        }
    }

    /// Records bytes that crossed the wire from or to `side`.
    pub(crate) fn bytes(&self, side: Side, bytes: usize) {
        if let Some(observer) = self.0.as_deref() {
            observer.bytes_transferred(side, bytes as u64);
        }
    }
}

/// One in-flight request, recorded when it is dropped (`MET-010`).
#[derive(Debug)]
pub(crate) struct RequestGuard<'a> {
    observer: &'a dyn RequestObserver,
    side: Side,
    operation: Operation,
    started: Instant,
}

impl Drop for RequestGuard<'_> {
    /// A failed request is timed too: a coordinator that takes thirty seconds to time out is the
    /// single most informative latency sample a struggling cluster produces, and dropping it would
    /// make the percentiles look *better* the worse the target got.
    fn drop(&mut self) {
        self.observer
            .request_finished(self.side, self.operation, self.started.elapsed());
    }
}

/// Which of `MET-010`'s six causes a failure falls under.
///
/// Deliberately coarse. The driver's taxonomy has dozens of shapes and an operator reading a retry
/// breakdown is asking one question: is the target overloaded, unavailable, or merely slow? The
/// error's boxed source is the driver's own [`ExecutionError`], so the classification is made from
/// the same value the retry policy classified, not from a message.
fn retry_cause(error: &CdmError) -> RetryCause {
    let kind = error.kind();
    let Some(execution) =
        std::error::Error::source(error).and_then(|source| source.downcast_ref::<ExecutionError>())
    else {
        return timeout_for(kind);
    };
    match execution {
        ExecutionError::LastAttemptError(attempt) => attempt_cause(attempt, kind),
        ExecutionError::ConnectionPoolError(_) => RetryCause::ConnectionError,
        ExecutionError::RequestTimeout(_) => timeout_for(kind),
        _ => RetryCause::Other,
    }
}

/// The cause one driver attempt error carries.
fn attempt_cause(attempt: &RequestAttemptError, kind: ErrorKind) -> RetryCause {
    match attempt {
        RequestAttemptError::DbError(DbError::ReadTimeout { .. }, _) => RetryCause::ReadTimeout,
        RequestAttemptError::DbError(DbError::WriteTimeout { .. }, _) => RetryCause::WriteTimeout,
        RequestAttemptError::DbError(DbError::Unavailable { .. }, _) => RetryCause::Unavailable,
        RequestAttemptError::DbError(DbError::Overloaded, _) => RetryCause::Overloaded,
        RequestAttemptError::BrokenConnectionError(_)
        | RequestAttemptError::DbError(DbError::IsBootstrapping, _)
        | RequestAttemptError::UnableToAllocStreamId => RetryCause::ConnectionError,
        _ => timeout_for(kind),
    }
}

/// A client-side timeout has no server verdict attached, so the side of the statement decides
/// which of the two timeout causes it is.
const fn timeout_for(kind: ErrorKind) -> RetryCause {
    match kind {
        ErrorKind::Read => RetryCause::ReadTimeout,
        ErrorKind::Write => RetryCause::WriteTimeout,
        _ => RetryCause::Other,
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
    use std::sync::Mutex;
    use std::time::Duration;

    use scylla::statement::Consistency;

    use super::*;

    /// Everything an observer was told, for a test that has to prove the *wiring* rather than the
    /// recorder. `cdm-cql` cannot depend on `cdm-metrics`, so the double lives here and the real
    /// `Instruments` are exercised where they are built.
    #[derive(Debug, Default)]
    pub(crate) struct RecordingObserver {
        state: Mutex<Recorded>,
    }

    /// What [`RecordingObserver`] saw.
    #[derive(Debug, Default, Clone)]
    pub(crate) struct Recorded {
        /// Every completed request, in order.
        pub(crate) requests: Vec<(Side, Operation, Duration)>,
        /// The largest number of requests outstanding at once.
        pub(crate) peak_inflight: i64,
        /// The in-flight count now, which must be zero once a run is over.
        pub(crate) inflight: i64,
        /// Every retry's cause.
        pub(crate) retries: Vec<RetryCause>,
        /// Every executed batch's size.
        pub(crate) batches: Vec<u64>,
        /// Bytes reported per side.
        pub(crate) bytes: Vec<(Side, u64)>,
        /// Rate-limiter waits.
        pub(crate) waits: Vec<(Side, Duration)>,
    }

    impl RecordingObserver {
        /// A snapshot of everything recorded so far.
        pub(crate) fn recorded(&self) -> Recorded {
            self.state.lock().map(|s| s.clone()).unwrap_or_default()
        }

        fn with(&self, f: impl FnOnce(&mut Recorded)) {
            if let Ok(mut state) = self.state.lock() {
                f(&mut state);
            }
        }
    }

    impl RequestObserver for RecordingObserver {
        fn request_started(&self, _side: Side) {
            self.with(|state| {
                state.inflight += 1;
                state.peak_inflight = state.peak_inflight.max(state.inflight);
            });
        }

        fn request_finished(&self, side: Side, operation: Operation, elapsed: Duration) {
            self.with(|state| {
                state.inflight -= 1;
                state.requests.push((side, operation, elapsed));
            });
        }

        fn request_retried(&self, cause: RetryCause) {
            self.with(|state| state.retries.push(cause));
        }

        fn batch_executed(&self, statements: u64) {
            self.with(|state| state.batches.push(statements));
        }

        fn bytes_transferred(&self, side: Side, bytes: u64) {
            self.with(|state| state.bytes.push((side, bytes)));
        }

        fn ratelimit_waited(&self, side: Side, waited: Duration) {
            self.with(|state| state.waits.push((side, waited)));
        }
    }

    fn execution_error(db: DbError) -> ExecutionError {
        ExecutionError::LastAttemptError(RequestAttemptError::DbError(db, "boom".to_owned()))
    }

    fn read_failure(db: DbError) -> CdmError {
        CdmError::new(ErrorKind::Read, "the origin scan failed").with_source(execution_error(db))
    }

    #[test]
    fn met_010_an_unobserved_request_reads_no_clock() {
        // The default costs one null check. If this ever needs a clock, the `--tui`-less run — the
        // one every benchmark measures — starts paying for observability nobody asked for.
        let metrics = RequestMetrics::default();
        assert!(!metrics.is_observed());
        assert!(metrics.begin(Side::Origin, Operation::RangeRead).is_none());
        metrics.batch(5);
        metrics.bytes(Side::Origin, 1_024);
        metrics.retried(&read_failure(DbError::Overloaded));
    }

    #[test]
    fn met_010_a_guard_records_the_side_the_operation_and_the_in_flight_count() {
        let observer = Arc::new(RecordingObserver::default());
        let metrics = RequestMetrics::new(Arc::clone(&observer) as Arc<dyn RequestObserver>);

        {
            let _outer = metrics.begin(Side::Target, Operation::Write);
            let _inner = metrics.begin(Side::Target, Operation::Batch);
            assert_eq!(observer.recorded().inflight, 2);
        }

        let recorded = observer.recorded();
        assert_eq!(recorded.peak_inflight, 2);
        assert_eq!(recorded.inflight, 0, "every guard must balance its start");
        // Dropped innermost first.
        assert_eq!(recorded.requests[0].1, Operation::Batch);
        assert_eq!(recorded.requests[1].1, Operation::Write);
        assert!(recorded
            .requests
            .iter()
            .all(|(side, ..)| *side == Side::Target));
    }

    #[test]
    fn met_010_a_retry_is_classified_from_the_drivers_own_error() {
        let observer = Arc::new(RecordingObserver::default());
        let metrics = RequestMetrics::new(Arc::clone(&observer) as Arc<dyn RequestObserver>);

        metrics.retried(&read_failure(DbError::Overloaded));
        metrics.retried(&read_failure(DbError::ReadTimeout {
            consistency: Consistency::LocalQuorum,
            received: 1,
            required: 2,
            data_present: false,
        }));
        metrics.retried(
            &CdmError::new(ErrorKind::Write, "the target write failed").with_source(
                execution_error(DbError::WriteTimeout {
                    consistency: Consistency::LocalQuorum,
                    received: 1,
                    required: 2,
                    write_type: scylla::errors::WriteType::Simple,
                }),
            ),
        );
        metrics.retried(&read_failure(DbError::Unavailable {
            consistency: Consistency::LocalQuorum,
            required: 2,
            alive: 1,
        }));
        // A failure with no driver error under it still has a side, and a client-side timeout on a
        // read is a read timeout.
        metrics.retried(&CdmError::new(ErrorKind::Read, "no source"));
        metrics.retried(&CdmError::new(ErrorKind::Internal, "no side either"));

        assert_eq!(
            observer.recorded().retries,
            vec![
                RetryCause::Overloaded,
                RetryCause::ReadTimeout,
                RetryCause::WriteTimeout,
                RetryCause::Unavailable,
                RetryCause::ReadTimeout,
                RetryCause::Other,
            ]
        );
    }

    /// The files that issue the four requests `Operation` names, and nothing else.
    ///
    /// Deliberately not every file that touches a session: introspection (`schema::introspect`)
    /// and the capability probe (`connect::probe`) each issue a handful of queries at startup,
    /// which are not one of `MET-010`'s four operations and would only add noise to a
    /// distribution meant to describe the run.
    const REQUEST_PATHS: [(&str, &str); 3] = [
        ("src/exec/scan.rs", include_str!("exec/scan.rs")),
        ("src/exec/write.rs", include_str!("exec/write.rs")),
        ("src/rows.rs", include_str!("rows.rs")),
    ];

    /// Calls that put a statement on the wire.
    const REQUEST_CALLS: [&str; 3] = ["execute_unpaged(", "execute_single_page(", "session.batch("];

    /// Either the function times the request itself, or it delegates to the one helper that does.
    const TIMED: [&str; 2] = ["metrics.begin(", "self.retrying("];

    #[test]
    fn met_010_every_request_this_crate_issues_is_bracketed_by_the_latency_recorder() {
        // This is the test the original defect needed. `MET-010` was marked delivered with the
        // histograms in place and *nothing feeding them*, and every behavioural test still
        // passed, because a request that is not measured behaves exactly like one that is. The
        // only way to notice is to ask the source whether each call site is bracketed — and to
        // keep asking, so that the fifth request path somebody adds cannot repeat it.
        let mut checked = 0usize;
        for (name, source) in REQUEST_PATHS {
            let production = source.split("#[cfg(test)]").next().unwrap_or(source);
            // Methods in these files sit at one level of indentation, so `\n    }` closes one.
            for body in production.split("\n    }") {
                let Some(call) = REQUEST_CALLS.iter().find(|call| body.contains(**call)) else {
                    continue;
                };
                checked += 1;
                assert!(
                    TIMED.iter().any(|marker| body.contains(marker)),
                    "{name} issues `{call}` without timing it; MET-010's latency histogram for \
                     that operation would be short by every request this path makes"
                );
            }
        }
        assert_eq!(
            checked, 8,
            "the sweep must see all eight request paths: the range scan's page, the paged \
             source's page, the target write, the batch, the counter write and the three key \
             reads"
        );

        // `write` and `write_batch` are timed *by* `retrying`, so the marker they carry is only
        // evidence for as long as `retrying` still holds the bracket.
        let write = include_str!("exec/write.rs");
        let retrying = write
            .split("async fn retrying")
            .nth(1)
            .and_then(|rest| rest.split("\n    }").next())
            .expect("the shared retry loop is in exec/write.rs");
        assert!(
            retrying.contains("metrics.begin("),
            "the shared retry loop must time each attempt; `write` and `write_batch` have no \
             other bracket"
        );
    }

    #[test]
    fn met_010_batch_sizes_and_bytes_reach_the_observer() {
        let observer = Arc::new(RecordingObserver::default());
        let metrics = RequestMetrics::new(Arc::clone(&observer) as Arc<dyn RequestObserver>);
        metrics.batch(7);
        metrics.bytes(Side::Origin, 4_096);

        let recorded = observer.recorded();
        assert_eq!(recorded.batches, vec![7]);
        assert_eq!(recorded.bytes, vec![(Side::Origin, 4_096)]);
    }
}
