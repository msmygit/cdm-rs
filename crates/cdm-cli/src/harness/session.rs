//! Opening both sides (`CON-001`, `CON-008`), or only the origin (`GRD-001`).

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind, Side};
use cdm_cql::connect::{self, ClusterSession};

/// A session to each cluster the run needs.
///
/// Held together because every job but one needs both, and because opening them together is what
/// makes a credential mistake on the *target* surface before the origin scan has read anything.
///
/// # Why the target is optional (`GRD-001`)
///
/// A guardrail run reads the origin and reports on it. `GRD-001` requires that it open **no target
/// connection at all** — not that it opens one and declines to write through it — and the
/// requirement is explicit that this must be structural rather than a matter of care. So the
/// target is an `Option` that [`Sessions::open_origin`] leaves empty, and the guardrail builder is
/// handed the [`ClusterSession`] for the origin directly rather than this pair: there is no value
/// in scope, anywhere on that path, through which a target could be reached.
#[derive(Debug)]
pub struct Sessions {
    /// The cluster data is read from.
    pub origin: ClusterSession,
    target: Option<ClusterSession>,
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
        Ok(Self {
            origin,
            target: Some(target),
        })
    }

    /// Connects to the origin and to nothing else (`GRD-001`).
    ///
    /// # Errors
    ///
    /// As [`Sessions::open`], for the origin alone.
    pub async fn open_origin(config: &EffectiveConfig) -> Result<Self, CdmError> {
        Ok(Self {
            origin: connect::connect(config.config(), Side::Origin).await?,
            target: None,
        })
    }

    /// The cluster nodes both sides are connected to, as the live display shows them (`MET-031`).
    ///
    /// Origin first, then target, each already sorted by address, so a display redrawn twice a
    /// second does not reorder its own rows. A guardrail run contributes only the origin, because
    /// `GRD-001` means there is no target session to ask.
    ///
    /// These are the *database* nodes. The cdm-rs nodes of a distributed run are a different thing
    /// entirely and belong to `cdm-cluster`, which is roadmap items #50–#52 and not started; see
    /// [`cdm_metrics::NodeStatus`] for why this display does not pretend otherwise.
    pub fn node_status(&self) -> Vec<cdm_metrics::NodeStatus> {
        let sides = [Some(&self.origin), self.target.as_ref()];
        sides
            .into_iter()
            .flatten()
            .flat_map(|session| {
                let side = session.side();
                session
                    .nodes()
                    .into_iter()
                    .map(move |node| cdm_metrics::NodeStatus {
                        side,
                        address: node.address,
                        datacenter: node.datacenter,
                        rack: node.rack,
                        connected: node.connected,
                    })
            })
            .collect()
    }

    /// The target session.
    ///
    /// # Errors
    ///
    /// [`cdm_core::ErrorKind::Internal`] when this run opened the origin only. A caller that asks
    /// an origin-only run for its target has confused two jobs, which is a defect in the harness
    /// rather than anything an operator did; it is reported rather than asserted because `ERR-004`
    /// leaves no room for a panic on a production path.
    pub fn target(&self) -> Result<&ClusterSession, CdmError> {
        self.target.as_ref().ok_or_else(|| {
            CdmError::new(
                ErrorKind::Internal,
                "this run opened the origin only, as `GRD-001` requires of a guardrail, and has no \
                 target session to offer",
            )
            .with_context(|c| c.with_side(Side::Target))
        })
    }
}
