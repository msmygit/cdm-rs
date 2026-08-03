//! The PEM reader (`CON-006`).
//!
//! PEM is the format the Astra bundle uses for `ca.crt`, `cert` and `key` (`CON-020`), and the
//! one operators reach for when they are not coming from a Java deployment. A single file may
//! hold both a chain and a key, in either order, which is why [`identity`] scans the whole file
//! rather than assuming a layout.

use cdm_core::{CdmError, Side};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::errors::{tls_error, tls_error_from};
use crate::tls::Identity;

/// Every certificate in a PEM file, in file order.
pub fn certificates(side: Side, bytes: &[u8]) -> Result<Vec<CertificateDer<'static>>, CdmError> {
    let mut reader = std::io::BufReader::new(bytes);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| tls_error_from(side, "the PEM file contains a malformed certificate", e))
}

/// The first private key in a PEM file, in PKCS#8, PKCS#1 or SEC1 encoding.
pub fn private_key(side: Side, bytes: &[u8]) -> Result<PrivateKeyDer<'static>, CdmError> {
    let mut reader = std::io::BufReader::new(bytes);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|e| tls_error_from(side, "the PEM file contains a malformed private key", e))?
        .ok_or_else(|| tls_error(side, "the PEM file contains no private key"))
}

/// A certificate chain and its key, read from one PEM file.
///
/// The end-entity certificate must come first, as PEM has no other way to say which certificate
/// the key belongs to; that is also the order `openssl` and every documented Astra bundle emit.
pub fn identity(side: Side, bytes: &[u8]) -> Result<Identity, CdmError> {
    let chain = certificates(side, bytes)?;
    if chain.is_empty() {
        return Err(tls_error(side, "the PEM file contains no certificate"));
    }
    let key = private_key(side, bytes)?;
    Ok(Identity::new(chain, key))
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
    fn con_006_a_pem_file_may_hold_a_chain_and_a_key_in_either_order() {
        let pki = Pki::new();
        let key_first = format!("{}{}", pki.client_key_pem(), pki.client_cert_pem());
        let cert_first = format!("{}{}", pki.client_cert_pem(), pki.client_key_pem());

        for text in [key_first, cert_first] {
            let identity = identity(Side::Origin, text.as_bytes()).unwrap();
            assert_eq!(identity.chain().len(), 1);
            assert_eq!(
                identity.key().secret_der(),
                pki.client_key_der().secret_der()
            );
        }
    }

    #[test]
    fn con_006_a_pem_chain_keeps_file_order() {
        let pki = Pki::new();
        let bundle = format!("{}{}", pki.client_cert_pem(), pki.ca_pem());
        let certs = certificates(Side::Origin, bundle.as_bytes()).unwrap();
        assert_eq!(certs.len(), 2);
        assert_eq!(certs[0].as_ref(), pki.client_cert_der().as_ref());
        assert_eq!(certs[1].as_ref(), pki.ca_der().as_ref());
    }

    #[test]
    fn con_006_a_pem_file_without_a_key_is_an_error() {
        let pki = Pki::new();
        let err = identity(Side::Origin, pki.client_cert_pem().as_bytes()).unwrap_err();
        assert!(err.to_string().contains("no private key"));
    }

    #[test]
    fn con_006_a_pem_file_without_a_certificate_is_an_error() {
        let pki = Pki::new();
        let err = identity(Side::Origin, pki.client_key_pem().as_bytes()).unwrap_err();
        assert!(err.to_string().contains("no certificate"));
    }

    #[test]
    fn con_006_garbage_is_not_mistaken_for_pem() {
        assert!(certificates(Side::Origin, b"hello").unwrap().is_empty());
        assert!(private_key(Side::Origin, b"hello").is_err());
    }
}
