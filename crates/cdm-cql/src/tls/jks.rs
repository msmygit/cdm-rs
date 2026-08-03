//! A pure-Rust JKS / JCEKS reader (`CON-006`, `ADR-0002`).
//!
//! Java CDM accepts `.jks` trust and key stores because the JVM reads them for free. cdm-rs has
//! no JVM, and nothing in the Rust ecosystem reads a Java keystore, so the format is implemented
//! here. It is small, stable since 1997 and entirely undocumented outside the OpenJDK sources
//! (`sun.security.provider.JavaKeyStore`, `sun.security.provider.KeyProtector`), which this
//! module follows.
//!
//! # Layout
//!
//! Everything is big-endian.
//!
//! ```text
//! u32   magic          0xfeedfeed (JKS) or 0xcececece (JCEKS)
//! u32   version        1 or 2
//! u32   entry count
//! entry*                one of:
//!   u32  tag = 1 (private key)   utf alias, i64 created,
//!                                u32 len + EncryptedPrivateKeyInfo, u32 chain len, cert*
//!   u32  tag = 2 (trusted cert)  utf alias, i64 created, cert
//!   u32  tag = 3 (secret key)    JCEKS only; skipped, cdm-rs has no use for one
//! [u8; 20] digest      SHA-1(password as UTF-16BE ++ "Mighty Aphrodite" ++ everything above)
//! ```
//!
//! A `cert` is the DER of an X.509 certificate, preceded in version 2 stores by its type name
//! (always `X.509`) as a modified-UTF-8 string.
//!
//! # Integrity
//!
//! The trailing digest is *keyed* with the store password, so verifying it proves both that the
//! file is intact and that the password is right. `CON-006` says a store must be readable, not
//! that it must be trusted blindly: when a password is supplied the digest is verified and a
//! mismatch is an error naming the likely cause. A trust store may legitimately be opened with no
//! password — `keytool` and the JVM both allow it — in which case there is nothing to verify with
//! and the digest is skipped.
//!
//! # Key protection
//!
//! Private keys are wrapped in a PKCS#8 `EncryptedPrivateKeyInfo` under Sun's own algorithm OID
//! `1.3.6.1.4.1.42.2.17.1.1`, whose "cipher" is a SHA-1 keystream: with `W(0) = salt`,
//! `W(i) = SHA-1(password_utf16be ++ W(i-1))`, the plaintext is the ciphertext XORed with the
//! concatenated `W(i)`. A trailing SHA-1 over `password_utf16be ++ plaintext` authenticates the
//! result, so a wrong password is detected rather than yielding garbage. The plaintext is a
//! PKCS#8 `PrivateKeyInfo`, which is exactly what rustls wants.

use cdm_core::{CdmError, Side};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha1::{Digest, Sha1};

use crate::errors::tls_error;
use crate::tls::Identity;

/// The JKS magic number.
const MAGIC_JKS: u32 = 0xfeed_feed;
/// The JCEKS magic number. The container format is identical; only JCEKS may hold secret keys.
const MAGIC_JCEKS: u32 = 0xcece_cece;
/// The salt that turns the trailing SHA-1 into a keyed digest. Yes, really.
const DIGEST_SALT: &[u8] = b"Mighty Aphrodite";
/// `1.3.6.1.4.1.42.2.17.1.1` — Sun's JKS key-protection algorithm — DER-encoded, without its
/// tag and length. This is the only algorithm `keytool` writes into a JKS private key entry.
const JKS_KEY_PROTECTOR_OID: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0x2a, 0x02, 0x11, 0x01, 0x01];
/// Length of a SHA-1 digest.
const SHA1_LEN: usize = 20;

/// One entry of a Java keystore.
#[derive(Debug, Clone)]
pub enum Entry {
    /// A trusted certificate entry, the only kind a trust store holds.
    TrustedCertificate {
        /// The alias it was stored under.
        alias: String,
        /// The certificate, DER-encoded.
        certificate: CertificateDer<'static>,
    },
    /// A private key entry: the protected key plus the certificate chain it belongs to.
    PrivateKey {
        /// The alias it was stored under.
        alias: String,
        /// The `EncryptedPrivateKeyInfo`, still protected.
        protected_key: Vec<u8>,
        /// The certificate chain, end-entity first.
        chain: Vec<CertificateDer<'static>>,
    },
}

impl Entry {
    /// The alias the entry was stored under.
    pub fn alias(&self) -> &str {
        match self {
            Self::TrustedCertificate { alias, .. } | Self::PrivateKey { alias, .. } => alias,
        }
    }
}

/// A parsed Java keystore.
#[derive(Debug, Clone)]
pub struct JavaKeyStore {
    entries: Vec<Entry>,
    jceks: bool,
}

impl JavaKeyStore {
    /// The entries, in file order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Whether the store was a JCEKS rather than a plain JKS.
    pub fn is_jceks(&self) -> bool {
        self.jceks
    }

    /// Every certificate in the store: trusted anchors and the chains of key entries.
    pub fn certificates(&self) -> Vec<CertificateDer<'static>> {
        self.entries
            .iter()
            .flat_map(|entry| match entry {
                Entry::TrustedCertificate { certificate, .. } => vec![certificate.clone()],
                Entry::PrivateKey { chain, .. } => chain.clone(),
            })
            .collect()
    }
}

/// Parses a JKS or JCEKS store, verifying the keyed digest when a password is supplied.
pub fn parse(side: Side, bytes: &[u8], password: Option<&str>) -> Result<JavaKeyStore, CdmError> {
    let mut reader = Reader::new(side, bytes);
    let magic = reader.u32("magic")?;
    let jceks = match magic {
        MAGIC_JKS => false,
        MAGIC_JCEKS => true,
        other => {
            let message = format!(
                "not a Java keystore: expected magic 0xfeedfeed or 0xcececece, found {other:#010x}"
            );
            return Err(tls_error(side, message));
        }
    };

    let version = reader.u32("version")?;
    if version != 1 && version != 2 {
        return Err(tls_error(
            side,
            format!("unsupported Java keystore version {version}; only 1 and 2 exist"),
        ));
    }

    if let Some(password) = password {
        verify_digest(side, bytes, password)?;
    }

    let count = reader.u32("entry count")?;
    let mut entries = Vec::new();
    for index in 0..count {
        let tag = reader.u32("entry tag")?;
        let alias = reader.utf(&format!("alias of entry {index}"))?;
        let _created = reader.u64("creation timestamp")?;
        match tag {
            1 => {
                let key_len = reader.u32("protected key length")? as usize;
                let protected_key = reader.bytes(key_len, "protected key")?.to_vec();
                let chain_len = reader.u32("certificate chain length")?;
                let mut chain = Vec::with_capacity(chain_len as usize);
                for _ in 0..chain_len {
                    chain.push(reader.certificate(version)?);
                }
                entries.push(Entry::PrivateKey {
                    alias,
                    protected_key,
                    chain,
                });
            }
            2 => {
                let certificate = reader.certificate(version)?;
                entries.push(Entry::TrustedCertificate { alias, certificate });
            }
            3 => {
                // A JCEKS secret key. cdm-rs has no use for one, and skipping it keeps a store
                // that happens to contain one readable for its certificates.
                let len = reader.u32("secret key length")? as usize;
                reader.bytes(len, "secret key")?;
            }
            other => {
                return Err(tls_error(
                    side,
                    format!("unknown Java keystore entry tag {other} for alias {alias}"),
                ))
            }
        }
    }

    Ok(JavaKeyStore { entries, jceks })
}

/// Every certificate in a JKS store (`CON-006`).
pub fn certificates(
    side: Side,
    bytes: &[u8],
    password: Option<&str>,
) -> Result<Vec<CertificateDer<'static>>, CdmError> {
    Ok(parse(side, bytes, password)?.certificates())
}

/// The first private key entry of a JKS store, decrypted, with its chain (`CON-006`).
pub fn identity(side: Side, bytes: &[u8], password: Option<&str>) -> Result<Identity, CdmError> {
    let store = parse(side, bytes, password)?;
    let entry = store
        .entries()
        .iter()
        .find_map(|entry| match entry {
            Entry::PrivateKey {
                protected_key,
                chain,
                ..
            } => Some((protected_key, chain)),
            Entry::TrustedCertificate { .. } => None,
        })
        .ok_or_else(|| {
            tls_error(
                side,
                "the JKS key store contains no private key entry, only trusted certificates",
            )
        })?;
    let (protected_key, chain) = entry;

    if chain.is_empty() {
        return Err(tls_error(
            side,
            "the JKS private key entry has no certificate chain",
        ));
    }
    let password = password.ok_or_else(|| {
        tls_error(
            side,
            "a JKS key store password is required to decrypt the private key",
        )
    })?;
    let pkcs8 = unprotect_key(side, protected_key, password)?;
    Ok(Identity::new(
        chain.clone(),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8)),
    ))
}

/// Verifies the trailing keyed SHA-1 (`CON-006`).
fn verify_digest(side: Side, bytes: &[u8], password: &str) -> Result<(), CdmError> {
    if bytes.len() < SHA1_LEN {
        return Err(tls_error(side, "the Java keystore is truncated"));
    }
    let split = bytes.len() - SHA1_LEN;
    let (body, expected) = bytes.split_at(split);
    let actual = keyed_digest(password, body);
    if actual.as_slice() != expected {
        return Err(tls_error(
            side,
            "the Java keystore integrity check failed; the store password is wrong or the file \
             has been altered",
        ));
    }
    Ok(())
}

/// `SHA-1(password_utf16be ++ "Mighty Aphrodite" ++ data)`, the JKS store digest.
pub(crate) fn keyed_digest(password: &str, data: &[u8]) -> [u8; SHA1_LEN] {
    let mut hasher = Sha1::new();
    hasher.update(utf16be(password));
    hasher.update(DIGEST_SALT);
    hasher.update(data);
    hasher.finalize().into()
}

/// Removes the Sun key protection from an `EncryptedPrivateKeyInfo`, yielding PKCS#8.
fn unprotect_key(side: Side, protected: &[u8], password: &str) -> Result<Vec<u8>, CdmError> {
    let encrypted = encrypted_private_key_data(side, protected)?;
    if encrypted.len() <= SHA1_LEN * 2 {
        return Err(tls_error(
            side,
            "the JKS protected key is too short to contain salt, ciphertext and digest",
        ));
    }
    let (salt, rest) = encrypted.split_at(SHA1_LEN);
    let (ciphertext, expected_digest) = rest.split_at(rest.len() - SHA1_LEN);

    let plaintext = keystream_xor(password, salt, ciphertext);

    let mut hasher = Sha1::new();
    hasher.update(utf16be(password));
    hasher.update(&plaintext);
    let actual: [u8; SHA1_LEN] = hasher.finalize().into();
    if actual.as_slice() != expected_digest {
        return Err(tls_error(
            side,
            "the JKS private key could not be decrypted; the key password is wrong",
        ));
    }
    Ok(plaintext)
}

/// XORs `data` with the SHA-1 keystream JKS derives from `password` and `salt`.
///
/// The transform is its own inverse, so the test fixture writer uses it to encrypt.
pub(crate) fn keystream_xor(password: &str, salt: &[u8], data: &[u8]) -> Vec<u8> {
    let password = utf16be(password);
    let mut out = Vec::with_capacity(data.len());
    let mut previous = salt.to_vec();
    while out.len() < data.len() {
        let mut hasher = Sha1::new();
        hasher.update(&password);
        hasher.update(&previous);
        let block: [u8; SHA1_LEN] = hasher.finalize().into();
        let take = SHA1_LEN.min(data.len() - out.len());
        let start = out.len();
        let plaintext = data.get(start..start + take).unwrap_or_default();
        for (byte, key) in plaintext.iter().zip(block.iter()) {
            out.push(byte ^ key);
        }
        previous = block.to_vec();
    }
    out
}

/// Extracts the `encryptedData` octet string of an `EncryptedPrivateKeyInfo`, checking that the
/// algorithm really is Sun's key protector.
fn encrypted_private_key_data(side: Side, der: &[u8]) -> Result<&[u8], CdmError> {
    let mut outer = Der::new(side, der);
    let mut sequence = Der::new(side, outer.expect(0x30, "EncryptedPrivateKeyInfo")?);
    let mut algorithm = Der::new(side, sequence.expect(0x30, "AlgorithmIdentifier")?);
    let oid = algorithm.expect(0x06, "algorithm OID")?;
    if oid != JKS_KEY_PROTECTOR_OID {
        return Err(tls_error(
            side,
            "the JKS private key uses an unsupported protection algorithm; only Sun's \
             1.3.6.1.4.1.42.2.17.1.1 is written by keytool",
        ));
    }
    sequence.expect(0x04, "encryptedData")
}

/// A password as Java encodes it for these two algorithms: UTF-16, big-endian, no BOM.
fn utf16be(password: &str) -> Vec<u8> {
    password.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

/// A cursor over the big-endian keystore body.
struct Reader<'a> {
    side: Side,
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(side: Side, bytes: &'a [u8]) -> Self {
        Self {
            side,
            bytes,
            offset: 0,
        }
    }

    fn bytes(&mut self, len: usize, what: &str) -> Result<&'a [u8], CdmError> {
        let end = self.offset.checked_add(len).ok_or_else(|| {
            tls_error(
                self.side,
                format!("the Java keystore declares an impossible length for {what}"),
            )
        })?;
        let slice = self.bytes.get(self.offset..end).ok_or_else(|| {
            tls_error(
                self.side,
                format!("the Java keystore ends in the middle of {what}"),
            )
        })?;
        self.offset = end;
        Ok(slice)
    }

    fn u32(&mut self, what: &str) -> Result<u32, CdmError> {
        let bytes = self.bytes(4, what)?;
        let array: [u8; 4] = bytes.try_into().map_err(|_| {
            tls_error(
                self.side,
                format!("the Java keystore is truncated at {what}"),
            )
        })?;
        Ok(u32::from_be_bytes(array))
    }

    fn u64(&mut self, what: &str) -> Result<u64, CdmError> {
        let bytes = self.bytes(8, what)?;
        let array: [u8; 8] = bytes.try_into().map_err(|_| {
            tls_error(
                self.side,
                format!("the Java keystore is truncated at {what}"),
            )
        })?;
        Ok(u64::from_be_bytes(array))
    }

    fn u16(&mut self, what: &str) -> Result<u16, CdmError> {
        let bytes = self.bytes(2, what)?;
        let array: [u8; 2] = bytes.try_into().map_err(|_| {
            tls_error(
                self.side,
                format!("the Java keystore is truncated at {what}"),
            )
        })?;
        Ok(u16::from_be_bytes(array))
    }

    /// A Java modified-UTF-8 string: a `u16` length followed by that many bytes.
    fn utf(&mut self, what: &str) -> Result<String, CdmError> {
        let len = self.u16(what)? as usize;
        let bytes = self.bytes(len, what)?;
        modified_utf8(self.side, bytes, what)
    }

    /// A certificate: its type name in version-2 stores, then a length-prefixed DER blob.
    fn certificate(&mut self, version: u32) -> Result<CertificateDer<'static>, CdmError> {
        if version == 2 {
            let kind = self.utf("certificate type")?;
            if kind != "X.509" && kind != "X509" {
                return Err(tls_error(
                    self.side,
                    format!("unsupported certificate type {kind} in the Java keystore"),
                ));
            }
        }
        let len = self.u32("certificate length")? as usize;
        Ok(CertificateDer::from(
            self.bytes(len, "certificate")?.to_vec(),
        ))
    }
}

/// Decodes Java's modified UTF-8, which differs from UTF-8 in encoding `U+0000` as two bytes and
/// in having no four-byte form — a supplementary character arrives as a surrogate pair.
fn modified_utf8(side: Side, bytes: &[u8], what: &str) -> Result<String, CdmError> {
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let malformed = || tls_error(side, format!("malformed modified UTF-8 in the {what}"));
    while index < bytes.len() {
        let first = *bytes.get(index).ok_or_else(malformed)?;
        match first {
            0x00..=0x7f => {
                units.push(u16::from(first));
                index += 1;
            }
            0xc0..=0xdf => {
                let second = *bytes.get(index + 1).ok_or_else(malformed)?;
                units.push((u16::from(first & 0x1f) << 6) | u16::from(second & 0x3f));
                index += 2;
            }
            0xe0..=0xef => {
                let second = *bytes.get(index + 1).ok_or_else(malformed)?;
                let third = *bytes.get(index + 2).ok_or_else(malformed)?;
                units.push(
                    (u16::from(first & 0x0f) << 12)
                        | (u16::from(second & 0x3f) << 6)
                        | u16::from(third & 0x3f),
                );
                index += 3;
            }
            _ => return Err(malformed()),
        }
    }
    String::from_utf16(&units).map_err(|_| malformed())
}

/// The smallest DER reader that will do: enough to walk an `EncryptedPrivateKeyInfo`.
///
/// A general ASN.1 crate would be a heavier dependency than the three tag reads this needs.
struct Der<'a> {
    side: Side,
    bytes: &'a [u8],
}

impl<'a> Der<'a> {
    fn new(side: Side, bytes: &'a [u8]) -> Self {
        Self { side, bytes }
    }

    /// Reads the next TLV, requiring the given tag, and returns its contents.
    fn expect(&mut self, tag: u8, what: &str) -> Result<&'a [u8], CdmError> {
        let err = || tls_error(self.side, format!("malformed DER while reading {what}"));
        let actual = *self.bytes.first().ok_or_else(err)?;
        if actual != tag {
            return Err(tls_error(
                self.side,
                format!("expected DER tag {tag:#04x} for {what}, found {actual:#04x}"),
            ));
        }
        let length_byte = *self.bytes.get(1).ok_or_else(err)?;
        let (length, header) = if length_byte < 0x80 {
            (usize::from(length_byte), 2)
        } else {
            let count = usize::from(length_byte & 0x7f);
            if count == 0 || count > 4 {
                return Err(err());
            }
            let mut length = 0usize;
            for index in 0..count {
                let byte = *self.bytes.get(2 + index).ok_or_else(err)?;
                length = (length << 8) | usize::from(byte);
            }
            (length, 2 + count)
        };
        let end = header.checked_add(length).ok_or_else(err)?;
        let contents = self.bytes.get(header..end).ok_or_else(err)?;
        self.bytes = self.bytes.get(end..).unwrap_or_default();
        Ok(contents)
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
    use crate::testfixtures::{jks_writer, Pki};

    #[test]
    fn con_006_a_jks_trust_store_round_trips() {
        let pki = Pki::new();
        let store = pki.truststore_jks(pki.password());
        let parsed = parse(Side::Origin, &store, Some(pki.password())).unwrap();
        assert!(!parsed.is_jceks());
        assert_eq!(parsed.entries().len(), 1);
        assert_eq!(parsed.entries()[0].alias(), "cdmca");
        assert_eq!(parsed.certificates()[0].as_ref(), pki.ca_der().as_ref());
    }

    #[test]
    fn con_006_a_jks_key_store_decrypts_to_pkcs8() {
        let pki = Pki::new();
        let identity = identity(
            Side::Origin,
            &pki.keystore_jks(pki.password()),
            Some(pki.password()),
        )
        .unwrap();
        assert_eq!(identity.chain()[0].as_ref(), pki.client_cert_der().as_ref());
        assert_eq!(
            identity.key().secret_der(),
            pki.client_key_der().secret_der()
        );
    }

    #[test]
    fn con_006_a_wrong_jks_store_password_fails_the_integrity_check() {
        let pki = Pki::new();
        let err = parse(
            Side::Origin,
            &pki.truststore_jks(pki.password()),
            Some(&format!("not-{}", pki.password())),
        )
        .unwrap_err();
        assert!(err.to_string().contains("integrity check failed"), "{err}");
    }

    #[test]
    fn con_006_a_jks_store_may_be_read_without_a_password() {
        // `keytool` and the JVM both allow it; there is then simply nothing to verify against.
        let pki = Pki::new();
        let parsed = parse(Side::Origin, &pki.truststore_jks(pki.password()), None).unwrap();
        assert_eq!(parsed.entries().len(), 1);
    }

    #[test]
    fn con_006_a_jks_key_store_needs_a_password_to_yield_its_key() {
        let pki = Pki::new();
        let err = identity(Side::Origin, &pki.keystore_jks(pki.password()), None).unwrap_err();
        assert!(err.to_string().contains("password is required"), "{err}");
    }

    #[test]
    fn con_006_a_jceks_magic_is_accepted() {
        let pki = Pki::new();
        let store = jks_writer::Writer::new(true)
            .trusted_certificate("cdmca", pki.ca_der().as_ref())
            .finish(pki.password());
        let parsed = parse(Side::Origin, &store, Some(pki.password())).unwrap();
        assert!(parsed.is_jceks());
    }

    #[test]
    fn con_006_a_file_that_is_not_a_keystore_says_which_magic_it_found() {
        let err = parse(
            Side::Origin,
            &[0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 1],
            Some("x"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("0xdeadbeef"), "{err}");
    }

    #[test]
    fn con_006_a_truncated_keystore_is_an_error_not_a_panic() {
        let pki = Pki::new();
        let store = pki.truststore_jks(pki.password());
        for cut in [4, 8, 12, store.len() / 2] {
            let err = parse(Side::Origin, &store[..cut], None).unwrap_err();
            assert_eq!(err.kind(), cdm_core::ErrorKind::Tls);
        }
    }

    #[test]
    fn con_006_an_unknown_entry_tag_is_rejected() {
        let pw = crate::testfixtures::generated_password();
        let store = jks_writer::Writer::new(false)
            .raw_entry(99, "weird", &[])
            .finish(pw.as_str());
        let err = parse(Side::Origin, &store, Some(pw.as_str())).unwrap_err();
        assert!(
            err.to_string().contains("unknown Java keystore entry tag"),
            "{err}"
        );
    }

    #[test]
    fn con_006_a_jceks_secret_key_entry_is_skipped_not_fatal() {
        let pki = Pki::new();
        let store = jks_writer::Writer::new(true)
            .secret_key("apikey", &[1, 2, 3, 4])
            .trusted_certificate("cdmca", pki.ca_der().as_ref())
            .finish(pki.password());
        let parsed = parse(Side::Origin, &store, Some(pki.password())).unwrap();
        assert_eq!(parsed.certificates().len(), 1);
    }

    #[test]
    fn con_006_an_unsupported_store_version_is_rejected() {
        let pw = crate::testfixtures::generated_password();
        let mut store = jks_writer::Writer::new(false).finish(pw.as_str());
        store[7] = 9;
        let err = parse(Side::Origin, &store, Some(pw.as_str())).unwrap_err();
        assert!(err.to_string().contains("version 9"), "{err}");
    }

    #[test]
    fn con_006_the_key_protection_stream_is_its_own_inverse() {
        let pw = crate::testfixtures::generated_password();
        let plaintext = b"a PKCS#8 blob, more or less, of some length beyond twenty bytes";
        let salt = crate::testfixtures::generated_salt::<SHA1_LEN>();
        let ciphertext = keystream_xor(pw.as_str(), &salt, plaintext);
        assert_ne!(ciphertext, plaintext.to_vec());
        assert_eq!(keystream_xor(pw.as_str(), &salt, &ciphertext), plaintext);
    }

    #[test]
    fn con_006_a_wrong_key_password_is_detected_by_the_trailing_digest() {
        let pki = Pki::new();
        // Store password and key password differ, so the store digest passes and the key does not.
        // The two must not be equal or the test proves nothing; `Pki` hands out distinct values.
        let store_password = pki.password();
        let key_password = pki.other_password();
        assert_ne!(store_password, key_password);

        let store = jks_writer::Writer::new(false)
            .private_key(
                "client",
                pki.client_key_der().secret_der(),
                &[pki.client_cert_der().to_vec()],
                key_password,
            )
            .finish(store_password);
        let err = identity(Side::Origin, &store, Some(store_password)).unwrap_err();
        assert!(err.to_string().contains("key password is wrong"), "{err}");
    }

    #[test]
    fn con_006_a_key_protected_by_another_algorithm_is_named_as_such() {
        // A PBES2-protected key, which keytool never writes for JKS but a hand-built file might.
        let mut der = vec![0x30, 0x0d, 0x30, 0x0b, 0x06, 0x09];
        der.extend_from_slice(&[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x05, 0x0c]);
        let err = encrypted_private_key_data(Side::Origin, &der).unwrap_err();
        assert!(
            err.to_string().contains("unsupported protection algorithm"),
            "{err}"
        );
    }

    #[test]
    fn con_006_modified_utf8_aliases_are_decoded() {
        assert_eq!(
            modified_utf8(Side::Origin, b"plain", "alias").unwrap(),
            "plain"
        );
        // U+0000 encoded the modified way, and a three-byte character.
        assert_eq!(
            modified_utf8(Side::Origin, &[0xc0, 0x80, 0xe2, 0x82, 0xac], "alias").unwrap(),
            "\u{0}\u{20ac}"
        );
        assert!(modified_utf8(Side::Origin, &[0xf0, 0x9f, 0x92, 0xa9], "alias").is_err());
        assert!(modified_utf8(Side::Origin, &[0xe2, 0x82], "alias").is_err());
    }

    #[test]
    fn con_006_the_der_reader_rejects_nonsense() {
        let mut der = Der::new(Side::Origin, &[0x31, 0x01, 0x00]);
        assert!(der.expect(0x30, "sequence").is_err());
        let mut empty = Der::new(Side::Origin, &[]);
        assert!(empty.expect(0x30, "sequence").is_err());
        // A long-form length that runs past the end of the buffer.
        let mut long = Der::new(Side::Origin, &[0x30, 0x84, 0xff, 0xff, 0xff, 0xff]);
        assert!(long.expect(0x30, "sequence").is_err());
    }

    #[test]
    fn con_006_long_form_der_lengths_are_read() {
        let mut body = vec![0x04, 0x82, 0x01, 0x00];
        body.extend(std::iter::repeat_n(0xabu8, 256));
        let mut der = Der::new(Side::Origin, &body);
        assert_eq!(der.expect(0x04, "octet string").unwrap().len(), 256);
    }
}
