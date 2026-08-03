//! Which of the four connection modes a side uses (`CON-002`).
//!
//! Java CDM decides this in `ConnectionFetcher.getConnection`, by a chain of `if`s over the
//! configured properties. `CON-002` requires cdm-rs to select "exactly as Java's
//! `ConnectionFetcher` does", so the order below is that chain, with the DevOps download of
//! `CON-004` inserted where `ConnectionFetcher` performs it — before anything else looks at the
//! bundle path:
//!
//! | Order | Condition | Mode |
//! |---|---|---|
//! | 1 | `connect.{side}.scb` is set | [`ConnectionMode::Bundle`] |
//! | 2 | `astra.database_id` is set | [`ConnectionMode::BundleDownload`] |
//! | 3 | a trust store is set **and** `tls.is_astra` | [`ConnectionMode::AstraFromStores`] |
//! | 4 | a trust store is set, or `tls.enabled` | [`ConnectionMode::Tls`] |
//! | 5 | otherwise | [`ConnectionMode::Plain`] |
//!
//! # Two deliberate differences from Java
//!
//! * Java attempts the download only when **both** `astra.database.id` and `astra.scb.region` are
//!   set, and swallows a failure. `CON-004` makes `database_id` alone the trigger and `ERR-001`
//!   makes the failure fatal: a run that silently falls back to `localhost:9042` because a token
//!   expired is worse than one that stops.
//! * Java's mode 2 writes a synthetic bundle zip to the working directory
//!   (`DataUtility.generateSCB`) so that the Spark connector can be pointed at a file. cdm-rs
//!   needs no file: the same material is held in memory, which is both faster and one fewer place
//!   for a private key to be left behind (`SEC-001`, `CON-005`).

use std::path::PathBuf;

use cdm_config::model::{CdmConfig, SideConnect};
use cdm_core::{CdmError, Side};

use crate::astra::bundle::strip_file_scheme;
use crate::errors::config_error;

/// One side's connection settings.
pub fn side_config(config: &CdmConfig, side: Side) -> &SideConnect {
    match side {
        Side::Origin => &config.connect.origin,
        Side::Target => &config.connect.target,
    }
}

/// How a side reaches its cluster (`CON-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionMode {
    /// A secure-connect-bundle on disk.
    Bundle {
        /// The bundle, with any `file://` prefix removed.
        path: PathBuf,
    },
    /// A bundle to be downloaded from the DevOps API first (`CON-004`).
    BundleDownload {
        /// The Astra database whose bundle to fetch.
        database_id: String,
    },
    /// Astra reached with operator-supplied trust and key stores rather than a bundle.
    AstraFromStores,
    /// A self-managed cluster with client encryption.
    Tls,
    /// A self-managed cluster without encryption.
    Plain,
}

impl ConnectionMode {
    /// The mode's name, as `cdm connect test` prints it (`CON-008`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bundle { .. } => "secure-connect-bundle",
            Self::BundleDownload { .. } => "secure-connect-bundle (downloaded)",
            Self::AstraFromStores => "astra (truststore)",
            Self::Tls => "tls",
            Self::Plain => "plain",
        }
    }

    /// Whether this mode talks to Astra, and therefore needs the `astra` module.
    pub fn is_astra(&self) -> bool {
        matches!(
            self,
            Self::Bundle { .. } | Self::BundleDownload { .. } | Self::AstraFromStores
        )
    }
}

/// Selects the connection mode for a side (`CON-002`).
///
/// `CFG-041` — host and bundle both set — is a Tier-2 rule and is reported there; it is repeated
/// here because a session may be built from a configuration that was never validated, and
/// choosing silently between two contradictory settings is the failure mode that produces a run
/// against the wrong cluster.
pub fn detect(config: &CdmConfig, side: Side) -> Result<ConnectionMode, CdmError> {
    let settings = side_config(config, side);

    if let Some(path) = &settings.scb {
        if settings.astra.database_id.is_some() {
            tracing::info!(
                side = side.as_str(),
                rule = "CON-004",
                "both connect.{}.scb and astra.database_id are set; the bundle on disk wins and \
                 nothing is downloaded",
                side.as_str()
            );
        }
        return Ok(ConnectionMode::Bundle {
            path: strip_file_scheme(path),
        });
    }

    if let Some(database_id) = &settings.astra.database_id {
        return Ok(ConnectionMode::BundleDownload {
            database_id: database_id.to_string(),
        });
    }

    let has_truststore = settings.tls.truststore.path.is_some();
    if has_truststore && settings.tls.is_astra {
        return Ok(ConnectionMode::AstraFromStores);
    }
    if settings.tls.is_astra && !has_truststore {
        return Err(config_error(
            side,
            format!(
                "connect.{}.tls.is_astra is set but no trust store is configured, and no bundle \
                 was given; Astra cannot be reached without one of them (CON-002)",
                side.as_str()
            ),
            "connect.{side}.tls.truststore.path",
        ));
    }
    if has_truststore || settings.tls.enabled {
        return Ok(ConnectionMode::Tls);
    }
    Ok(ConnectionMode::Plain)
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

    fn config() -> CdmConfig {
        CdmConfig::default()
    }

    fn database_id() -> cdm_config::types::AstraDatabaseId {
        cdm_config::types::AstraDatabaseId::new(
            uuid::Uuid::parse_str("2b3d9e1f-0000-0000-0000-000000000000").unwrap(),
        )
    }

    #[test]
    fn con_002_a_bundle_path_wins() {
        let mut config = config();
        config.connect.origin.scb = Some(PathBuf::from("file:///tmp/scb.zip"));
        let mode = detect(&config, Side::Origin).unwrap();
        assert_eq!(
            mode,
            ConnectionMode::Bundle {
                path: PathBuf::from("/tmp/scb.zip")
            },
            "the file:// prefix Java writes must be stripped"
        );
        assert!(mode.is_astra());
        assert_eq!(mode.as_str(), "secure-connect-bundle");
    }

    #[test]
    fn con_004_a_database_id_without_a_bundle_means_download() {
        let mut config = config();
        config.connect.target.astra.database_id = Some(database_id());
        let mode = detect(&config, Side::Target).unwrap();
        match &mode {
            ConnectionMode::BundleDownload { database_id } => {
                assert_eq!(database_id, "2b3d9e1f-0000-0000-0000-000000000000");
            }
            other => panic!("expected a download, got {other:?}"),
        }
        assert!(mode.is_astra());
    }

    #[test]
    fn con_004_a_bundle_on_disk_beats_a_database_id() {
        let mut config = config();
        config.connect.origin.scb = Some(PathBuf::from("/tmp/scb.zip"));
        config.connect.origin.astra.database_id = Some(database_id());
        assert!(matches!(
            detect(&config, Side::Origin).unwrap(),
            ConnectionMode::Bundle { .. }
        ));
    }

    #[test]
    fn con_002_a_truststore_with_is_astra_generates_astra_material() {
        let mut config = config();
        config.connect.origin.tls.truststore.path = Some(PathBuf::from("/tmp/ts.jks"));
        config.connect.origin.tls.is_astra = true;
        assert_eq!(
            detect(&config, Side::Origin).unwrap(),
            ConnectionMode::AstraFromStores
        );
    }

    #[test]
    fn con_002_is_astra_without_any_material_is_a_config_error() {
        let mut config = config();
        config.connect.target.tls.is_astra = true;
        let err = detect(&config, Side::Target).unwrap_err();
        assert_eq!(err.kind(), cdm_core::ErrorKind::Config);
        assert!(err.to_string().contains("trust store"), "{err}");
    }

    #[test]
    fn con_002_a_truststore_alone_is_ordinary_tls() {
        let mut config = config();
        config.connect.origin.tls.truststore.path = Some(PathBuf::from("/tmp/ts.jks"));
        assert_eq!(detect(&config, Side::Origin).unwrap(), ConnectionMode::Tls);
        assert!(!ConnectionMode::Tls.is_astra());
    }

    #[test]
    fn con_002_tls_enabled_without_a_truststore_is_still_tls() {
        let mut config = config();
        config.connect.origin.tls.enabled = true;
        assert_eq!(detect(&config, Side::Origin).unwrap(), ConnectionMode::Tls);
    }

    #[test]
    fn con_002_the_default_configuration_is_plain() {
        assert_eq!(
            detect(&config(), Side::Origin).unwrap(),
            ConnectionMode::Plain
        );
        assert_eq!(
            detect(&config(), Side::Target).unwrap(),
            ConnectionMode::Plain
        );
        assert_eq!(ConnectionMode::Plain.as_str(), "plain");
    }

    #[test]
    fn con_001_the_two_sides_are_selected_independently() {
        let mut config = config();
        config.connect.origin.scb = Some(PathBuf::from("/tmp/scb.zip"));
        config.connect.target.tls.enabled = true;
        assert!(detect(&config, Side::Origin).unwrap().is_astra());
        assert_eq!(detect(&config, Side::Target).unwrap(), ConnectionMode::Tls);
        assert_eq!(side_config(&config, Side::Target).port, 9042);
    }
}
