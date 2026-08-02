//! The cluster metadata the planner needs, behind a trait (`TOK-008`, `TOK-009`, `TOK-010`).
//!
//! Two of the three planning strategies, and the estimates `cdm plan` prints, need facts only the
//! origin cluster knows: who owns which slice of the ring, and how much data each slice holds.
//! Those facts arrive through [`ClusterTopology`] rather than through a `cdm-cql` session, for
//! the same reason `cdm-config` takes a `SchemaProvider` instead of a session
//! (`ARCHITECTURE.md` §3.2): the planner stays unit-testable without a cluster, the dependency
//! graph of §3 stays acyclic, and swapping the driver cannot reach the splitter.
//!
//! `cdm-cql` implements this trait over `system.local`, the driver's ring metadata and
//! `system.size_estimates`. [`InMemoryTopology`] implements it over a literal, which is what the
//! tests here and the `cdm plan --dry-run` path use.

use cdm_core::{CdmError, ErrorKind, TableRef, TokenRange};

use super::partitioner::Partitioner;

/// One contiguous slice of the ring together with the nodes that hold a replica of it.
///
/// This is Cassandra's `TokenRange` + `getReplicas` pair, flattened. Replicas are opaque
/// identifiers — a host id, an address, whatever the implementation finds most stable — because
/// the planner only ever compares them for equality and prints them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingSegment {
    /// The slice of the ring.
    pub range: TokenRange,
    /// The nodes holding it, in the driver's preference order.
    pub replicas: Vec<String>,
}

impl RingSegment {
    /// Creates a segment.
    pub fn new(range: TokenRange, replicas: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            range,
            replicas: replicas.into_iter().map(Into::into).collect(),
        }
    }
}

/// One row of `system.size_estimates`, reduced to what planning needs.
///
/// Cassandra reports these per node and per *primary* range; an implementation must deduplicate
/// across replicas before returning them, or every estimate is multiplied by the replication
/// factor. [`InMemoryTopology`] documents its rows as already deduplicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeEstimate {
    /// The slice of the ring the estimate covers.
    pub range: TokenRange,
    /// Estimated number of partitions in that slice.
    pub partitions_count: u64,
    /// Estimated mean size of one partition, in bytes.
    pub mean_partition_size: u64,
}

impl SizeEstimate {
    /// Creates an estimate.
    pub const fn new(range: TokenRange, partitions_count: u64, mean_partition_size: u64) -> Self {
        Self {
            range,
            partitions_count,
            mean_partition_size,
        }
    }
}

/// The cluster facts the planner may consult.
///
/// Every method returns a `Result`: a planner that cannot read the ring says so and falls back to
/// [`PlanStrategy::Fixed`](super::PlanStrategy) or reports the estimate as unknown, rather than
/// planning against a guess.
pub trait ClusterTopology: std::fmt::Debug {
    /// The partitioner reported by `system.local.partitioner` (`TOK-001`).
    ///
    /// # Errors
    ///
    /// Returns an error if the value cannot be read or is not one cdm-rs recognises.
    fn partitioner(&self) -> Result<Partitioner, CdmError>;

    /// The ring, as owned ranges with their replica sets (`TOK-008`).
    ///
    /// The segments must be non-overlapping and, taken together, cover the whole ring. The
    /// planner clips them to the configured bounds itself.
    ///
    /// # Errors
    ///
    /// Returns an error if the ring metadata is unavailable.
    fn ring(&self) -> Result<Vec<RingSegment>, CdmError>;

    /// `system.size_estimates` for one table, deduplicated to primary ranges (`TOK-009`).
    ///
    /// An empty result means "no estimate available" — a freshly written table has none — and is
    /// not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the estimates cannot be read.
    fn size_estimates(&self, table: &TableRef) -> Result<Vec<SizeEstimate>, CdmError>;
}

/// A [`ClusterTopology`] built from literals, for tests, `cdm plan` rehearsals and examples.
///
/// ```
/// use cdm_core::{TableRef, TokenRange};
/// use cdm_engine::planner::{ClusterTopology, InMemoryTopology, Partitioner, RingSegment};
///
/// let table = TableRef::new("ks", "tbl");
/// let topology = InMemoryTopology::new(Partitioner::Murmur3)
///     .with_segment(RingSegment::new(TokenRange::MURMUR3_FULL, ["node-1", "node-2"]));
///
/// assert_eq!(topology.partitioner().unwrap(), Partitioner::Murmur3);
/// assert_eq!(topology.ring().unwrap().len(), 1);
/// assert!(topology.size_estimates(&table).unwrap().is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct InMemoryTopology {
    partitioner: Partitioner,
    ring: Vec<RingSegment>,
    estimates: Vec<(TableRef, SizeEstimate)>,
    ring_failure: Option<String>,
}

impl InMemoryTopology {
    /// An empty topology for `partitioner`, with no ring segments and no estimates.
    pub const fn new(partitioner: Partitioner) -> Self {
        Self {
            partitioner,
            ring: Vec::new(),
            estimates: Vec::new(),
            ring_failure: None,
        }
    }

    /// Adds a ring segment.
    #[must_use]
    pub fn with_segment(mut self, segment: RingSegment) -> Self {
        self.ring.push(segment);
        self
    }

    /// Adds a size estimate for a table.
    #[must_use]
    pub fn with_estimate(mut self, table: TableRef, estimate: SizeEstimate) -> Self {
        self.estimates.push((table, estimate));
        self
    }

    /// Makes [`ClusterTopology::ring`] fail, so the fallback paths can be exercised.
    #[must_use]
    pub fn failing_ring(mut self, message: impl Into<String>) -> Self {
        self.ring_failure = Some(message.into());
        self
    }
}

impl ClusterTopology for InMemoryTopology {
    fn partitioner(&self) -> Result<Partitioner, CdmError> {
        Ok(self.partitioner)
    }

    fn ring(&self) -> Result<Vec<RingSegment>, CdmError> {
        match &self.ring_failure {
            Some(message) => Err(CdmError::new(ErrorKind::Read, message.clone())),
            None => Ok(self.ring.clone()),
        }
    }

    fn size_estimates(&self, table: &TableRef) -> Result<Vec<SizeEstimate>, CdmError> {
        Ok(self
            .estimates
            .iter()
            .filter(|(candidate, _)| candidate == table)
            .map(|(_, estimate)| *estimate)
            .collect())
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
    fn tok_008_the_in_memory_topology_answers_ring_and_partitioner_queries() {
        let topology = InMemoryTopology::new(Partitioner::Random).with_segment(RingSegment::new(
            TokenRange::new(0, 999).unwrap(),
            ["10.0.0.1", "10.0.0.2"],
        ));
        assert_eq!(topology.partitioner().unwrap(), Partitioner::Random);
        let ring = topology.ring().unwrap();
        assert_eq!(ring.len(), 1);
        assert_eq!(ring[0].replicas, vec!["10.0.0.1", "10.0.0.2"]);
    }

    #[test]
    fn tok_009_size_estimates_are_returned_per_table_and_default_to_empty() {
        let wanted = TableRef::new("ks", "orders");
        let other = TableRef::new("ks", "customers");
        let topology = InMemoryTopology::new(Partitioner::Murmur3).with_estimate(
            wanted.clone(),
            SizeEstimate::new(TokenRange::new(0, 99).unwrap(), 1_000, 256),
        );

        let estimates = topology.size_estimates(&wanted).unwrap();
        assert_eq!(estimates.len(), 1);
        assert_eq!(estimates[0].partitions_count, 1_000);
        assert_eq!(estimates[0].mean_partition_size, 256);
        assert!(topology.size_estimates(&other).unwrap().is_empty());
    }

    #[test]
    fn tok_008_a_topology_can_be_made_to_fail_so_fallbacks_are_testable() {
        let topology = InMemoryTopology::new(Partitioner::Murmur3).failing_ring("no ring metadata");
        let err = topology.ring().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Read);
        assert!(err.message().contains("no ring metadata"));
    }
}
