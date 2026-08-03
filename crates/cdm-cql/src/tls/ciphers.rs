//! Cipher-suite selection (`CON-007`).
//!
//! `tls.cipher_suites` is a JSSE property in Java CDM, so its values are JSSE names such as
//! `TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384`. rustls names the same suites identically for TLS 1.2
//! and uses the RFC 8446 names for TLS 1.3 (`TLS13_AES_256_GCM_SHA384`), so both spellings are
//! accepted and matching is case-insensitive.
//!
//! # Why an unsupported suite is an error
//!
//! JSSE silently drops a suite it does not know and negotiates whatever is left, which is how an
//! operator ends up believing they pinned a cipher that was never offered. `CON-007` requires the
//! opposite: an unknown or unavailable suite fails Tier-1 with the supported set printed, so the
//! mistake surfaces before any data moves.
//!
//! # The default value is not a request
//!
//! `connect.{side}.tls.cipher_suites` defaults to Java CDM's own default pair,
//! `TLS_RSA_WITH_AES_128_CBC_SHA` and `TLS_RSA_WITH_AES_256_CBC_SHA`. Neither exists in rustls
//! and neither ever will: static-RSA key exchange has no forward secrecy and was removed from
//! TLS 1.3. Treating the *default* as a request would therefore make TLS unusable out of the box.
//! The list is compared against that default first: if the operator did not change it, rustls'
//! own suite selection is used and a warning explains why. An operator who names those suites
//! explicitly still gets the `CON-007` error — the difference is intentional and is the only
//! place where cdm-rs looks at whether a value came from the default (a `SPEC` ambiguity noted in
//! the pull request).

use cdm_core::{CdmError, Side};
use rustls::crypto::{ring, CryptoProvider};
use rustls::SupportedCipherSuite;

use crate::errors::tls_error;

/// Java CDM's default `enabledAlgorithms`, which rustls cannot offer and which cdm-rs therefore
/// reads as "the operator expressed no preference".
pub const JAVA_DEFAULT_SUITES: &[&str] = &[
    "TLS_RSA_WITH_AES_128_CBC_SHA",
    "TLS_RSA_WITH_AES_256_CBC_SHA",
];

/// The cipher suites this build can offer, in rustls' preference order.
pub fn supported_suites() -> &'static [SupportedCipherSuite] {
    ring::DEFAULT_CIPHER_SUITES
}

/// The names of the cipher suites this build can offer (`CON-007`).
pub fn supported_names() -> Vec<String> {
    supported_suites().iter().map(name_of).collect()
}

/// The suite's name as rustls spells it, e.g. `TLS13_AES_256_GCM_SHA384`.
fn name_of(suite: &SupportedCipherSuite) -> String {
    format!("{:?}", suite.suite())
}

/// Whether `requested` is Java CDM's untouched default.
fn is_java_default(requested: &[String]) -> bool {
    requested.len() == JAVA_DEFAULT_SUITES.len()
        && requested.iter().all(|name| {
            JAVA_DEFAULT_SUITES
                .iter()
                .any(|d| d.eq_ignore_ascii_case(name))
        })
}

/// Resolves the requested suite names, or reports which ones are not available (`CON-007`).
///
/// Returns `None` when the request is empty or is Java's untouched default, meaning "use rustls'
/// own selection".
pub fn resolve(
    side: Side,
    requested: &[String],
) -> Result<Option<Vec<SupportedCipherSuite>>, CdmError> {
    if requested.is_empty() {
        return Ok(None);
    }
    if is_java_default(requested) {
        tracing::warn!(
            side = side.as_str(),
            rule = "CON-007",
            "connect.{}.tls.cipher_suites is Java CDM's default ({}), which rustls does not \
             implement: static-RSA key exchange offers no forward secrecy and TLS 1.3 removed it. \
             The TLS backend's own suite selection is used instead. Set the property explicitly \
             to pin a suite.",
            side.as_str(),
            JAVA_DEFAULT_SUITES.join(", ")
        );
        return Ok(None);
    }

    let supported = supported_suites();
    let mut resolved = Vec::with_capacity(requested.len());
    let mut unsupported = Vec::new();
    for name in requested {
        match supported
            .iter()
            .find(|suite| name_of(suite).eq_ignore_ascii_case(name.trim()))
        {
            Some(suite) => resolved.push(*suite),
            None => unsupported.push(name.trim().to_owned()),
        }
    }

    if !unsupported.is_empty() {
        return Err(tls_error(
            side,
            format!(
                "connect.{}.tls.cipher_suites requests {}, which this TLS backend does not \
                 support. Supported suites are: {}",
                side.as_str(),
                unsupported.join(", "),
                supported_names().join(", ")
            ),
        ));
    }
    Ok(Some(resolved))
}

/// A rustls crypto provider restricted to the requested cipher suites (`CON-007`).
pub fn provider_for(side: Side, requested: &[String]) -> Result<CryptoProvider, CdmError> {
    let mut provider = ring::default_provider();
    if let Some(suites) = resolve(side, requested)? {
        provider.cipher_suites = suites;
    }
    Ok(provider)
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

    #[test]
    fn con_007_an_unsupported_suite_lists_what_is_supported() {
        let err = resolve(Side::Origin, &["TLS_KRB5_WITH_DES_CBC_MD5".to_owned()]).unwrap_err();
        let rendered = err.to_string();
        assert!(rendered.contains("TLS_KRB5_WITH_DES_CBC_MD5"), "{rendered}");
        assert!(rendered.contains("TLS13_AES_256_GCM_SHA384"), "{rendered}");
        assert_eq!(err.kind(), cdm_core::ErrorKind::Tls);
    }

    #[test]
    fn con_007_a_supported_suite_is_honoured_and_nothing_else_is_offered() {
        let resolved = resolve(Side::Target, &["TLS13_AES_256_GCM_SHA384".to_owned()])
            .unwrap()
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(name_of(&resolved[0]), "TLS13_AES_256_GCM_SHA384");

        let provider =
            provider_for(Side::Target, &["TLS13_AES_256_GCM_SHA384".to_owned()]).unwrap();
        assert_eq!(provider.cipher_suites.len(), 1);
    }

    #[test]
    fn con_007_suite_names_are_matched_case_insensitively_and_trimmed() {
        let resolved = resolve(Side::Origin, &[" tls13_aes_128_gcm_sha256 ".to_owned()])
            .unwrap()
            .unwrap();
        assert_eq!(name_of(&resolved[0]), "TLS13_AES_128_GCM_SHA256");
    }

    #[test]
    fn con_007_java_default_suites_fall_back_to_the_backend_selection() {
        let requested: Vec<String> = JAVA_DEFAULT_SUITES
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert!(resolve(Side::Origin, &requested).unwrap().is_none());
        let provider = provider_for(Side::Origin, &requested).unwrap();
        assert_eq!(provider.cipher_suites.len(), supported_suites().len());
    }

    #[test]
    fn con_007_an_empty_request_uses_the_backend_selection() {
        assert!(resolve(Side::Origin, &[]).unwrap().is_none());
    }

    #[test]
    fn con_007_explicitly_naming_a_java_only_suite_is_still_an_error() {
        // One of the two, named on its own, is a real request rather than an untouched default.
        let err = resolve(Side::Origin, &["TLS_RSA_WITH_AES_128_CBC_SHA".to_owned()]).unwrap_err();
        assert!(err.to_string().contains("does not support"), "{err}");
    }

    #[test]
    fn con_007_the_supported_set_is_not_empty() {
        assert!(!supported_names().is_empty());
        assert!(supported_names().iter().any(|n| n.starts_with("TLS13_")));
    }
}
