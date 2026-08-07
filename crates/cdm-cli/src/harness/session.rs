//! Opening both sides (`CON-001`, `CON-008`).

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, Side};
use cdm_cql::connect::{self, ClusterSession};

/// A session to each cluster.
///
/// Held together because every later step needs both, and because opening them together is what
/// makes a credential mistake on the *target* surface before the origin scan has read anything.
#[derive(Debug)]
pub struct Sessions {
    /// The cluster data is read from.
    pub origin: ClusterSession,
    /// The cluster data is written to.
    pub target: ClusterSession,
}

impl Sessions {
    /// Connects to both sides.
    ///
    /// Sequential rather than concurrent, and deliberately origin-first: when both sides are
    /// misconfigured the operator gets the origin's diagnostic, which is the one they can act on
    /// without having fixed anything else. Two concurrent failures would report whichever lost the
    /// race.
    ///
    /// # Errors
    ///
    /// [`cdm_core::ErrorKind::Connect`], [`cdm_core::ErrorKind::Auth`] or
    /// [`cdm_core::ErrorKind::Tls`], carrying which side failed (`ERR-002`).
    pub async fn open(config: &EffectiveConfig) -> Result<Self, CdmError> {
        let origin = connect::connect(config.config(), Side::Origin).await?;
        let target = connect::connect(config.config(), Side::Target).await?;
        Ok(Self { origin, target })
    }
}
