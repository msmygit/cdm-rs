//! The Astra metadata service (`CON-022`, `CON-024`, `CON-025`).
//!
//! Astra never exposes node addresses. The bundle carries mutual-TLS material and the address of
//! a metadata service; a `GET https://{host}:{port}/metadata` over that mTLS connection returns
//! the SNI proxy address, the local datacenter and the cluster's host ids:
//!
//! ```json
//! {
//!   "version": 1,
//!   "region": "us-east1",
//!   "contact_info": {
//!     "type": "sni_proxy",
//!     "local_dc": "us-east1",
//!     "contact_points": ["<host-id-uuid>", "…"],
//!     "sni_proxy_address": "<proxy-host>:<proxy-port>"
//!   }
//! }
//! ```
//!
//! The contract is not formally published, so parsing is lenient in the same way `config.json`'s
//! is (`CON-021`): unknown fields are ignored, and a missing one produces a message naming it.
//!
//! # Refresh
//!
//! `sni_proxy_address` can change, so `CON-025` requires a re-fetch when every connection fails —
//! rate-limited to at most one per `connect.{side}.astra.metadata_refresh_interval` (default five
//! minutes). [`MetadataService`] owns that clock; it does not own the decision to refresh, which
//! belongs to whoever observes the failures.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cdm_core::{CdmError, Side};
use rustls::ClientConfig;
use serde::Deserialize;
use uuid::Uuid;

use crate::errors::connect_error;
use crate::http::{self, HttpRequest};

/// The metadata service response (`CON-022`).
#[derive(Debug, Clone, Deserialize)]
pub struct MetadataResponse {
    /// The contract version. Ignored beyond being reported in diagnostics.
    #[serde(default)]
    pub version: Option<u32>,
    /// The Astra region the database lives in.
    #[serde(default)]
    pub region: Option<String>,
    /// How to reach the cluster.
    pub contact_info: ContactInfo,
}

/// The `contact_info` object of a metadata response.
#[derive(Debug, Clone, Deserialize)]
pub struct ContactInfo {
    /// Always `sni_proxy` for Astra today. Retained so an unexpected value can be reported.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// The datacenter the load-balancing policy should treat as local (`CON-009`).
    pub local_dc: String,
    /// The host ids of the cluster's nodes; each is also its SNI name (`CON-022`).
    #[serde(default)]
    pub contact_points: Vec<String>,
    /// The single endpoint every CQL connection goes to, `host:port`.
    pub sni_proxy_address: String,
}

impl MetadataResponse {
    /// Parses a metadata response, ignoring unknown fields (`CON-021`, `CON-022`).
    pub fn parse(side: Side, bytes: &[u8]) -> Result<Self, CdmError> {
        serde_json::from_slice(bytes).map_err(|e| {
            connect_error(
                side,
                format!(
                    "the Astra metadata service returned a response cdm-rs cannot read ({e}); \
                     body: {}",
                    String::from_utf8_lossy(bytes)
                        .chars()
                        .take(400)
                        .collect::<String>()
                ),
            )
        })
    }

    /// The proxy host and port, split (`CON-022`).
    pub fn proxy_endpoint(&self, side: Side) -> Result<(String, u16), CdmError> {
        let address = self.contact_info.sni_proxy_address.trim();
        let (host, port) = address.rsplit_once(':').ok_or_else(|| {
            connect_error(
                side,
                format!(
                    "the metadata service returned sni_proxy_address {address}, which has no port"
                ),
            )
        })?;
        let port = port.parse::<u16>().map_err(|_| {
            connect_error(
                side,
                format!("the metadata service returned sni_proxy_address {address}, whose port is not a number"),
            )
        })?;
        if host.is_empty() {
            return Err(connect_error(
                side,
                format!(
                    "the metadata service returned sni_proxy_address {address}, which has no host"
                ),
            ));
        }
        Ok((host.to_owned(), port))
    }

    /// The contact points that parse as host-id UUIDs (`CON-022`).
    ///
    /// A contact point that is not a UUID is not a host id and cannot be an SNI name, so it is
    /// dropped rather than passed to the driver, which would fail later and less clearly.
    pub fn host_ids(&self) -> Vec<Uuid> {
        self.contact_info
            .contact_points
            .iter()
            .filter_map(|point| Uuid::parse_str(point.trim()).ok())
            .collect()
    }

    /// The datacenter to treat as local (`CON-009`).
    pub fn local_dc(&self) -> &str {
        &self.contact_info.local_dc
    }
}

/// A metadata service endpoint, with the rate limit of `CON-025`.
#[derive(Debug)]
pub struct MetadataService {
    side: Side,
    host: String,
    port: u16,
    tls: Arc<ClientConfig>,
    refresh_interval: Duration,
    last_fetch: Option<Instant>,
}

impl MetadataService {
    /// Builds a client for `https://{host}:{port}/metadata` over the bundle's mutual TLS.
    pub fn new(
        side: Side,
        host: impl Into<String>,
        port: u16,
        tls: Arc<ClientConfig>,
        refresh_interval: Duration,
    ) -> Self {
        Self {
            side,
            host: host.into(),
            port,
            tls,
            refresh_interval,
            last_fetch: None,
        }
    }

    /// The URL this client calls, for `cdm connect test` (`CON-029`).
    pub fn url(&self) -> String {
        format!("https://{}:{}/metadata", self.host, self.port)
    }

    /// Fetches the metadata document (`CON-022`).
    pub async fn fetch(&mut self) -> Result<MetadataResponse, CdmError> {
        let response = http::send(
            self.side,
            self.tls.clone(),
            HttpRequest {
                method: "GET",
                host: &self.host,
                port: self.port,
                path: "/metadata",
                headers: &[],
                body: None,
            },
        )
        .await?;
        self.last_fetch = Some(Instant::now());

        if !response.is_success() {
            return Err(connect_error(
                self.side,
                format!(
                    "the Astra metadata service at {} answered {}: {}",
                    self.url(),
                    response.status,
                    response.body_text().chars().take(400).collect::<String>()
                ),
            ));
        }
        MetadataResponse::parse(self.side, &response.body)
    }

    /// Whether a re-fetch is allowed yet (`CON-025`).
    pub fn may_refresh(&self) -> bool {
        match self.last_fetch {
            None => true,
            Some(at) => at.elapsed() >= self.refresh_interval,
        }
    }

    /// Re-fetches when the rate limit allows, and reports that it declined when it does not
    /// (`CON-025`).
    pub async fn refresh(&mut self) -> Result<Option<MetadataResponse>, CdmError> {
        if !self.may_refresh() {
            tracing::debug!(
                side = self.side.as_str(),
                rule = "CON-025",
                "declining to re-fetch Astra metadata: the last fetch was under {:?} ago",
                self.refresh_interval
            );
            return Ok(None);
        }
        self.fetch().await.map(Some)
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
    use crate::testfixtures::astra::metadata_json;

    const HOST_ID: &str = "0e2e5b39-2a3b-4e7f-9f1a-6a4f2a6b7c8d";

    #[test]
    fn con_022_a_metadata_response_is_parsed() {
        let json = metadata_json(&[HOST_ID], "proxy.example.invalid:29042", "us-east1");
        let response = MetadataResponse::parse(Side::Origin, json.as_bytes()).unwrap();

        assert_eq!(response.local_dc(), "us-east1");
        assert_eq!(response.region.as_deref(), Some("us-east1"));
        assert_eq!(response.contact_info.kind.as_deref(), Some("sni_proxy"));
        assert_eq!(
            response.proxy_endpoint(Side::Origin).unwrap(),
            ("proxy.example.invalid".to_owned(), 29042)
        );
        assert_eq!(response.host_ids(), vec![Uuid::parse_str(HOST_ID).unwrap()]);
    }

    #[test]
    fn con_022_unknown_metadata_fields_are_ignored() {
        let json = r#"{"version":2,"region":"eu-west1","something_new":[1,2],
                       "contact_info":{"type":"sni_proxy","local_dc":"eu-west1",
                       "contact_points":[],"sni_proxy_address":"p:1","extra":true}}"#;
        let response = MetadataResponse::parse(Side::Origin, json.as_bytes()).unwrap();
        assert_eq!(response.version, Some(2));
        assert!(response.host_ids().is_empty());
    }

    #[test]
    fn con_022_a_contact_point_that_is_not_a_host_id_is_dropped() {
        let json = metadata_json(&[HOST_ID, "not-a-uuid"], "p.example.invalid:29042", "dc1");
        let response = MetadataResponse::parse(Side::Origin, json.as_bytes()).unwrap();
        assert_eq!(response.host_ids().len(), 1);
    }

    #[test]
    fn con_022_a_malformed_proxy_address_is_reported_precisely() {
        for (address, expected) in [
            ("proxy.example.invalid", "no port"),
            ("proxy.example.invalid:http", "not a number"),
            (":29042", "no host"),
        ] {
            let json = metadata_json(&[HOST_ID], address, "dc1");
            let response = MetadataResponse::parse(Side::Origin, json.as_bytes()).unwrap();
            let err = response.proxy_endpoint(Side::Origin).unwrap_err();
            assert!(err.to_string().contains(expected), "{address}: {err}");
        }
    }

    #[test]
    fn con_022_an_unreadable_metadata_response_quotes_what_arrived() {
        let err = MetadataResponse::parse(Side::Origin, b"<html>maintenance</html>").unwrap_err();
        assert!(err.to_string().contains("maintenance"), "{err}");
        assert_eq!(err.kind(), cdm_core::ErrorKind::Connect);

        // A response missing `contact_info` entirely.
        let err = MetadataResponse::parse(Side::Origin, br#"{"version":1}"#).unwrap_err();
        assert!(err.to_string().contains("contact_info"), "{err}");
    }

    #[test]
    fn con_025_a_refresh_is_rate_limited() {
        let tls = crate::tls::TlsSpec::new(Side::Origin, crate::tls::TrustMaterial::default())
            .client_config()
            .unwrap();
        let mut service = MetadataService::new(
            Side::Origin,
            "metadata.example.invalid",
            29080,
            tls,
            Duration::from_secs(300),
        );
        assert!(service.may_refresh(), "the first fetch is always allowed");
        assert_eq!(
            service.url(),
            "https://metadata.example.invalid:29080/metadata"
        );

        service.last_fetch = Some(Instant::now());
        assert!(!service.may_refresh());

        service.last_fetch = Instant::now().checked_sub(Duration::from_secs(600));
        assert!(service.may_refresh());
    }

    #[tokio::test]
    async fn con_025_a_declined_refresh_reports_none_rather_than_failing() {
        let tls = crate::tls::TlsSpec::new(Side::Origin, crate::tls::TrustMaterial::default())
            .client_config()
            .unwrap();
        let mut service = MetadataService::new(
            Side::Origin,
            "metadata.invalid",
            29080,
            tls,
            Duration::from_secs(300),
        );
        service.last_fetch = Some(Instant::now());
        assert!(service.refresh().await.unwrap().is_none());
    }
}
