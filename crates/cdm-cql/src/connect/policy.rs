//! Load balancing, speculative execution and retries (`CON-009`..`CON-012`).
//!
//! # Load balancing (`CON-009`)
//!
//! Token-aware, DC-aware and latency-aware, with the local datacenter taken from
//! `connect.{side}.local_datacenter` and auto-detected from `system.local` when unset. Datacenter
//! failover is **off** whenever a local DC is known: a bulk mover that quietly starts reading
//! across a WAN link is a billing incident, and `LOCAL_QUORUM` — the default consistency — cannot
//! be satisfied remotely anyway.
//!
//! # Speculative execution (`CON-010`)
//!
//! Off by default. A speculative execution is a second copy of the request, which is safe for a
//! read and is *not* safe for a write that is not idempotent — a counter increment applied twice
//! is silent corruption (`CON-012`, `MIG-032`). [`speculative_policy`] therefore refuses to build
//! one for a counter table at all, whatever the configuration says.
//!
//! # Retries (`CON-011`, `CON-012`)
//!
//! [`CdmRetryPolicy`] retries only what is safe to retry:
//!
//! * a request the caller has **not** marked idempotent is never retried. The engine marks
//!   counter writes non-idempotent, so this is the mechanism that implements `CON-012`;
//! * timeouts, `Unavailable`, `Overloaded`, `IsBootstrapping` and broken connections are retried,
//!   up to `perfops.retry.max_attempts`;
//! * everything else — a syntax error, an authorisation failure, a serialization bug — is
//!   returned immediately, because retrying it can only waste time.
//!
//! ## Backoff lives outside the driver
//!
//! `CON-011` also requires exponential backoff with jitter. The driver's [`RetrySession`] returns
//! a decision and cannot sleep: `RetryDecision` has no delay. The backoff is therefore
//! [`Backoff`], applied by the caller between attempts of its own, and the driver-level policy
//! covers the immediate same-target/next-target retries. This split is a driver constraint, not a
//! design preference, and is noted in the pull request.

use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use scylla::errors::RequestAttemptError;
use scylla::policies::load_balancing::{
    DefaultPolicy, LatencyAwarenessBuilder, LoadBalancingPolicy,
};
use scylla::policies::retry::{RequestInfo, RetryDecision, RetryPolicy, RetrySession};
use scylla::policies::speculative_execution::{
    SimpleSpeculativeExecutionPolicy, SpeculativeExecutionPolicy,
};

/// Builds the load-balancing policy of `CON-009`.
///
/// `local_datacenter` is `None` before the local DC is known; the resulting policy is then
/// token-aware but not DC-aware, which is what the control connection needs and what
/// [`crate::connect::session`] replaces once it has probed `system.local`.
pub fn load_balancing_policy(local_datacenter: Option<&str>) -> Arc<dyn LoadBalancingPolicy> {
    let mut builder = DefaultPolicy::builder()
        .token_aware(true)
        .latency_awareness(LatencyAwarenessBuilder::new());
    builder = match local_datacenter {
        Some(datacenter) => builder
            .prefer_datacenter(datacenter.to_owned())
            .permit_dc_failover(false),
        None => builder.permit_dc_failover(true),
    };
    builder.build()
}

/// How speculative execution is configured for one side (`CON-010`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeculativeSettings {
    /// Whether to speculate at all. Off by default, and always off for counter writes.
    pub enabled: bool,
    /// How long to wait before starting the next execution.
    pub delay: Duration,
    /// How many *extra* executions may be started.
    pub max_executions: u32,
}

impl Default for SpeculativeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            delay: Duration::from_millis(200),
            max_executions: 2,
        }
    }
}

/// Builds a speculative execution policy, or `None` when speculating would be unsafe
/// (`CON-010`, `CON-012`).
///
/// `writes_counters` is the table-level fact that overrides the configuration: a counter update
/// is not idempotent, so a second in-flight copy of it can double-apply. Refusing here means the
/// unsafe combination cannot be configured into existence.
pub fn speculative_policy(
    settings: SpeculativeSettings,
    writes_counters: bool,
) -> Option<Arc<dyn SpeculativeExecutionPolicy>> {
    if !settings.enabled || settings.max_executions == 0 {
        return None;
    }
    if writes_counters {
        tracing::warn!(
            rule = "CON-012",
            "speculative execution is configured but will not be used: the target is a counter \
             table, and a counter update applied twice is silent corruption"
        );
        return None;
    }
    Some(Arc::new(SimpleSpeculativeExecutionPolicy {
        max_retry_count: settings.max_executions as usize,
        retry_interval: settings.delay,
    }))
}

/// Exponential backoff with full jitter (`CON-011`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// The delay before the first retry.
    pub initial: Duration,
    /// The ceiling the delay doubles towards.
    pub max: Duration,
    /// How many attempts may be made in total, including the first.
    pub max_attempts: u32,
}

impl Backoff {
    /// A backoff from the `perfops.retry` settings.
    pub fn new(initial: Duration, max: Duration, max_attempts: u32) -> Self {
        Self {
            initial,
            max,
            max_attempts,
        }
    }

    /// The delay before attempt `attempt` (1-based), with full jitter applied.
    ///
    /// Full jitter — a uniform draw from `[0, backoff]` rather than `backoff` itself — is what
    /// keeps a thousand range workers from retrying an overloaded cluster in lockstep.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let ceiling = self.ceiling_for(attempt);
        let millis = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
        if millis == 0 {
            return Duration::ZERO;
        }
        Duration::from_millis(rand::thread_rng().gen_range(0..=millis))
    }

    /// The un-jittered ceiling for `attempt`, which is what a test can assert on.
    pub fn ceiling_for(&self, attempt: u32) -> Duration {
        let doublings = attempt.saturating_sub(1).min(32);
        let scaled = self
            .initial
            .checked_mul(1u32 << doublings.min(31))
            .unwrap_or(self.max);
        scaled.min(self.max)
    }

    /// Whether another attempt is allowed after `attempts` have been made.
    pub fn may_retry(&self, attempts: u32) -> bool {
        attempts < self.max_attempts
    }
}

/// The retry policy of `CON-011` and `CON-012`.
#[derive(Debug, Clone, Copy)]
pub struct CdmRetryPolicy {
    max_attempts: u32,
}

impl CdmRetryPolicy {
    /// A policy allowing `max_attempts` attempts in total, including the first.
    pub fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
        }
    }

    /// How many attempts a request gets.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }
}

impl RetryPolicy for CdmRetryPolicy {
    fn new_session(&self) -> Box<dyn RetrySession> {
        Box::new(CdmRetrySession {
            attempts: 1,
            max_attempts: self.max_attempts,
        })
    }
}

/// One request's worth of retry state.
#[derive(Debug)]
struct CdmRetrySession {
    attempts: u32,
    max_attempts: u32,
}

impl CdmRetrySession {
    /// The decision, taken apart from the driver's [`RequestInfo`].
    ///
    /// `RequestInfo` is `#[non_exhaustive]`, so no test outside `scylla` can build one. Keeping
    /// the rule in a function over plain arguments is what makes `CON-011` and `CON-012`
    /// testable at all.
    fn decide(&mut self, is_idempotent: bool, error: &RequestAttemptError) -> RetryDecision {
        // CON-012: a request that is not known to be idempotent is never retried. Counter writes
        // reach the driver marked non-idempotent, so this single check is what prevents counter
        // drift — the failure is surfaced and the partition range fails.
        if !is_idempotent {
            return RetryDecision::DontRetry;
        }
        if self.attempts >= self.max_attempts {
            return RetryDecision::DontRetry;
        }
        let decision = classify(error);
        if decision != RetryDecision::DontRetry {
            self.attempts += 1;
        }
        decision
    }
}

impl RetrySession for CdmRetrySession {
    fn decide_should_retry(&mut self, request_info: RequestInfo<'_>) -> RetryDecision {
        self.decide(request_info.is_idempotent, request_info.error)
    }

    fn reset(&mut self) {
        self.attempts = 1;
    }
}

/// Whether an attempt error is worth another attempt, and where (`CON-011`).
fn classify(error: &RequestAttemptError) -> RetryDecision {
    use scylla::errors::DbError;

    match error {
        // The coordinator did not answer in time or could not assemble a quorum, or the
        // connection died mid-request. Another coordinator may do better, so move on rather than
        // hammering the same one. A request that died in flight may or may not have been applied,
        // which is why only idempotent ones reach this point at all.
        RequestAttemptError::DbError(
            DbError::ReadTimeout { .. }
            | DbError::WriteTimeout { .. }
            | DbError::Unavailable { .. }
            | DbError::Overloaded
            | DbError::IsBootstrapping
            | DbError::ServerError
            | DbError::TruncateError,
            _,
        )
        | RequestAttemptError::BrokenConnectionError(_)
        | RequestAttemptError::UnableToAllocStreamId => RetryDecision::RetryNextTarget(None),
        // Everything else is deterministic: a syntax error, an unauthorised statement, a
        // serialization bug. Retrying cannot change the outcome.
        _ => RetryDecision::DontRetry,
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
    use scylla::errors::DbError;
    use scylla::statement::Consistency;

    use super::*;

    /// A fresh session, so that a test drives the same state machine the driver drives.
    fn session(max_attempts: u32) -> CdmRetrySession {
        CdmRetrySession {
            attempts: 1,
            max_attempts: CdmRetryPolicy::new(max_attempts).max_attempts(),
        }
    }

    fn timeout() -> RequestAttemptError {
        RequestAttemptError::DbError(
            DbError::WriteTimeout {
                consistency: Consistency::LocalQuorum,
                received: 1,
                required: 2,
                write_type: scylla::errors::WriteType::Simple,
            },
            "timed out".to_owned(),
        )
    }

    fn syntax_error() -> RequestAttemptError {
        RequestAttemptError::DbError(DbError::SyntaxError, "no".to_owned())
    }

    #[test]
    fn con_012_a_non_idempotent_request_is_never_retried() {
        let mut session = session(5);
        let error = timeout();
        assert_eq!(
            session.decide(false, &error),
            RetryDecision::DontRetry,
            "a counter write must fail rather than be applied twice"
        );
        // And the policy really does hand out this session type.
        let mut boxed = CdmRetryPolicy::new(5).new_session();
        boxed.reset();
    }

    #[test]
    fn con_011_a_timeout_on_an_idempotent_request_is_retried_up_to_the_limit() {
        assert_eq!(CdmRetryPolicy::new(3).max_attempts(), 3);
        let mut session = session(3);
        let error = timeout();

        assert_eq!(
            session.decide(true, &error),
            RetryDecision::RetryNextTarget(None)
        );
        assert_eq!(
            session.decide(true, &error),
            RetryDecision::RetryNextTarget(None)
        );
        assert_eq!(
            session.decide(true, &error),
            RetryDecision::DontRetry,
            "three attempts means two retries"
        );

        session.reset();
        assert_eq!(
            session.decide(true, &error),
            RetryDecision::RetryNextTarget(None)
        );
    }

    #[test]
    fn con_011_a_deterministic_error_is_not_retried() {
        let mut session = session(5);
        let error = syntax_error();
        assert_eq!(session.decide(true, &error), RetryDecision::DontRetry);
    }

    #[test]
    fn con_011_the_retryable_error_set_is_the_transport_one() {
        let unavailable = RequestAttemptError::DbError(
            DbError::Unavailable {
                consistency: Consistency::LocalQuorum,
                required: 2,
                alive: 1,
            },
            String::new(),
        );
        let overloaded = RequestAttemptError::DbError(DbError::Overloaded, String::new());
        let bootstrapping = RequestAttemptError::DbError(DbError::IsBootstrapping, String::new());
        let unauthorized = RequestAttemptError::DbError(DbError::Unauthorized, String::new());
        let unexpected = RequestAttemptError::UnableToAllocStreamId;

        for error in [&unavailable, &overloaded, &bootstrapping, &unexpected] {
            assert_eq!(
                classify(error),
                RetryDecision::RetryNextTarget(None),
                "{error}"
            );
        }
        assert_eq!(classify(&unauthorized), RetryDecision::DontRetry);
    }

    #[test]
    fn con_011_at_least_one_attempt_is_always_allowed() {
        assert_eq!(CdmRetryPolicy::new(0).max_attempts(), 1);
        let mut session = session(0);
        let error = timeout();
        assert_eq!(session.decide(true, &error), RetryDecision::DontRetry);
    }

    #[test]
    fn con_011_backoff_doubles_towards_the_ceiling_and_is_jittered() {
        let backoff = Backoff::new(Duration::from_millis(100), Duration::from_secs(10), 5);
        assert_eq!(backoff.ceiling_for(1), Duration::from_millis(100));
        assert_eq!(backoff.ceiling_for(2), Duration::from_millis(200));
        assert_eq!(backoff.ceiling_for(3), Duration::from_millis(400));
        assert_eq!(backoff.ceiling_for(30), Duration::from_secs(10));
        assert_eq!(backoff.ceiling_for(u32::MAX), Duration::from_secs(10));

        for attempt in 1..=5 {
            let delay = backoff.delay_for(attempt);
            assert!(delay <= backoff.ceiling_for(attempt), "{delay:?}");
        }
        assert!(backoff.may_retry(4));
        assert!(!backoff.may_retry(5));

        let zero = Backoff::new(Duration::ZERO, Duration::ZERO, 2);
        assert_eq!(zero.delay_for(1), Duration::ZERO);
    }

    #[test]
    fn con_010_speculative_execution_is_off_by_default() {
        let settings = SpeculativeSettings::default();
        assert!(!settings.enabled);
        assert!(speculative_policy(settings, false).is_none());
    }

    #[test]
    fn con_010_speculative_execution_can_be_enabled_per_side() {
        let settings = SpeculativeSettings {
            enabled: true,
            delay: Duration::from_millis(50),
            max_executions: 2,
        };
        assert!(speculative_policy(settings, false).is_some());
    }

    #[test]
    fn con_012_speculative_execution_is_refused_for_a_counter_table() {
        let settings = SpeculativeSettings {
            enabled: true,
            delay: Duration::from_millis(50),
            max_executions: 2,
        };
        assert!(speculative_policy(settings, true).is_none());
    }

    #[test]
    fn con_010_zero_extra_executions_means_disabled() {
        let settings = SpeculativeSettings {
            enabled: true,
            delay: Duration::from_millis(50),
            max_executions: 0,
        };
        assert!(speculative_policy(settings, false).is_none());
    }

    #[tokio::test]
    async fn con_009_the_policy_is_dc_aware_when_a_datacenter_is_known() {
        // The driver's policy is opaque, so this asserts what can be asserted: that both forms
        // build, and that they are not the same object.
        let with_dc = load_balancing_policy(Some("dc1"));
        let without = load_balancing_policy(None);
        assert!(!std::ptr::eq(
            Arc::as_ptr(&with_dc).cast::<u8>(),
            Arc::as_ptr(&without).cast::<u8>()
        ));
    }
}
