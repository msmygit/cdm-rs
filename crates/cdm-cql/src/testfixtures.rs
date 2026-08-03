//! Test-only fixtures: a throwaway PKI, keystore writers and Astra bundle builders.
//!
//! Nothing here is compiled into a release build, and nothing here is committed as a file:
//! certificates, keys and bundles are generated per test run, so no key material — real or
//! sample — ever enters the repository (`SEC-001`). The writers double as evidence that the
//! readers are right: a store written the way `keytool` writes one and read back byte-identical
//! exercises the format, not the reader's own assumptions.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// The DNS name every generated server certificate carries.
pub(crate) const SERVER_NAME: &str = "cdm-node.example.invalid";
/// The DNS name every generated client certificate carries.
pub(crate) const CLIENT_NAME: &str = "cdm-client.example.invalid";

/// A keystore password, different on every call.
///
/// Deliberately not a literal. These protect stores that are generated per test run and never
/// leave the process, so the value itself is irrelevant — but a password literal in the tree is
/// indistinguishable, both to a reader and to a scanner, from a real one that leaked. Generating
/// it also proves the readers genuinely use the password rather than ignoring it: a fixed value
/// cannot tell those two cases apart.
pub(crate) fn generated_password() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "pw-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A salt that differs on every call.
///
/// `keytool` randomises this, so a constant would let a reader that quietly ignored the salt still
/// pass — and it reads to a scanner as an embedded cryptographic constant.
pub(crate) fn generated_salt<const N: usize>() -> [u8; N] {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let seed = NEXT
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(u64::from(std::process::id()));
    let mut salt = [0u8; N];
    for (index, byte) in salt.iter_mut().enumerate() {
        // A counter-derived pattern: no randomness is needed, only that it varies.
        *byte = seed
            .rotate_left(u32::try_from(index).unwrap_or(0) % 64)
            .to_le_bytes()[index % 8];
    }
    salt
}

/// A self-signed CA plus a server and a client certificate issued by it.
pub(crate) struct Pki {
    ca_cert: rcgen::Certificate,
    server_cert: rcgen::Certificate,
    server_key: rcgen::KeyPair,
    client_cert: rcgen::Certificate,
    client_key: rcgen::KeyPair,
    password: String,
    other_password: String,
}

impl Pki {
    /// Generates a fresh, unrelated PKI. Two calls share nothing, which is how the "signed by
    /// another CA" tests get a stranger to present.
    /// The password protecting every store this instance writes.
    pub(crate) fn password(&self) -> &str {
        &self.password
    }

    /// A second, different password, for the tests that must show the reader tells them apart.
    pub(crate) fn other_password(&self) -> &str {
        &self.other_password
    }

    pub(crate) fn new() -> Self {
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let mut ca_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "cdm-rs test CA");
        ca_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let issuer = rcgen::Issuer::new(ca_params, ca_key);

        let server_key = rcgen::KeyPair::generate().unwrap();
        let server_cert = rcgen::CertificateParams::new(vec![SERVER_NAME.to_owned()])
            .unwrap()
            .signed_by(&server_key, &issuer)
            .unwrap();

        let client_key = rcgen::KeyPair::generate().unwrap();
        let client_cert = rcgen::CertificateParams::new(vec![CLIENT_NAME.to_owned()])
            .unwrap()
            .signed_by(&client_key, &issuer)
            .unwrap();

        Self {
            ca_cert,
            server_cert,
            server_key,
            client_cert,
            client_key,
            password: generated_password(),
            other_password: generated_password(),
        }
    }

    pub(crate) fn ca_pem(&self) -> String {
        self.ca_cert.pem()
    }

    pub(crate) fn ca_der(&self) -> CertificateDer<'static> {
        self.ca_cert.der().clone()
    }

    pub(crate) fn server_cert_der(&self) -> CertificateDer<'static> {
        self.server_cert.der().clone()
    }

    pub(crate) fn server_key_der(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.server_key.serialize_der()))
    }

    pub(crate) fn client_cert_der(&self) -> CertificateDer<'static> {
        self.client_cert.der().clone()
    }

    pub(crate) fn client_cert_pem(&self) -> String {
        self.client_cert.pem()
    }

    pub(crate) fn client_key_pem(&self) -> String {
        self.client_key.serialize_pem()
    }

    pub(crate) fn client_key_der(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.client_key.serialize_der()))
    }

    /// A PKCS#12 trust store holding the CA certificate.
    pub(crate) fn truststore_pkcs12(&self, password: &str) -> Vec<u8> {
        let mut store = p12_keystore::KeyStore::new();
        store.add_entry(
            "cdmca",
            p12_keystore::KeyStoreEntry::Certificate(
                p12_keystore::Certificate::from_der(self.ca_der().as_ref()).unwrap(),
            ),
        );
        store.writer(password).write().unwrap()
    }

    /// A PKCS#12 key store holding the client certificate and its key.
    pub(crate) fn keystore_pkcs12(&self, password: &str) -> Vec<u8> {
        let chain = p12_keystore::PrivateKeyChain::new(
            vec![1, 2, 3, 4],
            p12_keystore::PrivateKey::from_der(self.client_key_der().secret_der()).unwrap(),
            vec![p12_keystore::Certificate::from_der(self.client_cert_der().as_ref()).unwrap()],
        );
        let mut store = p12_keystore::KeyStore::new();
        store.add_entry(
            "client",
            p12_keystore::KeyStoreEntry::PrivateKeyChain(chain),
        );
        store.writer(password).write().unwrap()
    }

    /// A JKS trust store holding the CA certificate, exactly as `keytool -importcert` writes one.
    pub(crate) fn truststore_jks(&self, password: &str) -> Vec<u8> {
        jks_writer::Writer::new(false)
            .trusted_certificate("cdmca", self.ca_der().as_ref())
            .finish(password)
    }

    /// A JKS key store holding the client certificate and its key, protected with the same
    /// password as the store — which is what `keytool -genkeypair` does when not told otherwise.
    pub(crate) fn keystore_jks(&self, password: &str) -> Vec<u8> {
        jks_writer::Writer::new(false)
            .private_key(
                "client",
                self.client_key_der().secret_der(),
                &[self.client_cert_der().to_vec()],
                password,
            )
            .finish(password)
    }
}

/// Builders for Astra fixtures: bundles, metadata responses and DevOps API payloads.
pub(crate) mod astra {
    use std::io::Write;

    use super::Pki;

    /// The metadata service host every fixture bundle names.
    pub(crate) const METADATA_HOST: &str = "metadata.example.invalid";
    /// The metadata service port every fixture bundle names.
    pub(crate) const METADATA_PORT: u16 = 29080;
    /// The CQL host `cqlshrc` names, which is not the metadata host.
    pub(crate) const CQL_HOST: &str = "cql.example.invalid";
    /// The CQL port `cqlshrc` names, which is not the metadata port (`CON-026`).
    pub(crate) const CQL_PORT: u16 = 29042;

    /// Builds a secure-connect-bundle zip in memory.
    pub(crate) struct BundleBuilder {
        members: Vec<(String, Vec<u8>)>,
        prefix: String,
    }

    impl BundleBuilder {
        /// A bundle with the four required members plus `cqlshrc`, as Astra ships one.
        pub(crate) fn new(pki: &Pki) -> Self {
            let config = format!(
                r#"{{"host":"{METADATA_HOST}","port":{METADATA_PORT},"keyspace":"cdm","localDC":"us-east1"}}"#
            );
            let cqlshrc = format!(
                "[connection]\nhostname = {CQL_HOST}\nport = {CQL_PORT}\nfactory = cqlshlib.ssl.ssl_transport_factory\n"
            );
            Self {
                members: vec![
                    ("config.json".to_owned(), config.into_bytes()),
                    ("ca.crt".to_owned(), pki.ca_pem().into_bytes()),
                    ("cert".to_owned(), pki.client_cert_pem().into_bytes()),
                    ("key".to_owned(), pki.client_key_pem().into_bytes()),
                    ("cqlshrc".to_owned(), cqlshrc.into_bytes()),
                ],
                prefix: String::new(),
            }
        }

        /// Adds the three members `CON-020` requires cdm-rs to ignore.
        pub(crate) fn with_java_members(mut self) -> Self {
            for name in ["cert.pfx", "identity.jks", "trustStore.jks"] {
                self.members
                    .push((name.to_owned(), b"deliberate garbage".to_vec()));
            }
            self
        }

        /// Removes a member.
        pub(crate) fn without(mut self, name: &str) -> Self {
            self.members.retain(|(member, _)| member != name);
            self
        }

        /// Renames a member.
        pub(crate) fn rename(mut self, from: &str, to: &str) -> Self {
            for (name, _) in &mut self.members {
                if name == from {
                    *name = to.to_owned();
                }
            }
            self
        }

        /// Replaces `config.json`.
        pub(crate) fn config_json(mut self, json: &str) -> Self {
            self = self.without("config.json");
            self.members
                .push(("config.json".to_owned(), json.as_bytes().to_vec()));
            self
        }

        /// Nests every member under a directory, as a re-zipped bundle would be.
        pub(crate) fn nested(mut self, prefix: &str) -> Self {
            self.prefix = prefix.to_owned();
            self
        }

        /// Produces the zip.
        pub(crate) fn build(self) -> Vec<u8> {
            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut writer = zip::ZipWriter::new(&mut cursor);
                let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
                for (name, contents) in &self.members {
                    writer
                        .start_file(format!("{}{name}", self.prefix), options)
                        .unwrap();
                    writer.write_all(contents).unwrap();
                }
                writer.finish().unwrap();
            }
            cursor.into_inner()
        }
    }

    /// A metadata service response of the shape `CON-022` documents.
    pub(crate) fn metadata_json(host_ids: &[&str], proxy: &str, local_dc: &str) -> String {
        let contact_points = host_ids
            .iter()
            .map(|id| format!("\"{id}\""))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"version":1,"region":"us-east1","contact_info":{{"type":"sni_proxy","local_dc":"{local_dc}","contact_points":[{contact_points}],"sni_proxy_address":"{proxy}"}}}}"#
        )
    }
}

/// A minimal JKS writer, used only to produce fixtures for the reader in [`crate::tls::jks`].
pub(crate) mod jks_writer {
    use crate::tls::jks::{keyed_digest, keystream_xor};
    use sha1::{Digest, Sha1};

    /// Builds a JKS or JCEKS file.
    pub(crate) struct Writer {
        jceks: bool,
        body: Vec<u8>,
        count: u32,
    }

    impl Writer {
        pub(crate) fn new(jceks: bool) -> Self {
            Self {
                jceks,
                body: Vec::new(),
                count: 0,
            }
        }

        fn utf(&mut self, text: &str) {
            let bytes = text.as_bytes();
            self.body
                .extend_from_slice(&u16::try_from(bytes.len()).unwrap().to_be_bytes());
            self.body.extend_from_slice(bytes);
        }

        fn header(&mut self, tag: u32, alias: &str) {
            self.body.extend_from_slice(&tag.to_be_bytes());
            self.utf(alias);
            self.body
                .extend_from_slice(&1_700_000_000_000u64.to_be_bytes());
            self.count += 1;
        }

        fn certificate(&mut self, der: &[u8]) {
            self.utf("X.509");
            self.body
                .extend_from_slice(&u32::try_from(der.len()).unwrap().to_be_bytes());
            self.body.extend_from_slice(der);
        }

        pub(crate) fn trusted_certificate(mut self, alias: &str, der: &[u8]) -> Self {
            self.header(2, alias);
            self.certificate(der);
            self
        }

        pub(crate) fn private_key(
            mut self,
            alias: &str,
            pkcs8: &[u8],
            chain: &[Vec<u8>],
            key_password: &str,
        ) -> Self {
            let protected = protect(pkcs8, key_password);
            self.header(1, alias);
            self.body
                .extend_from_slice(&u32::try_from(protected.len()).unwrap().to_be_bytes());
            self.body.extend_from_slice(&protected);
            self.body
                .extend_from_slice(&u32::try_from(chain.len()).unwrap().to_be_bytes());
            for certificate in chain {
                self.certificate(certificate);
            }
            self
        }

        pub(crate) fn secret_key(mut self, alias: &str, material: &[u8]) -> Self {
            self.header(3, alias);
            self.body
                .extend_from_slice(&u32::try_from(material.len()).unwrap().to_be_bytes());
            self.body.extend_from_slice(material);
            self
        }

        pub(crate) fn raw_entry(mut self, tag: u32, alias: &str, payload: &[u8]) -> Self {
            self.header(tag, alias);
            self.body.extend_from_slice(payload);
            self
        }

        pub(crate) fn finish(self, store_password: &str) -> Vec<u8> {
            let magic: u32 = if self.jceks { 0xcece_cece } else { 0xfeed_feed };
            let mut out = Vec::new();
            out.extend_from_slice(&magic.to_be_bytes());
            out.extend_from_slice(&2u32.to_be_bytes());
            out.extend_from_slice(&self.count.to_be_bytes());
            out.extend_from_slice(&self.body);
            out.extend_from_slice(&keyed_digest(store_password, &out));
            out
        }
    }

    /// Wraps a PKCS#8 key the way `sun.security.provider.KeyProtector` does.
    fn protect(pkcs8: &[u8], password: &str) -> Vec<u8> {
        // `keytool` uses a random salt here, so vary it: a fixed one would let a reader that
        // ignored the salt still pass, and it reads to a scanner as an embedded crypto constant.
        let salt = super::generated_salt::<20>();
        let ciphertext = keystream_xor(password, &salt, pkcs8);
        let mut hasher = Sha1::new();
        hasher.update(
            password
                .encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<u8>>(),
        );
        hasher.update(pkcs8);
        let digest: [u8; 20] = hasher.finalize().into();

        let mut encrypted = Vec::new();
        encrypted.extend_from_slice(&salt);
        encrypted.extend_from_slice(&ciphertext);
        encrypted.extend_from_slice(&digest);

        // EncryptedPrivateKeyInfo ::= SEQUENCE { AlgorithmIdentifier, OCTET STRING }
        let mut algorithm = Vec::new();
        algorithm.extend_from_slice(&[
            0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x2a, 0x02, 0x11, 0x01, 0x01,
        ]);
        algorithm.extend_from_slice(&[0x05, 0x00]); // NULL parameters, as OpenJDK writes them
        let mut inner = der(0x30, &algorithm);
        inner.extend(der(0x04, &encrypted));
        der(0x30, &inner)
    }

    /// A DER tag-length-value.
    fn der(tag: u8, contents: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let len = contents.len();
        if len < 0x80 {
            out.push(u8::try_from(len).unwrap());
        } else {
            let bytes = len.to_be_bytes();
            let significant: Vec<u8> = bytes.iter().copied().skip_while(|b| *b == 0).collect();
            out.push(0x80 | u8::try_from(significant.len()).unwrap());
            out.extend_from_slice(&significant);
        }
        out.extend_from_slice(contents);
        out
    }
}
