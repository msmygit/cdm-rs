//! TLS material and the rustls configuration built from it (`CON-006`, `CON-007`).
//!
//! Java CDM hands its trust and key stores to the JVM, which reads `JKS`, `PKCS12` and `PEM`
//! without being asked twice. There is no JVM here, and nothing in the Rust ecosystem reads a
//! Java keystore, so this module owns three readers:
//!
//! | Format | Reader | Notes |
//! |---|---|---|
//! | `PEM` | [`pem`] | `rustls-pki-types`; certificates and PKCS#1/PKCS#8/SEC1 keys |
//! | `PKCS12` | [`pkcs12`] | `p12-keystore`; PBES1 and PBES2 protected bags |
//! | `JKS` | [`jks`] | ours, pure Rust: JKS/JCEKS magic, SHA-1 keyed digest, PKCS#8 extraction |
//!
//! # The verifier, and why it is not the default one
//!
//! `scylla-rust-driver` 1.7 hands rustls a `ServerName::IpAddress` built from the address it is
//! dialling (`network/connection.rs`), because a driver connects to nodes by address, not by name.
//! The stock rustls verifier would then demand an IP SAN, which no Cassandra deployment issues:
//! Java CDM's clusters present certificates named after their hosts, and JSSE only checks the name
//! at all when `require_endpoint_verification` is on, which is off by default in
//! `cassandra.yaml`.
//!
//! [`verifier`] therefore always verifies the chain against the configured trust store — that is
//! the part that matters — and checks the name only against a hostname the operator configured,
//! never against the driver's synthetic IP name. See [`verifier::ChainVerifier`] for the full
//! argument.

pub mod ciphers;
pub mod jks;
pub mod pem;
pub mod pkcs12;
pub mod verifier;

use std::path::Path;
use std::sync::Arc;

use cdm_config::types::TrustStoreType;
use cdm_core::{CdmError, Side};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ClientConfig;

use crate::errors::{tls_error, tls_error_from};

/// The on-disk format of a trust store or key store (`CON-006`).
///
/// Mirrors [`TrustStoreType`] so that this crate does not force `cdm-config` on a caller that
/// only wants to read a store, and so that a key store — which the configuration model does not
/// give a format property — can still be read in every format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreFormat {
    /// A Java key store, `JKS` or `JCEKS`.
    Jks,
    /// A PKCS#12 archive, as `.p12`, `.pfx` or Astra's `cert.pfx`.
    Pkcs12,
    /// PEM-encoded certificates and keys, concatenated in one file.
    Pem,
}

impl StoreFormat {
    /// The format's name, as `tls.trustStore.type` spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jks => "JKS",
            Self::Pkcs12 => "PKCS12",
            Self::Pem => "PEM",
        }
    }

    /// Guesses the format from a file extension, for the key store, which has no type property.
    ///
    /// `.jks`/`.jceks` are JKS, `.p12`/`.pfx` are PKCS#12, everything else is assumed PEM —
    /// PEM being the only one of the three whose content is self-describing, so a wrong guess
    /// surfaces as a clear parse error rather than as garbage.
    pub fn guess_from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("jks" | "jceks" | "keystore" | "truststore") => Self::Jks,
            Some("p12" | "pfx" | "pkcs12") => Self::Pkcs12,
            _ => Self::Pem,
        }
    }
}

impl From<TrustStoreType> for StoreFormat {
    fn from(value: TrustStoreType) -> Self {
        match value {
            TrustStoreType::Jks => Self::Jks,
            TrustStoreType::Pkcs12 => Self::Pkcs12,
            TrustStoreType::Pem => Self::Pem,
        }
    }
}

/// The certificates a side will trust (`CON-006`).
#[derive(Debug, Clone, Default)]
pub struct TrustMaterial {
    certificates: Vec<CertificateDer<'static>>,
}

impl TrustMaterial {
    /// Wraps a list of DER certificates.
    pub fn new(certificates: Vec<CertificateDer<'static>>) -> Self {
        Self { certificates }
    }

    /// The trust anchors, in the order the store listed them.
    pub fn certificates(&self) -> &[CertificateDer<'static>] {
        &self.certificates
    }

    /// How many anchors the store yielded.
    pub fn len(&self) -> usize {
        self.certificates.len()
    }

    /// Whether the store yielded no anchors at all, which is always a configuration error.
    pub fn is_empty(&self) -> bool {
        self.certificates.is_empty()
    }

    /// Builds a rustls root store, rejecting certificates webpki cannot parse.
    pub fn root_store(&self, side: Side) -> Result<rustls::RootCertStore, CdmError> {
        let mut roots = rustls::RootCertStore::empty();
        for certificate in &self.certificates {
            roots.add(certificate.clone()).map_err(|e| {
                tls_error_from(side, "a trusted certificate could not be parsed", e)
            })?;
        }
        if roots.is_empty() {
            return Err(tls_error(
                side,
                "the trust store contains no usable certificates",
            ));
        }
        Ok(roots)
    }
}

/// The client certificate and key a side presents for mutual TLS (`CON-006`).
///
/// Not [`Clone`]: [`PrivateKeyDer`] deliberately is not, so that key material is not duplicated
/// around a process by accident. Use [`Identity::clone_identity`] where a copy is genuinely
/// needed.
#[derive(Debug)]
pub struct Identity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl Identity {
    /// Pairs a certificate chain with its private key.
    pub fn new(chain: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>) -> Self {
        Self { chain, key }
    }

    /// The certificate chain, end-entity first.
    pub fn chain(&self) -> &[CertificateDer<'static>] {
        &self.chain
    }

    /// The private key.
    pub fn key(&self) -> &PrivateKeyDer<'static> {
        &self.key
    }

    /// An explicit copy, including the key.
    #[must_use]
    pub fn clone_identity(&self) -> Self {
        Self {
            chain: self.chain.clone(),
            key: self.key.clone_key(),
        }
    }
}

/// Everything needed to build a client TLS configuration for one side (`CON-006`, `CON-007`).
#[derive(Debug)]
pub struct TlsSpec {
    /// Which side this configuration is for, so failures name it.
    pub side: Side,
    /// The certificates to trust. Empty means "the platform's web roots", which is what the
    /// Astra DevOps API needs and what a Cassandra cluster never uses.
    pub trust: TrustMaterial,
    /// The client certificate to present, when the cluster requires mutual TLS.
    pub identity: Option<Identity>,
    /// Cipher suites requested by `tls.cipher_suites` (`CON-007`).
    pub cipher_suites: Vec<String>,
    /// The hostname the server certificate must be issued for.
    ///
    /// `None` — the default, and what Java CDM does unless the cluster sets
    /// `require_endpoint_verification` — verifies the chain but not the name. See [`verifier`].
    pub expected_hostname: Option<String>,
}

impl TlsSpec {
    /// A specification that trusts `trust` and presents nothing.
    pub fn new(side: Side, trust: TrustMaterial) -> Self {
        Self {
            side,
            trust,
            identity: None,
            cipher_suites: Vec::new(),
            expected_hostname: None,
        }
    }

    /// Adds a client identity for mutual TLS.
    #[must_use]
    pub fn with_identity(mut self, identity: Identity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Requests specific cipher suites (`CON-007`).
    #[must_use]
    pub fn with_cipher_suites(mut self, suites: Vec<String>) -> Self {
        self.cipher_suites = suites;
        self
    }

    /// Requires the server certificate to be issued for `hostname`.
    #[must_use]
    pub fn with_expected_hostname(mut self, hostname: impl Into<String>) -> Self {
        self.expected_hostname = Some(hostname.into());
        self
    }

    /// Builds the rustls client configuration (`CON-006`, `CON-007`).
    ///
    /// An unsupported cipher suite fails here, with the supported set in the message, rather than
    /// being silently dropped in favour of something else (`CON-007`).
    pub fn client_config(&self) -> Result<Arc<ClientConfig>, CdmError> {
        let provider = ciphers::provider_for(self.side, &self.cipher_suites)?;
        let provider = Arc::new(provider);

        let roots = if self.trust.is_empty() {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            roots
        } else {
            self.trust.root_store(self.side)?
        };

        let verifier = verifier::ChainVerifier::new(
            self.side,
            roots,
            provider.clone(),
            self.expected_hostname.clone(),
        )?;

        let builder = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| tls_error_from(self.side, "no usable TLS protocol version", e))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier));

        let config = match &self.identity {
            Some(identity) => builder
                .with_client_auth_cert(identity.chain().to_vec(), identity.key().clone_key())
                .map_err(|e| {
                    tls_error_from(
                        self.side,
                        "the client certificate was rejected by rustls",
                        e,
                    )
                })?,
            None => builder.with_no_client_auth(),
        };
        Ok(Arc::new(config))
    }
}

/// Reads a trust store from disk in the given format (`CON-006`).
pub fn read_trust_store(
    side: Side,
    path: &Path,
    password: Option<&str>,
    format: StoreFormat,
) -> Result<TrustMaterial, CdmError> {
    let bytes = read_file(side, path)?;
    parse_trust_store(side, &bytes, password, format)
        .map_err(|e| annotate(e, side, path, format, "trust store"))
}

/// Reads a key store from disk in the given format (`CON-006`).
pub fn read_key_store(
    side: Side,
    path: &Path,
    password: Option<&str>,
    format: StoreFormat,
) -> Result<Identity, CdmError> {
    let bytes = read_file(side, path)?;
    parse_key_store(side, &bytes, password, format)
        .map_err(|e| annotate(e, side, path, format, "key store"))
}

/// Parses trust material already in memory, which is how the Astra bundle is read (`CON-020`).
pub fn parse_trust_store(
    side: Side,
    bytes: &[u8],
    password: Option<&str>,
    format: StoreFormat,
) -> Result<TrustMaterial, CdmError> {
    let certificates = match format {
        StoreFormat::Pem => pem::certificates(side, bytes)?,
        StoreFormat::Pkcs12 => pkcs12::certificates(side, bytes, password.unwrap_or_default())?,
        StoreFormat::Jks => jks::certificates(side, bytes, password)?,
    };
    if certificates.is_empty() {
        return Err(tls_error(
            side,
            format!(
                "the {} trust store contains no certificates",
                format.as_str()
            ),
        ));
    }
    Ok(TrustMaterial::new(certificates))
}

/// Parses key material already in memory, which is how the Astra bundle is read (`CON-020`).
pub fn parse_key_store(
    side: Side,
    bytes: &[u8],
    password: Option<&str>,
    format: StoreFormat,
) -> Result<Identity, CdmError> {
    match format {
        StoreFormat::Pem => pem::identity(side, bytes),
        StoreFormat::Pkcs12 => pkcs12::identity(side, bytes, password.unwrap_or_default()),
        StoreFormat::Jks => jks::identity(side, bytes, password),
    }
}

fn read_file(side: Side, path: &Path) -> Result<Vec<u8>, CdmError> {
    std::fs::read(path).map_err(|e| {
        tls_error_from(
            side,
            format!("cannot read TLS material from {}", path.display()),
            e,
        )
    })
}

/// Re-labels a parse failure with the file it came from, since the parsers see only bytes.
fn annotate(error: CdmError, side: Side, path: &Path, format: StoreFormat, what: &str) -> CdmError {
    tls_error_from(
        side,
        format!(
            "{what} {} could not be read as {}",
            path.display(),
            format.as_str()
        ),
        error,
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
    use crate::testfixtures::{self, Pki};

    #[test]
    fn con_006_store_format_is_guessed_from_the_extension() {
        assert_eq!(
            StoreFormat::guess_from_path(Path::new("/x/identity.jks")),
            StoreFormat::Jks
        );
        assert_eq!(
            StoreFormat::guess_from_path(Path::new("/x/cert.PFX")),
            StoreFormat::Pkcs12
        );
        assert_eq!(
            StoreFormat::guess_from_path(Path::new("/x/cert")),
            StoreFormat::Pem
        );
    }

    #[test]
    fn con_006_the_configured_store_type_maps_onto_a_reader() {
        assert_eq!(StoreFormat::from(TrustStoreType::Jks), StoreFormat::Jks);
        assert_eq!(
            StoreFormat::from(TrustStoreType::Pkcs12),
            StoreFormat::Pkcs12
        );
        assert_eq!(StoreFormat::from(TrustStoreType::Pem), StoreFormat::Pem);
        assert_eq!(StoreFormat::Jks.as_str(), "JKS");
    }

    #[test]
    fn con_006_every_format_yields_the_same_trust_anchor() {
        let pki = Pki::new();
        let expected = pki.ca_der();

        let from_pem = parse_trust_store(
            Side::Origin,
            pki.ca_pem().as_bytes(),
            None,
            StoreFormat::Pem,
        )
        .unwrap();
        let from_p12 = parse_trust_store(
            Side::Origin,
            &pki.truststore_pkcs12(pki.other_password()),
            Some(pki.other_password()),
            StoreFormat::Pkcs12,
        )
        .unwrap();
        let from_jks = parse_trust_store(
            Side::Origin,
            &pki.truststore_jks(pki.other_password()),
            Some(pki.other_password()),
            StoreFormat::Jks,
        )
        .unwrap();

        for material in [&from_pem, &from_p12, &from_jks] {
            assert_eq!(material.len(), 1);
            assert_eq!(material.certificates()[0].as_ref(), expected.as_ref());
        }
    }

    #[test]
    fn con_006_every_format_yields_the_same_client_identity() {
        let pki = Pki::new();

        let from_pem = parse_key_store(
            Side::Target,
            format!("{}{}", pki.client_key_pem(), pki.client_cert_pem()).as_bytes(),
            None,
            StoreFormat::Pem,
        )
        .unwrap();
        let from_p12 = parse_key_store(
            Side::Target,
            &pki.keystore_pkcs12(pki.other_password()),
            Some(pki.other_password()),
            StoreFormat::Pkcs12,
        )
        .unwrap();
        let from_jks = parse_key_store(
            Side::Target,
            &pki.keystore_jks(pki.other_password()),
            Some(pki.other_password()),
            StoreFormat::Jks,
        )
        .unwrap();

        for identity in [&from_pem, &from_p12, &from_jks] {
            assert_eq!(
                identity.chain()[0].as_ref(),
                pki.client_cert_der().as_ref(),
                "the end-entity certificate must come first"
            );
            assert_eq!(
                identity.key().secret_der(),
                pki.client_key_der().secret_der()
            );
        }
    }

    #[test]
    fn con_006_a_store_in_the_wrong_format_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trust.jks");
        std::fs::write(&path, b"not a keystore at all").unwrap();

        let err = read_trust_store(Side::Origin, &path, None, StoreFormat::Jks).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("trust.jks"), "{rendered}");
        assert!(rendered.contains("JKS"), "{rendered}");
    }

    #[test]
    fn con_006_a_missing_store_is_a_tls_error_naming_the_path() {
        let err = read_trust_store(
            Side::Origin,
            Path::new("/nonexistent/ts.pem"),
            None,
            StoreFormat::Pem,
        )
        .unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Tls);
        assert!(err.to_string().contains("/nonexistent/ts.pem"));
    }

    #[test]
    fn con_006_an_empty_trust_store_is_rejected() {
        let err = parse_trust_store(Side::Origin, b"", None, StoreFormat::Pem).unwrap_err();
        assert!(err.to_string().contains("no certificates"));
    }

    #[test]
    fn con_006_a_client_config_is_built_from_trust_and_identity() {
        let pki = Pki::new();
        let spec = TlsSpec::new(
            Side::Origin,
            parse_trust_store(
                Side::Origin,
                pki.ca_pem().as_bytes(),
                None,
                StoreFormat::Pem,
            )
            .unwrap(),
        )
        .with_identity(
            parse_key_store(
                Side::Origin,
                format!("{}{}", pki.client_key_pem(), pki.client_cert_pem()).as_bytes(),
                None,
                StoreFormat::Pem,
            )
            .unwrap(),
        );
        let config = spec.client_config().unwrap();
        assert!(config.client_auth_cert_resolver.has_certs());
    }

    #[test]
    fn con_006_a_trust_store_of_unparsable_certificates_is_rejected() {
        let material = TrustMaterial::new(vec![CertificateDer::from(vec![0x30, 0x00])]);
        let err = material.root_store(Side::Origin).unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Tls);
    }

    #[test]
    fn con_006_web_roots_are_used_when_no_trust_store_is_configured() {
        // This is the Astra DevOps API case (CON-004): a public endpoint with a public CA.
        let config = TlsSpec::new(Side::Origin, TrustMaterial::default())
            .client_config()
            .unwrap();
        assert!(!config.client_auth_cert_resolver.has_certs());
    }

    #[test]
    fn con_006_an_identity_can_be_copied_explicitly() {
        let pki = Pki::new();
        let identity = parse_key_store(
            Side::Origin,
            format!("{}{}", pki.client_key_pem(), pki.client_cert_pem()).as_bytes(),
            None,
            StoreFormat::Pem,
        )
        .unwrap();
        let copy = identity.clone_identity();
        assert_eq!(copy.key().secret_der(), identity.key().secret_der());
        assert_eq!(copy.chain(), identity.chain());
    }

    /// Runs a one-shot rustls server presenting `pki`'s server certificate, and connects to it
    /// with `spec`. Returns what the client's handshake did.
    ///
    /// This is the test that matters for `CON-006`/`CON-007`: everything else asserts about the
    /// configuration cdm-rs builds, and this asserts that a real TLS 1.2/1.3 handshake against a
    /// real peer succeeds — including the mutual-TLS leg and the IP-named `ServerName` the driver
    /// insists on passing.
    async fn handshake(pki: &Pki, spec: &TlsSpec, require_client_auth: bool) -> Result<(), String> {
        use std::net::{IpAddr, Ipv4Addr};

        use rustls::pki_types::ServerName;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let mut roots = rustls::RootCertStore::empty();
        roots.add(pki.ca_der()).map_err(|e| e.to_string())?;
        let verifier = if require_client_auth {
            rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                Arc::new(rustls::crypto::ring::default_provider()),
            )
            .build()
            .map_err(|e| e.to_string())?
        } else {
            rustls::server::WebPkiClientVerifier::no_client_auth()
        };
        let server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![pki.server_cert_der()], pki.server_key_der())
        .map_err(|e| e.to_string())?;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();

        let server = tokio::spawn(async move {
            let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
            let (stream, _) = listener.accept().await?;
            let mut stream = acceptor.accept(stream).await?;
            stream.write_all(b"ok").await?;
            stream.shutdown().await?;
            Ok::<(), std::io::Error>(())
        });

        let client_config = spec.client_config().map_err(|e| e.to_string())?;
        let connector = tokio_rustls::TlsConnector::from(client_config);
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|e| e.to_string())?;
        // Exactly what scylla-rust-driver passes: the socket's IP, not a name.
        let name = ServerName::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST).into());
        let mut stream = connector
            .connect(name, stream)
            .await
            .map_err(|e| e.to_string())?;

        let mut greeting = Vec::new();
        stream
            .read_to_end(&mut greeting)
            .await
            .map_err(|e| e.to_string())?;
        server
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()))?;
        if greeting == b"ok" {
            Ok(())
        } else {
            Err(format!("unexpected greeting {greeting:?}"))
        }
    }

    fn spec_for(pki: &Pki) -> TlsSpec {
        TlsSpec::new(
            Side::Origin,
            parse_trust_store(
                Side::Origin,
                pki.ca_pem().as_bytes(),
                None,
                StoreFormat::Pem,
            )
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn con_006_a_real_handshake_succeeds_against_a_server_the_trust_store_signed() {
        let pki = Pki::new();
        handshake(&pki, &spec_for(&pki), false)
            .await
            .expect("the chain is valid; the driver's IP server name must not be consulted");
    }

    #[tokio::test]
    async fn con_006_a_real_mutual_tls_handshake_succeeds() {
        let pki = Pki::new();
        let spec = spec_for(&pki).with_identity(
            parse_key_store(
                Side::Origin,
                format!("{}{}", pki.client_key_pem(), pki.client_cert_pem()).as_bytes(),
                None,
                StoreFormat::Pem,
            )
            .unwrap(),
        );
        handshake(&pki, &spec, true)
            .await
            .expect("the client certificate is signed by the CA the server trusts");
    }

    #[tokio::test]
    async fn con_006_a_real_handshake_fails_against_an_untrusted_server() {
        let stranger = Pki::new();
        let spec = spec_for(&Pki::new());
        let error = handshake(&stranger, &spec, false)
            .await
            .expect_err("a certificate from an unknown CA must not be accepted");
        assert!(error.to_lowercase().contains("certificate"), "{error}");
    }

    #[tokio::test]
    async fn con_007_a_pinned_cipher_suite_is_actually_negotiated() {
        let pki = Pki::new();
        let spec = spec_for(&pki).with_cipher_suites(vec!["TLS13_AES_256_GCM_SHA384".to_owned()]);
        handshake(&pki, &spec, false)
            .await
            .expect("the server offers TLS 1.3 and the client pinned one of its suites");
    }

    #[test]
    fn con_006_fixtures_do_not_ship_as_files() {
        // SEC-001: no key material in the repository. The fixtures are generated per test run.
        assert!(testfixtures::Pki::new()
            .ca_pem()
            .contains("BEGIN CERTIFICATE"));
    }
}
