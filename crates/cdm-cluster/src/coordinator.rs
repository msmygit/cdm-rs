//! The coordinator: joining a run, claiming ranges, and keeping the claims alive.
//!
//! `DST-001`..`DST-003` and `DST-010`..`DST-013` in one type. Everything it does is a conditional
//! write through [`LeaseStore`] plus a decision this module makes about *whether* to issue it —
//! and the split is deliberate: the store knows how to run a lightweight transaction and nothing
//! about policy, this file knows the policy and nothing about CQL, and the policy is therefore
//! testable without a cluster.

use std::sync::Arc;
use std::time::Duration;

use cdm_core::{
    CdmError, ErrorKind, LeaseOutcome, LeaseStore, RangeRecord, RunClaim, RunId, RunRecord,
    RunStatus, TokenRange,
};
use tokio_util::sync::CancellationToken;

use crate::clock::{Clock, SystemClock};
use crate::lease::{ClaimOutcome, JoinOutcome, Lease, LeaseEnd};
use crate::node::NodeId;
use crate::settings::{CoordinatorSettings, ReclaimPolicy};

/// The prefix `DST-003`'s configuration hash is recorded under in `cdm_run_info.run_info`.
///
/// Tagged rather than bare so that the column stays self-describing: `TRK-022` replaces the value
/// with the run's metrics string when the run ends, and a joining node must be able to tell "this
/// is a hash and it differs from mine" from "this run is over and that is its metrics block".
pub const CONFIG_HASH_PREFIX: &str = "config_hash=";

/// What a node uses to take part in a distributed run (`DST-001`).
///
/// # The safety property, stated once
///
/// **At most one node holds the lease on a range at a time**, because every claim is a
/// lightweight transaction at `SERIAL` (`DST-011`) and Paxos totally orders the claims on one
/// row. That is a guarantee about *leases*. It is not, on its own, a guarantee that at most one
/// node is *processing* a range: a lease can expire while its holder is still alive but stalled,
/// skewed or partitioned, and the reclaiming node then works on a range someone else has not
/// finished. [`LeaseStore`] enumerates those three failure modes precisely.
///
/// For migrate and validate over a non-counter table that is a performance problem, not a
/// correctness one — the writes are idempotent upserts carrying the origin's writetime. For a
/// **counter** table it is data corruption, and cdm-rs's answer is `DST-014`/`DST-015`: refuse
/// the reclaim, mark the range `FAIL`, tell the operator that manual reconciliation is required.
/// Those are roadmap item #51 and are **not implemented here**. What is here is the seam they
/// land on — [`ReclaimPolicy`], which has no `Default` precisely so that a counter target cannot
/// be wired to a reclaiming coordinator by omission.
#[derive(Clone)]
pub struct Coordinator {
    store: Arc<dyn LeaseStore>,
    node: NodeId,
    settings: CoordinatorSettings,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coordinator")
            .field("node", &self.node)
            .field("settings", &self.settings)
            // The two trait objects are named rather than printed: `LeaseStore` has no `Debug`
            // bound — it is a plugin trait, and a store may hold a session — and a clock's only
            // interesting state is the time, which a `Debug` line would date the moment it was
            // written.
            .field("store", &self.store.name())
            .field("clock", &self.clock)
            .finish()
    }
}

impl Coordinator {
    /// A coordinator on the host's clock.
    #[must_use]
    pub fn new(store: Arc<dyn LeaseStore>, node: NodeId, settings: CoordinatorSettings) -> Self {
        Self::with_clock(store, node, settings, Arc::new(SystemClock))
    }

    /// A coordinator on an explicit clock, which is what makes expiry testable (`DST-012`).
    #[must_use]
    pub fn with_clock(
        store: Arc<dyn LeaseStore>,
        node: NodeId,
        settings: CoordinatorSettings,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            node,
            settings,
            clock,
        }
    }

    /// This node's identity.
    #[must_use]
    pub const fn node(&self) -> &NodeId {
        &self.node
    }

    /// The lease policy this node runs with.
    #[must_use]
    pub const fn settings(&self) -> &CoordinatorSettings {
        &self.settings
    }

    /// Joins the run, initialising it if this node gets there first (`DST-001`..`DST-003`).
    ///
    /// One node wins the `INSERT ... IF NOT EXISTS` on the run row and performs `TRK-020`; the
    /// rest check that their configuration hashes the same as the initialiser's (`DST-003`) and
    /// wait for the run row to reach `STARTED`, which is the initialiser's signal that every
    /// range row exists.
    ///
    /// `config_hash` is `EffectiveConfig::config_hash`: a digest of the effective configuration
    /// in which every secret has already serialised as `***`, so nodes with the same plan and
    /// different credentials agree, and no credential is written to the target keyspace
    /// (`SEC-001`).
    ///
    /// # Errors
    ///
    /// * [`ErrorKind::Config`] if the initialiser's configuration hash differs from this node's,
    ///   or if it recorded no hash at all — a run started by Java CDM, or by a single-node cdm-rs
    ///   run, has no hash to agree with, and joining it would mean asserting a consistency nobody
    ///   checked.
    /// * [`ErrorKind::Lease`] if the run finished, or was never marked `STARTED`, while this node
    ///   waited.
    pub async fn join(
        &self,
        run: &RunRecord,
        ranges: &[RangeRecord],
        config_hash: &str,
    ) -> Result<JoinOutcome, CdmError> {
        self.store.initialise().await?;
        self.store.initialise_leases().await?;

        let recorded = format!("{CONFIG_HASH_PREFIX}{config_hash}");
        match self.store.initialise_run(run, ranges, &recorded).await? {
            RunClaim::Won => {
                tracing::info!(
                    node = %self.node,
                    run_id = %run.run_id,
                    ranges = ranges.len(),
                    "this node initialised the run"
                );
                Ok(JoinOutcome::Initialised)
            }
            RunClaim::Lost(existing) => {
                check_config_hash(&existing, config_hash)?;
                self.await_started(run.run_id).await?;
                tracing::info!(
                    node = %self.node,
                    run_id = %run.run_id,
                    "joined a run another node initialised"
                );
                Ok(JoinOutcome::Joined)
            }
        }
    }

    /// Waits for the initialising node to finish `TRK-020` (`DST-002`).
    ///
    /// Polled on the heartbeat and bounded by the lease duration: an initialiser that has not
    /// reached `STARTED` within the time a lease would have expired is not slow, it is gone, and
    /// waiting longer only delays the diagnostic. No new configuration key — the bound is derived
    /// from `cluster.lease_duration`, which is already the deployment's statement about how long
    /// an unresponsive node is tolerated.
    async fn await_started(&self, run_id: RunId) -> Result<(), CdmError> {
        let deadline = self.clock.now() + chrono_duration(self.settings.lease_duration());
        loop {
            let run = self.store.run(run_id).await?.ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Lease,
                    format!(
                        "run {run_id} lost the initialisation election to another node and then \
                         disappeared; the target keyspace is not answering consistently"
                    ),
                )
            })?;
            match run.status {
                RunStatus::Started => return Ok(()),
                RunStatus::NotStarted => {}
                terminal => {
                    return Err(CdmError::new(
                        ErrorKind::Lease,
                        format!(
                            "run {run_id} is already {terminal}; a node cannot join a run that \
                             has finished. Start a new run, or resume this one with \
                             track_run.previous_run_id = {run_id}"
                        ),
                    ))
                }
            }
            if self.clock.now() >= deadline {
                return Err(CdmError::new(
                    ErrorKind::Lease,
                    format!(
                        "the node initialising run {run_id} did not reach STARTED within \
                         cluster.lease_duration; it is not coming back. Check whether it is \
                         still running before starting the run again"
                    ),
                ));
            }
            tokio::time::sleep(self.settings.heartbeat_interval()).await;
        }
    }

    /// Tries to claim one range (`DST-010`..`DST-013`).
    ///
    /// The lease row is read first — to learn the attempt count and the holder — and that read
    /// decides nothing: it selects which conditional write to issue, and the write is what grants
    /// the range. See [`LeaseStore::claim_range`].
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Lease`] if the store cannot be reached. A range held by another node,
    /// one that has exhausted its attempts, and one this coordinator may not reclaim are all
    /// [`ClaimOutcome`]s, not errors: they are ordinary facts about a fleet sharing a plan.
    pub async fn claim(&self, run_id: RunId, range: TokenRange) -> Result<ClaimOutcome, CdmError> {
        let now = self.clock.now();
        let observed = self.store.lease(run_id, range).await?;

        if let Some(current) = &observed {
            if current.lease_until > now {
                return Ok(ClaimOutcome::Held {
                    node: current.node_id.clone(),
                    until: current.lease_until,
                    attempt: current.attempt,
                });
            }
            // DST-013. The next claim would be `attempt + 1`, so a range that has already been
            // claimed `max_attempts` times is done being tried.
            if current.attempt >= self.settings.max_attempts() {
                return Ok(ClaimOutcome::Exhausted {
                    attempts: current.attempt,
                    node: current.node_id.clone(),
                });
            }
            if self.settings.reclaim() == ReclaimPolicy::Refuse {
                return Ok(ClaimOutcome::ReclaimRefused {
                    node: current.node_id.clone(),
                    attempt: current.attempt,
                });
            }
        }

        let expires_at = now + chrono_duration(self.settings.lease_duration());
        let outcome = self
            .store
            .claim_range(
                run_id,
                range,
                self.node.as_str(),
                now,
                expires_at,
                observed.as_ref(),
            )
            .await?;
        Ok(match outcome {
            LeaseOutcome::Granted(record) => {
                tracing::debug!(
                    node = %self.node,
                    run_id = %run_id,
                    range = %range,
                    attempt = record.attempt,
                    "claimed a range"
                );
                ClaimOutcome::Claimed(Lease::new(
                    range,
                    self.node.clone(),
                    record.lease_until,
                    record.attempt,
                ))
            }
            LeaseOutcome::Denied(record) => ClaimOutcome::Held {
                node: record.node_id,
                until: record.lease_until,
                attempt: record.attempt,
            },
        })
    }

    /// Claims the first range of `candidates` that this node can have (`DST-001`).
    ///
    /// Ranges that are held, exhausted or unreclaimable are skipped, which is what turns a shared
    /// plan into cooperating nodes: every node walks the same list and each takes what is free,
    /// so a node that joins late or runs slowly simply claims less. The candidates are walked in
    /// order, so a caller that wants nodes to spread over the ring should hand them the plan the
    /// token planner already shuffled (`TOK-006`).
    ///
    /// `Ok(None)` means nothing in `candidates` is claimable *right now* — not that the run is
    /// finished. Ranges held by other nodes may become claimable later if those nodes die.
    ///
    /// # Errors
    ///
    /// As [`Coordinator::claim`].
    pub async fn claim_first(
        &self,
        run_id: RunId,
        candidates: &[TokenRange],
    ) -> Result<Option<Lease>, CdmError> {
        for range in candidates {
            if let ClaimOutcome::Claimed(lease) = self.claim(run_id, *range).await? {
                return Ok(Some(lease));
            }
        }
        Ok(None)
    }

    /// Extends a lease this node holds (`DST-012`).
    ///
    /// `Ok(None)` means the lease is gone: it expired and another node claimed the range. The
    /// caller must stop processing it — the other node is processing it now.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Lease`] if the store cannot be reached, which is *not* the same as
    /// losing the lease and must not be treated as such: the lease may still be this node's, and
    /// [`Coordinator::keep_alive`] keeps trying until it demonstrably is not.
    pub async fn renew(&self, run_id: RunId, lease: &Lease) -> Result<Option<Lease>, CdmError> {
        let expires_at = self.clock.now() + chrono_duration(self.settings.lease_duration());
        let renewed = self
            .store
            .renew_lease(run_id, lease.range(), self.node.as_str(), expires_at)
            .await?;
        Ok(renewed.then(|| lease.renewed_until(expires_at)))
    }

    /// Gives a lease up, so that the range is claimable without anyone waiting out its expiry.
    ///
    /// Called when a range is finished, and — as `DST-017` (#51) will have it — for every held
    /// lease when a node shuts down cleanly.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Lease`] if the store cannot be reached. A lease that was already lost
    /// releases nothing and is not an error: the range has an owner either way.
    pub async fn release(&self, run_id: RunId, lease: &Lease) -> Result<(), CdmError> {
        let released = self
            .store
            .release_lease(run_id, lease.range(), self.node.as_str())
            .await?;
        if !released {
            tracing::debug!(
                node = %self.node,
                range = %lease.range(),
                "released a lease this node no longer held"
            );
        }
        Ok(())
    }

    /// Records a range as `FAIL` because it has exhausted its attempts (`DST-013`).
    ///
    /// The status is what a resume reads (`TRK-031`), so this is the write that stops the fleet
    /// re-claiming a range forever *and* leaves it visible to an operator. The `run_info` string
    /// says why in words rather than counters: nothing counted here, and a range abandoned before
    /// it was processed has no metrics to report.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if the range row cannot be written.
    pub async fn abandon(
        &self,
        run_id: RunId,
        range: TokenRange,
        attempts: u32,
    ) -> Result<(), CdmError> {
        tracing::warn!(
            node = %self.node,
            run_id = %run_id,
            range = %range,
            attempts,
            "abandoning a range that has exhausted cluster.max_attempts (DST-013)"
        );
        self.store
            .update_range(
                run_id,
                &RangeRecord {
                    range,
                    status: RunStatus::Fail,
                    started_at: None,
                    info: Some(format!(
                        "Abandoned after {attempts} attempts; no node completed this range \
                         (DST-013)"
                    )),
                },
            )
            .await
    }

    /// Renews `lease` on the heartbeat until the caller cancels or the lease is lost (`DST-012`).
    ///
    /// This is the loop that makes a lease mean something for longer than one lease duration. It
    /// is separate from the work it protects — the caller processes the range, this keeps the
    /// claim — so that a job which blocks, or a range which takes an hour, cannot starve the
    /// renewal by never yielding.
    ///
    /// A renewal that *fails* is not a lost lease: the store may be briefly unreachable while the
    /// lease is still perfectly valid. The loop keeps trying until either a renewal is refused
    /// (someone else holds the range — [`LeaseEnd::Lost`]) or the lease's own expiry passes
    /// without a successful renewal ([`LeaseEnd::Expired`]). Both mean the same thing to the
    /// caller and it must stop; they are distinguished because they mean different things to
    /// whoever reads the log.
    pub async fn keep_alive(
        &self,
        run_id: RunId,
        lease: Lease,
        cancel: CancellationToken,
    ) -> LeaseEnd {
        let mut lease = lease;
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    if let Err(error) = self.release(run_id, &lease).await {
                        tracing::warn!(
                            node = %self.node,
                            range = %lease.range(),
                            %error,
                            "could not release a lease on shutdown; it will expire instead"
                        );
                    }
                    return LeaseEnd::Released;
                }
                () = tokio::time::sleep(self.settings.heartbeat_interval()) => {}
            }
            match self.renew(run_id, &lease).await {
                Ok(Some(renewed)) => lease = renewed,
                Ok(None) => {
                    tracing::warn!(
                        node = %self.node,
                        run_id = %run_id,
                        range = %lease.range(),
                        "lost the lease on a range: another node holds it now"
                    );
                    return LeaseEnd::Lost;
                }
                Err(error) => {
                    if lease.is_expired_at(self.clock.now()) {
                        tracing::warn!(
                            node = %self.node,
                            range = %lease.range(),
                            %error,
                            "a lease expired while its renewals were failing"
                        );
                        return LeaseEnd::Expired;
                    }
                    tracing::debug!(
                        node = %self.node,
                        range = %lease.range(),
                        %error,
                        "a lease renewal failed; retrying before the lease expires"
                    );
                }
            }
        }
    }
}

/// `DST-003`: the joining node's configuration must hash the same as the initialiser's.
///
/// # What this check is and is not
///
/// It compares digests, so it detects *that* two nodes disagree, never *where*. A hash cannot be
/// diffed, and the alternative — writing the effective configuration into the target keyspace so
/// that a diff could be computed — is exactly what `SEC-001` forbids. The diagnostic therefore
/// names both digests and the command that prints a node's own, which is the smallest thing that
/// lets an operator find the difference locally.
fn check_config_hash(existing: &RunRecord, config_hash: &str) -> Result<(), CdmError> {
    let recorded = existing
        .info
        .as_deref()
        .and_then(|info| info.strip_prefix(CONFIG_HASH_PREFIX));
    match recorded {
        Some(recorded) if recorded == config_hash => Ok(()),
        Some(recorded) => Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "run {} was initialised by a node whose configuration hashes to {recorded}; this \
                 node's hashes to {config_hash}. Every node of a distributed run must be started \
                 with the same configuration (DST-003) — differing token splits, column mappings \
                 or feature settings would have the nodes writing different data into one table. \
                 `cdm plan --summary-out` prints a node's own hash; compare them and re-run.",
                existing.run_id
            ),
        )
        .with_context(|ctx| ctx.with_config_key("cluster.enabled"))),
        None => Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "run {} exists but records no configuration hash, so this node cannot verify \
                 that it was started with the same configuration (DST-003). A run initialised \
                 by Java CDM, or by cdm-rs with cluster.enabled unset, cannot be joined; start a \
                 new run id with cluster.enabled on every node.",
                existing.run_id
            ),
        )
        .with_context(|ctx| ctx.with_config_key("track_run.run_id"))),
    }
}

/// A `chrono` duration from a `std` one, saturating rather than failing.
///
/// Kept private and used everywhere a lease deadline is computed, so there is exactly one place
/// where the two duration types meet.
///
/// The conversion fails only beyond 2^63 milliseconds — 292 million years — which
/// [`CoordinatorSettings`] has no way to reject usefully and no deployment can mean. Saturating
/// keeps a lease that long *long*, which errs towards not reclaiming.
fn chrono_duration(value: Duration) -> chrono::Duration {
    chrono::Duration::from_std(value).unwrap_or(chrono::Duration::MAX)
}

// Tests may panic freely: a failed assertion *is* the reporting mechanism, and the no-panic rule
// (ERR-004) exists to protect production paths, not test bodies.
//
// Every test here is deterministic by construction. Expiry is decided by a `ManualClock` the test
// moves, never by a `sleep` long enough to "probably" have expired; contention is decided by the
// store's critical sections, so a test that asserts "exactly one claim was granted" asserts a
// property of the code rather than of the scheduler that happened to run it.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use std::collections::BTreeSet;

    use cdm_core::{JobKind, TableRef, TrackingStore as _};
    use cdm_track::store::MemoryStore;

    use crate::clock::ManualClock;

    use super::*;

    const HASH: &str = "0123456789abcdef";
    const LEASE: Duration = Duration::from_secs(60);
    const HEARTBEAT: Duration = Duration::from_secs(15);

    fn run_id() -> RunId {
        RunId::from_raw(1)
    }

    fn run_record() -> RunRecord {
        RunRecord {
            run_id: run_id(),
            previous_run_id: None,
            table: TableRef::new("target_ks", "customers"),
            job: JobKind::Migrate,
            status: RunStatus::NotStarted,
            started_at: None,
            ended_at: None,
            info: None,
        }
    }

    fn plan(count: i128) -> Vec<TokenRange> {
        (0..count)
            .map(|i| TokenRange::new(i * 100, i * 100 + 99).unwrap())
            .collect()
    }

    fn range_records(ranges: &[TokenRange]) -> Vec<RangeRecord> {
        ranges
            .iter()
            .map(|range| RangeRecord {
                range: *range,
                status: RunStatus::NotStarted,
                started_at: None,
                info: None,
            })
            .collect()
    }

    fn settings(reclaim: ReclaimPolicy) -> CoordinatorSettings {
        CoordinatorSettings::new(LEASE, HEARTBEAT, 3, reclaim).unwrap()
    }

    fn node(
        store: &Arc<MemoryStore>,
        name: &str,
        clock: &Arc<ManualClock>,
        reclaim: ReclaimPolicy,
    ) -> Coordinator {
        Coordinator::with_clock(
            Arc::clone(store) as Arc<dyn LeaseStore>,
            NodeId::new(name).unwrap(),
            settings(reclaim),
            Arc::clone(clock) as Arc<dyn Clock>,
        )
    }

    /// A store with the run already initialised by `node-a`, and the ranges of the plan.
    async fn initialised(plan: &[TokenRange]) -> (Arc<MemoryStore>, Arc<ManualClock>) {
        let store = Arc::new(MemoryStore::new());
        let clock = Arc::new(ManualClock::epoch());
        let outcome = node(&store, "node-a", &clock, ReclaimPolicy::Reclaim)
            .join(&run_record(), &range_records(plan), HASH)
            .await
            .unwrap();
        assert_eq!(outcome, JoinOutcome::Initialised);
        (store, clock)
    }

    #[tokio::test]
    async fn dst_001_a_fleet_splits_one_plan_and_no_range_is_claimed_twice() {
        let plan = plan(9);
        let (store, clock) = initialised(&plan).await;
        let nodes: Vec<Coordinator> = ["node-a", "node-b", "node-c"]
            .into_iter()
            .map(|name| node(&store, name, &clock, ReclaimPolicy::Reclaim))
            .collect();

        // Every node walks the same list and takes what is free, one range at a time, until
        // nothing is left. That is the whole cooperation protocol: there is no assignment.
        let mut claimed: Vec<(String, TokenRange)> = Vec::new();
        let mut progress = true;
        while progress {
            progress = false;
            for coordinator in &nodes {
                if let Some(lease) = coordinator.claim_first(run_id(), &plan).await.unwrap() {
                    claimed.push((coordinator.node().to_string(), lease.range()));
                    progress = true;
                }
            }
        }

        let ranges: BTreeSet<TokenRange> = claimed.iter().map(|(_, range)| *range).collect();
        assert_eq!(
            ranges.len(),
            plan.len(),
            "every range is claimed exactly once"
        );
        assert_eq!(claimed.len(), plan.len(), "and no range is claimed twice");
        for name in ["node-a", "node-b", "node-c"] {
            assert!(
                claimed.iter().any(|(node, _)| node == name),
                "{name} claimed nothing; the fleet is not sharing the plan"
            );
        }
        assert_eq!(store.leases(run_id()).await.unwrap().len(), plan.len());
    }

    #[tokio::test]
    async fn dst_002_exactly_one_node_initialises_a_run_however_many_start_together() {
        let plan = plan(4);
        let store = Arc::new(MemoryStore::new());
        let clock = Arc::new(ManualClock::epoch());

        let mut tasks = Vec::new();
        for name in ["node-a", "node-b", "node-c", "node-d"] {
            let coordinator = node(&store, name, &clock, ReclaimPolicy::Reclaim);
            let records = range_records(&plan);
            tasks.push(tokio::spawn(async move {
                coordinator.join(&run_record(), &records, HASH).await
            }));
        }
        let mut initialised = 0;
        let mut joined = 0;
        for task in tasks {
            match task.await.unwrap().unwrap() {
                JoinOutcome::Initialised => initialised += 1,
                JoinOutcome::Joined => joined += 1,
            }
        }
        assert_eq!(initialised, 1, "TRK-020 must be performed exactly once");
        assert_eq!(joined, 3);

        // And the losers waited for a run whose range rows were all there (TRK-020's order).
        let run = store.run(run_id()).await.unwrap().unwrap();
        assert_eq!(run.status, RunStatus::Started);
        assert_eq!(store.ranges(run_id()).await.unwrap().len(), plan.len());
    }

    #[tokio::test]
    async fn dst_003_a_node_configured_differently_refuses_to_join() {
        let plan = plan(2);
        let (store, clock) = initialised(&plan).await;
        let error = node(&store, "node-b", &clock, ReclaimPolicy::Reclaim)
            .join(&run_record(), &range_records(&plan), "fedcba9876543210")
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        let message = error.to_string();
        assert!(message.contains(HASH), "the diagnostic names both hashes");
        assert!(message.contains("fedcba9876543210"));
    }

    #[tokio::test]
    async fn dst_003_a_matching_hash_joins_and_the_hash_survives_initialisation() {
        let plan = plan(2);
        let (store, clock) = initialised(&plan).await;
        assert_eq!(
            node(&store, "node-b", &clock, ReclaimPolicy::Reclaim)
                .join(&run_record(), &range_records(&plan), HASH)
                .await
                .unwrap(),
            JoinOutcome::Joined
        );
        let run = store.run(run_id()).await.unwrap().unwrap();
        assert_eq!(run.info.as_deref(), Some("config_hash=0123456789abcdef"));
    }

    #[tokio::test]
    async fn dst_003_a_run_that_recorded_no_hash_cannot_be_joined() {
        // What a run initialised by Java CDM, or by a single-node cdm-rs run, looks like.
        let plan = plan(2);
        let store = Arc::new(MemoryStore::new());
        let clock = Arc::new(ManualClock::epoch());
        store
            .create_run(&run_record(), &range_records(&plan))
            .await
            .unwrap();

        let error = node(&store, "node-b", &clock, ReclaimPolicy::Reclaim)
            .join(&run_record(), &range_records(&plan), HASH)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.to_string().contains("records no configuration hash"));
    }

    #[tokio::test]
    async fn dst_010_a_claim_writes_the_holder_the_expiry_and_the_attempt() {
        let plan = plan(1);
        let (store, clock) = initialised(&plan).await;
        let coordinator = node(&store, "node-a", &clock, ReclaimPolicy::Reclaim);

        let ClaimOutcome::Claimed(lease) = coordinator.claim(run_id(), plan[0]).await.unwrap()
        else {
            panic!("an unclaimed range must be claimable");
        };
        assert_eq!(lease.range(), plan[0]);
        assert_eq!(lease.node().as_str(), "node-a");
        assert_eq!(lease.attempt(), 1, "a first claim is attempt one");
        assert_eq!(lease.expires_at(), clock.now() + chrono_duration(LEASE));

        let rows = store.leases(run_id()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].token_min, 0, "keyed by the lower bound (DST-010)");
        assert_eq!(rows[0].node_id, "node-a");
        assert_eq!(rows[0].lease_until, lease.expires_at());
        assert_eq!(rows[0].attempt, 1);
    }

    #[tokio::test]
    async fn dst_011_only_one_of_many_concurrent_claims_on_one_range_is_granted() {
        let plan = plan(1);
        let (store, clock) = initialised(&plan).await;

        let mut tasks = Vec::new();
        for name in ["node-a", "node-b", "node-c", "node-d", "node-e"] {
            let coordinator = node(&store, name, &clock, ReclaimPolicy::Reclaim);
            let range = plan[0];
            tasks.push(tokio::spawn(async move {
                coordinator.claim(run_id(), range).await
            }));
        }
        let mut granted = Vec::new();
        let mut denied = 0;
        for task in tasks {
            match task.await.unwrap().unwrap() {
                ClaimOutcome::Claimed(lease) => granted.push(lease),
                ClaimOutcome::Held { .. } => denied += 1,
                other => panic!("unexpected outcome {other:?}"),
            }
        }
        assert_eq!(granted.len(), 1, "at most one node may hold a range");
        assert_eq!(denied, 4);
        // The row agrees with the winner: a denial cannot have written anything.
        let rows = store.leases(run_id()).await.unwrap();
        assert_eq!(rows[0].node_id, granted[0].node().as_str());
        assert_eq!(rows[0].attempt, 1, "four denials are not four attempts");
    }

    #[tokio::test]
    async fn dst_012_a_live_lease_is_held_and_an_expired_one_is_reclaimed_on_a_new_attempt() {
        let plan = plan(1);
        let (store, clock) = initialised(&plan).await;
        let a = node(&store, "node-a", &clock, ReclaimPolicy::Reclaim);
        let b = node(&store, "node-b", &clock, ReclaimPolicy::Reclaim);

        let ClaimOutcome::Claimed(lease) = a.claim(run_id(), plan[0]).await.unwrap() else {
            panic!("the first claim must be granted");
        };

        // One millisecond before expiry the range is still node-a's.
        clock.advance(LEASE.saturating_sub(Duration::from_millis(1)));
        match b.claim(run_id(), plan[0]).await.unwrap() {
            ClaimOutcome::Held {
                node,
                until,
                attempt,
            } => {
                assert_eq!(node, "node-a");
                assert_eq!(until, lease.expires_at());
                assert_eq!(attempt, 1);
            }
            other => panic!("a live lease must not be reclaimable: {other:?}"),
        }

        // At expiry it is not.
        clock.advance(Duration::from_millis(1));
        let ClaimOutcome::Claimed(reclaimed) = b.claim(run_id(), plan[0]).await.unwrap() else {
            panic!("an expired lease must be reclaimable (DST-012)");
        };
        assert_eq!(reclaimed.node().as_str(), "node-b");
        assert_eq!(reclaimed.attempt(), 2, "a reclaim increments the attempt");
        assert_eq!(store.leases(run_id()).await.unwrap()[0].node_id, "node-b");
    }

    #[tokio::test]
    async fn dst_012_a_renewal_extends_a_held_lease_and_a_lost_one_cannot_be_renewed() {
        let plan = plan(1);
        let (store, clock) = initialised(&plan).await;
        let a = node(&store, "node-a", &clock, ReclaimPolicy::Reclaim);
        let b = node(&store, "node-b", &clock, ReclaimPolicy::Reclaim);

        let ClaimOutcome::Claimed(lease) = a.claim(run_id(), plan[0]).await.unwrap() else {
            panic!("the first claim must be granted");
        };
        clock.advance(HEARTBEAT);
        let renewed = a.renew(run_id(), &lease).await.unwrap().unwrap();
        assert_eq!(
            renewed.expires_at(),
            lease.expires_at() + chrono_duration(HEARTBEAT),
            "a renewal moves expiry a full lease duration from now"
        );
        assert_eq!(renewed.attempt(), lease.attempt());

        // node-a stalls until its lease expires and node-b takes the range.
        clock.advance(LEASE);
        assert!(matches!(
            b.claim(run_id(), plan[0]).await.unwrap(),
            ClaimOutcome::Claimed(_)
        ));

        // node-a's next renewal is refused rather than silently extending node-b's lease.
        assert_eq!(a.renew(run_id(), &renewed).await.unwrap(), None);
        assert_eq!(store.leases(run_id()).await.unwrap()[0].node_id, "node-b");
    }

    #[tokio::test]
    async fn dst_012_a_released_lease_is_claimable_at_once_and_keeps_its_attempt() {
        let plan = plan(1);
        let (store, clock) = initialised(&plan).await;
        let a = node(&store, "node-a", &clock, ReclaimPolicy::Reclaim);
        let b = node(&store, "node-b", &clock, ReclaimPolicy::Reclaim);

        let ClaimOutcome::Claimed(lease) = a.claim(run_id(), plan[0]).await.unwrap() else {
            panic!("the first claim must be granted");
        };
        a.release(run_id(), &lease).await.unwrap();

        // No clock movement at all: the range is free the moment it is released.
        let ClaimOutcome::Claimed(next) = b.claim(run_id(), plan[0]).await.unwrap() else {
            panic!("a released lease must be claimable without waiting for expiry");
        };
        assert_eq!(next.attempt(), 2, "the attempt count survives a release");

        // A node that no longer holds a lease cannot release the new holder's.
        a.release(run_id(), &lease).await.unwrap();
        assert_eq!(store.leases(run_id()).await.unwrap()[0].node_id, "node-b");
    }

    #[tokio::test]
    async fn dst_012_an_expired_lease_is_not_reclaimed_when_the_policy_refuses() {
        // The seam DST-014/DST-015 (#51) lands on: for a counter target, a reclaim is not a
        // repeated write but a wrong number, so the coordinator is told not to take one over.
        let plan = plan(1);
        let (store, clock) = initialised(&plan).await;
        let a = node(&store, "node-a", &clock, ReclaimPolicy::Reclaim);
        let b = node(&store, "node-b", &clock, ReclaimPolicy::Refuse);

        assert!(matches!(
            a.claim(run_id(), plan[0]).await.unwrap(),
            ClaimOutcome::Claimed(_)
        ));
        clock.advance(LEASE);
        match b.claim(run_id(), plan[0]).await.unwrap() {
            ClaimOutcome::ReclaimRefused { node, attempt } => {
                assert_eq!(node, "node-a");
                assert_eq!(attempt, 1);
            }
            other => panic!("a refusing coordinator must not reclaim: {other:?}"),
        }
        assert_eq!(
            store.leases(run_id()).await.unwrap()[0].node_id,
            "node-a",
            "a refusal writes nothing"
        );
    }

    #[tokio::test]
    async fn dst_013_a_range_stops_being_reclaimed_after_max_attempts_and_is_marked_fail() {
        let plan = plan(1);
        let (store, clock) = initialised(&plan).await;
        let nodes: Vec<Coordinator> = ["node-a", "node-b", "node-c"]
            .into_iter()
            .map(|name| node(&store, name, &clock, ReclaimPolicy::Reclaim))
            .collect();

        // Three nodes each claim the range and die without finishing it.
        for (attempt, coordinator) in nodes.iter().enumerate() {
            let ClaimOutcome::Claimed(lease) = coordinator.claim(run_id(), plan[0]).await.unwrap()
            else {
                panic!("attempt {} must be granted", attempt + 1);
            };
            assert_eq!(lease.attempt() as usize, attempt + 1);
            clock.advance(LEASE);
        }

        // The fourth claim would be attempt four, and `cluster.max_attempts` is three.
        let outcome = nodes[0].claim(run_id(), plan[0]).await.unwrap();
        let ClaimOutcome::Exhausted { attempts, node } = outcome else {
            panic!("a range that has defeated three nodes must not be claimed again: {outcome:?}");
        };
        assert_eq!(attempts, 3);
        assert_eq!(node, "node-c");

        nodes[0].abandon(run_id(), plan[0], attempts).await.unwrap();
        let ranges = store.ranges(run_id()).await.unwrap();
        assert_eq!(ranges[0].status, RunStatus::Fail);
        assert!(ranges[0].info.as_deref().unwrap().contains("Abandoned"));

        // And the lease row is left alone, so the abandonment is not mistaken for a claim.
        assert_eq!(store.leases(run_id()).await.unwrap()[0].attempt, 3);
    }

    #[tokio::test]
    async fn dst_013_an_exhausted_range_is_skipped_and_the_rest_of_the_plan_is_not() {
        let plan = plan(2);
        let (store, clock) = initialised(&plan).await;
        let coordinator = node(&store, "node-a", &clock, ReclaimPolicy::Reclaim);

        for _ in 0..3 {
            assert!(matches!(
                coordinator.claim(run_id(), plan[0]).await.unwrap(),
                ClaimOutcome::Claimed(_)
            ));
            clock.advance(LEASE);
        }
        let lease = coordinator
            .claim_first(run_id(), &plan)
            .await
            .unwrap()
            .expect("the second range is still work");
        assert_eq!(lease.range(), plan[1]);
    }

    /// A clock that moves with Tokio's virtual time, so a renewal loop can be observed without
    /// waiting for one. Under `start_paused` the runtime advances time only when every task is
    /// idle, which makes the interleaving below deterministic rather than merely likely.
    #[derive(Debug)]
    struct TokioClock {
        base: tokio::time::Instant,
    }

    impl TokioClock {
        fn new() -> Self {
            Self {
                base: tokio::time::Instant::now(),
            }
        }
    }

    impl Clock for TokioClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            chrono::DateTime::UNIX_EPOCH + chrono_duration(self.base.elapsed())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn dst_012_keep_alive_renews_on_the_heartbeat_and_releases_on_cancellation() {
        let plan = plan(1);
        let store = Arc::new(MemoryStore::new());
        let clock: Arc<dyn Clock> = Arc::new(TokioClock::new());
        let coordinator = Coordinator::with_clock(
            Arc::clone(&store) as Arc<dyn LeaseStore>,
            NodeId::new("node-a").unwrap(),
            settings(ReclaimPolicy::Reclaim),
            Arc::clone(&clock),
        );
        coordinator
            .join(&run_record(), &range_records(&plan), HASH)
            .await
            .unwrap();
        let ClaimOutcome::Claimed(lease) = coordinator.claim(run_id(), plan[0]).await.unwrap()
        else {
            panic!("the first claim must be granted");
        };
        let granted_until = lease.expires_at();

        let cancel = CancellationToken::new();
        let keeper = {
            let coordinator = coordinator.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { coordinator.keep_alive(run_id(), lease, cancel).await })
        };

        // Three heartbeats of virtual time. The lease would have expired without them only after
        // four, which is the point: renewal has to outrun expiry.
        tokio::time::sleep(HEARTBEAT * 3 + Duration::from_secs(1)).await;
        let renewed_until = store.leases(run_id()).await.unwrap()[0].lease_until;
        assert!(
            renewed_until > granted_until,
            "the lease must have been extended: {renewed_until} is not after {granted_until}"
        );
        assert!(renewed_until > clock.now(), "and it must still be live");

        cancel.cancel();
        assert_eq!(keeper.await.unwrap(), LeaseEnd::Released);
        assert_eq!(
            store.leases(run_id()).await.unwrap()[0].lease_until,
            chrono::DateTime::UNIX_EPOCH,
            "a released lease is expired for every clock (DST-017 will reuse this)"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dst_012_keep_alive_reports_a_lease_another_node_has_taken() {
        let plan = plan(1);
        let (store, clock) = initialised(&plan).await;
        let a = node(&store, "node-a", &clock, ReclaimPolicy::Reclaim);
        let b = node(&store, "node-b", &clock, ReclaimPolicy::Reclaim);

        let ClaimOutcome::Claimed(lease) = a.claim(run_id(), plan[0]).await.unwrap() else {
            panic!("the first claim must be granted");
        };
        // node-a stalls past its own expiry — the GC-pause case — and node-b takes the range.
        clock.advance(LEASE);
        assert!(matches!(
            b.claim(run_id(), plan[0]).await.unwrap(),
            ClaimOutcome::Claimed(_)
        ));

        // node-a resumes and discovers at its next heartbeat that the range is not its own.
        assert_eq!(
            a.keep_alive(run_id(), lease, CancellationToken::new())
                .await,
            LeaseEnd::Lost
        );
    }
}
