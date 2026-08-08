//! Reading the origin's token ring and size estimates (`TOK-008`, `TOK-009`, `TOK-010`).
//!
//! `plan.strategy = ring_aware` splits along ring-ownership boundaries and `plan.strategy =
//! adaptive` subdivides ranges by their estimated row count. Both need facts only the cluster
//! has, and `ARCHITECTURE.md` §3 puts every cluster conversation in this crate. `cdm-engine`
//! consumes what is returned here through its own `ClusterTopology` trait, so the planner never
//! sees a driver type and stays testable against a literal.
//!
//! # Where the ring comes from
//!
//! From `system.local.tokens` and `system.peers.tokens`, not from the driver's metadata: the
//! driver exposes a replica *locator* but no public list of ring boundaries, and the two system
//! tables are the portable answer across Apache Cassandra, DSE and ScyllaDB — the same matrix
//! `TST-002` fixes and the same tables `nodetool ring` reads.
//!
//! A node owns the tokens from just after its predecessor's token up to and including its own.
//! The ring is circular and cdm-rs's [`TokenRange`] is not, so the segment that wraps past the
//! partitioner's maximum is emitted as the two halves it is made of. Nothing is lost by that: the
//! two halves have the same owner, and the planner splits each of them independently anyway.
//!
//! # Where the estimates come from, and what they are worth
//!
//! `system.size_estimates` is a **local** table: a query answers with the coordinator's rows and
//! nobody else's, so what comes back covers that node's primary ranges rather than the ring.
//! `TOK-010` is a `SHOULD` sized by estimates that Cassandra itself documents as approximate, and
//! the planner already treats a range it has no estimate for as one that needs no subdivision —
//! so partial coverage costs less subdivision, never a wrong plan. Saying so here is better than
//! implying a completeness the table cannot give.

use std::collections::BTreeSet;

use cdm_core::{CdmError, Side, TokenRange};
use scylla::client::session::Session;
use scylla::routing::Token;

use crate::connect::ClusterSession;
use crate::errors::side_error_from;

/// One slice of the ring and the nodes that hold it (`TOK-008`).
///
/// Driver-free by construction: replicas are rendered addresses, because that is all the planner
/// does with them — compare for equality, and print.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingOwnership {
    /// The tokens owned, inclusive at both ends.
    pub range: TokenRange,
    /// The nodes holding a replica of them, the primary first.
    pub replicas: Vec<String>,
}

/// One row of `system.size_estimates`, reduced to what planning needs (`TOK-009`, `TOK-010`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeSizeEstimate {
    /// The tokens the estimate covers.
    pub range: TokenRange,
    /// Estimated partitions in that slice.
    pub partitions_count: u64,
    /// Estimated mean partition size, in bytes.
    pub mean_partition_size: u64,
}

/// Reads the ring boundaries and their owners (`TOK-008`).
///
/// `keyspace` and `table` are needed only to resolve replica *sets*: replication is per keyspace,
/// so the same token has different owners in two keyspaces. When the driver does not know the
/// keyspace the segment still comes back, with an empty replica list — the boundaries are the
/// part `TOK-008` is about, and a range that names no replica is still a range that does not
/// straddle one.
///
/// # Errors
///
/// [`ErrorKind::Read`](cdm_core::ErrorKind::Read) if `system.local` or `system.peers` cannot be
/// read, and [`ErrorKind::Config`](cdm_core::ErrorKind::Config) if the ring they describe cannot
/// be made sense of.
pub async fn read_ring(
    cluster: &ClusterSession,
    bounds: TokenRange,
    keyspace: &str,
    table: &str,
) -> Result<Vec<RingOwnership>, CdmError> {
    let side = cluster.side();
    let session = cluster.session().as_ref();
    let mut tokens = read_tokens(side, session, "SELECT tokens FROM system.local").await?;
    tokens.append(&mut read_tokens(side, session, "SELECT tokens FROM system.peers").await?);

    let boundaries = ring_boundaries(&tokens, bounds);
    let state = session.get_cluster_state();
    Ok(boundaries
        .into_iter()
        .map(|(range, owner_token)| RingOwnership {
            range,
            replicas: replicas_for(&state, keyspace, table, owner_token),
        })
        .collect())
}

/// Reads `system.size_estimates` for one table (`TOK-009`, `TOK-010`).
///
/// An empty result means "no estimate", which a freshly written table genuinely has, and is not
/// an error. See the [module documentation](self) for why the result covers the coordinator's
/// ranges rather than the whole ring.
///
/// # Errors
///
/// [`ErrorKind::Read`](cdm_core::ErrorKind::Read) if the table cannot be read. A cluster without
/// `system.size_estimates` at all reports no estimates rather than failing: the planner's answer
/// without estimates is `fixed`, which is always a usable plan.
pub async fn read_size_estimates(
    cluster: &ClusterSession,
    keyspace: &str,
    table: &str,
) -> Result<Vec<RangeSizeEstimate>, CdmError> {
    let side = cluster.side();
    let session = cluster.session().as_ref();
    let query = "SELECT range_start, range_end, partitions_count, mean_partition_size \
                 FROM system.size_estimates WHERE keyspace_name = ? AND table_name = ?";
    let Ok(result) = session.query_unpaged(query, (keyspace, table)).await else {
        // Not every cluster has the table; `TOK-010` is a SHOULD and degrades to `fixed`.
        tracing::debug!(
            target: "cdm::cql::ring",
            keyspace,
            table,
            "system.size_estimates is unavailable; planning without row estimates (TOK-010)"
        );
        return Ok(Vec::new());
    };
    let rows = result
        .into_rows_result()
        .map_err(|error| read_error(side, "system.size_estimates", error))?;
    let typed = rows
        .rows::<(String, String, i64, i64)>()
        .map_err(|error| read_error(side, "system.size_estimates", error))?;

    let mut estimates = Vec::new();
    for row in typed {
        let (start, end, partitions, mean) =
            row.map_err(|error| read_error(side, "system.size_estimates", error))?;
        // `range_start`/`range_end` are the *exclusive* start and inclusive end of an owned
        // range, spelled as decimal token strings.
        let (Ok(start), Ok(end)) = (start.trim().parse::<i128>(), end.trim().parse::<i128>())
        else {
            continue;
        };
        let Ok(range) = TokenRange::new(start.saturating_add(1), end) else {
            // A wrapping row (`start > end`) covers the ring's seam; skipping it costs one range
            // its estimate and never mis-sizes another.
            continue;
        };
        estimates.push(RangeSizeEstimate {
            range,
            partitions_count: u64::try_from(partitions).unwrap_or(0),
            mean_partition_size: u64::try_from(mean).unwrap_or(0),
        });
    }
    Ok(estimates)
}

/// The ring boundaries `tokens` describe, clipped to `bounds`.
///
/// Pure, so the whole of the ring geometry — deduplication, ordering, the wrap past the
/// partitioner's maximum, tokens outside the configured bounds — is unit-testable with no
/// cluster. Returns each range together with the ring token that decides its owner.
fn ring_boundaries(tokens: &[i128], bounds: TokenRange) -> Vec<(TokenRange, i128)> {
    let sorted: Vec<i128> = tokens
        .iter()
        .copied()
        .filter(|token| bounds.contains(*token) || *token == bounds.min())
        .collect::<BTreeSet<i128>>()
        .into_iter()
        .collect();
    if sorted.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(sorted.len() + 1);
    let mut lower = bounds.min();
    for token in &sorted {
        if let Ok(range) = TokenRange::new(lower, *token) {
            out.push((range, *token));
        }
        lower = token.saturating_add(1);
    }
    // The ring wraps: everything past the last token belongs to the node that owns the *first*
    // one. It is emitted as its own range because a `TokenRange` cannot wrap.
    if lower <= bounds.max() {
        if let Ok(range) = TokenRange::new(lower, bounds.max()) {
            let first = sorted.first().copied().unwrap_or(lower);
            out.push((range, first));
        }
    }
    out
}

/// The replicas the driver believes own `token` in `keyspace`.
fn replicas_for(
    state: &scylla::cluster::ClusterState,
    keyspace: &str,
    table: &str,
    token: i128,
) -> Vec<String> {
    let Ok(value) = i64::try_from(token) else {
        // A RandomPartitioner token does not fit the driver's `Token`. The boundary is still
        // correct, which is what `TOK-008` is about; only the replica names are unavailable.
        return Vec::new();
    };
    state
        .get_token_endpoints(keyspace, table, Token::new(value))
        .into_iter()
        .map(|(node, _shard)| node.address.to_string())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// Reads one `tokens` column from a system table, ignoring a table that does not exist.
async fn read_tokens(side: Side, session: &Session, query: &str) -> Result<Vec<i128>, CdmError> {
    let table = query.rsplit(' ').next().unwrap_or("system.local");
    let rows = session
        .query_unpaged(query, ())
        .await
        .map_err(|error| read_error(side, table, error))?
        .into_rows_result()
        .map_err(|error| read_error(side, table, error))?;
    let typed = rows
        .rows::<(Option<Vec<String>>,)>()
        .map_err(|error| read_error(side, table, error))?;

    let mut tokens = Vec::new();
    for row in typed {
        let (values,) = row.map_err(|error| read_error(side, table, error))?;
        for value in values.unwrap_or_default() {
            if let Ok(token) = value.trim().parse::<i128>() {
                tokens.push(token);
            }
        }
    }
    Ok(tokens)
}

/// A failure to read cluster metadata.
fn read_error(
    side: Side,
    table: &str,
    cause: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
) -> CdmError {
    side_error_from(
        cdm_core::ErrorKind::Read,
        side,
        format!("cannot read {table}, which `plan.strategy` needs (TOK-008)"),
        cause,
    )
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

    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    #[test]
    fn tok_008_the_ring_is_the_gaps_between_the_nodes_tokens() {
        let bounds = range(-100, 100);
        let segments = ring_boundaries(&[-50, 0, 50], bounds);
        assert_eq!(
            segments,
            vec![
                (range(-100, -50), -50),
                (range(-49, 0), 0),
                (range(1, 50), 50),
                // The wrap: everything past the last token is owned by the first token's node.
                (range(51, 100), -50),
            ]
        );
    }

    #[test]
    fn tok_008_duplicate_and_unordered_tokens_describe_the_same_ring() {
        let bounds = range(-100, 100);
        let ordered = ring_boundaries(&[-50, 0, 50], bounds);
        // `system.peers` reports a set per node, in no order, and a token can be reported twice
        // by two nodes that both list it.
        let shuffled = ring_boundaries(&[50, -50, 0, 50, -50], bounds);
        assert_eq!(ordered, shuffled);
    }

    #[test]
    fn tok_008_the_segments_cover_the_bounds_exactly_once() {
        let bounds = range(i128::from(i64::MIN), i128::from(i64::MAX));
        let segments = ring_boundaries(&[-4_000, 0, 900_000, i128::from(i64::MAX)], bounds);
        assert_eq!(segments[0].0.min(), bounds.min());
        assert_eq!(segments[segments.len() - 1].0.max(), bounds.max());
        for pair in segments.windows(2) {
            assert_eq!(pair[0].0.max() + 1, pair[1].0.min());
        }
        let covered: u128 = segments.iter().map(|(r, _)| r.token_count()).sum();
        assert_eq!(covered, bounds.token_count());
    }

    #[test]
    fn tok_008_a_token_on_the_upper_bound_does_not_produce_an_empty_wrap() {
        let bounds = range(0, 100);
        let segments = ring_boundaries(&[100], bounds);
        assert_eq!(segments, vec![(range(0, 100), 100)]);
    }

    #[test]
    fn tok_008_tokens_outside_the_configured_bounds_are_ignored() {
        // `filter.token.*` narrows the ring; a boundary the run will never reach must not
        // manufacture a range outside it (`TOK-002`).
        let bounds = range(0, 100);
        let segments = ring_boundaries(&[-5_000, 40, 8_000], bounds);
        assert_eq!(segments, vec![(range(0, 40), 40), (range(41, 100), 40)]);
    }

    #[test]
    fn tok_008_a_cluster_that_reports_no_tokens_describes_no_ring() {
        assert!(ring_boundaries(&[], range(0, 100)).is_empty());
    }
}
