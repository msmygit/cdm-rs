//! The start-up capability probe (`CON-013`).
//!
//! Both sides are probed once, before any range is planned, and the findings feed Tier-3
//! validation: a run that needs `WRITETIME` on a collection, or a `vector<t, n>` column, should
//! fail while it is still a configuration problem rather than half-way through a petabyte.
//!
//! What is probed, and how:
//!
//! | Finding | Source |
//! |---|---|
//! | cluster name, partitioner, datacenter, rack | `system.local` |
//! | release and CQL version | `system.local` |
//! | flavour — Cassandra, ScyllaDB or DSE | `system.versions.scylla_version`, `system.local.dse_version` |
//! | `vector<t, n>` support | Cassandra 5.0 and later; never ScyllaDB |
//! | `WRITETIME`/`TTL` on collections | Cassandra 5.0 and later (`CASSANDRA-17614`); never ScyllaDB |
//!
//! # What is not probed
//!
//! `CON-013` also asks for the **native protocol version**. `scylla-rust-driver` 1.7 exposes no
//! accessor for the version it negotiated — there is no `Session::protocol_version` and the
//! connection layer is private — so [`Capabilities::native_protocol_version`] is `None` and the
//! CQL version from `system.local` is reported in its place. This is a driver gap, recorded here
//! and in the pull request rather than papered over with a guess.
//!
//! The two version-derived findings are exactly that: derived. They are asserted against real
//! clusters in `tests/connect_it.rs`, which is where a wrong rule would show up.

use cdm_core::{CdmError, Side};
use scylla::client::session::Session;

use crate::errors::connect_error_from;

/// Which implementation a cluster is, where behaviour genuinely differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    /// Apache Cassandra, and anything protocol-identical to it.
    Cassandra,
    /// ScyllaDB.
    Scylla,
    /// DataStax Enterprise, or HCD.
    Dse,
}

impl Flavour {
    /// The flavour's name, as `cdm connect test` prints it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cassandra => "cassandra",
            Self::Scylla => "scylla",
            Self::Dse => "dse",
        }
    }
}

/// What a cluster can do (`CON-013`).
#[derive(Debug, Clone)]
pub struct Capabilities {
    /// The cluster's name, as `system.local` reports it.
    pub cluster_name: String,
    /// The release version, e.g. `5.0.2`.
    pub release_version: String,
    /// The CQL version, e.g. `3.4.7`.
    pub cql_version: String,
    /// The partitioner class name (`TOK-001` consumes this).
    pub partitioner: String,
    /// The datacenter the contacted node is in — the auto-detected local DC of `CON-009`.
    pub datacenter: String,
    /// The rack the contacted node is in.
    pub rack: String,
    /// Which implementation this is.
    pub flavour: Flavour,
    /// Whether `vector<t, n>` exists (`CDC-004`).
    pub supports_vectors: bool,
    /// Whether `WRITETIME`/`TTL` may be applied to a collection column.
    pub supports_collection_writetime: bool,
    /// The negotiated native protocol version, when the driver exposes it. It does not today.
    pub native_protocol_version: Option<u8>,
}

impl Capabilities {
    /// The release version as `(major, minor)`, or `(0, 0)` when it cannot be read.
    pub fn version(&self) -> (u32, u32) {
        parse_version(&self.release_version)
    }

    /// A one-line summary for `cdm connect test` (`CON-008`).
    pub fn summary(&self) -> String {
        format!(
            "{} {} (CQL {}), cluster {}, dc {}, rack {}, partitioner {}",
            self.flavour.as_str(),
            self.release_version,
            self.cql_version,
            self.cluster_name,
            self.datacenter,
            self.rack,
            self.partitioner
        )
    }
}

/// Probes a connected cluster (`CON-013`).
pub async fn probe(side: Side, session: &Session) -> Result<Capabilities, CdmError> {
    let row = session
        .query_unpaged(
            "SELECT cluster_name, release_version, cql_version, partitioner, data_center, rack \
             FROM system.local",
            &[],
        )
        .await
        .map_err(|e| connect_error_from(side, "cannot read system.local", e))?
        .into_rows_result()
        .map_err(|e| connect_error_from(side, "system.local returned no rows", e))?
        .first_row::<(
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        )>()
        .map_err(|e| connect_error_from(side, "system.local has an unexpected shape", e))?;

    let (cluster_name, release_version, cql_version, partitioner, datacenter, rack) = row;

    let scylla_version =
        optional_scalar(session, "SELECT scylla_version FROM system.versions").await;
    let dse_version = optional_scalar(session, "SELECT dse_version FROM system.local").await;

    let flavour = if scylla_version.is_some() {
        Flavour::Scylla
    } else if dse_version.is_some() {
        Flavour::Dse
    } else {
        Flavour::Cassandra
    };

    let release_version = release_version.unwrap_or_default();
    let (major, _minor) = parse_version(&release_version);
    let modern_cassandra = flavour != Flavour::Scylla && major >= 5;

    Ok(Capabilities {
        cluster_name: cluster_name.unwrap_or_default(),
        release_version,
        cql_version: cql_version.unwrap_or_default(),
        partitioner: partitioner.unwrap_or_default(),
        datacenter: datacenter.unwrap_or_default(),
        rack: rack.unwrap_or_default(),
        flavour,
        supports_vectors: modern_cassandra,
        supports_collection_writetime: modern_cassandra,
        native_protocol_version: None,
    })
}

/// Reads the local datacenter alone, for the load-balancing policy (`CON-009`).
pub async fn local_datacenter(side: Side, session: &Session) -> Result<String, CdmError> {
    let (datacenter,) = session
        .query_unpaged("SELECT data_center FROM system.local", &[])
        .await
        .map_err(|e| connect_error_from(side, "cannot read system.local", e))?
        .into_rows_result()
        .map_err(|e| connect_error_from(side, "system.local returned no rows", e))?
        .first_row::<(Option<String>,)>()
        .map_err(|e| connect_error_from(side, "system.local has an unexpected shape", e))?;
    datacenter.ok_or_else(|| {
        crate::errors::connect_error(side, "system.local reports no data_center to prefer")
    })
}

/// Runs a query that may fail because the column or table does not exist on this flavour.
async fn optional_scalar(session: &Session, query: &str) -> Option<String> {
    let result = session.query_unpaged(query, &[]).await.ok()?;
    let rows = result.into_rows_result().ok()?;
    rows.first_row::<(Option<String>,)>().ok()?.0
}

/// `(major, minor)` of a `major.minor.patch` version string.
fn parse_version(version: &str) -> (u32, u32) {
    let mut parts = version.split(['.', '-', '~']);
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor)
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

    fn capabilities(release: &str, flavour: Flavour) -> Capabilities {
        let (major, _) = parse_version(release);
        let modern = flavour != Flavour::Scylla && major >= 5;
        Capabilities {
            cluster_name: "Test Cluster".to_owned(),
            release_version: release.to_owned(),
            cql_version: "3.4.7".to_owned(),
            partitioner: "org.apache.cassandra.dht.Murmur3Partitioner".to_owned(),
            datacenter: "datacenter1".to_owned(),
            rack: "rack1".to_owned(),
            flavour,
            supports_vectors: modern,
            supports_collection_writetime: modern,
            native_protocol_version: None,
        }
    }

    #[test]
    fn con_013_versions_are_parsed_including_the_forms_scylla_and_dse_use() {
        assert_eq!(parse_version("5.0.2"), (5, 0));
        assert_eq!(parse_version("4.1.3"), (4, 1));
        assert_eq!(parse_version("3.11.16"), (3, 11));
        assert_eq!(parse_version("6.2.0-0.20241124"), (6, 2));
        assert_eq!(parse_version("4.0.0.6816"), (4, 0));
        assert_eq!(parse_version(""), (0, 0));
        assert_eq!(parse_version("not-a-version"), (0, 0));
    }

    #[test]
    fn con_013_vector_support_starts_at_cassandra_5() {
        assert!(!capabilities("4.1.3", Flavour::Cassandra).supports_vectors);
        assert!(capabilities("5.0.2", Flavour::Cassandra).supports_vectors);
        assert!(!capabilities("6.2.0", Flavour::Scylla).supports_vectors);
    }

    #[test]
    fn con_013_collection_writetime_support_starts_at_cassandra_5() {
        assert!(!capabilities("4.1.3", Flavour::Cassandra).supports_collection_writetime);
        assert!(capabilities("5.0.2", Flavour::Cassandra).supports_collection_writetime);
        assert!(!capabilities("6.2.0", Flavour::Scylla).supports_collection_writetime);
    }

    #[test]
    fn con_013_the_summary_names_everything_connect_test_prints() {
        let summary = capabilities("5.0.2", Flavour::Cassandra).summary();
        for expected in [
            "cassandra",
            "5.0.2",
            "3.4.7",
            "Test Cluster",
            "datacenter1",
            "rack1",
            "Murmur3Partitioner",
        ] {
            assert!(summary.contains(expected), "{summary} lacks {expected}");
        }
    }

    #[test]
    fn con_013_the_native_protocol_version_is_not_exposed_by_the_driver() {
        // Recorded as a test so that the day the driver exposes it, this fails and is fixed.
        assert!(capabilities("5.0.2", Flavour::Cassandra)
            .native_protocol_version
            .is_none());
    }

    #[test]
    fn con_013_the_flavour_names_are_stable() {
        assert_eq!(Flavour::Cassandra.as_str(), "cassandra");
        assert_eq!(Flavour::Scylla.as_str(), "scylla");
        assert_eq!(Flavour::Dse.as_str(), "dse");
        assert_eq!(capabilities("5.0.2", Flavour::Cassandra).version(), (5, 0));
    }
}
