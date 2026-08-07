//! Fault injection against the retry rule (`TST-040`, `CON-011`, `CON-012`).
//!
//! # What is under test, and what is not
//!
//! `CON-011` has two halves and they live in different places. The driver-level half —
//! *which* `DbError`s are worth another attempt — is `CdmRetryPolicy`, and its own unit tests
//! cover it, because a `RequestInfo` is `#[non_exhaustive]` and cannot be built outside `scylla`.
//! The caller-level half is [`Backoff`]: how many attempts a request gets, how long it waits
//! between them, and the predicate that ends the loop.
//!
//! That predicate is what a fault suite can actually drive. Every retrying path in this crate —
//! the range scan, the target write, the counter lookup — ends its loop on
//! [`Backoff::should_retry`], so injecting a fault and counting attempts tests the rule those
//! three share rather than a fourth copy written for the test.
//!
//! # Why there is a loop in this file at all
//!
//! The production loops are private and take a driver session. Reproducing the loop's *shape*
//! here, over a [`FaultySession`], is what lets a test say "this fault, six times, with a budget
//! of three" and count what happened. To keep the copy honest,
//! [`con_011_every_retry_loop_in_the_crate_ends_on_the_shared_predicate`] sweeps the sources: if a
//! fourth retry loop appears that decides for itself, this file stops being evidence and the
//! sweep says so.
//!
//! # Nothing here observes real time
//!
//! `Backoff` sleeps between attempts. Under `#[tokio::test(start_paused = true)]` the clock is
//! virtual and advances only when every task is parked, so a loop with a ten-second ceiling costs
//! no wall-clock time and the assertions are equalities rather than tolerances.

// A failed assertion *is* the reporting mechanism in a test; the no-panic rule (ERR-004) exists
// to protect production paths, not test bodies.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;
use std::time::Duration;

use cdm_core::{CdmError, ErrorKind};
use cdm_cql::connect::Backoff;
use cdm_testkit::{Fault, FaultKind, FaultPlan, FaultySession, TestSession};

/// A three-attempt budget with a short, bounded backoff — the shape a `perfops.retry` block
/// resolves to, and small enough that the virtual clock's advance is easy to read.
fn backoff(max_attempts: u32) -> Backoff {
    Backoff::new(
        Duration::from_millis(100),
        Duration::from_secs(10),
        max_attempts,
    )
}

/// How one request ended.
#[derive(Debug)]
struct Attempted {
    /// The outcome the caller would see.
    outcome: Result<(), CdmError>,
    /// How many attempts were made, including the first.
    attempts: u32,
}

/// The retry loop of `CON-011`, in the shape every retrying path in `cdm-cql` uses it.
///
/// Two conditions end it, and [`Backoff::should_retry`] is both of them: the budget, and whether
/// the failure is one another attempt could fix.
async fn retrying(session: &FaultySession, backoff: Backoff, cql: &str) -> Attempted {
    let mut attempts = 0u32;
    loop {
        attempts = attempts.saturating_add(1);
        match session.execute(cql).await {
            Ok(_) => {
                return Attempted {
                    outcome: Ok(()),
                    attempts,
                }
            }
            Err(error) => {
                if !backoff.should_retry(&error, attempts) {
                    return Attempted {
                        outcome: Err(error),
                        attempts,
                    };
                }
                tokio::time::sleep(backoff.delay_for(attempts)).await;
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn tst_040_a_retryable_fault_is_retried_until_the_budget_runs_out() {
    // Every transport fault, one at a time, failing on every attempt it is allowed.
    for kind in [
        FaultKind::ReadTimeout,
        FaultKind::WriteTimeout,
        FaultKind::Unavailable,
        FaultKind::Overloaded,
        FaultKind::ConnectionDropped,
    ] {
        let session = FaultySession::new(FaultPlan::none().always("SELECT", Fault::origin(kind)));
        let attempted = retrying(&session, backoff(3), "SELECT * FROM ks.t").await;

        assert!(attempted.outcome.is_err(), "{kind} must surface eventually");
        assert_eq!(
            attempted.attempts, 3,
            "{kind} is retryable, so the loop must use its whole budget"
        );
        assert_eq!(session.injected_count(), 3);
        let error = attempted.outcome.unwrap_err();
        assert!(
            error.is_retryable(),
            "{kind} surfaced as {} which CON-011 would not retry",
            error.kind()
        );
    }
}

#[tokio::test(start_paused = true)]
async fn tst_040_a_fault_that_clears_costs_only_the_attempts_it_took() {
    // The common case: one bad coordinator, then a good one. Two attempts, and the caller never
    // learns anything went wrong.
    let session = FaultySession::new(FaultPlan::none().on_attempts(
        "SELECT",
        Fault::origin(FaultKind::ReadTimeout),
        [1usize],
    ));
    let attempted = retrying(&session, backoff(5), "SELECT * FROM ks.t").await;

    assert!(attempted.outcome.is_ok());
    assert_eq!(attempted.attempts, 2);
    assert_eq!(session.injected_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn tst_040_a_schema_change_is_not_retried_at_all() {
    // `SCH-009` is the one injected fault that no number of attempts can fix: every statement,
    // conversion plan and bind order was resolved against the schema that just moved. Retrying it
    // would burn the budget and then report the same thing, an hour later.
    let session = FaultySession::new(
        FaultPlan::none().always("SELECT", Fault::origin(FaultKind::SchemaChanged)),
    );
    let attempted = retrying(&session, backoff(5), "SELECT * FROM ks.t").await;

    assert_eq!(attempted.attempts, 1, "a schema change must not be retried");
    let error = attempted.outcome.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::SchemaChanged);
    assert!(error.kind().is_fatal(), "and it must not be contained");
}

#[tokio::test(start_paused = true)]
async fn tst_040_a_budget_of_one_makes_every_fault_terminal() {
    // `perfops.retry.max_attempts = 1` is the configuration an operator reaches for when they
    // would rather see the failure than wait for it. It must not silently become two.
    let session = FaultySession::new(
        FaultPlan::none().always("INSERT", Fault::target(FaultKind::WriteTimeout)),
    );
    let attempted = retrying(&session, backoff(1), "INSERT INTO ks.t (k) VALUES (1)").await;
    assert_eq!(attempted.attempts, 1);
    assert!(attempted.outcome.is_err());
}

#[tokio::test(start_paused = true)]
async fn tst_040_the_retry_budget_is_per_request_not_per_run() {
    // A run that hits one timeout per range must not exhaust a shared budget three ranges in.
    let session = FaultySession::new(FaultPlan::none().every_nth(
        "SELECT",
        Fault::origin(FaultKind::Overloaded),
        2,
    ));
    let mut total = 0u32;
    for _ in 0..4 {
        let attempted = retrying(&session, backoff(3), "SELECT * FROM ks.t").await;
        assert!(attempted.outcome.is_ok(), "one fault in two always clears");
        total += attempted.attempts;
    }
    // Every second statement fails, so the first request costs one attempt and each of the other
    // three costs two: the budget is spent and restored per request, never carried between them.
    assert_eq!(total, 7);
    assert_eq!(session.injected_count(), 3);
}

#[test]
fn con_012_a_counter_fault_is_retryable_and_must_still_never_be_retried() {
    // The distinction `CON-012` turns on, stated as a test rather than left to the prose: a
    // counter write fails with an ordinary, perfectly retryable `Write`. Nothing about the
    // *error* says not to retry it — the "do not" is carried by the type, since `CounterWrite`
    // does not implement the sealed `Idempotent` and so cannot reach a retry helper at all
    // (asserted by `mig_012_a_counter_write_is_typed_so_that_it_cannot_be_retried`).
    let fault = Fault::target(FaultKind::WriteTimeout);
    assert!(fault.is_retryable());
    assert!(backoff(5).should_retry(&fault.error(), 1));

    let source = include_str!("../src/exec/write.rs");
    let production = source.split("#[cfg(test)]").next().unwrap();
    let counter_fn = production
        .split("pub async fn write_counter")
        .nth(1)
        .unwrap()
        .split("/// Reads the target row")
        .next()
        .unwrap();
    assert!(
        !counter_fn.contains("should_retry") && !counter_fn.contains("loop {"),
        "write_counter must issue exactly one attempt (CON-012, MIG-032)"
    );
}

#[test]
fn con_011_every_retry_loop_in_the_crate_ends_on_the_shared_predicate() {
    // The loop in this file is only evidence for as long as it is the same loop. A retrying path
    // that decides for itself — `attempts < max` written out again, an `is_retryable` check
    // without the budget — would drift from the rule this suite injects faults against, and the
    // suite would keep passing.
    for path in ["src/exec/scan.rs", "src/exec/write.rs"] {
        let source =
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
                .unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(
            !production.contains("may_retry("),
            "{path} decides its own retry budget; use Backoff::should_retry (CON-011)"
        );
        assert!(
            production.contains("should_retry("),
            "{path} has a retry loop that does not use the shared predicate"
        );
    }
}

#[tokio::test]
async fn tst_040_a_plan_can_inject_every_fault_the_specification_names() {
    // `TST-040` names six. A suite that quietly covered five would still be green, so the count
    // is asserted rather than assumed.
    let mut plan = FaultPlan::none();
    for (index, kind) in FaultKind::ALL.into_iter().enumerate() {
        plan = plan.always(format!("STATEMENT-{index}"), Fault::target(kind));
    }
    let session = FaultySession::new(plan);

    let mut kinds = Vec::new();
    for index in 0..FaultKind::ALL.len() {
        let error = session
            .execute(&format!("STATEMENT-{index}"))
            .await
            .unwrap_err();
        kinds.push(error.kind());
    }
    assert_eq!(session.injected_count(), FaultKind::ALL.len());
    assert_eq!(
        kinds,
        vec![
            ErrorKind::Read,
            ErrorKind::Write,
            ErrorKind::Write,
            ErrorKind::RateLimited,
            ErrorKind::Write,
            ErrorKind::SchemaChanged,
        ]
    );
}

#[tokio::test]
async fn tst_040_a_faulty_session_wraps_a_real_one_without_changing_it() {
    // The fixture seeds through the same seam it later fails, so a fault test and a clean test
    // differ by the plan and by nothing else.
    let inner = Arc::new(cdm_testkit::MockSession::new());
    let session = FaultySession::wrapping(
        Arc::clone(&inner) as Arc<dyn TestSession>,
        FaultPlan::none().always("DROP", Fault::target(FaultKind::Unavailable)),
    );
    session
        .execute("INSERT INTO ks.t (k) VALUES (1)")
        .await
        .unwrap();
    assert!(session.execute("DROP TABLE ks.t").await.is_err());

    assert_eq!(
        inner.executed(),
        vec!["INSERT INTO ks.t (k) VALUES (1)"],
        "the injected statement must not have reached the cluster"
    );
    assert_eq!(session.attempted(), 2);
}
