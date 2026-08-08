//! What a node holds when it holds a range, and what it is told when it does not.

use cdm_core::TokenRange;
use chrono::{DateTime, Utc};

use crate::node::NodeId;

/// A granted lease on one token range (`DST-011`).
///
/// Holding one means: at the instant the lightweight transaction applied, this node was the only
/// node entitled to process `range`, and it stays entitled until [`Lease::expires_at`] — provided
/// it renews (`DST-012`) and provided the clocks of the nodes involved agree to within the lease
/// duration. Neither proviso is a guarantee this type can make, and
/// [`LeaseStore`](cdm_core::LeaseStore) documents exactly what each failure costs.
///
/// The value is a *fact about the past* — it does not renew itself, and nothing here observes the
/// clock. [`Coordinator::keep_alive`](crate::Coordinator::keep_alive) is what keeps one alive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    range: TokenRange,
    node: NodeId,
    expires_at: DateTime<Utc>,
    attempt: u32,
}

impl Lease {
    /// Records a lease as granted.
    #[must_use]
    pub const fn new(
        range: TokenRange,
        node: NodeId,
        expires_at: DateTime<Utc>,
        attempt: u32,
    ) -> Self {
        Self {
            range,
            node,
            expires_at,
            attempt,
        }
    }

    /// The leased range.
    #[must_use]
    pub const fn range(&self) -> TokenRange {
        self.range
    }

    /// The node that holds it.
    #[must_use]
    pub const fn node(&self) -> &NodeId {
        &self.node
    }

    /// When it expires, by the granting node's clock (`DST-012`).
    #[must_use]
    pub const fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Which claim of this range this is, counting from one (`DST-013`).
    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Whether the lease has expired at `now`.
    ///
    /// Expiry is `>=`, not `>`: a lease that expires exactly now is over. The boundary is
    /// resolved towards *not holding it*, because believing a lease is alive one millisecond too
    /// long is the direction in which two nodes end up on one range.
    #[must_use]
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at
    }

    /// The same lease, running until `expires_at` — the result of a successful renewal.
    #[must_use]
    pub fn renewed_until(&self, expires_at: DateTime<Utc>) -> Self {
        Self {
            expires_at,
            node: self.node.clone(),
            ..*self
        }
    }
}

/// What happened when a node tried to claim a range (`DST-011`, `DST-013`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The range is this node's until the lease expires.
    Claimed(Lease),
    /// Another node holds an unexpired lease. Not an error, and not something to retry against
    /// this range: there is other work in the plan.
    Held {
        /// The node named in the lease row.
        node: String,
        /// When its lease expires, by *its* clock.
        until: DateTime<Utc>,
        /// The attempt it is on.
        attempt: u32,
    },
    /// The range has been claimed `cluster.max_attempts` times already (`DST-013`).
    ///
    /// It is abandoned rather than retried: something about this range has now defeated that many
    /// nodes, and a fleet that keeps re-claiming it makes no progress and may keep dying.
    /// [`Coordinator::abandon`](crate::Coordinator::abandon) records the `FAIL`.
    Exhausted {
        /// How many times it has been claimed.
        attempts: u32,
        /// The node that held it last.
        node: String,
    },
    /// The lease has expired but this coordinator is not allowed to take it over.
    ///
    /// Only produced under [`ReclaimPolicy::Refuse`](crate::ReclaimPolicy) — see that type for
    /// why a counter target must use it until `DST-014`/`DST-015` (#51) land.
    ReclaimRefused {
        /// The node whose lease expired.
        node: String,
        /// The attempt it was on.
        attempt: u32,
    },
}

/// How this node came to be part of the run (`DST-002`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinOutcome {
    /// This node won the election and performed `TRK-020`'s initialisation.
    Initialised,
    /// Another node initialised the run; this one checked the configuration hash of `DST-003`,
    /// waited for the run row to reach `STARTED`, and joined.
    Joined,
}

/// Why a lease stopped being renewed (`DST-012`).
#[derive(Debug, PartialEq, Eq)]
pub enum LeaseEnd {
    /// The caller cancelled: the range is finished, and the lease was released so that nobody
    /// waits out its expiry.
    Released,
    /// A renewal was refused because another node holds the range now. Whatever the caller is
    /// doing with this range, it must stop — the other node is doing it too.
    Lost,
    /// Renewals kept failing until the lease ran out. Indistinguishable, from here, from being
    /// partitioned away from the coordination keyspace, and treated the same way: the range is no
    /// longer this node's.
    Expired,
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

    fn lease(expires_at: DateTime<Utc>) -> Lease {
        Lease::new(
            TokenRange::new(0, 99).unwrap(),
            NodeId::new("node-a").unwrap(),
            expires_at,
            1,
        )
    }

    #[test]
    fn dst_012_a_lease_that_expires_exactly_now_is_expired() {
        let at = DateTime::UNIX_EPOCH + chrono::Duration::seconds(60);
        let lease = lease(at);
        assert!(!lease.is_expired_at(at - chrono::Duration::milliseconds(1)));
        assert!(lease.is_expired_at(at));
        assert!(lease.is_expired_at(at + chrono::Duration::milliseconds(1)));
    }

    #[test]
    fn dst_012_renewal_moves_the_expiry_and_nothing_else() {
        let lease = lease(DateTime::UNIX_EPOCH);
        let renewed = lease.renewed_until(DateTime::UNIX_EPOCH + chrono::Duration::seconds(60));
        assert_eq!(renewed.range(), lease.range());
        assert_eq!(renewed.node(), lease.node());
        assert_eq!(renewed.attempt(), lease.attempt());
        assert!(renewed.expires_at() > lease.expires_at());
    }
}
