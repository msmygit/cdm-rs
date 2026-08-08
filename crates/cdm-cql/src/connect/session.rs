//! Building an origin or a target session (`CON-001`, `CON-002`, `CON-009`, `CON-013`).
//!
//! One function, [`connect`], turns a [`CdmConfig`] and a [`Side`] into a live [`Session`]. The
//! two sides share no state: separate credentials, separate TLS material, separate consistency
//! level, separate rate limit, separate everything (`CON-001`). That is why this takes a side
//! rather than returning a pair — nothing in the path can accidentally read the other side's
//! settings.
//!
//! # The two-phase local datacenter (`CON-009`)
//!
//! The load-balancing policy is DC-aware, and the local DC is auto-detected when
//! `connect.{side}.local_datacenter` is unset. But the DC can only be read *through* a session,
//! which needs a policy. cdm-rs therefore connects with a token-aware, DC-agnostic policy, reads
//! `system.local.data_center`, and swaps a DC-aware profile into the same
//! [`ExecutionProfileHandle`] — the driver's own mechanism for changing a profile under a live
//! session. No reconnection, no second session, and the very first request already uses the final
//! policy.

use std::num::NonZeroUsize;
use std::sync::Arc;

use cdm_config::model::CdmConfig;
use cdm_config::types::ConsistencyLevel;
use cdm_core::{CdmError, Side};
use scylla::client::execution_profile::{ExecutionProfile, ExecutionProfileHandle};
use scylla::client::session::{Session, TlsContext};
use scylla::client::session_builder::SessionBuilder;
use scylla::client::PoolSize;
use scylla::policies::host_filter::AllowListHostFilter;
use scylla::statement::{Consistency, SerialConsistency};

use crate::astra::bundle::SecureConnectBundle;
use crate::astra::strategy::{self, AstraConnection, AstraCredentials};
use crate::astra::tempdir::BundleTempDir;
use crate::astra::DevOpsClient;
use crate::connect::mode::{self, ConnectionMode};
use crate::connect::policy::{
    load_balancing_policy, speculative_policy, Backoff, CdmRetryPolicy, SpeculativeSettings,
};
use crate::connect::probe::{self, Capabilities};
use crate::errors::{config_error, connect_error, connect_error_from};
use crate::tls::{self, Identity, StoreFormat, TlsSpec, TrustMaterial};

/// A connected cluster, with everything the run needs to know about it.
#[derive(Debug)]
pub struct ClusterSession {
    session: Arc<Session>,
    side: Side,
    mode: ConnectionMode,
    contact_points: Vec<String>,
    capabilities: Capabilities,
    local_datacenter: String,
    backoff: Backoff,
    astra: Option<AstraConnection>,
    // Removes a downloaded bundle when the session goes away (`CON-005`).
    _bundle_dir: Option<BundleTempDir>,
    profile: ExecutionProfileHandle,
}

impl ClusterSession {
    /// The driver session.
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Which side this is.
    pub fn side(&self) -> Side {
        self.side
    }

    /// How the connection was made (`CON-002`).
    pub fn mode(&self) -> &ConnectionMode {
        &self.mode
    }

    /// The addresses the session was given.
    pub fn contact_points(&self) -> &[String] {
        &self.contact_points
    }

    /// What the cluster can do (`CON-013`).
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// The datacenter the load-balancing policy prefers (`CON-009`).
    pub fn local_datacenter(&self) -> &str {
        &self.local_datacenter
    }

    /// The backoff the caller should apply between its own retries (`CON-011`).
    pub fn backoff(&self) -> Backoff {
        self.backoff
    }

    /// The Astra details, when this side is Astra (`CON-029`).
    pub fn astra(&self) -> Option<&AstraConnection> {
        self.astra.as_ref()
    }

    /// The execution profile in force, so that a job can derive its own from it.
    pub fn execution_profile(&self) -> &ExecutionProfileHandle {
        &self.profile
    }

    /// The cluster's nodes, as the driver's metadata currently sees them (`MET-031`).
    ///
    /// Read from the driver rather than from `system.peers`: the driver keeps this view refreshed
    /// by its own topology events, so a live display gets a node going down without issuing a query
    /// of its own on every frame.
    ///
    /// Sorted by address, so that a display redrawn twice a second does not reorder its own rows.
    /// `connected` is the driver's `Node::is_connected`, which means "this process currently holds a
    /// connection pool to it" — not "the node is up as far as its own gossip is concerned". The
    /// two differ exactly when they are interesting: a node the host filter excluded, or one this
    /// client alone cannot reach.
    pub fn nodes(&self) -> Vec<ClusterNode> {
        let mut nodes: Vec<ClusterNode> = self
            .session
            .get_cluster_state()
            .get_nodes_info()
            .iter()
            .map(|node| ClusterNode {
                address: node.address.to_string(),
                datacenter: node.datacenter.clone(),
                rack: node.rack.clone(),
                connected: node.is_connected(),
            })
            .collect();
        nodes.sort_by(|left, right| left.address.cmp(&right.address));
        nodes
    }
}

/// One node of a connected cluster (`MET-031`).
///
/// A plain value rather than the driver's `Node`, so that the live display of `MET-031` can be
/// rendered by `cdm-cli` without `scylla` leaving this crate — `ARCHITECTURE.md` §3 allows the
/// dependency here and nowhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterNode {
    /// The address the driver connects to it on.
    pub address: String,
    /// Its datacenter, when the driver knows one.
    pub datacenter: Option<String>,
    /// Its rack, when the driver knows one.
    pub rack: Option<String>,
    /// Whether this process currently holds a connection pool to it.
    pub connected: bool,
}

/// Connects one side (`CON-001`, `CON-002`).
pub async fn connect(config: &CdmConfig, side: Side) -> Result<ClusterSession, CdmError> {
    let mode = mode::detect(config, side)?;
    // The driver's `build()` future is large; boxing keeps it off this frame rather than growing
    // every caller's (`clippy::large_futures`).
    Box::pin(connect_with_mode(config, side, mode)).await
}

/// Everything the mode decides: where to connect, with what TLS, and as whom.
#[derive(Debug)]
struct Endpoint {
    contact_points: Vec<String>,
    tls: Option<Arc<rustls::ClientConfig>>,
    username: String,
    password: String,
    astra: Option<AstraConnection>,
    bundle_dir: Option<BundleTempDir>,
}

/// Resolves the material and endpoints for a mode (`CON-002`).
async fn resolve_endpoint(
    config: &CdmConfig,
    side: Side,
    mode: &ConnectionMode,
) -> Result<Endpoint, CdmError> {
    let settings = mode::side_config(config, side);
    let cipher_suites = settings.tls.cipher_suites.clone();
    let direct = || vec![format!("{}:{}", settings.host, settings.port)];

    let mut endpoint = Endpoint {
        contact_points: direct(),
        tls: None,
        username: settings.username.clone(),
        password: settings.password.expose().clone(),
        astra: None,
        bundle_dir: None,
    };

    match mode {
        ConnectionMode::Bundle { path } => {
            let bundle = SecureConnectBundle::from_path(side, path)?;
            let resolved = resolve_astra(side, &bundle, config, cipher_suites).await?;
            endpoint.contact_points = vec![resolved.contact_point_string()];
            endpoint.tls = Some(resolved.tls.clone());
            endpoint.astra = Some(resolved);
            astra_credentials(side, &mut endpoint)?;
        }
        ConnectionMode::BundleDownload { database_id } => {
            let selector = crate::astra::BundleSelector {
                scb_type: settings.astra.scb_type,
                region: settings.astra.region.clone(),
                custom_domain: settings.astra.custom_domain.clone(),
            };
            // The DevOps API's bearer token is the side's password (CON-004, CON-028).
            let (dir, path, url) = DevOpsClient::new(side)?
                .fetch_bundle(database_id, &endpoint.password, &selector)
                .await?;
            let bundle = SecureConnectBundle::from_path(side, &path)?;
            let mut resolved = resolve_astra(side, &bundle, config, cipher_suites).await?;
            resolved.bundle_origin = url;
            endpoint.contact_points = vec![resolved.contact_point_string()];
            endpoint.tls = Some(resolved.tls.clone());
            endpoint.astra = Some(resolved);
            endpoint.bundle_dir = Some(dir);
            astra_credentials(side, &mut endpoint)?;
        }
        ConnectionMode::AstraFromStores => {
            endpoint.tls = Some(tls_spec_from_stores(config, side)?.client_config()?);
            astra_credentials(side, &mut endpoint)?;
        }
        ConnectionMode::Tls => {
            endpoint.tls = Some(tls_spec_from_stores(config, side)?.client_config()?);
        }
        ConnectionMode::Plain => {}
    }
    Ok(endpoint)
}

/// Normalises the credentials for an Astra side (`CON-028`).
fn astra_credentials(side: Side, endpoint: &mut Endpoint) -> Result<(), CdmError> {
    let credentials = AstraCredentials::resolve(side, &endpoint.username, &endpoint.password)?;
    credentials.username().clone_into(&mut endpoint.username);
    credentials.password().clone_into(&mut endpoint.password);
    Ok(())
}

/// Connects one side with a mode already chosen, which is what `cdm connect test` needs when it
/// reports the mode before connecting.
pub async fn connect_with_mode(
    config: &CdmConfig,
    side: Side,
    mode: ConnectionMode,
) -> Result<ClusterSession, CdmError> {
    let Endpoint {
        contact_points,
        tls: tls_context,
        username,
        password,
        astra,
        bundle_dir,
    } = resolve_endpoint(config, side, &mode).await?;
    let settings = mode::side_config(config, side);

    tracing::info!(
        side = side.as_str(),
        rule = "CON-002",
        mode = mode.as_str(),
        contact_points = contact_points.join(","),
        "connecting"
    );

    // The profile the session starts with: everything final except the DC preference, which
    // needs a session to discover (CON-009).
    let configured_dc = settings.local_datacenter.clone();
    let profile = build_profile(config, side, configured_dc.as_deref(), false)?;
    let handle = profile.into_handle();

    let mut builder = SessionBuilder::new()
        .known_nodes(&contact_points)
        .user(username, password)
        .connection_timeout(config.perfops.request_timeout.get())
        .default_execution_profile_handle(handle.clone())
        .pool_size(PoolSize::PerHost(
            NonZeroUsize::new(config.perfops.connection_pool_size.max(1) as usize)
                .unwrap_or(NonZeroUsize::MIN),
        ));

    if let Some(context) = tls_context {
        builder = builder.tls_context(Some(TlsContext::Rustls023(context)));
    }

    // The single-endpoint Astra strategy terminates on one address; the private peer addresses
    // Astra advertises are unroutable, so the driver is told not to try them (`CON-026`).
    if astra.is_some() {
        let filter = AllowListHostFilter::new(contact_points.clone()).map_err(|e| {
            connect_error_from(side, "cannot resolve the Astra endpoint address", e)
        })?;
        builder = builder.host_filter(Arc::new(filter));
    }

    // The driver's own `build()` future is ~30 kB; boxing it keeps this frame small
    // (`clippy::large_futures`).
    let session = Box::pin(builder.build())
        .await
        .map_err(|e| connect_error_from(side, "cannot establish a session", e))?;

    let capabilities = probe::probe(side, &session).await?;
    let local_datacenter = match configured_dc {
        Some(datacenter) => datacenter,
        None => astra
            .as_ref()
            .and_then(|a| a.local_dc.clone())
            .unwrap_or_else(|| capabilities.datacenter.clone()),
    };

    if local_datacenter.is_empty() {
        tracing::warn!(
            side = side.as_str(),
            rule = "CON-009",
            "no local datacenter could be detected; the load-balancing policy stays DC-agnostic"
        );
    } else {
        let mut handle = handle.clone();
        handle.map_to_another_profile(build_profile(config, side, Some(&local_datacenter), false)?);
        tracing::info!(
            side = side.as_str(),
            rule = "CON-009",
            local_datacenter,
            configured = false,
            "load balancing is token-aware, latency-aware and pinned to the local datacenter"
        );
    }

    tracing::info!(
        side = side.as_str(),
        rule = "CON-013",
        "{}",
        capabilities.summary()
    );

    Ok(ClusterSession {
        session: Arc::new(session),
        side,
        mode,
        contact_points,
        capabilities,
        local_datacenter,
        backoff: Backoff::new(
            config.perfops.retry.initial_backoff.get(),
            config.perfops.retry.max_backoff.get(),
            config.perfops.retry.max_attempts,
        ),
        astra,
        _bundle_dir: bundle_dir,
        profile: handle,
    })
}

/// Builds the execution profile for a side (`CON-009`, `CON-010`, `CON-011`).
///
/// `writes_counters` is known only after schema introspection, so the profile is built once
/// without it at connect time and rebuilt by the job when it knows (`CON-012`).
pub fn build_profile(
    config: &CdmConfig,
    side: Side,
    local_datacenter: Option<&str>,
    writes_counters: bool,
) -> Result<ExecutionProfile, CdmError> {
    let settings = mode::side_config(config, side);
    let consistency = match side {
        Side::Origin => config.perfops.consistency.read,
        Side::Target => config.perfops.consistency.write,
    };
    let (consistency, serial) = driver_consistency(side, consistency)?;

    let speculative = SpeculativeSettings {
        enabled: settings.speculative.enabled,
        delay: settings.speculative.delay.get(),
        max_executions: settings.speculative.max_executions,
    };

    let mut builder = ExecutionProfile::builder()
        .consistency(consistency)
        .serial_consistency(serial)
        .request_timeout(Some(config.perfops.request_timeout.get()))
        .load_balancing_policy(load_balancing_policy(local_datacenter))
        .retry_policy(Arc::new(CdmRetryPolicy::new(
            config.perfops.retry.max_attempts,
        )));

    builder =
        builder.speculative_execution_policy(speculative_policy(speculative, writes_counters));

    Ok(builder.build())
}

/// Maps a configured consistency level onto the driver's, splitting off the serial levels.
///
/// `SERIAL` and `LOCAL_SERIAL` are not ordinary consistency levels: they belong to the
/// lightweight-transaction path and the driver models them separately. Java's Spark connector
/// accepts them in the same property, so cdm-rs accepts them too and routes them to the right
/// place rather than rejecting the configuration.
pub fn driver_consistency(
    side: Side,
    level: ConsistencyLevel,
) -> Result<(Consistency, Option<SerialConsistency>), CdmError> {
    Ok(match level {
        ConsistencyLevel::Any => (Consistency::Any, None),
        ConsistencyLevel::One => (Consistency::One, None),
        ConsistencyLevel::Two => (Consistency::Two, None),
        ConsistencyLevel::Three => (Consistency::Three, None),
        ConsistencyLevel::Quorum => (Consistency::Quorum, None),
        ConsistencyLevel::LocalOne => (Consistency::LocalOne, None),
        ConsistencyLevel::LocalQuorum => (Consistency::LocalQuorum, None),
        ConsistencyLevel::EachQuorum => (Consistency::EachQuorum, None),
        ConsistencyLevel::All => (Consistency::All, None),
        ConsistencyLevel::Serial => (Consistency::Quorum, Some(SerialConsistency::Serial)),
        ConsistencyLevel::LocalSerial => {
            if side == Side::Origin {
                tracing::info!(
                    side = side.as_str(),
                    "LOCAL_SERIAL is a serial consistency level; it applies to the lightweight \
                     transaction path and reads use LOCAL_QUORUM"
                );
            }
            (
                Consistency::LocalQuorum,
                Some(SerialConsistency::LocalSerial),
            )
        }
    })
}

/// Builds the TLS specification from `connect.{side}.tls` (`CON-002` modes 2 and 3, `CON-006`).
pub fn tls_spec_from_stores(config: &CdmConfig, side: Side) -> Result<TlsSpec, CdmError> {
    let settings = mode::side_config(config, side);
    let tls = &settings.tls;

    let trust = match &tls.truststore.path {
        Some(path) => tls::read_trust_store(
            side,
            path,
            tls.truststore
                .password
                .as_ref()
                .map(|p| p.expose().as_str()),
            StoreFormat::from(tls.truststore.store_type),
        )?,
        None => TrustMaterial::default(),
    };

    let identity: Option<Identity> = match &tls.keystore.path {
        Some(path) => Some(tls::read_key_store(
            side,
            path,
            tls.keystore.password.as_ref().map(|p| p.expose().as_str()),
            StoreFormat::guess_from_path(path),
        )?),
        None => None,
    };

    if tls.is_astra && identity.is_none() {
        return Err(config_error(
            side,
            format!(
                "connect.{}.tls.is_astra is set but no key store is configured; Astra requires \
                 mutual TLS (CON-002)",
                side.as_str()
            ),
            "connect.{side}.tls.keystore.path",
        ));
    }

    let mut spec = TlsSpec::new(side, trust).with_cipher_suites(tls.cipher_suites.clone());
    if let Some(identity) = identity {
        spec = spec.with_identity(identity);
    }
    // Astra terminates on a name, and its certificate is issued for that name, so it is checked.
    // A self-managed cluster is reached by address and Cassandra's own default is not to verify
    // the endpoint name at all, so it is not (see `tls::verifier`).
    if tls.is_astra {
        spec = spec.with_expected_hostname(settings.host.clone());
    }
    Ok(spec)
}

/// Resolves an Astra bundle into a connection (`CON-022`, `CON-026`).
async fn resolve_astra(
    side: Side,
    bundle: &SecureConnectBundle,
    config: &CdmConfig,
    cipher_suites: Vec<String>,
) -> Result<AstraConnection, CdmError> {
    let settings = mode::side_config(config, side);
    let resolved = strategy::resolve(
        side,
        bundle,
        settings.astra.mode,
        settings.astra.metadata_refresh_interval.get(),
        cipher_suites,
    )
    .await?;
    if resolved.contact_point.0.is_empty() {
        return Err(connect_error(
            side,
            "the Astra bundle yielded no endpoint to connect to",
        ));
    }
    Ok(resolved)
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
    use std::path::PathBuf;

    use cdm_config::Secret;

    use super::*;
    use crate::testfixtures::Pki;

    fn config() -> CdmConfig {
        CdmConfig::default()
    }

    #[tokio::test]
    async fn con_001_each_side_takes_its_own_consistency_level() {
        let mut config = config();
        config.perfops.consistency.read = ConsistencyLevel::LocalOne;
        config.perfops.consistency.write = ConsistencyLevel::EachQuorum;

        let origin = build_profile(&config, Side::Origin, Some("dc1"), false).unwrap();
        let target = build_profile(&config, Side::Target, Some("dc1"), false).unwrap();
        assert_eq!(origin.get_consistency(), Consistency::LocalOne);
        assert_eq!(target.get_consistency(), Consistency::EachQuorum);
    }

    #[test]
    fn con_001_the_serial_levels_are_routed_to_serial_consistency() {
        assert_eq!(
            driver_consistency(Side::Target, ConsistencyLevel::Serial).unwrap(),
            (Consistency::Quorum, Some(SerialConsistency::Serial))
        );
        assert_eq!(
            driver_consistency(Side::Origin, ConsistencyLevel::LocalSerial).unwrap(),
            (
                Consistency::LocalQuorum,
                Some(SerialConsistency::LocalSerial)
            )
        );
        assert_eq!(
            driver_consistency(Side::Origin, ConsistencyLevel::All).unwrap(),
            (Consistency::All, None)
        );
    }

    #[tokio::test]
    async fn con_010_speculative_execution_is_off_by_default_for_both_sides() {
        let config = config();
        for side in [Side::Origin, Side::Target] {
            let profile = build_profile(&config, side, Some("dc1"), false).unwrap();
            assert!(
                profile.get_speculative_execution_policy().is_none(),
                "{side} must not speculate unless asked"
            );
        }
    }

    #[tokio::test]
    async fn con_010_speculative_execution_is_configurable_per_side() {
        let mut config = config();
        config.connect.origin.speculative.enabled = true;
        let origin = build_profile(&config, Side::Origin, Some("dc1"), false).unwrap();
        let target = build_profile(&config, Side::Target, Some("dc1"), false).unwrap();
        assert!(origin.get_speculative_execution_policy().is_some());
        assert!(target.get_speculative_execution_policy().is_none());
    }

    #[tokio::test]
    async fn con_012_a_counter_target_never_speculates_however_it_is_configured() {
        let mut config = config();
        config.connect.target.speculative.enabled = true;
        let profile = build_profile(&config, Side::Target, Some("dc1"), true).unwrap();
        assert!(profile.get_speculative_execution_policy().is_none());
    }

    #[tokio::test]
    async fn con_011_the_profile_carries_the_configured_request_timeout() {
        let config = config();
        let profile = build_profile(&config, Side::Origin, None, false).unwrap();
        assert_eq!(
            profile.get_request_timeout(),
            Some(config.perfops.request_timeout.get())
        );
    }

    #[test]
    fn con_006_a_tls_side_reads_both_stores() {
        let pki = Pki::new();
        let dir = tempfile::tempdir().unwrap();
        let truststore = dir.path().join("ts.p12");
        let keystore = dir.path().join("ks.p12");
        std::fs::write(&truststore, pki.truststore_pkcs12(pki.password())).unwrap();
        std::fs::write(&keystore, pki.keystore_pkcs12(pki.password())).unwrap();

        let mut config = config();
        let tls = &mut config.connect.origin.tls;
        tls.enabled = true;
        tls.truststore.path = Some(truststore);
        tls.truststore.password = Some(Secret::new(pki.password()));
        tls.truststore.store_type = cdm_config::types::TrustStoreType::Pkcs12;
        tls.keystore.path = Some(keystore);
        tls.keystore.password = Some(Secret::new(pki.password()));

        let spec = tls_spec_from_stores(&config, Side::Origin).unwrap();
        assert_eq!(spec.trust.len(), 1);
        assert!(spec.identity.is_some());
        assert!(
            spec.expected_hostname.is_none(),
            "a self-managed cluster is reached by address"
        );
        assert!(spec.client_config().is_ok());
    }

    #[test]
    fn con_002_astra_from_stores_requires_a_key_store() {
        let pki = Pki::new();
        let dir = tempfile::tempdir().unwrap();
        let truststore = dir.path().join("ts.p12");
        std::fs::write(&truststore, pki.truststore_pkcs12(pki.password())).unwrap();

        let mut config = config();
        config.connect.origin.tls.is_astra = true;
        config.connect.origin.tls.truststore.path = Some(truststore);
        config.connect.origin.tls.truststore.password = Some(Secret::new(pki.password()));
        config.connect.origin.tls.truststore.store_type = cdm_config::types::TrustStoreType::Pkcs12;

        let err = tls_spec_from_stores(&config, Side::Origin).unwrap_err();
        assert!(err.to_string().contains("mutual TLS"), "{err}");
    }

    #[test]
    fn con_006_astra_from_stores_verifies_the_endpoint_name() {
        let pki = Pki::new();
        let dir = tempfile::tempdir().unwrap();
        let truststore = dir.path().join("ts.p12");
        let keystore = dir.path().join("ks.p12");
        std::fs::write(&truststore, pki.truststore_pkcs12(pki.password())).unwrap();
        std::fs::write(&keystore, pki.keystore_pkcs12(pki.password())).unwrap();

        let mut config = config();
        config.connect.origin.host = "db.astra.datastax.com".to_owned();
        let tls = &mut config.connect.origin.tls;
        tls.is_astra = true;
        tls.truststore.path = Some(truststore);
        tls.truststore.password = Some(Secret::new(pki.password()));
        tls.truststore.store_type = cdm_config::types::TrustStoreType::Pkcs12;
        tls.keystore.path = Some(keystore);
        tls.keystore.password = Some(Secret::new(pki.password()));

        let spec = tls_spec_from_stores(&config, Side::Origin).unwrap();
        assert_eq!(
            spec.expected_hostname.as_deref(),
            Some("db.astra.datastax.com")
        );
    }

    #[test]
    fn con_006_a_missing_trust_store_file_fails_before_any_connection() {
        let mut config = config();
        config.connect.target.tls.enabled = true;
        config.connect.target.tls.truststore.path = Some(PathBuf::from("/nowhere/ts.jks"));
        let err = tls_spec_from_stores(&config, Side::Target).unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Tls);
    }

    #[tokio::test]
    async fn con_002_a_missing_bundle_fails_with_a_config_error() {
        let mut config = config();
        config.connect.origin.scb = Some(PathBuf::from("/nowhere/secure-connect.zip"));
        let err = connect(&config, Side::Origin).await.unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Config);
        assert!(err.to_string().contains("secure-connect.zip"), "{err}");
    }

    #[tokio::test]
    async fn con_009_a_configured_local_datacenter_is_used_verbatim() {
        // No cluster is reachable here, so this asserts the profile rather than the session.
        let mut config = config();
        config.connect.origin.local_datacenter = Some("dc-west".to_owned());
        let profile = build_profile(
            &config,
            Side::Origin,
            config.connect.origin.local_datacenter.as_deref(),
            false,
        )
        .unwrap();
        assert!(format!("{:?}", profile.get_load_balancing_policy()).contains("dc-west"));
    }
}
