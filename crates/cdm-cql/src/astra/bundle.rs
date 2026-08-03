//! Reading an Astra secure-connect-bundle (`CON-020`, `CON-021`, `CON-026`).
//!
//! The bundle is a zip of eight files, of which cdm-rs needs four and ignores three:
//!
//! | Member | Use |
//! |---|---|
//! | `config.json` | the metadata service host and port, and the optional hints of `CON-021` |
//! | `ca.crt` | the trust anchor |
//! | `cert` | the client certificate for mutual TLS |
//! | `key` | its private key |
//! | `cqlshrc` | the **CQL port**, and the only correct source for it (`CON-026`) |
//! | `cert.pfx`, `identity.jks`, `trustStore.jks` | ignored: the same material, in formats that would have to be decrypted to obtain what the PEM members already give (`CON-020`) |
//!
//! The zip is read **in memory**. A bundle contains a private key; extracting it to disk to parse
//! it would leave credentials in the filesystem for no benefit. The only file cdm-rs ever writes
//! is a bundle it downloaded itself, into the `0700` directory of `CON-005`.
//!
//! # Leniency
//!
//! `CON-021` requires unknown `config.json` fields to be ignored and missing required ones to
//! produce a diagnostic that names the field and the bundle. The metadata contract is not
//! formally published, so a field cdm-rs does not know about today is expected, not exceptional.

use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use cdm_core::{CdmError, Side};
use serde::Deserialize;

use crate::errors::{config_error, tls_error_from};
use crate::tls::{self, Identity, StoreFormat, TrustMaterial};

/// Bundle members that are deliberately not read (`CON-020`).
pub const IGNORED_MEMBERS: &[&str] = &["cert.pfx", "identity.jks", "trustStore.jks"];

/// The `config.json` of a secure-connect-bundle, parsed leniently (`CON-021`).
///
/// Every field is optional at the parse step so that one missing value produces a diagnostic
/// naming it rather than a serde error naming a line offset. Fields cdm-rs does not know are
/// ignored.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BundleConfig {
    /// The metadata service hostname.
    pub host: Option<String>,
    /// The metadata service port.
    pub port: Option<u16>,
    /// The keyspace the bundle was issued for, if any.
    pub keyspace: Option<String>,
    /// The local datacenter named in the bundle, if any.
    #[serde(alias = "localDC", alias = "local_dc")]
    pub local_dc: Option<String>,
    /// The member holding the trust anchor, when it is not `ca.crt`.
    #[serde(alias = "caCertLocation")]
    pub ca_cert_location: Option<String>,
    /// The member holding the client key, when it is not `key`.
    #[serde(alias = "keyLocation")]
    pub key_location: Option<String>,
    /// The member holding the client certificate, when it is not `cert`.
    #[serde(alias = "certLocation")]
    pub cert_location: Option<String>,
    /// The SNI proxy host, when the bundle states it rather than leaving it to the metadata
    /// service.
    #[serde(alias = "sniHost")]
    pub sni_host: Option<String>,
    /// The SNI proxy port, likewise.
    #[serde(alias = "sniPort")]
    pub sni_port: Option<u16>,
    /// Host ids stated by the bundle, when present.
    #[serde(alias = "hostIds")]
    pub host_ids: Option<Vec<String>>,
}

impl BundleConfig {
    /// Parses `config.json`, ignoring anything it does not recognise (`CON-021`).
    pub fn parse(side: Side, bytes: &[u8], bundle: &str) -> Result<Self, CdmError> {
        serde_json::from_slice(bytes).map_err(|e| {
            config_error(
                side,
                format!("{bundle}: config.json is not valid JSON ({e})"),
                "connect.{side}.scb",
            )
        })
    }

    /// The metadata service hostname, or a diagnostic naming the field and the bundle
    /// (`CON-021`).
    pub fn require_host(&self, side: Side, bundle: &str) -> Result<&str, CdmError> {
        self.host
            .as_deref()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| {
                config_error(
                    side,
                    format!(
                        "{bundle}: config.json has no `host`, which is the address of the Astra \
                     metadata service and cannot be defaulted"
                    ),
                    "connect.{side}.scb",
                )
            })
    }

    /// The metadata service port, or a diagnostic naming the field and the bundle (`CON-021`).
    pub fn require_port(&self, side: Side, bundle: &str) -> Result<u16, CdmError> {
        self.port.ok_or_else(|| {
            config_error(
                side,
                format!(
                    "{bundle}: config.json has no `port`, which is the port of the Astra \
                     metadata service and cannot be defaulted"
                ),
                "connect.{side}.scb",
            )
        })
    }
}

/// A secure-connect-bundle, read into memory (`CON-020`).
#[derive(Debug)]
pub struct SecureConnectBundle {
    origin: String,
    config: BundleConfig,
    ca_cert: Vec<u8>,
    client_cert: Vec<u8>,
    client_key: Vec<u8>,
    cqlshrc: Option<String>,
    ignored: Vec<String>,
    side: Side,
}

impl SecureConnectBundle {
    /// Reads a bundle from a zip already in memory.
    pub fn from_bytes(
        side: Side,
        bytes: &[u8],
        origin: impl Into<String>,
    ) -> Result<Self, CdmError> {
        let origin = origin.into();
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| {
            config_error(
                side,
                format!("{origin} is not a readable secure-connect-bundle zip ({e})"),
                "connect.{side}.scb",
            )
        })?;

        let mut members: Vec<(String, Vec<u8>)> = Vec::new();
        let mut ignored = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|e| {
                config_error(
                    side,
                    format!("{origin}: cannot read bundle member {index} ({e})"),
                    "connect.{side}.scb",
                )
            })?;
            if entry.is_dir() {
                continue;
            }
            // Zip entries may be nested, e.g. when a bundle was re-zipped with its directory.
            let name = entry
                .name()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            if IGNORED_MEMBERS
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&name))
            {
                ignored.push(name);
                continue;
            }
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents).map_err(|e| {
                config_error(
                    side,
                    format!("{origin}: cannot read bundle member {name} ({e})"),
                    "connect.{side}.scb",
                )
            })?;
            members.push((name, contents));
        }

        let find = |wanted: &str| -> Option<Vec<u8>> {
            members
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
                .map(|(_, bytes)| bytes.clone())
        };

        let config_bytes = find("config.json").ok_or_else(|| {
            config_error(
                side,
                format!(
                    "{origin}: the bundle has no config.json. Members found: {}",
                    member_list(&members, &ignored)
                ),
                "connect.{side}.scb",
            )
        })?;
        let config = BundleConfig::parse(side, &config_bytes, &origin)?;

        let ca_cert = required_member(
            side,
            &origin,
            &members,
            config.ca_cert_location.as_deref(),
            "ca.crt",
            "the trust anchor",
        )?;
        let client_cert = required_member(
            side,
            &origin,
            &members,
            config.cert_location.as_deref(),
            "cert",
            "the client certificate",
        )?;
        let client_key = required_member(
            side,
            &origin,
            &members,
            config.key_location.as_deref(),
            "key",
            "the client private key",
        )?;
        let cqlshrc = find("cqlshrc").map(|bytes| String::from_utf8_lossy(&bytes).into_owned());

        Ok(Self {
            origin,
            config,
            ca_cert,
            client_cert,
            client_key,
            cqlshrc,
            ignored,
            side,
        })
    }

    /// Reads a bundle from a file, accepting the `file://` form Java CDM writes.
    pub fn from_path(side: Side, path: &Path) -> Result<Self, CdmError> {
        let path = strip_file_scheme(path);
        let bytes = std::fs::read(&path).map_err(|e| {
            config_error(
                side,
                format!(
                    "cannot read the secure-connect-bundle {} ({e})",
                    path.display()
                ),
                "connect.{side}.scb",
            )
        })?;
        Self::from_bytes(side, &bytes, path.display().to_string())
    }

    /// Where the bundle came from: a path, or a DevOps API download URL.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The parsed `config.json`.
    pub fn config(&self) -> &BundleConfig {
        &self.config
    }

    /// The members that were present and deliberately not read (`CON-020`).
    pub fn ignored_members(&self) -> &[String] {
        &self.ignored
    }

    /// The metadata service URL, `https://host:port/metadata` (`CON-022`).
    pub fn metadata_url(&self) -> Result<String, CdmError> {
        Ok(format!(
            "https://{}:{}/metadata",
            self.config.require_host(self.side, &self.origin)?,
            self.config.require_port(self.side, &self.origin)?
        ))
    }

    /// The metadata service host and port (`CON-022`).
    pub fn metadata_endpoint(&self) -> Result<(String, u16), CdmError> {
        Ok((
            self.config
                .require_host(self.side, &self.origin)?
                .to_owned(),
            self.config.require_port(self.side, &self.origin)?,
        ))
    }

    /// The CQL port, taken from `cqlshrc` — **not** from `config.json` (`CON-026`).
    ///
    /// The ports in `config.json` serve the metadata service and the proxy's own management
    /// endpoints; DataStax's guidance for drivers without bundle support is explicit that the
    /// port to speak CQL on is the one in `cqlshrc`. Getting this wrong produces a connection
    /// that opens and then does nothing, which is why it is called out in `AGENTS.md`.
    pub fn cql_port(&self) -> Option<u16> {
        self.cqlshrc.as_deref().and_then(cqlshrc_port)
    }

    /// The `cqlshrc` member verbatim, when the bundle carried one.
    pub fn cqlshrc(&self) -> Option<&str> {
        self.cqlshrc.as_deref()
    }

    /// The CQL host, taken from `cqlshrc` when it names one and from `config.json` otherwise.
    pub fn cql_host(&self) -> Option<String> {
        self.cqlshrc
            .as_deref()
            .and_then(cqlshrc_hostname)
            .or_else(|| self.config.host.clone())
    }

    /// The trust anchor, parsed from the bundle's `ca.crt`.
    pub fn trust_material(&self) -> Result<TrustMaterial, CdmError> {
        tls::parse_trust_store(self.side, &self.ca_cert, None, StoreFormat::Pem)
            .map_err(|e| tls_error_from(self.side, format!("{}: ca.crt", self.origin), e))
    }

    /// The client identity, assembled from the bundle's `cert` and `key`.
    pub fn identity(&self) -> Result<Identity, CdmError> {
        let mut pem = self.client_cert.clone();
        pem.push(b'\n');
        pem.extend_from_slice(&self.client_key);
        tls::parse_key_store(self.side, &pem, None, StoreFormat::Pem)
            .map_err(|e| tls_error_from(self.side, format!("{}: cert and key", self.origin), e))
    }
}

/// Strips the `file://` prefix Java CDM's `spark.cdm.connect.{side}.scb` values carry.
pub fn strip_file_scheme(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix("file://") {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

/// The `port` of the `[connection]` section of a `cqlshrc` (`CON-026`).
pub fn cqlshrc_port(cqlshrc: &str) -> Option<u16> {
    ini_value(cqlshrc, "connection", "port")?.parse().ok()
}

/// The `hostname` of the `[connection]` section of a `cqlshrc`.
pub fn cqlshrc_hostname(cqlshrc: &str) -> Option<String> {
    ini_value(cqlshrc, "connection", "hostname")
}

/// Reads one key of one section of an INI file, tolerating `=` and `:` separators and comments.
fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.trim().to_ascii_lowercase();
            continue;
        }
        if current != section {
            continue;
        }
        let (name, value) = line.split_once('=').or_else(|| line.split_once(':'))?;
        if name.trim().eq_ignore_ascii_case(key) {
            return Some(value.trim().to_owned());
        }
    }
    None
}

fn required_member(
    side: Side,
    origin: &str,
    members: &[(String, Vec<u8>)],
    configured: Option<&str>,
    default_name: &str,
    what: &str,
) -> Result<Vec<u8>, CdmError> {
    let wanted = configured
        .and_then(|location| location.rsplit('/').next())
        .filter(|name| !name.is_empty())
        .unwrap_or(default_name);
    members
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
        .map(|(_, bytes)| bytes.clone())
        .ok_or_else(|| {
            config_error(
                side,
                format!(
                    "{origin}: the bundle has no `{wanted}`, which holds {what}. Members found: {}",
                    member_list(members, &[])
                ),
                "connect.{side}.scb",
            )
        })
}

fn member_list(members: &[(String, Vec<u8>)], ignored: &[String]) -> String {
    let mut names: Vec<&str> = members.iter().map(|(name, _)| name.as_str()).collect();
    names.extend(ignored.iter().map(String::as_str));
    if names.is_empty() {
        return "none".to_owned();
    }
    names.sort_unstable();
    names.join(", ")
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
    use crate::testfixtures::astra::BundleBuilder;
    use crate::testfixtures::Pki;

    #[test]
    fn con_020_a_bundle_is_read_from_memory_and_yields_its_tls_material() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki).build();
        let bundle = SecureConnectBundle::from_bytes(Side::Origin, &zip, "test.zip").unwrap();

        assert_eq!(bundle.origin(), "test.zip");
        assert_eq!(
            bundle.trust_material().unwrap().certificates()[0].as_ref(),
            pki.ca_der().as_ref()
        );
        let identity = bundle.identity().unwrap();
        assert_eq!(identity.chain()[0].as_ref(), pki.client_cert_der().as_ref());
        assert_eq!(
            identity.key().secret_der(),
            pki.client_key_der().secret_der()
        );
    }

    #[test]
    fn con_020_the_jks_and_pfx_members_are_ignored() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki).with_java_members().build();
        let bundle = SecureConnectBundle::from_bytes(Side::Origin, &zip, "test.zip").unwrap();

        let mut ignored = bundle.ignored_members().to_vec();
        ignored.sort();
        assert_eq!(ignored, vec!["cert.pfx", "identity.jks", "trustStore.jks"]);
        // And the PEM members are still what was used.
        assert!(bundle.trust_material().is_ok());
    }

    #[test]
    fn con_020_a_bundle_missing_a_required_member_names_it() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki).without("ca.crt").build();
        let err = SecureConnectBundle::from_bytes(Side::Origin, &zip, "scb.zip").unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("ca.crt"), "{rendered}");
        assert!(rendered.contains("scb.zip"), "{rendered}");
        assert!(rendered.contains("trust anchor"), "{rendered}");
    }

    #[test]
    fn con_020_a_bundle_without_config_json_names_the_members_it_did_find() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki).without("config.json").build();
        let err = SecureConnectBundle::from_bytes(Side::Origin, &zip, "scb.zip").unwrap_err();
        assert!(err.to_string().contains("no config.json"), "{err}");
        assert!(err.to_string().contains("cqlshrc"), "{err}");
    }

    #[test]
    fn con_020_something_that_is_not_a_zip_is_a_config_error() {
        let err =
            SecureConnectBundle::from_bytes(Side::Origin, b"not a zip", "scb.zip").unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Config);
        assert!(err.to_string().contains("not a readable"), "{err}");
    }

    #[test]
    fn con_020_members_nested_in_a_directory_are_found() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki)
            .nested("secure-connect-db/")
            .build();
        let bundle = SecureConnectBundle::from_bytes(Side::Origin, &zip, "scb.zip").unwrap();
        assert!(bundle.identity().is_ok());
    }

    #[test]
    fn con_021_unknown_config_json_fields_are_ignored() {
        let config = BundleConfig::parse(
            Side::Origin,
            br#"{"host":"h.example.com","port":29080,"somethingNew":{"a":1},"pfxCertPassword":"x"}"#,
            "scb.zip",
        )
        .unwrap();
        assert_eq!(config.host.as_deref(), Some("h.example.com"));
        assert_eq!(config.port, Some(29080));
    }

    #[test]
    fn con_021_the_documented_optional_fields_are_read() {
        let config = BundleConfig::parse(
            Side::Origin,
            br#"{"host":"h","port":1,"keyspace":"ks","localDC":"us-east1",
                 "caCertLocation":"./ca.crt","keyLocation":"./key","certLocation":"./cert",
                 "sniHost":"proxy.example.com","sniPort":29042,"hostIds":["a","b"]}"#,
            "scb.zip",
        )
        .unwrap();
        assert_eq!(config.keyspace.as_deref(), Some("ks"));
        assert_eq!(config.local_dc.as_deref(), Some("us-east1"));
        assert_eq!(config.sni_host.as_deref(), Some("proxy.example.com"));
        assert_eq!(config.sni_port, Some(29042));
        assert_eq!(config.host_ids.unwrap().len(), 2);
        assert_eq!(config.ca_cert_location.as_deref(), Some("./ca.crt"));
    }

    #[test]
    fn con_021_a_missing_required_field_names_the_field_and_the_bundle() {
        let config = BundleConfig::parse(Side::Origin, br#"{"port":29080}"#, "scb.zip").unwrap();
        let err = config.require_host(Side::Origin, "scb.zip").unwrap_err();
        assert!(err.to_string().contains("`host`"), "{err}");
        assert!(err.to_string().contains("scb.zip"), "{err}");
        assert_eq!(err.kind(), cdm_core::ErrorKind::Config);

        let config = BundleConfig::parse(Side::Origin, br#"{"host":"h"}"#, "scb.zip").unwrap();
        assert!(config.require_port(Side::Origin, "scb.zip").is_err());
    }

    #[test]
    fn con_021_malformed_json_is_a_config_error_not_a_panic() {
        let err = BundleConfig::parse(Side::Origin, b"{not json", "scb.zip").unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Config);
        assert!(err.to_string().contains("config.json"), "{err}");
    }

    #[test]
    fn con_021_a_member_named_by_config_json_is_preferred_over_the_default_name() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki)
            .rename("ca.crt", "authority.pem")
            .config_json(
                r#"{"host":"h.example.com","port":29080,"caCertLocation":"./authority.pem"}"#,
            )
            .build();
        let bundle = SecureConnectBundle::from_bytes(Side::Origin, &zip, "scb.zip").unwrap();
        assert_eq!(bundle.trust_material().unwrap().len(), 1);
    }

    #[test]
    fn con_022_the_metadata_url_is_built_from_config_json() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki).build();
        let bundle = SecureConnectBundle::from_bytes(Side::Origin, &zip, "scb.zip").unwrap();
        assert_eq!(
            bundle.metadata_url().unwrap(),
            "https://metadata.example.invalid:29080/metadata"
        );
        assert_eq!(
            bundle.metadata_endpoint().unwrap(),
            ("metadata.example.invalid".to_owned(), 29080)
        );
    }

    #[test]
    fn con_026_the_cql_port_comes_from_cqlshrc() {
        let pki = Pki::new();
        // config.json says 29080 for the metadata service; cqlshrc says 29042 for CQL.
        let zip = BundleBuilder::new(&pki).build();
        let bundle = SecureConnectBundle::from_bytes(Side::Origin, &zip, "scb.zip").unwrap();
        assert_eq!(bundle.config().port, Some(29080));
        assert_eq!(bundle.cql_port(), Some(29042));
        assert_eq!(
            bundle.cql_host().as_deref(),
            Some("cql.example.invalid"),
            "cqlshrc names the endpoint to speak CQL to"
        );
    }

    #[test]
    fn con_026_a_bundle_without_cqlshrc_has_no_cql_port() {
        let pki = Pki::new();
        let zip = BundleBuilder::new(&pki).without("cqlshrc").build();
        let bundle = SecureConnectBundle::from_bytes(Side::Origin, &zip, "scb.zip").unwrap();
        assert_eq!(bundle.cql_port(), None);
        // The host then falls back to config.json's, which is at least the right cluster.
        assert_eq!(
            bundle.cql_host().as_deref(),
            Some("metadata.example.invalid")
        );
    }

    #[test]
    fn con_026_cqlshrc_parsing_tolerates_comments_and_other_sections() {
        let text = "# a comment\n[authentication]\nport = 1\n\n[connection]\n; another\nhostname = h.example.com\nport = 29042\nfactory = x\n";
        assert_eq!(cqlshrc_port(text), Some(29042));
        assert_eq!(cqlshrc_hostname(text).as_deref(), Some("h.example.com"));
        assert_eq!(cqlshrc_port("[connection]\nport = not-a-number\n"), None);
        assert_eq!(cqlshrc_port("nothing here"), None);
        assert_eq!(cqlshrc_port("[connection]\nhostname = h\n"), None);
    }

    #[test]
    fn con_020_a_bundle_is_read_from_a_file_url() {
        let pki = Pki::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secure-connect-db.zip");
        std::fs::write(&path, BundleBuilder::new(&pki).build()).unwrap();

        let bundle = SecureConnectBundle::from_path(
            Side::Origin,
            Path::new(&format!("file://{}", path.display())),
        )
        .unwrap();
        assert_eq!(bundle.cql_port(), Some(29042));

        let missing = SecureConnectBundle::from_path(Side::Origin, Path::new("/nowhere/scb.zip"))
            .unwrap_err();
        assert!(missing.to_string().contains("cannot read"), "{missing}");
    }
}
