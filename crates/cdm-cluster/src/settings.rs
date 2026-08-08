//! What the coordinator was configured to do (`DST-012`, `DST-013`).

use std::time::Duration;

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind};

/// Whether a range whose lease has expired may be taken over (`DST-012`, `DST-014`, `DST-015`).
///
/// # Why this has no `Default`
///
/// Reclaiming is safe *because migrate is idempotent*: an upsert carries the origin's writetime,
/// so re-writing a range another node had half-written changes nothing at the storage layer. That
/// argument does not hold for a counter table, where `SET c = c + delta` applied twice is not a
/// repeated write but a wrong number, and no later reconciliation can tell the difference.
///
/// `DST-014` and `DST-015` are the requirements that turn this into a decision cdm-rs makes for
/// the operator — refuse the reclaim of an in-flight counter range, mark it `FAIL`, and say that
/// manual reconciliation is required — and they are **not** implemented here; they are roadmap
/// item #51. Until they are, the choice is the caller's and the type refuses to guess: there is
/// no `Default`, so wiring a counter target into a coordinator is a decision somebody has to
/// write down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclaimPolicy {
    /// Expired leases may be reclaimed, and the reclaiming node increments `attempt`.
    ///
    /// Correct for migrate and validate over a non-counter table, where re-processing a range is
    /// wasted work and nothing worse.
    Reclaim,
    /// Expired leases are never taken over.
    ///
    /// The conservative setting, and the only defensible one for a counter target until
    /// `DST-014`/`DST-015` land: a range whose holder died stays unfinished and visible rather
    /// than being silently double-counted. `Coordinator::claim` reports
    /// [`ClaimOutcome::ReclaimRefused`](crate::ClaimOutcome::ReclaimRefused) so the caller can
    /// record why.
    Refuse,
}

/// The lease policy one node runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordinatorSettings {
    lease_duration: Duration,
    heartbeat_interval: Duration,
    max_attempts: u32,
    reclaim: ReclaimPolicy,
}

impl CoordinatorSettings {
    /// Validates a lease policy.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] when the policy cannot keep a lease alive:
    ///
    /// * a zero `lease_duration` or `heartbeat_interval` — a lease that has expired by the time
    ///   it is granted is not a lease;
    /// * `heartbeat_interval * 2 > lease_duration`. `DST-012` renews on the heartbeat and expires
    ///   on the lease, so a node needs at least one renewal it can *miss* — one slow round trip,
    ///   one paused thread — before another node concludes it is dead and takes its range. With
    ///   the defaults (15s, 60s) it gets three.
    /// * `max_attempts == 0`, which would abandon every range before it was ever tried.
    pub fn new(
        lease_duration: Duration,
        heartbeat_interval: Duration,
        max_attempts: u32,
        reclaim: ReclaimPolicy,
    ) -> Result<Self, CdmError> {
        if lease_duration.is_zero() {
            return Err(error(
                "cluster.lease_duration",
                "cluster.lease_duration is zero, so every lease would be expired the moment it \
                 was granted and two nodes could process one range at once",
            ));
        }
        if heartbeat_interval.is_zero() {
            return Err(error(
                "cluster.heartbeat_interval",
                "cluster.heartbeat_interval is zero; a node would renew its leases in a loop \
                 with no pause, saturating the target cluster with lightweight transactions",
            ));
        }
        if heartbeat_interval.saturating_mul(2) > lease_duration {
            return Err(error(
                "cluster.heartbeat_interval",
                "cluster.heartbeat_interval must be at most half of cluster.lease_duration, so \
                 that a node can miss a renewal without another node reclaiming the range it is \
                 still processing (DST-012)",
            ));
        }
        if max_attempts == 0 {
            return Err(error(
                "cluster.max_attempts",
                "cluster.max_attempts is zero, which would abandon every range before any node \
                 had tried it (DST-013)",
            ));
        }
        Ok(Self {
            lease_duration,
            heartbeat_interval,
            max_attempts,
            reclaim,
        })
    }

    /// The policy `cluster.*` describes, with the reclaim decision supplied by the caller.
    ///
    /// `reclaim` is not read from the configuration because there is no configuration key for it:
    /// it is a property of the *target table* — counter or not — which `DST-014`/`DST-015` (#51)
    /// will decide from the schema. Passing it here is the seam that decision lands on.
    ///
    /// # Errors
    ///
    /// As [`CoordinatorSettings::new`].
    pub fn from_config(config: &EffectiveConfig, reclaim: ReclaimPolicy) -> Result<Self, CdmError> {
        let cluster = &config.config().cluster;
        Self::new(
            cluster.lease_duration.get(),
            cluster.heartbeat_interval.get(),
            cluster.max_attempts,
            reclaim,
        )
    }

    /// How long a granted lease runs for (`DST-012`).
    #[must_use]
    pub const fn lease_duration(&self) -> Duration {
        self.lease_duration
    }

    /// How often a held lease is renewed (`DST-012`).
    #[must_use]
    pub const fn heartbeat_interval(&self) -> Duration {
        self.heartbeat_interval
    }

    /// How many times a range may be claimed before it is abandoned (`DST-013`).
    #[must_use]
    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Whether an expired lease may be taken over (`DST-012`, `DST-014`).
    #[must_use]
    pub const fn reclaim(&self) -> ReclaimPolicy {
        self.reclaim
    }
}

fn error(key: &'static str, message: &str) -> CdmError {
    CdmError::new(ErrorKind::Config, message.to_owned())
        .with_context(|ctx| ctx.with_config_key(key))
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
    use cdm_config::CdmConfig;

    use super::*;

    #[test]
    fn dst_012_the_defaults_leave_room_for_three_missed_renewals() {
        let config = EffectiveConfig::resolve(CdmConfig::default());
        let settings = CoordinatorSettings::from_config(&config, ReclaimPolicy::Reclaim).unwrap();
        assert_eq!(settings.lease_duration(), Duration::from_secs(60));
        assert_eq!(settings.heartbeat_interval(), Duration::from_secs(15));
        assert_eq!(settings.max_attempts(), 3, "DST-013's documented default");
        assert_eq!(settings.reclaim(), ReclaimPolicy::Reclaim);
    }

    #[test]
    fn dst_012_a_heartbeat_that_cannot_beat_the_expiry_is_refused() {
        // Exactly half is the boundary and is allowed; anything slower is not.
        assert!(CoordinatorSettings::new(
            Duration::from_secs(60),
            Duration::from_secs(30),
            3,
            ReclaimPolicy::Reclaim
        )
        .is_ok());
        let err = CoordinatorSettings::new(
            Duration::from_secs(60),
            Duration::from_secs(31),
            3,
            ReclaimPolicy::Reclaim,
        )
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.to_string().contains("half"));
    }

    #[test]
    fn dst_012_a_zero_lease_or_heartbeat_is_refused() {
        assert!(CoordinatorSettings::new(
            Duration::ZERO,
            Duration::from_secs(1),
            3,
            ReclaimPolicy::Reclaim
        )
        .is_err());
        assert!(CoordinatorSettings::new(
            Duration::from_secs(60),
            Duration::ZERO,
            3,
            ReclaimPolicy::Reclaim
        )
        .is_err());
    }

    #[test]
    fn dst_013_zero_attempts_would_abandon_every_range() {
        let err = CoordinatorSettings::new(
            Duration::from_secs(60),
            Duration::from_secs(15),
            0,
            ReclaimPolicy::Reclaim,
        )
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
    }
}
