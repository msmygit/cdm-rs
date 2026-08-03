//! The PKCS#12 reader (`CON-006`).
//!
//! Delegates to [`p12_keystore`], which is pure Rust and understands both the legacy PBES1
//! schemes the JVM wrote for years (`PBEWithSHA1And3KeyTripleDES`, 40-bit RC2) and the
//! `AES-256`/`HMAC-SHA256` scheme it writes today. Two import policies are used deliberately:
//!
//! * a **trust store** is read with [`Pkcs12ImportPolicy::Raw`], because a store exported by
//!   `keytool -importcert` may or may not carry Oracle's `trustedKeyUsage` bag attribute, and a
//!   certificate an operator put in a trust store is one they meant to trust either way;
//! * a **key store** is read with [`Pkcs12ImportPolicy::Relaxed`], which links each key to its
//!   certificate chain by `localKeyId` but still yields the key when the archive omits the link.

use cdm_core::{CdmError, Side};
use p12_keystore::{KeyStore, KeyStoreEntry, Pkcs12ImportPolicy};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::errors::{tls_error, tls_error_from};
use crate::tls::Identity;

/// Every certificate in a PKCS#12 archive, alias order.
pub fn certificates(
    side: Side,
    bytes: &[u8],
    password: &str,
) -> Result<Vec<CertificateDer<'static>>, CdmError> {
    let store = open(side, bytes, password, Pkcs12ImportPolicy::Raw)?;
    let mut certificates = Vec::new();
    for (_alias, entry) in store.entries() {
        match entry {
            KeyStoreEntry::Certificate(certificate) => {
                certificates.push(CertificateDer::from(certificate.as_der().to_vec()));
            }
            KeyStoreEntry::PrivateKeyChain(chain) => {
                certificates.extend(
                    chain
                        .certs()
                        .iter()
                        .map(|c| CertificateDer::from(c.as_der().to_vec())),
                );
            }
            KeyStoreEntry::Secret(_) => {}
        }
    }
    Ok(certificates)
}

/// The first private key in a PKCS#12 archive, with the chain it belongs to.
pub fn identity(side: Side, bytes: &[u8], password: &str) -> Result<Identity, CdmError> {
    let store = open(side, bytes, password, Pkcs12ImportPolicy::Relaxed)?;
    let (alias, chain) = store
        .private_key_chain()
        .ok_or_else(|| tls_error(side, "the PKCS#12 key store contains no private key entry"))?;
    if chain.certs().is_empty() {
        return Err(tls_error(
            side,
            format!("the PKCS#12 entry {alias} has a private key but no certificate chain"),
        ));
    }
    let certificates = chain
        .certs()
        .iter()
        .map(|c| CertificateDer::from(c.as_der().to_vec()))
        .collect();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(chain.key().as_der().to_vec()));
    Ok(Identity::new(certificates, key))
}

fn open(
    side: Side,
    bytes: &[u8],
    password: &str,
    policy: Pkcs12ImportPolicy,
) -> Result<KeyStore, CdmError> {
    KeyStore::from_pkcs12(bytes, password, policy).map_err(|e| {
        tls_error_from(
            side,
            "the PKCS#12 archive could not be opened; check the store password",
            e,
        )
    })
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
    use crate::testfixtures::Pki;

    #[test]
    fn con_006_a_pkcs12_trust_store_yields_its_anchors() {
        let pki = Pki::new();
        let certificates = certificates(
            Side::Origin,
            &pki.truststore_pkcs12(pki.password()),
            pki.password(),
        )
        .unwrap();
        assert_eq!(certificates.len(), 1);
        assert_eq!(certificates[0].as_ref(), pki.ca_der().as_ref());
    }

    #[test]
    fn con_006_a_pkcs12_key_store_yields_key_and_chain() {
        let pki = Pki::new();
        let identity = identity(
            Side::Origin,
            &pki.keystore_pkcs12(pki.password()),
            pki.password(),
        )
        .unwrap();
        assert_eq!(identity.chain()[0].as_ref(), pki.client_cert_der().as_ref());
        assert_eq!(
            identity.key().secret_der(),
            pki.client_key_der().secret_der()
        );
    }

    #[test]
    fn con_006_a_wrong_pkcs12_password_says_so() {
        let pki = Pki::new();
        let err = identity(
            Side::Origin,
            &pki.keystore_pkcs12(pki.password()),
            &format!("not-{}", pki.password()),
        )
        .unwrap_err();
        assert!(err.to_string().contains("store password"), "{err}");
    }

    #[test]
    fn con_006_a_pkcs12_trust_store_has_no_identity() {
        let pki = Pki::new();
        let err = identity(
            Side::Origin,
            &pki.truststore_pkcs12(pki.password()),
            pki.password(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no private key entry"), "{err}");
    }
}
