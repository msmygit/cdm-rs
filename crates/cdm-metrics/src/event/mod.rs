//! The structured run-event bus (`MET-030`) and its redaction policy (`SEC-002`).
//!
//! Seven event kinds — `RunStarted`, `RangeStarted`, `RangeCompleted`, `Discrepancy`, `Warning`,
//! `Error`, `RunCompleted` — published on a bounded broadcast channel and consumed by whoever is
//! listening: the NDJSON sink of [`ndjson`], the SSE endpoint of `API-003`, the terminal UI of
//! `MET-031`, and the discrepancy report of `VAL-013`.
//!
//! # The bus never applies backpressure to the data path
//!
//! [`EventBus::publish`] is one non-blocking send. A subscriber that cannot keep up lags and is
//! told so ([`EventStreamError::Lagged`]); it is never able to slow a migration down.
//! `ARCHITECTURE.md` §9 is explicit about this, and it is the reason the channel is a
//! `tokio::sync::broadcast` rather than an mpsc queue with a bounded writer.
//!
//! # `SEC-002`: what an event may carry
//!
//! An event carries **identifiers and counts**, never row payloads. That rule has one edge, and it
//! is [`EventPayload::Discrepancy`]: a validate finding is only actionable if you can say *which*
//! row, and a primary key is itself row data.
//!
//! The resolution is that redaction happens at **construction**, not at the sink. [`EventBus`]
//! holds a [`Redaction`], and the constructors that take a key apply it before the event exists, so
//! a plain key never enters the channel and no downstream consumer — NDJSON file, SSE stream, log
//! — can leak one that was never there. The default is [`Redaction::Fingerprint`].
//!
//! A fingerprint is a 64-bit FNV-1a digest rendered as hex. It is a *correlation token*: it lets
//! an operator match a discrepancy in the event stream against one in the diff log, or count
//! distinct affected rows, without the key itself leaving the process. It is deliberately not a
//! cryptographic commitment — a low-cardinality key space (a boolean, a small enum) can be
//! enumerated against it, and anyone who needs the guarantee that the value cannot be recovered
//! should not be emitting the event at all. Saying so here is better than implying a security
//! property the digest does not have.
//!
//! Column *names* are carried; column *values* never are, in either redaction mode. The full
//! per-column detail of `VAL-006` goes to `cdm_diff.log` (`VAL-012`), which is the sanctioned
//! exception `SEC-002` names, and to the discrepancy report of `VAL-013` with its own
//! `validate.report.redact_values` switch.

pub mod ndjson;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use cdm_core::{Diagnostic, JobKind, RunId, RunStatus, TokenRange};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub use ndjson::NdjsonSink;

/// How many events the bus buffers before slow subscribers start to lag.
///
/// A thousand events is a few hundred kilobytes and, at the range granularity most events are
/// emitted at, several seconds of head start for a subscriber that has to write to a file.
pub const DEFAULT_CAPACITY: usize = 1_024;

/// Whether a primary key may appear in an event in the clear (`SEC-002`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Redaction {
    /// Keys are replaced by a fingerprint. The default, and what `SEC-002` requires.
    #[default]
    Fingerprint,
    /// Keys appear in the clear. Only ever set by an explicit operator opt-in, for a run whose
    /// event stream is going somewhere as sensitive as the data itself.
    IncludeKeys,
}

/// A primary key as an event carries it (`SEC-002`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum KeyRef {
    /// A digest of the key. See the module documentation for exactly what this does and does not
    /// guarantee.
    Fingerprint {
        /// Sixteen hexadecimal characters.
        fingerprint: String,
    },
    /// The key itself, present only when the operator opted in.
    Plain {
        /// The key, as the diff log renders it.
        key: String,
    },
}

impl KeyRef {
    /// Builds a key reference under a redaction policy.
    #[must_use]
    pub fn new(key: &str, redaction: Redaction) -> Self {
        match redaction {
            Redaction::Fingerprint => Self::Fingerprint {
                fingerprint: fingerprint(key),
            },
            Redaction::IncludeKeys => Self::Plain {
                key: key.to_owned(),
            },
        }
    }

    /// Whether this reference carries the key itself.
    #[must_use]
    pub const fn is_plain(&self) -> bool {
        matches!(self, Self::Plain { .. })
    }
}

/// The FNV-1a 64-bit digest of a key, as sixteen hexadecimal characters.
#[must_use]
pub fn fingerprint(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// What a validate run found (`VAL-002`, `VAL-003`, `VAL-006`, `VAL-007`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscrepancyKind {
    /// The row is absent from the target (`VAL-002`).
    Missing,
    /// The row was absent and autocorrect inserted it (`VAL-003`).
    CorrectedMissing,
    /// The row is present and differs (`VAL-006`).
    Mismatch,
    /// The row differed and autocorrect rewrote it (`VAL-007`).
    CorrectedMismatch,
}

impl DiscrepancyKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 4] = [
        Self::Missing,
        Self::CorrectedMissing,
        Self::Mismatch,
        Self::CorrectedMismatch,
    ];

    /// The stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::CorrectedMissing => "corrected_missing",
            Self::Mismatch => "mismatch",
            Self::CorrectedMismatch => "corrected_mismatch",
        }
    }
}

/// A token range as an event carries it (`MET-030`).
///
/// The bounds are decimal **strings**, not JSON numbers, for two reasons. A `RandomPartitioner`
/// token is a 127-bit integer (`TOK-002`), which no JSON consumer parses safely — JavaScript loses
/// precision above 2^53 and would silently round a range bound. And serde's flattening buffer, the
/// mechanism that gives [`Event`] its flat shape, does not carry `i128` at all. Strings are exact
/// for both, and are what `ENG-011`'s span fields and the OTLP attributes of `MET-021` already use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRange {
    /// The inclusive lower bound, in decimal.
    pub min: String,
    /// The inclusive upper bound, in decimal.
    pub max: String,
}

impl From<TokenRange> for EventRange {
    fn from(range: TokenRange) -> Self {
        Self {
            min: range.min().to_string(),
            max: range.max().to_string(),
        }
    }
}

/// One structured run event (`MET-030`).
///
/// The envelope is the same for every kind — run, node, timestamp — and the payload says what
/// happened. Serialises as one flat JSON object with a `type` discriminator, which is what makes
/// the NDJSON sink readable with `jq` and the SSE stream usable without a schema in hand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// The run.
    pub run_id: RunId,
    /// The node that emitted it (`DST-018`).
    pub node_id: String,
    /// When it happened. RFC 3339 UTC on the wire (`NFR-007`).
    pub at: DateTime<Utc>,
    /// What happened.
    #[serde(flatten)]
    pub payload: EventPayload,
}

/// The seven event kinds of `MET-030`.
///
/// Every field here is an identifier, a status, a count or a name. No field holds a row value, and
/// none holds a configuration value beyond the keyspace and table already published as metric
/// labels (`SEC-001`, `SEC-002`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    /// The run has been planned and is about to start.
    RunStarted {
        /// Which job.
        job: JobKind,
        /// The keyspace being processed, if resolved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keyspace: Option<String>,
        /// The table being processed, if resolved.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        table: Option<String>,
        /// How many ranges the plan holds (`TOK-003`).
        ranges_planned: u64,
    },
    /// A worker claimed a range (`ENG-002`).
    RangeStarted {
        /// The range.
        range: EventRange,
    },
    /// A range reached a terminal status (`ENG-002`, `TRK-021`).
    RangeCompleted {
        /// The range.
        range: EventRange,
        /// Its status, in the `TRK-012` spelling.
        status: RunStatus,
        /// The range's metrics string (`MET-005`) — the same text `cdm_run_details.run_info`
        /// stores, which is counts only.
        run_info: String,
    },
    /// Validate found a difference (`VAL-002`, `VAL-006`).
    Discrepancy {
        /// The range it was found in.
        range: EventRange,
        /// What kind of difference.
        kind: DiscrepancyKind,
        /// Which row, under the bus's redaction policy (`SEC-002`).
        key: KeyRef,
        /// The names of the columns that differ. Names only — never values.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        columns: Vec<String>,
    },
    /// Something an operator should know about but that did not fail anything.
    Warning {
        /// The structured diagnostic (`ERR-002`).
        diagnostic: Diagnostic,
    },
    /// Something failed. A failed range also produces a `RangeCompleted` with status `FAIL`.
    Error {
        /// The structured diagnostic (`ERR-002`).
        diagnostic: Diagnostic,
        /// The range it failed in, when it failed in one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<EventRange>,
    },
    /// The run finished, however it finished (`ENG-009`, `ENG-010`).
    RunCompleted {
        /// The run's terminal status.
        status: RunStatus,
        /// The committed counters (`MET-004`), under their `MET-001` names.
        counters: BTreeMap<String, u64>,
        /// How long the run took, in seconds.
        elapsed_secs: f64,
    },
}

impl EventPayload {
    /// The `type` discriminator this payload serialises with.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::RunStarted { .. } => "run_started",
            Self::RangeStarted { .. } => "range_started",
            Self::RangeCompleted { .. } => "range_completed",
            Self::Discrepancy { .. } => "discrepancy",
            Self::Warning { .. } => "warning",
            Self::Error { .. } => "error",
            Self::RunCompleted { .. } => "run_completed",
        }
    }

    /// Every discriminator, for documentation and for the schema of `MET-030`.
    pub const KINDS: [&'static str; 7] = [
        "run_started",
        "range_started",
        "range_completed",
        "discrepancy",
        "warning",
        "error",
        "run_completed",
    ];
}

/// Why a subscriber stopped receiving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStreamError {
    /// The run finished and the bus was dropped.
    Closed,
    /// The subscriber fell behind and missed this many events (`ARCHITECTURE.md` §9).
    ///
    /// Recoverable: the next receive returns the oldest event still buffered. A consumer that
    /// cares — the NDJSON sink does — should record the gap rather than pretend it did not happen.
    Lagged(u64),
}

/// The run event bus (`MET-030`).
///
/// ```
/// use cdm_core::{JobKind, RunId, RunStatus, TokenRange};
/// use chrono::{DateTime, Utc};
/// use cdm_metrics::{EventBus, EventPayload};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), cdm_core::CdmError> {
/// let bus = EventBus::new(RunId::from_raw(7), "node-a");
/// let mut events = bus.subscribe();
///
/// bus.range_started(DateTime::UNIX_EPOCH, TokenRange::new(0, 99)?);
///
/// let event = events.recv().await.expect("one event");
/// assert_eq!(event.payload.kind(), "range_started");
/// assert_eq!(event.node_id, "node-a");
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct EventBus {
    run_id: RunId,
    node_id: String,
    redaction: Redaction,
    sender: broadcast::Sender<Event>,
    published: AtomicU64,
    undelivered: AtomicU64,
}

impl EventBus {
    /// A bus with [`DEFAULT_CAPACITY`] and the default redaction of `SEC-002`.
    #[must_use]
    pub fn new(run_id: RunId, node_id: impl Into<String>) -> Self {
        Self::with_capacity(run_id, node_id, DEFAULT_CAPACITY, Redaction::default())
    }

    /// A bus with an explicit buffer size and redaction policy.
    ///
    /// A capacity of zero is raised to one, because a broadcast channel cannot have none.
    #[must_use]
    pub fn with_capacity(
        run_id: RunId,
        node_id: impl Into<String>,
        capacity: usize,
        redaction: Redaction,
    ) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            run_id,
            node_id: node_id.into(),
            redaction,
            sender,
            published: AtomicU64::new(0),
            undelivered: AtomicU64::new(0),
        }
    }

    /// The redaction policy in force (`SEC-002`).
    #[must_use]
    pub const fn redaction(&self) -> Redaction {
        self.redaction
    }

    /// How many events have been published.
    #[must_use]
    pub fn published(&self) -> u64 {
        self.published.load(Ordering::Relaxed)
    }

    /// How many were published with nobody subscribed, and therefore went nowhere.
    #[must_use]
    pub fn undelivered(&self) -> u64 {
        self.undelivered.load(Ordering::Relaxed)
    }

    /// How many subscribers are listening.
    #[must_use]
    pub fn subscribers(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Subscribes. A subscriber sees events published from now on, not the backlog.
    #[must_use]
    pub fn subscribe(&self) -> EventSubscriber {
        EventSubscriber {
            inner: self.sender.subscribe(),
        }
    }

    /// Publishes an event. Never blocks, never fails, never slows a worker down.
    pub fn publish(&self, at: DateTime<Utc>, payload: EventPayload) {
        let event = Event {
            run_id: self.run_id,
            node_id: self.node_id.clone(),
            at,
            payload,
        };
        self.published.fetch_add(1, Ordering::Relaxed);
        if self.sender.send(event).is_err() {
            // Nobody is listening. That is the normal case for a run with no event sink
            // configured (`metrics.events.sink = none`), so it is counted rather than logged.
            self.undelivered.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Publishes `RunStarted` (`MET-030`).
    pub fn run_started(
        &self,
        at: DateTime<Utc>,
        job: JobKind,
        keyspace: Option<String>,
        table: Option<String>,
        ranges_planned: u64,
    ) {
        self.publish(
            at,
            EventPayload::RunStarted {
                job,
                keyspace,
                table,
                ranges_planned,
            },
        );
    }

    /// Publishes `RangeStarted` (`ENG-002`).
    pub fn range_started(&self, at: DateTime<Utc>, range: TokenRange) {
        self.publish(
            at,
            EventPayload::RangeStarted {
                range: range.into(),
            },
        );
    }

    /// Publishes `RangeCompleted` (`ENG-002`, `TRK-021`).
    pub fn range_completed(
        &self,
        at: DateTime<Utc>,
        range: TokenRange,
        status: RunStatus,
        run_info: impl Into<String>,
    ) {
        self.publish(
            at,
            EventPayload::RangeCompleted {
                range: range.into(),
                status,
                run_info: run_info.into(),
            },
        );
    }

    /// Publishes `Discrepancy`, applying the bus's redaction policy to the key (`SEC-002`).
    ///
    /// This is the only constructor that takes row-derived data, and it is why redaction lives on
    /// the bus: the key is transformed here, before the event exists, so no consumer can leak a
    /// key that the policy said to withhold.
    pub fn discrepancy(
        &self,
        at: DateTime<Utc>,
        range: TokenRange,
        kind: DiscrepancyKind,
        key: &str,
        columns: Vec<String>,
    ) {
        self.publish(
            at,
            EventPayload::Discrepancy {
                range: range.into(),
                kind,
                key: KeyRef::new(key, self.redaction),
                columns,
            },
        );
    }

    /// Publishes `Warning` (`ERR-002`).
    pub fn warning(&self, at: DateTime<Utc>, diagnostic: Diagnostic) {
        self.publish(at, EventPayload::Warning { diagnostic });
    }

    /// Publishes `Error` (`ERR-002`, `ENG-008`).
    pub fn error(&self, at: DateTime<Utc>, diagnostic: Diagnostic, range: Option<TokenRange>) {
        self.publish(
            at,
            EventPayload::Error {
                diagnostic,
                range: range.map(EventRange::from),
            },
        );
    }

    /// Publishes `RunCompleted` (`MET-030`).
    pub fn run_completed(
        &self,
        at: DateTime<Utc>,
        status: RunStatus,
        counters: BTreeMap<String, u64>,
        elapsed: std::time::Duration,
    ) {
        self.publish(
            at,
            EventPayload::RunCompleted {
                status,
                counters,
                elapsed_secs: elapsed.as_secs_f64(),
            },
        );
    }
}

/// One consumer of the event bus (`MET-030`).
#[derive(Debug)]
pub struct EventSubscriber {
    inner: broadcast::Receiver<Event>,
}

impl EventSubscriber {
    /// Waits for the next event.
    ///
    /// # Errors
    ///
    /// [`EventStreamError::Lagged`] when this subscriber fell behind, naming how many events it
    /// missed; receiving again after that succeeds. [`EventStreamError::Closed`] when the run is
    /// over and the bus has been dropped.
    pub async fn recv(&mut self) -> Result<Event, EventStreamError> {
        match self.inner.recv().await {
            Ok(event) => Ok(event),
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                Err(EventStreamError::Lagged(missed))
            }
            Err(broadcast::error::RecvError::Closed) => Err(EventStreamError::Closed),
        }
    }

    /// Takes the next event if one is already buffered.
    ///
    /// # Errors
    ///
    /// As [`EventSubscriber::recv`], and `None` rather than an error when the queue is merely
    /// empty.
    pub fn try_recv(&mut self) -> Result<Option<Event>, EventStreamError> {
        match self.inner.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(broadcast::error::TryRecvError::Empty) => Ok(None),
            Err(broadcast::error::TryRecvError::Lagged(missed)) => {
                Err(EventStreamError::Lagged(missed))
            }
            Err(broadcast::error::TryRecvError::Closed) => Err(EventStreamError::Closed),
        }
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
    use cdm_core::{ErrorKind, Severity};

    use super::*;

    pub(crate) fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    /// One event of every kind, in `MET-030`'s declaration order.
    pub(crate) fn every_kind(bus: &EventBus) {
        let at = DateTime::UNIX_EPOCH;
        bus.run_started(
            at,
            JobKind::Validate,
            Some("target_ks".to_owned()),
            Some("orders".to_owned()),
            4,
        );
        bus.range_started(at, range(0, 99));
        bus.range_completed(at, range(0, 99), RunStatus::Diff, "Read: 3; Valid: 2");
        bus.discrepancy(
            at,
            range(0, 99),
            DiscrepancyKind::Mismatch,
            "customer_id=8f2c1b04-0000-0000-0000-000000000001",
            vec!["email".to_owned(), "updated_at".to_owned()],
        );
        bus.warning(
            at,
            Diagnostic::new(
                "CDM-CONFIG",
                Severity::Warning,
                "the origin has no local datacenter configured",
            ),
        );
        bus.error(
            at,
            cdm_core::CdmError::new(ErrorKind::Read, "the coordinator timed out").to_diagnostic(),
            Some(range(0, 99)),
        );
        bus.run_completed(
            at,
            RunStatus::Ended,
            [("READ".to_owned(), 3_u64)].into_iter().collect(),
            std::time::Duration::from_secs(90),
        );
    }

    #[tokio::test]
    async fn met_030_every_specified_event_kind_is_emitted_and_named() {
        let bus = EventBus::new(RunId::from_raw(7), "node-a");
        let mut events = bus.subscribe();
        every_kind(&bus);

        let mut kinds = Vec::new();
        while let Ok(Some(event)) = events.try_recv() {
            assert_eq!(event.run_id, RunId::from_raw(7));
            assert_eq!(event.node_id, "node-a");
            kinds.push(event.payload.kind());
        }
        assert_eq!(kinds, EventPayload::KINDS);
        assert_eq!(bus.published(), 7);
        assert_eq!(bus.undelivered(), 0);
    }

    #[tokio::test]
    async fn met_030_events_serialise_with_a_type_discriminator() {
        let bus = EventBus::new(RunId::from_raw(7), "node-a");
        let mut events = bus.subscribe();
        bus.range_completed(
            DateTime::UNIX_EPOCH,
            range(-9_000, -1),
            RunStatus::Pass,
            "Read: 10; Write: 10",
        );

        let event = events.recv().await.unwrap();
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "range_completed");
        assert_eq!(json["run_id"], 7);
        assert_eq!(json["status"], "PASS");
        assert_eq!(json["range"]["min"], "-9000");
        assert_eq!(json["at"], "1970-01-01T00:00:00Z");
        assert_eq!(
            serde_json::from_value::<Event>(json).unwrap(),
            event,
            "SSE and NDJSON consumers read these back"
        );
    }

    #[tokio::test]
    async fn sec_002_a_discrepancy_carries_a_fingerprint_not_the_key() {
        let bus = EventBus::new(RunId::from_raw(1), "node-a");
        let mut events = bus.subscribe();
        assert_eq!(bus.redaction(), Redaction::Fingerprint);

        bus.discrepancy(
            DateTime::UNIX_EPOCH,
            range(0, 9),
            DiscrepancyKind::Missing,
            "id=alice@example.com",
            vec!["email".to_owned()],
        );

        let event = events.recv().await.unwrap();
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("alice"), "{json}");
        assert!(!json.contains("example.com"), "{json}");
        assert!(json.contains("\"form\":\"fingerprint\""), "{json}");
        // Column names are carried; column values are not, in either mode.
        assert!(json.contains("\"email\""), "{json}");

        let EventPayload::Discrepancy { key, .. } = &event.payload else {
            panic!("expected a discrepancy");
        };
        assert!(!key.is_plain());
        assert_eq!(
            *key,
            KeyRef::Fingerprint {
                fingerprint: fingerprint("id=alice@example.com")
            }
        );
    }

    #[tokio::test]
    async fn sec_002_a_key_appears_in_the_clear_only_on_an_explicit_opt_in() {
        let bus = EventBus::with_capacity(RunId::from_raw(1), "node-a", 16, Redaction::IncludeKeys);
        let mut events = bus.subscribe();
        bus.discrepancy(
            DateTime::UNIX_EPOCH,
            range(0, 9),
            DiscrepancyKind::Mismatch,
            "id=alice@example.com",
            Vec::new(),
        );

        let event = events.recv().await.unwrap();
        let EventPayload::Discrepancy { key, .. } = &event.payload else {
            panic!("expected a discrepancy");
        };
        assert!(key.is_plain());
        assert!(serde_json::to_string(&event).unwrap().contains("alice"));
    }

    #[test]
    fn sec_002_a_fingerprint_is_stable_and_distinguishes_keys() {
        assert_eq!(fingerprint("a"), fingerprint("a"));
        assert_ne!(fingerprint("a"), fingerprint("b"));
        assert_eq!(fingerprint("").len(), 16);
        assert!(fingerprint("anything")
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn met_030_a_slow_subscriber_lags_and_is_told_so_rather_than_blocking_the_run() {
        // `ARCHITECTURE.md` §9: the bus never applies backpressure to the data path.
        let bus = EventBus::with_capacity(RunId::from_raw(1), "n", 2, Redaction::default());
        let mut slow = bus.subscribe();

        for index in 0..10 {
            bus.range_started(DateTime::UNIX_EPOCH, range(index, index));
        }
        assert_eq!(bus.published(), 10, "publishing never blocked");

        let missed = match slow.recv().await {
            Err(EventStreamError::Lagged(missed)) => missed,
            other => panic!("expected a lag, got {other:?}"),
        };
        assert_eq!(missed, 8);
        // And the subscriber carries on from the oldest event still buffered.
        assert!(slow.recv().await.is_ok());
    }

    #[tokio::test]
    async fn met_030_publishing_with_nobody_listening_is_counted_not_lost_silently() {
        let bus = EventBus::new(RunId::from_raw(1), "n");
        assert_eq!(bus.subscribers(), 0);
        bus.range_started(DateTime::UNIX_EPOCH, range(0, 1));
        assert_eq!(bus.published(), 1);
        assert_eq!(bus.undelivered(), 1);

        let subscriber = bus.subscribe();
        assert_eq!(bus.subscribers(), 1);
        bus.range_started(DateTime::UNIX_EPOCH, range(2, 3));
        assert_eq!(bus.undelivered(), 1);
        drop(subscriber);
    }

    #[tokio::test]
    async fn met_030_a_subscriber_is_told_when_the_run_is_over() {
        let bus = EventBus::new(RunId::from_raw(1), "n");
        let mut events = bus.subscribe();
        bus.range_started(DateTime::UNIX_EPOCH, range(0, 1));
        drop(bus);

        assert!(events.recv().await.is_ok());
        assert_eq!(events.recv().await, Err(EventStreamError::Closed));
        assert_eq!(events.try_recv(), Err(EventStreamError::Closed));
    }

    #[test]
    fn met_030_the_discrepancy_kinds_are_the_validate_outcomes() {
        assert_eq!(
            DiscrepancyKind::ALL.map(DiscrepancyKind::as_str),
            [
                "missing",
                "corrected_missing",
                "mismatch",
                "corrected_mismatch"
            ]
        );
        for kind in DiscrepancyKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
        }
    }
}
