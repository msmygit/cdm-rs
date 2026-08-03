//! Choosing how CQL traffic reaches Astra, and the driver limitation that decides it
//! (`CON-022`, `CON-023`, `CON-024`, `CON-026`, `CON-027`, `CON-028`).
//!
//! # The mechanism Astra uses
//!
//! Astra publishes no node addresses. Every CQL connection goes to one SNI proxy endpoint, and
//! the node it reaches is selected by the TLS **`server_name`** of that connection: the target
//! node's host id. One endpoint therefore addresses every node in a private cluster, which is
//! what makes token-aware routing possible at all.
//!
//! # Why the primary strategy is not reachable today
//!
//! `CON-022` requires cdm-rs to set that `server_name` per connection. `scylla-rust-driver` 1.7
//! cannot:
//!
//! * [`SessionBuilder::tls_context`](scylla::client::session_builder::SessionBuilder::tls_context)
//!   takes **one** `TlsContext` for the whole session;
//! * the driver's `TlsProvider`/`TlsConfig` pair, which is where a per-endpoint name would be
//!   chosen, is `pub(crate)` and has exactly one variant, `GlobalContext`;
//! * with the `rustls-023` backend, `network/connection.rs` builds the name itself —
//!   `ServerName::IpAddress(node_address.ip())` — so no `ClientConfig` cdm-rs supplies can change
//!   it. rustls never sends an SNI extension for an IP name, so the proxy receives no name to
//!   route on.
//!
//! Removing the `cloud` feature does not help: 1.7 has no such feature. The Scylla Cloud
//! serverless support that once carried a per-connection SNI hostname was removed, and its
//! remnant is the comment in `network/tls.rs` explaining that `TlsProvider` used to have a
//! `CloudConfig` variant.
//!
//! This is a missing hook, not an inconvenience, so `CON-023` applies: it is raised upstream and
//! worked around locally. The workaround is `CON-026`'s documented fallback, chosen automatically
//! and announced loudly (`CON-027`).
//!
//! What *is* implemented, tested, and waiting for the hook: the bundle reader, the mutual-TLS
//! metadata call, the metadata contract and its refresh rate limit, the local-DC and host-id
//! extraction, and [`ProxyAddressTranslator`] — `CON-022` step 6 — which does compile against the
//! driver's [`AddressTranslator`] trait once the elided-lifetime form is written correctly (see
//! its documentation).
//!
//! # Authentication
//!
//! `CON-028`: Astra accepts the literal token as the password with username `token`, and also a
//! Client ID / Client Secret pair. [`AstraCredentials`] normalises both and detects the common
//! mistake of pasting the `AstraCS:` token into the *username*.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use cdm_config::types::AstraMode;
use cdm_core::{CdmError, Side};
use scylla::errors::TranslationError;
use scylla::policies::address_translator::{AddressTranslator, UntranslatedPeer};
use uuid::Uuid;

use crate::astra::bundle::SecureConnectBundle;
use crate::astra::metadata::{MetadataResponse, MetadataService};
use crate::errors::{config_error, connect_error};
use crate::tls::TlsSpec;

/// The prefix of an Astra token, which is a password and never a username (`CON-028`).
pub const ASTRA_TOKEN_PREFIX: &str = "AstraCS:";
/// The username Astra expects alongside a token (`CON-028`).
pub const ASTRA_TOKEN_USERNAME: &str = "token";

/// Which of the two strategies of §4.1 is in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AstraStrategy {
    /// Per-connection SNI through the proxy (`CON-022`). Not reachable with the current driver.
    Sni,
    /// One mutual-TLS endpoint, no token awareness (`CON-026`).
    SingleEndpoint,
}

impl AstraStrategy {
    /// The name `cdm connect test` prints (`CON-029`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sni => "sni",
            Self::SingleEndpoint => "single_endpoint",
        }
    }
}

/// Whether the linked driver can set a TLS `server_name` per connection (`CON-022`, `CON-023`).
///
/// Constant today, and deliberately a function: when the upstream hook lands, this becomes a
/// capability check and the SNI path turns on with no other change. See the module documentation
/// for the analysis and `ADR-0009` for the decision.
pub const fn driver_supports_per_connection_sni() -> bool {
    false
}

/// Why the SNI strategy is unavailable, in a form fit to print to an operator.
pub const SNI_UNAVAILABLE_REASON: &str =
    "scylla-rust-driver 1.7 accepts only one TLS context per session and derives the TLS \
     server_name from the node's IP address, so the per-connection SNI name Astra's proxy routes \
     on cannot be set (CON-022, CON-023)";

/// Astra credentials, in either spelling Astra accepts (`CON-028`).
#[derive(Debug, Clone)]
pub struct AstraCredentials {
    /// The username to send. `token` for the token form, the Client ID otherwise.
    username: String,
    /// The password to send: the `AstraCS:` token, or the Client Secret.
    password: String,
    /// Whether this is the token form.
    token_form: bool,
}

impl AstraCredentials {
    /// Normalises a configured username and password (`CON-028`).
    ///
    /// The token form is recognised by the `AstraCS:` prefix on the password, whatever the
    /// username says, because that is what Astra actually accepts. A token supplied as the
    /// *username* is the common paste error and is rejected with an explanation.
    pub fn resolve(side: Side, username: &str, password: &str) -> Result<Self, CdmError> {
        if username.starts_with(ASTRA_TOKEN_PREFIX) {
            return Err(config_error(
                side,
                format!(
                    "connect.{}.username is an `AstraCS:` token. The token is the *password*; \
                     the username must be the literal `{ASTRA_TOKEN_USERNAME}` (CON-028)",
                    side.as_str()
                ),
                "connect.{side}.username",
            ));
        }
        if password.is_empty() {
            return Err(config_error(
                side,
                format!(
                    "connect.{}.password is empty; Astra requires either an `AstraCS:` token or \
                     a Client Secret (CON-028)",
                    side.as_str()
                ),
                "connect.{side}.password",
            ));
        }
        let token_form = password.starts_with(ASTRA_TOKEN_PREFIX);
        Ok(Self {
            username: if token_form {
                ASTRA_TOKEN_USERNAME.to_owned()
            } else {
                username.to_owned()
            },
            password: password.to_owned(),
            token_form,
        })
    }

    /// The username to authenticate with.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The password to authenticate with. A `SEC-001` review point: never log the result.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Whether the token spelling is in use, as opposed to Client ID / Client Secret.
    pub fn is_token_form(&self) -> bool {
        self.token_form
    }
}

/// Everything resolved about an Astra connection, ready to build a session with (`CON-029`).
#[derive(Debug)]
pub struct AstraConnection {
    /// The strategy actually in force.
    pub strategy: AstraStrategy,
    /// Where the bundle came from: a path, or a DevOps download URL.
    pub bundle_origin: String,
    /// The metadata service URL, whether or not it answered.
    pub metadata_url: String,
    /// The proxy address the metadata service reported, when it answered (`CON-022`).
    pub sni_proxy_address: Option<String>,
    /// The datacenter to treat as local (`CON-009`).
    pub local_dc: Option<String>,
    /// The host ids the metadata service reported (`CON-022`, `CON-024`).
    pub host_ids: Vec<Uuid>,
    /// The endpoint CQL connections are opened to.
    pub contact_point: (String, u16),
    /// The TLS configuration for those connections.
    pub tls: Arc<rustls::ClientConfig>,
    /// Why the SNI strategy was not used, when it was not.
    pub sni_unavailable_reason: Option<String>,
}

impl AstraConnection {
    /// The contact point as `host:port`.
    pub fn contact_point_string(&self) -> String {
        format!("{}:{}", self.contact_point.0, self.contact_point.1)
    }
}

/// Resolves the strategy and everything it needs from a bundle (`CON-022`, `CON-026`).
///
/// The metadata service is called whenever `mode` is `sni`, even though the result cannot drive
/// per-connection SNI yet: it yields the local datacenter, the host-id count and the proxy
/// address, all of which `CON-029` requires `cdm connect test` to report, and its failure is a
/// far better diagnostic than a timeout on the CQL port. When `mode` is `single_endpoint` the
/// call is skipped, because the operator has already said what they want.
pub async fn resolve(
    side: Side,
    bundle: &SecureConnectBundle,
    mode: AstraMode,
    metadata_refresh_interval: std::time::Duration,
    cipher_suites: Vec<String>,
) -> Result<AstraConnection, CdmError> {
    let (metadata_host, metadata_port) = bundle.metadata_endpoint()?;
    let metadata_url = bundle.metadata_url()?;

    // One TLS configuration serves both the metadata call and the CQL connections: same trust
    // anchor, same client identity. Only the expected server name differs, and only for the
    // metadata call, which is addressed by name.
    let trust = bundle.trust_material()?;
    let identity = bundle.identity()?;
    let metadata_tls = TlsSpec::new(side, trust.clone())
        .with_identity(identity.clone_identity())
        .with_cipher_suites(cipher_suites.clone())
        .with_expected_hostname(metadata_host.clone())
        .client_config()?;

    let mut metadata = None;
    if mode == AstraMode::Sni {
        let mut service = MetadataService::new(
            side,
            metadata_host.clone(),
            metadata_port,
            metadata_tls,
            metadata_refresh_interval,
        );
        match service.fetch().await {
            Ok(response) => metadata = Some(response),
            Err(error) => tracing::warn!(
                side = side.as_str(),
                rule = "CON-026",
                "the Astra metadata service at {metadata_url} could not be queried ({error}); \
                 falling back to the single-endpoint strategy"
            ),
        }
    }

    let strategy = if mode == AstraMode::Sni && driver_supports_per_connection_sni() {
        AstraStrategy::Sni
    } else {
        AstraStrategy::SingleEndpoint
    };

    let sni_unavailable_reason = match (mode, strategy) {
        (AstraMode::Sni, AstraStrategy::SingleEndpoint) => Some(SNI_UNAVAILABLE_REASON.to_owned()),
        _ => None,
    };

    let contact_point = single_endpoint(side, bundle)?;
    if strategy == AstraStrategy::SingleEndpoint {
        warn_single_endpoint(side, &contact_point, sni_unavailable_reason.as_deref());
    }

    // The CQL endpoint is reached by name, so the certificate must be issued for that name; the
    // driver would otherwise offer rustls its synthetic IP name (see `tls::verifier`).
    let cql_tls = TlsSpec::new(side, trust)
        .with_identity(identity)
        .with_cipher_suites(cipher_suites)
        .with_expected_hostname(contact_point.0.clone())
        .client_config()?;

    Ok(AstraConnection {
        strategy,
        bundle_origin: bundle.origin().to_owned(),
        metadata_url,
        sni_proxy_address: metadata
            .as_ref()
            .map(|m| m.contact_info.sni_proxy_address.clone()),
        local_dc: metadata
            .as_ref()
            .map(|m| m.local_dc().to_owned())
            .or_else(|| bundle.config().local_dc.clone()),
        host_ids: metadata
            .as_ref()
            .map(MetadataResponse::host_ids)
            .unwrap_or_default(),
        contact_point,
        tls: cql_tls,
        sni_unavailable_reason,
    })
}

/// The single CQL endpoint: the host from `config.json`, the port from `cqlshrc` (`CON-026`).
///
/// The port is the part that is easy to get wrong and is therefore normative: the ports in
/// `config.json` serve the metadata service, not CQL.
pub fn single_endpoint(
    side: Side,
    bundle: &SecureConnectBundle,
) -> Result<(String, u16), CdmError> {
    let host = bundle.cql_host().ok_or_else(|| {
        config_error(
            side,
            format!(
                "{}: neither cqlshrc nor config.json names a host to connect to",
                bundle.origin()
            ),
            "connect.{side}.scb",
        )
    })?;
    let port = bundle.cql_port().ok_or_else(|| {
        connect_error(
            side,
            format!(
                "{}: the bundle has no cqlshrc, which is the only correct source for the CQL \
                 port (CON-026). config.json's ports serve the metadata service and will not \
                 answer CQL.",
                bundle.origin()
            ),
        )
    })?;
    Ok((host, port))
}

/// The warning `CON-027` requires whenever the fallback is in force.
fn warn_single_endpoint(side: Side, endpoint: &(String, u16), reason: Option<&str>) {
    tracing::warn!(
        side = side.as_str(),
        rule = "CON-026",
        endpoint = format!("{}:{}", endpoint.0, endpoint.1),
        "connecting to Astra through a single endpoint (CON-026). Token-aware routing and \
         per-node load balancing are lost and throughput will be materially lower than a \
         SNI-routed connection. {} See docs/MIGRATION_FROM_JAVA.md#astra-connectivity.",
        reason.unwrap_or("This is what connect.{side}.astra.mode = single_endpoint asks for.")
    );
}

/// Maps every node address the cluster advertises onto the SNI proxy (`CON-022` step 6,
/// `CON-024`).
///
/// Astra's `system.local`/`system.peers` carry private addresses that no client can route to, so
/// a translator is required for topology discovery to produce anything usable. Newly discovered
/// peers translate to the same proxy without a restart, which is `CON-024`.
///
/// # Implementing the driver's trait
///
/// [`AddressTranslator`] is declared with `#[async_trait]` over `&UntranslatedPeer`, whose
/// lifetime parameter is elided. The elision must be reproduced **exactly**: writing
/// `&UntranslatedPeer<'_>` in the implementation makes the lifetime early-bound and fails with
/// `E0195`. The bare form below is the one that compiles, at the cost of an
/// `elided_lifetimes_in_paths` allow. This is worth recording, because the obvious fix for that
/// lint is the form that does not build.
#[derive(Debug, Clone)]
pub struct ProxyAddressTranslator {
    proxy: SocketAddr,
}

impl ProxyAddressTranslator {
    /// A translator that sends every address to `proxy`.
    pub fn new(proxy: SocketAddr) -> Self {
        Self { proxy }
    }

    /// The proxy every address resolves to.
    pub fn proxy(&self) -> SocketAddr {
        self.proxy
    }

    /// The equivalent explicit map, for the driver's blanket implementation over
    /// `HashMap<SocketAddr, SocketAddr>`.
    ///
    /// A map is static: an address absent from it fails translation, so a node added after the
    /// map was built would be unreachable, which `CON-024` forbids. It exists for tests and for
    /// callers that genuinely want a fixed set.
    pub fn as_map(&self, known: &[SocketAddr]) -> HashMap<SocketAddr, SocketAddr> {
        known.iter().map(|address| (*address, self.proxy)).collect()
    }
}

#[async_trait::async_trait]
impl AddressTranslator for ProxyAddressTranslator {
    // The elided form is load-bearing; see the type documentation.
    #[allow(elided_lifetimes_in_paths)]
    async fn translate_address(
        &self,
        _untranslated_peer: &UntranslatedPeer,
    ) -> Result<SocketAddr, TranslationError> {
        Ok(self.proxy)
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
    use crate::testfixtures::generated_password;

    /// A token with Astra's prefix, generated so no credential-shaped literal sits in the tree.
    fn astra_token() -> String {
        format!("AstraCS:{}", generated_password())
    }

    /// A client id, which Astra formats as a UUID but which these tests only need to be non-empty
    /// and distinguishable from a token.
    fn client_id() -> String {
        format!("id-{}", generated_password())
    }

    /// A client secret.
    fn client_secret() -> String {
        format!("secret-{}", generated_password())
    }
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;

    use super::*;
    use crate::testfixtures::astra::BundleBuilder;
    use crate::testfixtures::Pki;

    fn bundle(pki: &Pki) -> SecureConnectBundle {
        SecureConnectBundle::from_bytes(Side::Origin, &BundleBuilder::new(pki).build(), "scb.zip")
            .unwrap()
    }

    #[test]
    fn con_026_the_cql_port_comes_from_cqlshrc_and_the_host_from_the_bundle() {
        let pki = Pki::new();
        let endpoint = single_endpoint(Side::Origin, &bundle(&pki)).unwrap();
        assert_eq!(endpoint, ("cql.example.invalid".to_owned(), 29042));
    }

    #[test]
    fn con_026_a_bundle_without_cqlshrc_cannot_yield_a_cql_port() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki).without("cqlshrc").build();
        let bundle = SecureConnectBundle::from_bytes(Side::Origin, &zip, "scb.zip").unwrap();
        let err = single_endpoint(Side::Origin, &bundle).unwrap_err();
        assert!(err.to_string().contains("cqlshrc"), "{err}");
        assert!(err.to_string().contains("CON-026"), "{err}");
    }

    #[tokio::test]
    async fn con_022_the_sni_strategy_is_not_selected_while_the_driver_cannot_set_a_server_name() {
        let pki = Pki::new();
        // The metadata host does not resolve, so this also exercises the "metadata unreachable"
        // path of CON-026.
        let resolved = resolve(
            Side::Origin,
            &bundle(&pki),
            AstraMode::Sni,
            Duration::from_secs(300),
            Vec::new(),
        )
        .await
        .unwrap();

        assert_eq!(resolved.strategy, AstraStrategy::SingleEndpoint);
        assert_eq!(
            resolved.sni_unavailable_reason.as_deref(),
            Some(SNI_UNAVAILABLE_REASON)
        );
        assert!(!driver_supports_per_connection_sni());
    }

    #[tokio::test]
    async fn con_029_resolution_reports_everything_connect_test_must_print() {
        let pki = Pki::new();
        let resolved = resolve(
            Side::Target,
            &bundle(&pki),
            AstraMode::SingleEndpoint,
            Duration::from_secs(300),
            Vec::new(),
        )
        .await
        .unwrap();

        assert_eq!(resolved.bundle_origin, "scb.zip");
        assert_eq!(
            resolved.metadata_url,
            "https://metadata.example.invalid:29080/metadata"
        );
        assert_eq!(resolved.strategy.as_str(), "single_endpoint");
        assert_eq!(resolved.contact_point_string(), "cql.example.invalid:29042");
        // config.json's localDC is used when the metadata service was not called.
        assert_eq!(resolved.local_dc.as_deref(), Some("us-east1"));
        assert!(resolved.host_ids.is_empty());
        assert!(resolved.sni_proxy_address.is_none());
        // Asking for single_endpoint explicitly is not a driver limitation.
        assert!(resolved.sni_unavailable_reason.is_none());
    }

    #[tokio::test]
    async fn con_007_an_unsupported_cipher_suite_fails_astra_resolution_too() {
        let pki = Pki::new();
        let err = resolve(
            Side::Origin,
            &bundle(&pki),
            AstraMode::SingleEndpoint,
            Duration::from_secs(300),
            vec!["TLS_KRB5_WITH_DES_CBC_MD5".to_owned()],
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Tls);
    }

    #[test]
    fn con_028_a_token_password_forces_the_token_username() {
        let token = astra_token();
        let credentials = AstraCredentials::resolve(Side::Origin, "cassandra", &token).unwrap();

        assert_eq!(credentials.username(), "token");
        assert_eq!(
            credentials.password(),
            token,
            "the token is passed through verbatim"
        );
        assert!(credentials.is_token_form());
    }

    #[test]
    fn con_028_a_client_id_and_secret_pair_is_accepted_unchanged() {
        let id = client_id();
        let credentials = AstraCredentials::resolve(Side::Origin, &id, &client_secret()).unwrap();

        assert_eq!(credentials.username(), id, "a client id is not rewritten");
        assert!(!credentials.is_token_form());
    }

    #[test]
    fn con_028_a_token_supplied_as_the_username_is_detected() {
        let err =
            AstraCredentials::resolve(Side::Origin, &astra_token(), &client_secret()).unwrap_err();
        assert!(err.to_string().contains("is the *password*"), "{err}");
        assert_eq!(err.kind(), cdm_core::ErrorKind::Config);
    }

    #[test]
    fn con_028_an_empty_password_is_rejected() {
        let err = AstraCredentials::resolve(Side::Origin, "token", "").unwrap_err(); // empty, deliberately
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[tokio::test]
    async fn con_022_the_address_translator_sends_every_peer_to_the_proxy() {
        let proxy = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(34, 1, 2, 3)), 29042);
        let translator = ProxyAddressTranslator::new(proxy);
        assert_eq!(translator.proxy(), proxy);

        // The trait is exercised through the driver's own object type, which is how the session
        // builder consumes it — this is the compile-time proof that CON-022 step 6 is reachable.
        let object: Arc<dyn AddressTranslator> = Arc::new(translator.clone());
        assert!(format!("{translator:?}").contains("34.1.2.3"));
        drop(object);

        let private = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 4)), 9042);
        let map = translator.as_map(&[private]);
        assert_eq!(map.get(&private), Some(&proxy));
    }

    #[test]
    fn con_024_the_translator_needs_no_prior_knowledge_of_a_peer() {
        // A map would have to be rebuilt when a node appears; the translator does not.
        let proxy = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(34, 1, 2, 3)), 29042);
        let translator = ProxyAddressTranslator::new(proxy);
        assert!(translator.as_map(&[]).is_empty());
        assert_eq!(translator.proxy(), proxy);
    }
}
