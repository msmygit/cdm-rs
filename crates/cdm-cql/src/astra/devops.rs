//! Downloading a bundle from the Astra DevOps API (`CON-004`, `CON-005`).
//!
//! When `connect.{side}.astra.database_id` is set and no bundle path is given, cdm-rs asks the
//! DevOps API where the bundle is and fetches it:
//!
//! ```text
//! POST https://api.astra.datastax.com/v2/databases/{id}/secureBundleURL?all=true
//! Authorization: Bearer <connect.{side}.password>
//! ```
//!
//! `all=true` returns every region and every custom domain in one array, so the selection is made
//! here rather than by the API. The response shape is not in `SPEC`; it is taken from Java CDM's
//! `AstraDevOpsClient.extractDownloadUrl`, which cdm-rs matches field for field:
//!
//! ```json
//! [
//!   {
//!     "region": "us-east1",
//!     "downloadURL": "https://…",
//!     "customDomainBundles": [{ "domain": "cql.example.com", "downloadURL": "https://…" }]
//!   }
//! ]
//! ```
//!
//! Selection: the datacenter whose `region` matches `astra.region` — or the first, when no region
//! is configured — then either its `downloadURL` (`scb_type = default`) or the `downloadURL` of
//! the `customDomainBundles` entry whose `domain` matches `astra.custom_domain`
//! (`scb_type = custom`).
//!
//! # Divergence from Java, deliberately
//!
//! Java only attempts the download when **both** `database.id` and `scb.region` are set
//! (`ConnectionFetcher.getConnectionDetails`), and swallows a download failure, falling through
//! to whatever else is configured. `CON-004` says the trigger is `database_id` with no bundle
//! path, and `ERR-001` makes a failure to obtain credentials fatal rather than silent. cdm-rs
//! follows `SPEC`; the difference is noted in `docs/MIGRATION_FROM_JAVA.md`.

use std::sync::Arc;

use cdm_config::types::ScbType;
use cdm_core::{CdmError, Side};
use rustls::ClientConfig;
use serde::Deserialize;

use crate::astra::tempdir::BundleTempDir;
use crate::errors::{config_error, connect_error};
use crate::http::{self, HttpRequest};
use crate::tls::{TlsSpec, TrustMaterial};

/// The DevOps API host.
pub const DEVOPS_HOST: &str = "api.astra.datastax.com";

/// One datacenter's entry in the `secureBundleURL` response.
#[derive(Debug, Clone, Deserialize)]
pub struct BundleLocation {
    /// The Astra region this entry describes.
    #[serde(default)]
    pub region: Option<String>,
    /// Where the standard bundle for that region can be downloaded.
    #[serde(default, rename = "downloadURL")]
    pub download_url: Option<String>,
    /// The custom-domain bundles issued for that region.
    #[serde(default, rename = "customDomainBundles")]
    pub custom_domain_bundles: Vec<CustomDomainBundle>,
}

/// One custom-domain bundle.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomDomainBundle {
    /// The domain the bundle is issued for.
    #[serde(default)]
    pub domain: Option<String>,
    /// Where it can be downloaded.
    #[serde(default, rename = "downloadURL")]
    pub download_url: Option<String>,
}

/// What to select out of the API response (`CON-004`).
#[derive(Debug, Clone)]
pub struct BundleSelector {
    /// `default` or `custom`.
    pub scb_type: ScbType,
    /// The region whose bundle to take; the first datacenter when unset.
    pub region: Option<String>,
    /// The custom domain to match, required when `scb_type` is `custom`.
    pub custom_domain: Option<String>,
}

impl BundleSelector {
    /// Picks the download URL out of a parsed response (`CON-004`).
    pub fn select(&self, side: Side, locations: &[BundleLocation]) -> Result<String, CdmError> {
        let regions = || {
            locations
                .iter()
                .filter_map(|l| l.region.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };

        let location = match &self.region {
            Some(region) if !region.is_empty() => locations
                .iter()
                .find(|l| {
                    l.region
                        .as_deref()
                        .is_some_and(|r| r.eq_ignore_ascii_case(region))
                })
                .ok_or_else(|| {
                    config_error(
                        side,
                        format!(
                            "the Astra DevOps API returned no bundle for region {region}; it \
                             offers: {}",
                            regions()
                        ),
                        "connect.{side}.astra.region",
                    )
                })?,
            _ => locations.first().ok_or_else(|| {
                connect_error(
                    side,
                    "the Astra DevOps API returned no datacenters for this database",
                )
            })?,
        };

        match self.scb_type {
            ScbType::Default => location.download_url.clone().ok_or_else(|| {
                connect_error(
                    side,
                    format!(
                        "the Astra DevOps API entry for region {} has no downloadURL",
                        location.region.as_deref().unwrap_or("(unnamed)")
                    ),
                )
            }),
            ScbType::Custom => {
                let domain = self
                    .custom_domain
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .ok_or_else(|| {
                        config_error(
                            side,
                            "connect.{side}.astra.scb_type is `custom`, which requires \
                             connect.{side}.astra.custom_domain",
                            "connect.{side}.astra.custom_domain",
                        )
                    })?;
                location
                    .custom_domain_bundles
                    .iter()
                    .find(|bundle| {
                        bundle
                            .domain
                            .as_deref()
                            .is_some_and(|d| d.eq_ignore_ascii_case(domain))
                    })
                    .and_then(|bundle| bundle.download_url.clone())
                    .ok_or_else(|| {
                        config_error(
                            side,
                            format!(
                                "the Astra DevOps API has no custom-domain bundle for {domain} in \
                                 region {}; it offers: {}",
                                location.region.as_deref().unwrap_or("(unnamed)"),
                                location
                                    .custom_domain_bundles
                                    .iter()
                                    .filter_map(|b| b.domain.clone())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            "connect.{side}.astra.custom_domain",
                        )
                    })
            }
        }
    }
}

/// Parses a `secureBundleURL` response (`CON-004`).
pub fn parse_locations(side: Side, body: &[u8]) -> Result<Vec<BundleLocation>, CdmError> {
    serde_json::from_slice(body).map_err(|e| {
        connect_error(
            side,
            format!(
                "the Astra DevOps API returned a response cdm-rs cannot read ({e}); body: {}",
                String::from_utf8_lossy(body)
                    .chars()
                    .take(400)
                    .collect::<String>()
            ),
        )
    })
}

/// Splits an `https://host[:port]/path` URL into its parts.
pub fn split_https_url(side: Side, url: &str) -> Result<(String, u16, String), CdmError> {
    let rest = url.strip_prefix("https://").ok_or_else(|| {
        connect_error(
            side,
            format!("the Astra DevOps API returned a non-HTTPS download URL: {url}"),
        )
    })?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (
            rest.get(..index).unwrap_or_default(),
            rest.get(index..).unwrap_or("/"),
        ),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>().map_err(|_| {
                connect_error(side, format!("the download URL {url} has a malformed port"))
            })?,
        ),
        None => (authority, 443),
    };
    if host.is_empty() {
        return Err(connect_error(
            side,
            format!("the download URL {url} has no host"),
        ));
    }
    Ok((host.to_owned(), port, path.to_owned()))
}

/// The Astra DevOps API client (`CON-004`).
#[derive(Debug)]
pub struct DevOpsClient {
    side: Side,
    host: String,
    port: u16,
    tls: Arc<ClientConfig>,
}

impl DevOpsClient {
    /// A client against the public API, trusting the platform's web roots.
    pub fn new(side: Side) -> Result<Self, CdmError> {
        Self::with_endpoint(side, DEVOPS_HOST, 443)
    }

    /// A client against a specific endpoint. Exists so that tests need not reach the internet.
    pub fn with_endpoint(side: Side, host: impl Into<String>, port: u16) -> Result<Self, CdmError> {
        let tls = TlsSpec::new(side, TrustMaterial::default()).client_config()?;
        Ok(Self {
            side,
            host: host.into(),
            port,
            tls,
        })
    }

    /// Asks the API where the bundles are (`CON-004`).
    ///
    /// `token` is `connect.{side}.password`, which for Astra is the `AstraCS:` token
    /// (`CON-028`). It is passed as a bearer credential and never logged.
    pub async fn bundle_locations(
        &self,
        database_id: &str,
        token: &str,
    ) -> Result<Vec<BundleLocation>, CdmError> {
        let path = format!("/v2/databases/{database_id}/secureBundleURL?all=true");
        let response = http::send(
            self.side,
            self.tls.clone(),
            HttpRequest {
                method: "POST",
                host: &self.host,
                port: self.port,
                path: &path,
                headers: &[
                    ("Authorization", format!("Bearer {token}")),
                    ("Content-Type", "application/json".to_owned()),
                ],
                body: Some(b""),
            },
        )
        .await?;

        if response.status == 401 || response.status == 403 {
            return Err(CdmError::new(
                cdm_core::ErrorKind::Auth,
                format!(
                    "the Astra DevOps API rejected the token for database {database_id} ({}). \
                     For Astra, connect.{}.password must be the `AstraCS:` token (CON-028).",
                    response.status,
                    self.side.as_str()
                ),
            )
            .with_context(|c| c.with_side(self.side)));
        }
        if !response.is_success() {
            return Err(connect_error(
                self.side,
                format!(
                    "the Astra DevOps API answered {} for database {database_id}: {}",
                    response.status,
                    response.body_text().chars().take(400).collect::<String>()
                ),
            ));
        }
        parse_locations(self.side, &response.body)
    }

    /// Downloads a bundle zip from a URL the API handed back.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, CdmError> {
        let (host, port, path) = split_https_url(self.side, url)?;
        // The download URL points at object storage, not the API, and is pre-signed: no token.
        let response = http::send(
            self.side,
            self.tls.clone(),
            HttpRequest {
                method: "GET",
                host: &host,
                port,
                path: &path,
                headers: &[],
                body: None,
            },
        )
        .await?;
        if !response.is_success() {
            return Err(connect_error(
                self.side,
                format!(
                    "downloading the secure-connect-bundle from {host} answered {}",
                    response.status
                ),
            ));
        }
        Ok(response.body)
    }

    /// Fetches the bundle and writes it into a `0700` directory (`CON-004`, `CON-005`).
    ///
    /// The returned guard owns the directory: dropping it removes the bundle.
    pub async fn fetch_bundle(
        &self,
        database_id: &str,
        token: &str,
        selector: &BundleSelector,
    ) -> Result<(BundleTempDir, std::path::PathBuf, String), CdmError> {
        let locations = self.bundle_locations(database_id, token).await?;
        let url = selector.select(self.side, &locations)?;
        tracing::info!(
            side = self.side.as_str(),
            rule = "CON-004",
            database_id,
            scb_type = selector.scb_type.as_str(),
            "downloading the Astra secure-connect-bundle"
        );
        let zip = self.download(&url).await?;
        let dir = BundleTempDir::new(self.side)?;
        let name = format!("{}-secure-bundle.zip", self.side.as_str());
        let path = dir.write(self.side, &name, &zip)?;
        Ok((dir, path, url))
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

    const RESPONSE: &[u8] = br#"[
        {"region":"us-east1","downloadURL":"https://downloads.example.invalid/us-east1.zip",
         "customDomainBundles":[{"domain":"cql.example.com","downloadURL":"https://downloads.example.invalid/custom.zip"}]},
        {"region":"eu-west1","downloadURL":"https://downloads.example.invalid/eu-west1.zip","customDomainBundles":[]}
    ]"#;

    fn selector(scb_type: ScbType, region: Option<&str>, domain: Option<&str>) -> BundleSelector {
        BundleSelector {
            scb_type,
            region: region.map(str::to_owned),
            custom_domain: domain.map(str::to_owned),
        }
    }

    #[test]
    fn con_004_the_region_selects_the_bundle() {
        let locations = parse_locations(Side::Origin, RESPONSE).unwrap();
        assert_eq!(locations.len(), 2);
        assert_eq!(
            selector(ScbType::Default, Some("eu-west1"), None)
                .select(Side::Origin, &locations)
                .unwrap(),
            "https://downloads.example.invalid/eu-west1.zip"
        );
    }

    #[test]
    fn con_004_without_a_region_the_first_datacenter_is_used() {
        let locations = parse_locations(Side::Origin, RESPONSE).unwrap();
        assert_eq!(
            selector(ScbType::Default, None, None)
                .select(Side::Origin, &locations)
                .unwrap(),
            "https://downloads.example.invalid/us-east1.zip"
        );
    }

    #[test]
    fn con_004_a_custom_bundle_is_matched_by_domain() {
        let locations = parse_locations(Side::Origin, RESPONSE).unwrap();
        assert_eq!(
            selector(ScbType::Custom, Some("us-east1"), Some("CQL.example.com"))
                .select(Side::Origin, &locations)
                .unwrap(),
            "https://downloads.example.invalid/custom.zip"
        );
    }

    #[test]
    fn con_004_a_custom_bundle_without_a_domain_is_a_config_error() {
        let locations = parse_locations(Side::Origin, RESPONSE).unwrap();
        let err = selector(ScbType::Custom, None, None)
            .select(Side::Origin, &locations)
            .unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Config);
        assert!(err.to_string().contains("custom_domain"), "{err}");
    }

    #[test]
    fn con_004_an_unknown_region_lists_the_regions_that_exist() {
        let locations = parse_locations(Side::Origin, RESPONSE).unwrap();
        let err = selector(ScbType::Default, Some("ap-south1"), None)
            .select(Side::Origin, &locations)
            .unwrap_err();
        assert!(err.to_string().contains("us-east1, eu-west1"), "{err}");
    }

    #[test]
    fn con_004_an_unknown_custom_domain_lists_the_domains_that_exist() {
        let locations = parse_locations(Side::Origin, RESPONSE).unwrap();
        let err = selector(ScbType::Custom, Some("us-east1"), Some("other.example.com"))
            .select(Side::Origin, &locations)
            .unwrap_err();
        assert!(err.to_string().contains("cql.example.com"), "{err}");
    }

    #[test]
    fn con_004_an_empty_response_is_reported() {
        let locations = parse_locations(Side::Origin, b"[]").unwrap();
        let err = selector(ScbType::Default, None, None)
            .select(Side::Origin, &locations)
            .unwrap_err();
        assert!(err.to_string().contains("no datacenters"), "{err}");
    }

    #[test]
    fn con_004_an_entry_without_a_download_url_is_reported() {
        let locations = parse_locations(Side::Origin, br#"[{"region":"us-east1"}]"#).unwrap();
        let err = selector(ScbType::Default, None, None)
            .select(Side::Origin, &locations)
            .unwrap_err();
        assert!(err.to_string().contains("no downloadURL"), "{err}");
    }

    #[test]
    fn con_004_a_non_json_response_quotes_what_arrived() {
        let err = parse_locations(Side::Origin, b"<html>gateway timeout</html>").unwrap_err();
        assert!(err.to_string().contains("gateway timeout"), "{err}");
    }

    #[test]
    fn con_004_download_urls_are_split_into_host_port_and_path() {
        assert_eq!(
            split_https_url(
                Side::Origin,
                "https://downloads.example.invalid/a/b.zip?sig=x"
            )
            .unwrap(),
            (
                "downloads.example.invalid".to_owned(),
                443,
                "/a/b.zip?sig=x".to_owned()
            )
        );
        assert_eq!(
            split_https_url(Side::Origin, "https://host.invalid:8443").unwrap(),
            ("host.invalid".to_owned(), 8443, "/".to_owned())
        );
        assert!(split_https_url(Side::Origin, "http://insecure.invalid/a").is_err());
        assert!(split_https_url(Side::Origin, "https://host.invalid:http/a").is_err());
        assert!(split_https_url(Side::Origin, "https:///a").is_err());
    }

    #[tokio::test]
    async fn con_004_an_unreachable_devops_api_is_a_connect_error() {
        let client = DevOpsClient::with_endpoint(Side::Origin, "api.invalid", 443).unwrap();
        let err = client
            .bundle_locations("2b3d9e1f-0000-0000-0000-000000000000", "AstraCS:token")
            .await
            .unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Connect);
    }
}
