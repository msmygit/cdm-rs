//! The seam a crate that issues requests records them through (`MET-010`).
//!
//! # Why the dimensions live here and not in `cdm-metrics`
//!
//! `MET-010` asks for request-latency percentiles *per side and per operation*, in-flight
//! requests, batch sizes, retries by cause and rate-limiter wait time. The instruments that hold
//! those numbers are `cdm-metrics`' business. The places that can *measure* them are not: a
//! request is issued by `cdm-cql`, which owns the driver session, and a rate-limiter wait happens
//! in `cdm-engine`, which owns the limiter. Neither may depend on `cdm-metrics` —
//! `ARCHITECTURE.md` §3 draws no such edge, and adding one would put the metric registry
//! underneath the driver.
//!
//! So the vocabulary ([`Operation`], [`RetryCause`]) and the trait ([`RequestObserver`]) live in
//! `cdm-core`, which everything already depends on, and `cdm_metrics::Instruments` implements the
//! trait. `cdm-cql` records against `dyn RequestObserver` and never names a metrics type; the
//! wiring is done once, by the crate that builds both.
//!
//! # Cardinality is closed by construction
//!
//! [`Operation`] and [`RetryCause`] are enums with a fixed number of variants, and there is no
//! method here that takes a string. `MET-020` forbids a token range or a primary key from
//! becoming a label, and no such value can reach an observer: there is no function that would
//! accept one.
//!
//! # This crate still reads no clock
//!
//! Every method takes an already-measured [`Duration`]. `cdm-core` names no [`Instant`] and calls
//! no clock (`ARCHITECTURE.md` §3.2); the caller that issued the request is the one that timed it.
//!
//! [`Instant`]: std::time::Instant

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::Side;

/// A request cdm-rs issues, as a latency dimension (`MET-010`).
///
/// The four kinds are the four statements the jobs execute. A job that never issues one — a
/// guardrail run opens no target connection at all (`GRD-001`) — leaves its histogram empty, and
/// an empty histogram is not exported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// The origin token-range select of `FEA-060`, timed per page (`ENG-003`).
    RangeRead,
    /// A select by primary key: validate's comparison read (`VAL-001`) and the counter
    /// pre-read of `MIG-031`.
    KeyRead,
    /// A single-statement write to the target (`MIG-010`, `MIG-030`).
    Write,
    /// An unlogged batch (`MIG-020`).
    Batch,
}

impl Operation {
    /// Every operation, in declaration order.
    pub const ALL: [Self; 4] = [Self::RangeRead, Self::KeyRead, Self::Write, Self::Batch];

    /// The stable label value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RangeRead => "range_read",
            Self::KeyRead => "key_read",
            Self::Write => "write",
            Self::Batch => "batch",
        }
    }

    /// This operation's slot in [`Operation::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::RangeRead => 0,
            Self::KeyRead => 1,
            Self::Write => 2,
            Self::Batch => 3,
        }
    }
}

/// Why a request was retried (`MET-010`, `CON-011`).
///
/// Closed, and deliberately coarser than the driver's error taxonomy: an operator reading a retry
/// breakdown wants to know whether the target is overloaded, unavailable or simply slow, not which
/// of a dozen driver error codes was returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryCause {
    /// The coordinator timed out waiting for replicas on a read.
    ReadTimeout,
    /// The coordinator timed out waiting for replicas on a write.
    WriteTimeout,
    /// Not enough replicas were up to satisfy the consistency level.
    Unavailable,
    /// The replica reported `OVERLOADED` — the signal `ENG-006` reacts to.
    Overloaded,
    /// The connection failed, or the node went away mid-request.
    ConnectionError,
    /// Anything else the retry policy decided to retry.
    Other,
}

impl RetryCause {
    /// Every cause, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::ReadTimeout,
        Self::WriteTimeout,
        Self::Unavailable,
        Self::Overloaded,
        Self::ConnectionError,
        Self::Other,
    ];

    /// The stable label value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadTimeout => "read_timeout",
            Self::WriteTimeout => "write_timeout",
            Self::Unavailable => "unavailable",
            Self::Overloaded => "overloaded",
            Self::ConnectionError => "connection_error",
            Self::Other => "other",
        }
    }

    /// This cause's slot in [`RetryCause::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::ReadTimeout => 0,
            Self::WriteTimeout => 1,
            Self::Unavailable => 2,
            Self::Overloaded => 3,
            Self::ConnectionError => 4,
            Self::Other => 5,
        }
    }
}

/// Where a crate that issues requests reports them (`MET-010`).
///
/// One implementation matters — `cdm_metrics::Instruments` — and it is the reason every method
/// takes `&self` and returns nothing: recording must be a handful of relaxed atomics on the hot
/// path, with no allocation, no lock and nothing to await. An implementation that blocked here
/// would slow the migration down by exactly as much as it measured.
///
/// # What a caller is expected to do
///
/// Bracket each attempt: [`request_started`](Self::request_started) before it goes out,
/// [`request_finished`](Self::request_finished) when it comes back — including when it comes back
/// as an error, because a timeout is a latency observation and usually the interesting one. A
/// paced retry calls [`request_retried`](Self::request_retried) once per re-issue, so that
/// `attempts - 1` retries are counted for a request that eventually succeeded.
pub trait RequestObserver: std::fmt::Debug + Send + Sync {
    /// One more request is outstanding against `side` (`MET-010`, `ENG-007`).
    fn request_started(&self, side: Side);

    /// One request against `side` has come back, after `elapsed` (`MET-010`).
    ///
    /// Balances one [`request_started`](Self::request_started) and records the latency of
    /// `operation`.
    fn request_finished(&self, side: Side, operation: Operation, elapsed: Duration);

    /// One request was re-issued because of `cause` (`MET-010`, `CON-011`).
    fn request_retried(&self, cause: RetryCause);

    /// A batch of `statements` statements was executed (`MET-010`, `MIG-020`).
    fn batch_executed(&self, statements: u64);

    /// `bytes` bytes crossed the wire from or to `side` (`MET-010`).
    fn bytes_transferred(&self, side: Side, bytes: u64);

    /// A caller waited `waited` for `side`'s rate limiter (`MET-010`, `ENG-005`).
    fn ratelimit_waited(&self, side: Side, waited: Duration);
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
    fn met_010_the_observer_seam_names_nothing_a_cardinality_bomb_could_travel_through() {
        // `MET-020` forbids a token range or a primary key from becoming a label. The structural
        // guarantee is that no method here accepts a string or an unbounded type, so a caller
        // could not pass one even by mistake.
        let source = include_str!("observe.rs");
        let trait_body = source
            .split("pub trait RequestObserver")
            .nth(1)
            .and_then(|rest| rest.split("\n}").next())
            .expect("the trait definition is in this file");
        for forbidden in ["&str", "String", "TokenRange", "PrimaryKey"] {
            assert!(
                !trait_body.contains(forbidden),
                "`{forbidden}` must not be a dimension of a metric: {trait_body}"
            );
        }
    }
}
