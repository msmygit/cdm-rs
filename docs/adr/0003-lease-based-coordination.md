# ADR-0003: Lease-based distributed coordination in the target keyspace

- **Status:** Accepted
- **Date:** 2026-08-02
- **Relates to:** `DST-001`–`DST-019`, `TRK-011`

## Context

Having removed Spark (`ADR-0001`), multi-node runs need a way for N processes to agree on who
processes which token range, and to recover ranges owned by a process that died.

## Decision

Coordinate through the target keyspace, which every run already requires. No ZooKeeper, no etcd, no
Raft, no message broker.

- **Election.** The initialising node is chosen by `INSERT ... IF NOT EXISTS` on the `cdm_run_info`
  row. Losers wait for the row to reach `STARTED`.
- **Claiming.** A `cdm_run_leases` table keyed by `((table_name, run_id), token_min)`; a claim is an
  LWT conditional on the lease being absent or expired.
- **Renewal.** Every `cluster.heartbeat_interval` while the range is in flight; leases expire after
  `cluster.lease_duration`.
- **Reclaim.** Any node may claim an expired lease, incrementing `attempt`. After
  `cluster.max_attempts` the range is marked `FAIL` rather than looping forever.
- **Config consistency.** The initialiser records a secret-redacted hash of the effective config;
  joining nodes with a different hash refuse to join and print a diff.

## Consequences

**Positive.** Zero additional infrastructure. Failure recovery is the same mechanism as slow-node
handling. LWT cost is per range, not per row: with the default 5000 ranges and 60-second leases it is
negligible next to data traffic. Coordination state is inspectable with `cqlsh`.

**Negative.** LWTs require Paxos, adding latency and load to the target cluster. Mitigated by the
per-range granularity and by `cluster.enabled` being opt-in. Clock skew between nodes affects lease
expiry; `cluster.lease_duration` defaults to 60s, far above realistic NTP skew, and expiry is
evaluated by the coordinator's own clock via LWT.

**Critical constraint.** Reclaiming a range is safe only because migrate is idempotent — upserts
carry the origin writetime, so re-writing is a storage-layer no-op. This does **not** hold for
counter tables, where `SET c = c + delta` is not idempotent. `DST-015` therefore refuses to reclaim
an in-flight counter range and marks it `FAIL` with an explicit "manual reconciliation required"
message. Correctness over convenience.

## Alternatives considered

- **Static partitioning** (node *i* of *n* takes every *n*-th range). Trivially simple, but a dead
  node's work is silently never done, and stragglers cannot be rebalanced.
- **External coordinator** (etcd/ZooKeeper). Better primitives, but requiring users to deploy a
  consensus service to migrate a table is exactly the operational burden we removed with Spark.
- **A dedicated controller process.** Introduces a single point of failure and a second deployment
  artefact.
