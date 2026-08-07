//! Fault injection: the six failures a cluster produces, and the session double that produces
//! them on demand (`TST-040`).
//!
//! # What this is for
//!
//! `ENG-008` and `CON-011` are both statements about what happens when a cluster misbehaves, and a
//! cluster will not misbehave to order. A node that times out one write in five, an overloaded
//! coordinator, a connection that dies mid-page, a schema that moves underneath a running range —
//! these are the cases that decide whether a migration loses rows quietly or fails loudly, and
//! they are exactly the cases a container fixture cannot be asked to produce.
//!
//! [`FaultySession`] produces them. It wraps any [`TestSession`] and, before delegating, consults
//! a [`FaultPlan`]: a script that says *which statements* fail, *with what*, and *on which
//! attempt*.
//!
//! # Determinism is the whole design
//!
//! A fault test that fires "about a third of the time" is a test people re-run rather than read.
//! Every trigger here is therefore a function of a counter, never of a clock and never of a
//! random draw:
//!
//! * [`FaultPlan::always`] — every statement matching a substring;
//! * [`FaultPlan::on_attempts`] — the 1st, 3rd and 7th matching statement, say;
//! * [`FaultPlan::every_nth`] — every *n*th matching statement.
//!
//! The counter is per rule, so two rules over the same statement do not interfere. Given the same
//! sequence of statements, a `FaultySession` injects the same faults in the same order, on every
//! machine and under every scheduler. Seeded random *data* is still the right tool (see
//! [`Seed`](crate::Seed)); seeded random *failures* are not, because a proptest shrink and a
//! failure script disagree about what "the same run" means.
//!
//! # The error kind a fault becomes is a claim, not a detail
//!
//! [`ErrorKind::is_fatal`] decides whether a failure fails one range (`ENG-008`) or aborts the
//! whole run, and [`ErrorKind::is_retryable`] decides whether `CON-011` retries it. So the mapping
//! in [`Fault::error_kind`] is the part of this module worth arguing about:
//!
//! | Fault | `ErrorKind` | Fatal? | Retryable? |
//! |---|---|---|---|
//! | [`FaultKind::ReadTimeout`] | `Read` | no | yes |
//! | [`FaultKind::WriteTimeout`] | `Write` | no | yes |
//! | [`FaultKind::Unavailable`] | `Read` or `Write`, by side | no | yes |
//! | [`FaultKind::Overloaded`] | `RateLimited` | no | yes |
//! | [`FaultKind::ConnectionDropped`] | `Read` or `Write`, by side | no | yes |
//! | [`FaultKind::SchemaChanged`] | `SchemaChanged` | **yes** | no |
//!
//! Two of those rows are deliberate and easy to get wrong:
//!
//! * **A dropped connection is not `ErrorKind::Connect`.** `Connect` is fatal, because cdm-rs
//!   establishes its sessions at startup and a connection error there is a misconfiguration. A
//!   connection that dies *during* a run is an ordinary node failure, and turning it into a fatal
//!   kind would abort a twelve-hour migration over one flapping node. `ARCHITECTURE.md` §13 says
//!   as much: "in-run node failures surface as `Read`/`Write` instead".
//! * **A schema change is the one fault that must not be contained.** Every statement, conversion
//!   plan and bind order was resolved against the old schema (`SCH-009`), so failing the range and
//!   carrying on would write data nobody can reconcile.
//!
//! # Nothing a fault reports names a row (`SEC-002`)
//!
//! An [`InjectedFault`] records the *rule* that matched, not the statement it matched — a
//! generated `INSERT` carries its values as literals, and a fault log full of them is a row-value
//! log by another name.

use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use cdm_core::{CdmError, ErrorKind, Side};

use crate::session::{MockSession, TestRow, TestSession};

/// One of the six failures `TST-040` requires a double to inject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FaultKind {
    /// The coordinator did not assemble a read quorum in time.
    ReadTimeout,
    /// The coordinator did not assemble a write quorum in time. The write may or may not have
    /// been applied, which is why a counter write is never retried (`CON-012`).
    WriteTimeout,
    /// Not enough replicas were alive to satisfy the consistency level.
    Unavailable,
    /// The coordinator rejected the request rather than queue it.
    Overloaded,
    /// The connection died with the request in flight.
    ConnectionDropped,
    /// The schema moved underneath the run (`SCH-009`).
    SchemaChanged,
}

impl FaultKind {
    /// Every fault, in declaration order. Exhaustive by construction: a new variant that is not
    /// added here fails `tst_040_every_fault_kind_is_listed`.
    pub const ALL: [Self; 6] = [
        Self::ReadTimeout,
        Self::WriteTimeout,
        Self::Unavailable,
        Self::Overloaded,
        Self::ConnectionDropped,
        Self::SchemaChanged,
    ];

    /// The stable name, as it appears in a test's failure message.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadTimeout => "read timeout",
            Self::WriteTimeout => "write timeout",
            Self::Unavailable => "unavailable",
            Self::Overloaded => "overloaded",
            Self::ConnectionDropped => "connection dropped",
            Self::SchemaChanged => "schema changed",
        }
    }

    /// The explanation the injected error carries.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::ReadTimeout => "injected fault: the coordinator timed out assembling a read",
            Self::WriteTimeout => {
                "injected fault: the coordinator timed out assembling a write; the update may or \
                 may not have been applied"
            }
            Self::Unavailable => {
                "injected fault: not enough replicas were alive for the consistency level"
            }
            Self::Overloaded => "injected fault: the coordinator is overloaded",
            Self::ConnectionDropped => {
                "injected fault: the connection was closed with the request in flight"
            }
            Self::SchemaChanged => {
                "injected fault: the schema changed after the run planned against it (SCH-009)"
            }
        }
    }
}

impl fmt::Display for FaultKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A fault, and which cluster produced it.
///
/// The side is not decoration: an `Unavailable` on the origin is a failed read and on the target a
/// failed write, and the two are counted, retried and reported differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fault {
    kind: FaultKind,
    side: Side,
}

impl Fault {
    /// A fault raised by the origin cluster.
    #[must_use]
    pub const fn origin(kind: FaultKind) -> Self {
        Self {
            kind,
            side: Side::Origin,
        }
    }

    /// A fault raised by the target cluster.
    #[must_use]
    pub const fn target(kind: FaultKind) -> Self {
        Self {
            kind,
            side: Side::Target,
        }
    }

    /// Which failure this is.
    #[must_use]
    pub const fn kind(self) -> FaultKind {
        self.kind
    }

    /// Which cluster raised it.
    #[must_use]
    pub const fn side(self) -> Side {
        self.side
    }

    /// The [`ErrorKind`] this fault surfaces as.
    ///
    /// See the module documentation for the table, and for why a dropped connection is *not*
    /// [`ErrorKind::Connect`].
    #[must_use]
    pub const fn error_kind(self) -> ErrorKind {
        match self.kind {
            FaultKind::ReadTimeout => ErrorKind::Read,
            FaultKind::WriteTimeout => ErrorKind::Write,
            FaultKind::Unavailable | FaultKind::ConnectionDropped => match self.side {
                Side::Origin => ErrorKind::Read,
                Side::Target => ErrorKind::Write,
            },
            FaultKind::Overloaded => ErrorKind::RateLimited,
            FaultKind::SchemaChanged => ErrorKind::SchemaChanged,
        }
    }

    /// Whether `CON-011` would retry a request that failed this way, ignoring idempotence.
    ///
    /// Idempotence is the caller's half of the rule and is not a property of the error: a counter
    /// write that hits a retryable fault must still not be retried (`CON-012`).
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        self.error_kind().is_retryable()
    }

    /// Whether this fault must abort the whole run rather than fail one range (`ENG-008`).
    #[must_use]
    pub const fn is_fatal(self) -> bool {
        self.error_kind().is_fatal()
    }

    /// The error a session raises when this fault fires.
    #[must_use]
    pub fn error(self) -> CdmError {
        CdmError::new(self.error_kind(), self.kind.message())
            .with_context(|ctx| ctx.with_side(self.side))
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} on the {}", self.kind, self.side)
    }
}

/// When a rule fires, counted over the statements that rule matches.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Trigger {
    /// Every matching statement.
    Always,
    /// These 1-based matching statements, and no others.
    OnAttempts(Vec<usize>),
    /// Every *n*th matching statement, starting with the *n*th.
    EveryNth(usize),
}

impl Trigger {
    /// Whether the rule fires on the `attempt`-th statement it matched, 1-based.
    fn fires(&self, attempt: usize) -> bool {
        match self {
            Self::Always => true,
            Self::OnAttempts(attempts) => attempts.contains(&attempt),
            // A zero step would fire on nothing at all, which reads as "no fault" and would make
            // a test pass for the wrong reason; it is normalised to 1 at construction, and the
            // guard here is what makes that normalisation belt-and-braces rather than load-bearing.
            Self::EveryNth(step) => *step > 0 && attempt.is_multiple_of(*step),
        }
    }
}

/// One rule: which statements, which fault, and when.
#[derive(Debug, Clone)]
struct FaultRule {
    matching: String,
    fault: Fault,
    trigger: Trigger,
}

/// A deterministic script of faults (`TST-040`).
///
/// Rules are tried in the order they were added and the first match wins, so a specific rule
/// added before a general one takes precedence — the same precedence [`MockSession`] applies to
/// its response rules.
///
/// ```
/// use cdm_testkit::{Fault, FaultKind, FaultPlan, FaultySession, TestSession};
///
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// // The second and fourth writes time out; every read succeeds.
/// let plan = FaultPlan::none().on_attempts(
///     "INSERT",
///     Fault::target(FaultKind::WriteTimeout),
///     [2, 4],
/// );
/// let session = FaultySession::new(plan);
///
/// assert!(session.execute("INSERT INTO ks.t (k) VALUES (1)").await.is_ok());
/// assert!(session.execute("INSERT INTO ks.t (k) VALUES (2)").await.is_err());
/// assert_eq!(session.injected().len(), 1);
/// # });
/// ```
#[derive(Debug, Clone, Default)]
pub struct FaultPlan {
    rules: Vec<FaultRule>,
}

impl FaultPlan {
    /// A plan that injects nothing — the baseline a fault test compares against.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Fails every statement containing `matching`.
    #[must_use]
    pub fn always(mut self, matching: impl Into<String>, fault: Fault) -> Self {
        self.rules.push(FaultRule {
            matching: matching.into(),
            fault,
            trigger: Trigger::Always,
        });
        self
    }

    /// Fails the given 1-based occurrences of statements containing `matching`.
    ///
    /// `[1, 2, 3]` with a `max_attempts` of 3 is how a test says "every attempt this request is
    /// allowed fails", which is the only way to observe the *end* of a retry loop rather than its
    /// middle.
    #[must_use]
    pub fn on_attempts(
        mut self,
        matching: impl Into<String>,
        fault: Fault,
        attempts: impl IntoIterator<Item = usize>,
    ) -> Self {
        self.rules.push(FaultRule {
            matching: matching.into(),
            fault,
            trigger: Trigger::OnAttempts(attempts.into_iter().collect()),
        });
        self
    }

    /// Fails every `step`-th statement containing `matching`.
    ///
    /// A `step` of zero is read as one: a rule that fires on nothing would make a fault test pass
    /// without ever injecting a fault, which is the one failure mode a fault suite cannot afford.
    #[must_use]
    pub fn every_nth(mut self, matching: impl Into<String>, fault: Fault, step: usize) -> Self {
        self.rules.push(FaultRule {
            matching: matching.into(),
            fault,
            trigger: Trigger::EveryNth(step.max(1)),
        });
        self
    }

    /// How many rules the plan holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the plan injects nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Every fault the plan can produce, for a test that asserts its own coverage.
    #[must_use]
    pub fn faults(&self) -> Vec<Fault> {
        self.rules.iter().map(|rule| rule.fault).collect()
    }
}

/// A fault that fired.
///
/// Carries the rule that matched rather than the statement that matched it: a generated `INSERT`
/// inlines its values, so recording the statement would put row data in a test's output
/// (`SEC-002`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedFault {
    /// What was injected.
    pub fault: Fault,
    /// The rule's substring, which is written by the test rather than taken from the data.
    pub rule: String,
    /// Which matching statement it was, 1-based.
    pub attempt: usize,
}

impl fmt::Display for InjectedFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} on statement {} matching `{}`",
            self.fault, self.attempt, self.rule
        )
    }
}

/// A [`TestSession`] that injects the faults of a [`FaultPlan`] (`TST-040`).
///
/// Wraps another session, so a fixture can seed a real node and then start failing its statements
/// without changing anything else about the fixture.
#[derive(Debug)]
pub struct FaultySession {
    inner: Arc<dyn TestSession>,
    plan: FaultPlan,
    /// One match counter per rule, by rule index.
    matched: Mutex<Vec<usize>>,
    injected: Mutex<Vec<InjectedFault>>,
    attempted: Mutex<usize>,
}

impl FaultySession {
    /// A faulty session over a [`MockSession`], for a test that needs no cluster.
    #[must_use]
    pub fn new(plan: FaultPlan) -> Self {
        Self::wrapping(Arc::new(MockSession::new()), plan)
    }

    /// A faulty session over `inner`.
    #[must_use]
    pub fn wrapping(inner: Arc<dyn TestSession>, plan: FaultPlan) -> Self {
        let counters = vec![0; plan.len()];
        Self {
            inner,
            plan,
            matched: Mutex::new(counters),
            injected: Mutex::new(Vec::new()),
            attempted: Mutex::new(0),
        }
    }

    /// Every fault that fired, in order.
    #[must_use]
    pub fn injected(&self) -> Vec<InjectedFault> {
        lock(&self.injected).clone()
    }

    /// How many faults fired.
    #[must_use]
    pub fn injected_count(&self) -> usize {
        lock(&self.injected).len()
    }

    /// How many statements were attempted, injected ones included.
    #[must_use]
    pub fn attempted(&self) -> usize {
        *lock(&self.attempted)
    }

    /// The plan being applied.
    #[must_use]
    pub const fn plan(&self) -> &FaultPlan {
        &self.plan
    }

    /// The fault this statement should raise, if any, advancing the matching rule's counter.
    fn next_fault(&self, cql: &str) -> Option<InjectedFault> {
        let mut matched = lock(&self.matched);
        for (index, rule) in self.plan.rules.iter().enumerate() {
            if !cql.contains(rule.matching.as_str()) {
                continue;
            }
            let counter = matched.get_mut(index)?;
            *counter += 1;
            let attempt = *counter;
            if !rule.trigger.fires(attempt) {
                // First match wins, whether or not it fires: a rule that matched and declined is
                // the rule that governs this statement, and falling through to a later one would
                // make `on_attempts` mean something different in the presence of a second rule.
                return None;
            }
            return Some(InjectedFault {
                fault: rule.fault,
                rule: rule.matching.clone(),
                attempt,
            });
        }
        None
    }
}

#[async_trait]
impl TestSession for FaultySession {
    async fn execute(&self, cql: &str) -> Result<Vec<TestRow>, CdmError> {
        *lock(&self.attempted) += 1;
        if let Some(injected) = self.next_fault(cql) {
            let error = injected.fault.error();
            lock(&self.injected).push(injected);
            return Err(error);
        }
        self.inner.execute(cql).await
    }

    async fn await_schema_agreement(&self) -> Result<(), CdmError> {
        self.inner.await_schema_agreement().await
    }
}

/// The guarded value, recovering from a poisoned lock.
///
/// A panic in one test must not turn every later assertion on the same session into a second,
/// misleading failure; nothing here has an invariant a panic could break.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
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

    #[test]
    fn tst_040_every_fault_kind_is_listed() {
        assert_eq!(FaultKind::ALL.len(), 6);
        for kind in FaultKind::ALL {
            assert!(!kind.as_str().is_empty());
            assert!(kind.message().contains("injected fault"));
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn tst_040_a_dropped_connection_fails_the_range_rather_than_the_run() {
        // `ErrorKind::Connect` is fatal, because a connection error at startup is a
        // misconfiguration. Mid-run it is one flapping node, and aborting a twelve-hour migration
        // over it would be the wrong answer (`ARCHITECTURE.md` §13).
        let origin = Fault::origin(FaultKind::ConnectionDropped);
        let target = Fault::target(FaultKind::ConnectionDropped);
        assert_eq!(origin.error_kind(), ErrorKind::Read);
        assert_eq!(target.error_kind(), ErrorKind::Write);
        assert!(!origin.is_fatal() && !target.is_fatal());
        assert!(origin.is_retryable() && target.is_retryable());
    }

    #[test]
    fn tst_040_a_schema_change_is_the_one_fault_that_aborts_the_run() {
        let fault = Fault::origin(FaultKind::SchemaChanged);
        assert_eq!(fault.error_kind(), ErrorKind::SchemaChanged);
        assert!(fault.is_fatal(), "SCH-009 is not containable");
        assert!(!fault.is_retryable());

        for kind in FaultKind::ALL {
            if kind == FaultKind::SchemaChanged {
                continue;
            }
            assert!(
                !Fault::target(kind).is_fatal(),
                "{kind} must fail one range, not the run (ENG-008)"
            );
        }
    }

    #[test]
    fn tst_040_the_side_decides_whether_a_fault_is_a_read_or_a_write() {
        assert_eq!(
            Fault::origin(FaultKind::Unavailable).error_kind(),
            ErrorKind::Read
        );
        assert_eq!(
            Fault::target(FaultKind::Unavailable).error_kind(),
            ErrorKind::Write
        );
        // A timeout names its own direction, so the side does not move it.
        assert_eq!(
            Fault::target(FaultKind::ReadTimeout).error_kind(),
            ErrorKind::Read
        );
        assert_eq!(
            Fault::origin(FaultKind::WriteTimeout).error_kind(),
            ErrorKind::Write
        );
        assert_eq!(
            Fault::origin(FaultKind::Overloaded).error_kind(),
            ErrorKind::RateLimited
        );
    }

    #[test]
    fn tst_040_a_fault_carries_its_side_into_the_error_it_raises() {
        let error = Fault::target(FaultKind::WriteTimeout).error();
        assert_eq!(error.kind(), ErrorKind::Write);
        assert_eq!(error.context().side, Some(Side::Target));
        assert!(error.message().contains("may or may not have been applied"));
        assert_eq!(
            Fault::target(FaultKind::WriteTimeout).to_string(),
            "write timeout on the target"
        );
    }

    #[tokio::test]
    async fn tst_040_a_scripted_plan_fires_on_exactly_the_attempts_it_names() {
        let session = FaultySession::new(FaultPlan::none().on_attempts(
            "INSERT",
            Fault::target(FaultKind::WriteTimeout),
            [2, 5],
        ));
        let mut failed = Vec::new();
        for row in 1..=6 {
            if session
                .execute(&format!("INSERT INTO ks.t (k) VALUES ({row})"))
                .await
                .is_err()
            {
                failed.push(row);
            }
        }
        assert_eq!(failed, vec![2, 5]);
        assert_eq!(session.injected_count(), 2);
        assert_eq!(session.attempted(), 6);
        assert_eq!(session.injected()[0].attempt, 2);
        assert_eq!(session.injected()[1].attempt, 5);
    }

    #[tokio::test]
    async fn tst_040_the_same_script_injects_the_same_faults_every_time() {
        // The property the whole module rests on: no clock, no entropy, no scheduler.
        let script = || {
            FaultPlan::none()
                .every_nth("SELECT", Fault::origin(FaultKind::ReadTimeout), 3)
                .always("DROP", Fault::target(FaultKind::Overloaded))
        };
        let statements = [
            "SELECT a",
            "SELECT b",
            "SELECT c",
            "DROP TABLE t",
            "SELECT d",
            "SELECT e",
            "SELECT f",
        ];

        let mut runs = Vec::new();
        for _ in 0..8 {
            let session = FaultySession::new(script());
            for statement in statements {
                drop(session.execute(statement).await);
            }
            runs.push(session.injected());
        }
        for run in &runs {
            assert_eq!(run, &runs[0]);
        }
        // Two `SELECT`s in three fire, plus the DROP.
        assert_eq!(runs[0].len(), 3);
    }

    #[tokio::test]
    async fn tst_040_a_zero_step_is_read_as_every_statement() {
        let session = FaultySession::new(FaultPlan::none().every_nth(
            "X",
            Fault::origin(FaultKind::Overloaded),
            0,
        ));
        assert!(session.execute("X").await.is_err());
        assert!(session.execute("X").await.is_err());
    }

    #[tokio::test]
    async fn tst_040_the_first_matching_rule_governs_the_statement() {
        // The specific rule declines on attempt 1, and the general rule must not step in: two
        // rules over one statement would otherwise make `on_attempts` unreadable.
        let session = FaultySession::new(
            FaultPlan::none()
                .on_attempts("SELECT id", Fault::origin(FaultKind::ReadTimeout), [2usize])
                .always("SELECT", Fault::origin(FaultKind::Overloaded)),
        );
        assert!(session.execute("SELECT id FROM t").await.is_ok());
        assert!(session.execute("SELECT id FROM t").await.is_err());
        assert_eq!(session.injected()[0].fault.kind(), FaultKind::ReadTimeout);
        // A statement the specific rule does not match still reaches the general one.
        assert!(session.execute("SELECT name FROM t").await.is_err());
        assert_eq!(session.injected()[1].fault.kind(), FaultKind::Overloaded);
    }

    #[tokio::test]
    async fn tst_040_an_empty_plan_delegates_everything() {
        let inner =
            Arc::new(MockSession::new().responding("SELECT", vec![TestRow::of_text([("k", "v")])]));
        let session = FaultySession::wrapping(
            Arc::clone(&inner) as Arc<dyn TestSession>,
            FaultPlan::none(),
        );
        assert!(session.plan().is_empty());
        assert_eq!(session.plan().len(), 0);

        let rows = session.execute("SELECT k FROM t").await.unwrap();
        assert_eq!(rows.len(), 1);
        session.await_schema_agreement().await.unwrap();
        assert_eq!(session.injected_count(), 0);
        assert_eq!(inner.execution_count(), 1);
    }

    #[tokio::test]
    async fn tst_040_an_injected_statement_never_reaches_the_inner_session() {
        // The failure has to be indistinguishable from the cluster refusing the statement, which
        // means the statement must not have been applied.
        let inner = Arc::new(MockSession::new());
        let session = FaultySession::wrapping(
            Arc::clone(&inner) as Arc<dyn TestSession>,
            FaultPlan::none().always("INSERT", Fault::target(FaultKind::WriteTimeout)),
        );
        assert!(session
            .execute("INSERT INTO ks.t (k) VALUES (1)")
            .await
            .is_err());
        assert_eq!(
            inner.execution_count(),
            0,
            "the write must not have happened"
        );
    }

    #[test]
    fn sec_002_an_injected_fault_names_the_rule_and_never_the_statement() {
        let injected = InjectedFault {
            fault: Fault::target(FaultKind::WriteTimeout),
            rule: "INSERT INTO ks.t".to_owned(),
            attempt: 3,
        };
        let rendered = injected.to_string();
        assert!(rendered.contains("INSERT INTO ks.t"));
        assert!(rendered.contains("statement 3"));
        // The values a generated INSERT inlines are exactly what must not be here.
        assert!(!rendered.contains("VALUES"));
    }

    #[test]
    fn tst_040_a_plan_reports_the_faults_it_can_produce() {
        let plan = FaultPlan::none()
            .always("SELECT", Fault::origin(FaultKind::ReadTimeout))
            .always("INSERT", Fault::target(FaultKind::WriteTimeout));
        assert_eq!(plan.len(), 2);
        assert!(!plan.is_empty());
        assert_eq!(
            plan.faults(),
            vec![
                Fault::origin(FaultKind::ReadTimeout),
                Fault::target(FaultKind::WriteTimeout)
            ]
        );
        assert_eq!(plan.faults()[0].side(), Side::Origin);
        assert_eq!(plan.faults()[0].kind(), FaultKind::ReadTimeout);
    }
}
