//! Lease-based distributed coordination over the target keyspace.
//!
//! Part of [cdm-rs](https://github.com/msmygit/cdm-rs), a Rust reimplementation of the
//! Cassandra Data Migrator.
//!
//! # What distributed mode is for
//!
//! One process migrating a petabyte is both a throughput ceiling and a single point of failure
//! across a run that lasts days. `cluster.enabled` lets N cdm-rs processes, started with the same
//! `run_id` and the same configuration, execute one run together (`DST-001`). They coordinate
//! through the **target keyspace** — the one substrate every run already requires — so there is
//! no ZooKeeper, no etcd and no controller process to deploy (`ADR-0003`).
//!
//! ```text
//!   node A                       cdm_run_info                       node B
//!     │                                                                │
//!     ├── INSERT … IF NOT EXISTS ───►  applied ◄─── DST-002 ───────────┤
//!     │        (writes config_hash = DST-003)      not applied ────────┤
//!     ├── TRK-020: range rows, then STARTED                            │
//!     │                                                     waits for STARTED,
//!     │                                                     checks the hash
//!     │                          cdm_run_leases                        │
//!     ├── INSERT … IF NOT EXISTS  (token_min) ──►  granted             │
//!     │                                            denied ◄────────────┤
//!     ├── UPDATE … IF node_id = ?   every heartbeat_interval (DST-012) │
//!     │                                                                │
//!     │   lease expires ──► UPDATE … IF lease_until < now, attempt+1 ──┤
//! ```
//!
//! # The guarantee, and its edges
//!
//! A claim is a lightweight transaction at `SERIAL` (`DST-011`), so **at most one node holds the
//! lease on a range at a time**. Lease expiry, however, is a *timeout*, and a timeout is a
//! liveness heuristic rather than a fence: clock skew, a stalled process and a network partition
//! can each leave the previous holder believing it still owns a range that has been reclaimed.
//! [`cdm_core::LeaseStore`] states all three precisely.
//!
//! That is a wasted-work problem for migrate and validate, whose writes are idempotent upserts
//! carrying the origin's writetime. It is **data corruption** for a counter table, because
//! `SET c = c + delta` applied twice is simply a wrong number. `DST-014` and `DST-015` are the
//! requirements that close it — refuse the reclaim, mark the range `FAIL`, require manual
//! reconciliation — and they are roadmap item **#51**, not implemented here. What is here is the
//! seam they land on: [`ReclaimPolicy`] has no `Default`, so no caller can hand a counter target
//! to a reclaiming coordinator by omission.
//!
//! # What is deliberately not here yet
//!
//! | Concern | Requirement | Roadmap |
//! |---|---|---|
//! | Safe reclaim and the counter guard | `DST-014`, `DST-015` | #51 |
//! | Cross-node metric aggregation, global rate limits | `DST-016`, `ENG-004` | #51 |
//! | Clean deregistration, the membership view behind `cdm cluster status` | `DST-017`, `DST-018` | #51 |
//! | Multi-node integration with node-death injection | `DST-019`, `TST-042` | #52 |
//!
//! The scheduler is likewise untouched: `cdm-engine`'s `WorkQueue` still hands out ranges from an
//! in-process cursor, and replacing that cursor with [`Coordinator::claim_first`] plus a
//! [`Coordinator::keep_alive`] task per in-flight range is the wiring #51 and #52 do — once
//! `DST-014` can say whether a reclaim is safe for the table in hand. Landing the coordinator
//! first, with its own tests, is what makes that wiring reviewable in isolation.
//!
//! # Where the driver is
//!
//! Nowhere in this crate. `ARCHITECTURE.md` §3 reserves `scylla` for `cdm-cql`, with `cdm-track`
//! as the one documented exception — it owns the tracking tables, and `cdm_run_leases` lives
//! beside them (`TRK-011`). The trait is [`cdm_core::LeaseStore`], the Cassandra implementation
//! is `cdm-track`'s, and this crate holds the policy: how long a lease runs, when it is renewed,
//! how many attempts a range gets, and what happens when it runs out of them. That split is why
//! every requirement below has a real unit test and none of them needs a container.
//!
//! # Usage
//!
//! ```
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! use cdm_cluster::{ClaimOutcome, Coordinator, CoordinatorSettings, NodeId, ReclaimPolicy};
//! use cdm_core::{LeaseStore, RunId, TokenRange};
//!
//! # async fn example(store: Arc<dyn LeaseStore>) -> Result<(), cdm_core::CdmError> {
//! let settings = CoordinatorSettings::new(
//!     Duration::from_secs(60),
//!     Duration::from_secs(15),
//!     3,
//!     // A counter target must pass `Refuse` until DST-014/DST-015 land.
//!     ReclaimPolicy::Reclaim,
//! )?;
//! let coordinator = Coordinator::new(store, NodeId::new("node-a")?, settings);
//!
//! let run_id = RunId::from_raw(1);
//! let range = TokenRange::new(0, 99)?;
//! match coordinator.claim(run_id, range).await? {
//!     ClaimOutcome::Claimed(_lease) => { /* process the range, renewing the lease */ }
//!     ClaimOutcome::Held { .. } => { /* someone else has it; take another range */ }
//!     ClaimOutcome::Exhausted { attempts, .. } => {
//!         coordinator.abandon(run_id, range, attempts).await?;
//!     }
//!     ClaimOutcome::ReclaimRefused { .. } => { /* DST-014/DST-015 territory */ }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Specification
//!
//! This crate is the designated home for the following requirements from
//! [`docs/SPEC.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/SPEC.md); see
//! [`docs/TRACEABILITY.md`](https://github.com/msmygit/cdm-rs/blob/main/docs/TRACEABILITY.md)
//! for the full matrix:
//!
//! - `DST-001` — [`Coordinator`], [`NodeId`], [`Coordinator::claim_first`]
//! - `DST-002` — [`Coordinator::join`], [`JoinOutcome`]
//! - `DST-003` — [`Coordinator::join`], [`CONFIG_HASH_PREFIX`]
//! - `DST-010`, `DST-011` — [`Coordinator::claim`], [`Lease`]
//! - `DST-012` — [`Coordinator::renew`], [`Coordinator::keep_alive`], [`Clock`]
//! - `DST-013` — [`ClaimOutcome::Exhausted`], [`Coordinator::abandon`]

pub mod clock;
pub mod coordinator;
pub mod lease;
pub mod node;
pub mod settings;

pub use clock::{Clock, ManualClock, SystemClock};
pub use coordinator::{Coordinator, CONFIG_HASH_PREFIX};
pub use lease::{ClaimOutcome, JoinOutcome, Lease, LeaseEnd};
pub use node::NodeId;
pub use settings::{CoordinatorSettings, ReclaimPolicy};

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
}
