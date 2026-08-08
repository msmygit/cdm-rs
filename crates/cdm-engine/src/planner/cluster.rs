//! The origin's real ring, as a [`ClusterTopology`] (`TOK-008`, `TOK-010`).
//!
//! [`InMemoryTopology`](super::InMemoryTopology) is the trait's test implementation; this is its
//! production one. It is a *snapshot*, read once before planning and then answered from memory,
//! for two reasons:
//!
//! * [`ClusterTopology`] is synchronous, because the planner is pure and must stay so — a plan
//!   that could issue a query mid-split would be a plan that depends on when it ran;
//! * a plan is computed once per run (`ARCHITECTURE.md` §5.5), so a snapshot cannot go stale
//!   within the thing that uses it.
//!
//! Everything that touches the driver is in `cdm_cql::ring`, which is where `ARCHITECTURE.md` §3
//! requires it to be. This type holds three plain vectors.

use cdm_core::{CdmError, TableRef, TokenRange};
use cdm_cql::ring::{read_ring, read_size_estimates};

use super::partitioner::Partitioner;
use super::topology::{ClusterTopology, RingSegment, SizeEstimate};

/// A snapshot of the origin's ring and size estimates (`TOK-008`, `TOK-010`).
#[derive(Debug, Clone)]
pub struct CqlTopology {
    partitioner: Partitioner,
    ring: Vec<RingSegment>,
    table: TableRef,
    estimates: Vec<SizeEstimate>,
}

impl CqlTopology {
    /// Reads the ring and the table's size estimates from `session`.
    ///
    /// `bounds` is the segment of the ring the run covers, already resolved from
    /// `filter.token.*` (`TOK-002`): a boundary outside it can only produce a range the run would
    /// discard.
    ///
    /// # Errors
    ///
    /// Propagates a failure to read `system.local` or `system.peers`. Missing size estimates are
    /// not a failure — `TOK-010` is a `SHOULD`, and a plan without estimates is `fixed`, which is
    /// always usable.
    pub async fn load(
        session: &cdm_cql::connect::ClusterSession,
        partitioner: Partitioner,
        bounds: TokenRange,
        table: &TableRef,
    ) -> Result<Self, CdmError> {
        let keyspace = table.keyspace();
        let name = table.table();
        let ring = read_ring(session, bounds, keyspace, name)
            .await?
            .into_iter()
            .map(|owned| RingSegment::new(owned.range, owned.replicas))
            .collect();
        let estimates = read_size_estimates(session, keyspace, name)
            .await?
            .into_iter()
            .map(|estimate| {
                SizeEstimate::new(
                    estimate.range,
                    estimate.partitions_count,
                    estimate.mean_partition_size,
                )
            })
            .collect();
        Ok(Self {
            partitioner,
            ring,
            table: table.clone(),
            estimates,
        })
    }

    /// How many ring segments the snapshot holds.
    #[must_use]
    pub fn segments(&self) -> usize {
        self.ring.len()
    }

    /// How many size estimates the snapshot holds.
    #[must_use]
    pub fn estimates(&self) -> usize {
        self.estimates.len()
    }
}

impl ClusterTopology for CqlTopology {
    fn partitioner(&self) -> Result<Partitioner, CdmError> {
        // Detected from `system.local` by `TOK-001` before planning starts, and handed in: a
        // second detection here could disagree with the one the bounds were resolved from.
        Ok(self.partitioner)
    }

    fn ring(&self) -> Result<Vec<RingSegment>, CdmError> {
        Ok(self.ring.clone())
    }

    fn size_estimates(&self, table: &TableRef) -> Result<Vec<SizeEstimate>, CdmError> {
        // Estimates are per table and this snapshot holds exactly one table's. Answering a
        // question about another table with this table's rows would silently mis-size a plan.
        if table == &self.table {
            Ok(self.estimates.clone())
        } else {
            Ok(Vec::new())
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
mod tests {
    use super::*;

    fn snapshot() -> CqlTopology {
        CqlTopology {
            partitioner: Partitioner::Murmur3,
            ring: vec![RingSegment::new(
                TokenRange::MURMUR3_FULL,
                ["10.0.0.1:9042"],
            )],
            table: TableRef::new("ks", "orders"),
            estimates: vec![SizeEstimate::new(TokenRange::MURMUR3_FULL, 1_000, 128)],
        }
    }

    #[test]
    fn tok_008_a_snapshot_answers_the_planner_without_touching_the_cluster() {
        let topology = snapshot();
        assert_eq!(topology.partitioner().unwrap(), Partitioner::Murmur3);
        assert_eq!(topology.segments(), 1);
        assert_eq!(topology.ring().unwrap()[0].replicas, vec!["10.0.0.1:9042"]);
    }

    #[test]
    fn tok_010_estimates_are_only_answered_for_the_table_they_were_read_for() {
        let topology = snapshot();
        assert_eq!(topology.estimates(), 1);
        assert_eq!(
            topology
                .size_estimates(&TableRef::new("ks", "orders"))
                .unwrap()
                .len(),
            1
        );
        assert!(topology
            .size_estimates(&TableRef::new("ks", "customers"))
            .unwrap()
            .is_empty());
    }
}
