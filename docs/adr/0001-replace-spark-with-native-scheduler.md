# ADR-0001: Replace Spark with a native async scheduler

- **Status:** Accepted
- **Date:** 2026-08-02
- **Deciders:** cdm-rs maintainers
- **Relates to:** `ENG-001`, `TOK-003`, `DST-001`, `NFR-001`, `NFR-003`

## Context

Java CDM distributes work by turning token ranges into elements of a Spark RDD and calling
`parallelize(parts, parts.size)`. Spark provides three things: parallel execution across cores,
distribution across machines, and an accumulator for aggregating counters.

It also costs: a JVM, a version-pinned Spark installation (the single largest source of user support
issues), tens of gigabytes of configured heap, slow startup, and an operational model where the
migration tool is deployed like a data-processing cluster.

Critically, CDM's workload is **not** what Spark is good at. There is no shuffle, no join, no stage
boundary and no lineage. Every token range is embarrassingly parallel and completely independent.
The RDD is used purely as a work queue.

## Decision

Replace Spark with a Tokio-based work-stealing scheduler inside a single static binary.

- `perfops.workers` tasks pull token ranges from a queue and process each to completion.
- The token range remains the unit of work, tracking, resume and failure isolation — unchanged from
  Java, which preserves the resume semantics users depend on.
- Distribution is provided by `ADR-0003` (lease-based coordination in the target keyspace), not by a
  cluster manager.
- Counter aggregation, previously Spark's `AccumulatorV2`, becomes lock-free atomics plus periodic
  checkpoints to the tracking table.

## Consequences

**Positive.** No JVM and no Spark: the `NoSuchMethodError: scala.runtime.Statics.releaseFence()`
class of issue disappears entirely. Startup drops from tens of seconds to under two. Memory becomes
bounded and computable rather than heap-tuned. Async I/O means far higher request concurrency per
core than a thread-per-partition model. The tool can be embedded as a library.

**Negative.** We give up Spark's cluster management, so `ADR-0003` must earn its keep. We give up
the Spark UI, so `MET-010`–`MET-033` must provide better observability than what we removed — which
was, in fairness, already a user complaint.

**Neutral.** Users running CDM on an existing Spark cluster lose that integration. The migration
guide documents the replacement topology; in practice the overwhelming majority of documented usage
is `--master "local[*]"` on a single VM, which this replaces outright.

## Alternatives considered

- **Keep Spark via JNI or a subprocess shim.** Preserves the ops story exactly, but retains the JVM,
  caps the achievable performance, and makes the codebase harder to reason about than either pure
  option.
- **Rayon instead of Tokio.** Rayon is designed for CPU-bound data parallelism. This workload is
  I/O-bound with high request concurrency, which is Tokio's domain.
- **Ballista or DataFusion.** Reintroduces a distributed execution framework to solve a problem that
  is a work queue.
