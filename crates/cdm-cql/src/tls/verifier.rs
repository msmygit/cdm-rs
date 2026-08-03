//! The server-certificate verifier cdm-rs installs, and why it is not rustls' default
//! (`CON-006`, `CON-022`).
//!
//! # The problem
//!
//! `scylla-rust-driver` 1.7 dials nodes by address and hands rustls
//! `ServerName::IpAddress(node_address.ip())` (`network/connection.rs`, line 1503). rustls' stock
//! verifier then requires the server certificate to carry that IP in a subject-alternative name.
//! Cassandra deployments do not issue such certificates: `keytool` generates them with a DNS name
//! or with no SAN at all, and JSSE — which is what Java CDM uses — checks the name only when the
//! cluster sets `require_endpoint_verification: true`, which is off by default in
//! `cassandra.yaml`. With the stock verifier, every TLS-enabled cluster that works under Java CDM
//! would fail under cdm-rs with `CertNotValidForName`, which is neither parity nor a security
//! improvement — it is an outage.
//!
//! # What this verifier does instead
//!
//! * The **chain** is always verified against the configured trust store, using
//!   [`rustls::client::verify_server_cert_signed_by_trust_anchor`]. This is the part that
//!   establishes that the peer is who the operator's CA says it is, and it is never skipped.
//! * Signatures are always verified, by delegating to rustls' own TLS 1.2 and 1.3 routines.
//! * The **name** is checked against
//!   [`ChainVerifier::expected_name`] when the operator configured one — Astra's endpoint, or a
//!   cluster that does require endpoint verification — and against nothing otherwise. The
//!   driver's synthetic IP name is never used, because it describes the socket, not the peer's
//!   identity.
//!
//! Revocation is not checked, which matches both rustls' default verifier without a CRL and
//! JSSE's default.

use std::fmt;
use std::sync::Arc;

use cdm_core::{CdmError, Side};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{verify_server_cert_signed_by_trust_anchor, verify_server_name};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{DigitallySignedStruct, Error as RustlsError, RootCertStore, SignatureScheme};

use crate::errors::tls_error_from;

/// Verifies the chain against a configured trust store, and the name only when asked.
pub struct ChainVerifier {
    roots: RootCertStore,
    provider: Arc<CryptoProvider>,
    expected_name: Option<ServerName<'static>>,
    side: Side,
}

impl fmt::Debug for ChainVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChainVerifier")
            .field("side", &self.side)
            .field("anchors", &self.roots.len())
            .field("expected_name", &self.expected_name)
            .field("cipher_suites", &self.provider.cipher_suites.len())
            .finish()
    }
}

impl ChainVerifier {
    /// Builds a verifier.
    ///
    /// `expected_hostname` is the name the server certificate must be issued for; `None` verifies
    /// the chain only, which is the Java-parity default.
    pub fn new(
        side: Side,
        roots: RootCertStore,
        provider: Arc<CryptoProvider>,
        expected_hostname: Option<String>,
    ) -> Result<Self, CdmError> {
        let expected_name = match expected_hostname {
            Some(hostname) => Some(
                ServerName::try_from(hostname.clone())
                    .map_err(|e| {
                        tls_error_from(
                            side,
                            format!("{hostname} is not a valid TLS server name"),
                            e,
                        )
                    })?
                    .to_owned(),
            ),
            None => None,
        };
        Ok(Self {
            roots,
            provider,
            expected_name,
            side,
        })
    }

    /// The name the peer's certificate must be issued for, if any.
    pub fn expected_name(&self) -> Option<&ServerName<'static>> {
        self.expected_name.as_ref()
    }
}

impl ServerCertVerifier for ChainVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let certificate = ParsedCertificate::try_from(end_entity)?;
        verify_server_cert_signed_by_trust_anchor(
            &certificate,
            &self.roots,
            intermediates,
            now,
            self.provider.signature_verification_algorithms.all,
        )?;
        if let Some(expected) = &self.expected_name {
            verify_server_name(&certificate, expected)?;
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
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
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::testfixtures::Pki;

    fn verifier(pki: &Pki, expected: Option<&str>) -> ChainVerifier {
        let mut roots = RootCertStore::empty();
        roots.add(pki.ca_der()).unwrap();
        ChainVerifier::new(
            Side::Origin,
            roots,
            Arc::new(rustls::crypto::ring::default_provider()),
            expected.map(str::to_owned),
        )
        .unwrap()
    }

    /// The name the driver would pass: the socket's IP, which no cluster certificate carries.
    fn drivers_name() -> ServerName<'static> {
        ServerName::IpAddress(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)).into())
    }

    #[test]
    fn con_006_a_certificate_from_the_configured_ca_is_accepted_despite_the_drivers_ip_name() {
        let pki = Pki::new();
        let verifier = verifier(&pki, None);
        verifier
            .verify_server_cert(
                &pki.server_cert_der(),
                &[],
                &drivers_name(),
                &[],
                UnixTime::now(),
            )
            .expect("the chain is valid, and the IP name must not be consulted");
    }

    #[test]
    fn con_006_a_certificate_from_another_ca_is_rejected() {
        let pki = Pki::new();
        let stranger = Pki::new();
        let verifier = verifier(&pki, None);
        let err = verifier
            .verify_server_cert(
                &stranger.server_cert_der(),
                &[],
                &drivers_name(),
                &[],
                UnixTime::now(),
            )
            .unwrap_err();
        assert!(matches!(err, RustlsError::InvalidCertificate(_)), "{err}");
    }

    #[test]
    fn con_006_a_configured_hostname_is_enforced() {
        let pki = Pki::new();
        let verifier = verifier(&pki, Some("cdm-node.example.invalid"));
        verifier
            .verify_server_cert(
                &pki.server_cert_der(),
                &[],
                &drivers_name(),
                &[],
                UnixTime::now(),
            )
            .expect("the certificate is issued for this name");

        let wrong = verifier_for_name(&pki, "not-the-node.example.invalid");
        assert!(wrong
            .verify_server_cert(
                &pki.server_cert_der(),
                &[],
                &drivers_name(),
                &[],
                UnixTime::now()
            )
            .is_err());
    }

    fn verifier_for_name(pki: &Pki, name: &str) -> ChainVerifier {
        verifier(pki, Some(name))
    }

    #[test]
    fn con_006_an_invalid_expected_hostname_is_a_tls_error() {
        let pki = Pki::new();
        let mut roots = RootCertStore::empty();
        roots.add(pki.ca_der()).unwrap();
        let err = ChainVerifier::new(
            Side::Origin,
            roots,
            Arc::new(rustls::crypto::ring::default_provider()),
            Some("not a host name".to_owned()),
        )
        .unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Tls);
    }

    #[test]
    fn con_006_the_verifier_advertises_the_providers_schemes() {
        let pki = Pki::new();
        let verifier = verifier(&pki, None);
        assert!(!verifier.supported_verify_schemes().is_empty());
        assert!(verifier.expected_name().is_none());
        assert!(format!("{verifier:?}").contains("anchors"));
    }
}
