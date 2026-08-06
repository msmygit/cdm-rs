//! The counter-assertion DSL (`TST-100`), and the two Java defects it exists to prevent.
//!
//! Java CDM's SIT suite asserts on counters with `cdm-assert.sh`, which greps the final metrics
//! block out of a log and compares numbers with `awk`. That works, and it is the reason
//! `MET-005` and `MET-006` are a byte-for-byte contract — but it can only ever see the
//! **committed** numbers, because that is all a log line contains.
//!
//! cdm-rs keeps Java's two-level accounting (`MET-004`): a counter has an *interim* value, which
//! workers increment, and a *committed* value, which [`JobCounters::flush`] folds the interim
//! value into when a range completes. The distinction is not a detail. It is the direct cause of
//! two defects in Java that cdm-rs deliberately does not reproduce:
//!
//! * **`MIG-004`** — Java compares the *committed* `UNFLUSHED` count against its flush threshold,
//!   but `UNFLUSHED` is only ever incremented at the *interim* level, so the committed value is
//!   permanently zero and the threshold is unreachable. Java flushes once per range, buffering
//!   every write for the range in memory.
//! * **`ENG-008`** — Java's validate job computes a failed range's `ERROR` count from *committed*
//!   counts, on a path where `flush()` has not run, so every term is zero and `ERROR` is always
//!   incremented by exactly zero. The counter that exists to say how many rows were lost reports
//!   none.
//!
//! Both are the same mistake: reading the level you did not mean. An assertion helper that
//! silently picked one would be that mistake again, in the tests that are supposed to catch it.
//! So [`CounterExpectation::check`] takes the [`CounterView`] as an argument — there is no
//! default, and no `assert_counters` that guesses — and when an expectation fails against one
//! view but would have passed against the other, the failure message says so and names the
//! requirement.
//!
//! # Which view a test means
//!
//! * **Interim** — "what has this range counted so far", before `flush()`. This is the level the
//!   engine reads on the failure path (`ENG-008`) and for the flush threshold (`MIG-004`), so it
//!   is the level a test of either of those must assert.
//! * **Committed** — "what has been folded into the totals". This is what the final block
//!   (`MET-006`), the `run_info` strings (`MET-005`, `TRK-021`) and every `cdm-assert.sh`-style
//!   check see, so it is the level a parity test asserts.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use cdm_core::{CdmError, ErrorKind};
use cdm_metrics::{CounterKind, CounterView, JobCounters};

/// An expected set of counter values, checked against one accounting level (`TST-100`).
///
/// ```
/// use cdm_core::JobKind;
/// use cdm_metrics::{CounterKind, CounterView, JobCounters};
/// use cdm_testkit::CounterExpectation;
///
/// let range = JobCounters::new(JobKind::Migrate);
/// let read = range.counter(CounterKind::Read)?;
/// let write = range.counter(CounterKind::Write)?;
/// range.increment_by(read, 10);
/// range.increment_by(write, 9);
///
/// // Before the flush, the work is interim and the totals are still zero.
/// CounterExpectation::new().read(10).write(9).check(&range, CounterView::Interim)?;
/// CounterExpectation::new().check(&range, CounterView::Committed)?;
///
/// range.flush();
/// CounterExpectation::new().read(10).write(9).check(&range, CounterView::Committed)?;
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CounterExpectation {
    expected: BTreeMap<CounterKind, u64>,
    exhaustive: bool,
}

impl CounterExpectation {
    /// An expectation that every registered counter is zero.
    ///
    /// Exhaustive by default: a counter the test does not mention must be zero. That is the same
    /// contract as a SIT `.assert` file, which lists the whole final block, and it is what makes
    /// an accidental increment somewhere else in the engine fail a test that was not looking for
    /// it. Relax it with [`CounterExpectation::ignoring_unstated`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            expected: BTreeMap::new(),
            exhaustive: true,
        }
    }

    /// Stops requiring unstated counters to be zero.
    ///
    /// For the rare test that genuinely cares about one counter and cannot predict the others —
    /// a rate-limiting test, say, where the row count is timing-dependent.
    #[must_use]
    pub const fn ignoring_unstated(mut self) -> Self {
        self.exhaustive = false;
        self
    }

    /// Expects a counter to hold `value`.
    ///
    /// Stating the same counter twice replaces the earlier value rather than accumulating, so a
    /// helper that builds a base expectation can be overridden by the test that uses it.
    #[must_use]
    pub fn expect(mut self, kind: CounterKind, value: u64) -> Self {
        self.expected.insert(kind, value);
        self
    }

    /// The counters this expectation states, and their values.
    pub fn stated(&self) -> &BTreeMap<CounterKind, u64> {
        &self.expected
    }

    /// Checks the expectation against one accounting level of a counter set (`MET-004`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] describing every discrepancy at once — the first failure is rarely
    /// the informative one — and, when the *other* view would have satisfied the expectation, a
    /// note naming `MIG-004` and `ENG-008`, the two defects that reading the wrong level causes.
    ///
    /// Also [`ErrorKind::Internal`] if the expectation names a counter the job does not register
    /// (`MET-002`): a migrate job has no `MISMATCH`, and an assertion about one is a bug in the
    /// test, not a value of zero.
    pub fn check(&self, counters: &JobCounters, view: CounterView) -> Result<(), CdmError> {
        for kind in self.expected.keys() {
            if !counters.is_registered(*kind) {
                return Err(CdmError::new(
                    ErrorKind::Internal,
                    format!(
                        "the {} job does not register {kind} (MET-002), so no expectation about \
                         it can be meaningful",
                        counters.job()
                    ),
                ));
            }
        }

        let actual = counts(counters, view);
        let Some(report) = self.discrepancies(&actual, counters) else {
            return Ok(());
        };

        let other = match view {
            CounterView::Interim => CounterView::Committed,
            CounterView::Committed => CounterView::Interim,
        };
        let would_pass_at_other_view = self
            .discrepancies(&counts(counters, other), counters)
            .is_none();

        let mut message = format!(
            "counter expectation failed at the {} level for the {} job:\n{report}\n  actual: {}",
            view_name(view),
            counters.job(),
            counters.metrics(view)
        );
        if would_pass_at_other_view {
            let _ = write!(
                message,
                "\n\n  NOTE: this expectation holds at the {} level. Reading the wrong level is \
                 exactly the defect behind MIG-004 (an unreachable flush threshold read from a \
                 permanently-zero committed UNFLUSHED) and ENG-008 (a validate ERROR count \
                 computed from committed counts before flush, so always zero). Decide which \
                 accounting this test means: interim is what a range has counted so far, \
                 committed is what flush() folded into the totals and what the final block \
                 reports.",
                view_name(other)
            );
        }
        Err(CdmError::new(ErrorKind::Internal, message))
    }

    /// Checks the expectation against a Java-format final block (`MET-006`).
    ///
    /// The block reports *committed* counts by construction — it is printed after the run's
    /// counters have been folded — so no view argument is needed or accepted here. This is the
    /// `cdm-assert.sh` equivalent, for parity tests that have a log and not a registry.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the block cannot be parsed, or if any stated counter differs.
    pub fn check_final_block(&self, block: &str) -> Result<(), CdmError> {
        let actual = parse_final_block(block)?;
        self.check_parsed(&actual, "final block")
    }

    /// Checks the expectation against a metrics string such as `Read: 10; Write: 9`
    /// (`MET-005`) — the form stored in `cdm_run_info.run_info` and `cdm_run_details.run_info`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Internal`] if the string cannot be parsed, or if any stated counter differs.
    pub fn check_metrics_string(&self, metrics: &str) -> Result<(), CdmError> {
        let actual = parse_metrics_string(metrics)?;
        self.check_parsed(&actual, "metrics string")
    }

    /// Compares against counts parsed out of a rendered string, where "registered" means
    /// "present in the rendering".
    fn check_parsed(
        &self,
        actual: &BTreeMap<CounterKind, u64>,
        source: &str,
    ) -> Result<(), CdmError> {
        let mut problems = Vec::new();
        for (kind, expected) in &self.expected {
            match actual.get(kind) {
                Some(found) if found == expected => {}
                Some(found) => {
                    problems.push(format!("  {kind}: expected {expected}, found {found}"));
                }
                None => problems.push(format!(
                    "  {kind}: expected {expected}, but the {source} does not report it"
                )),
            }
        }
        if self.exhaustive {
            for (kind, found) in actual {
                if *found != 0 && !self.expected.contains_key(kind) {
                    problems.push(format!(
                        "  {kind}: expected 0 (unstated counters must be zero), found {found}"
                    ));
                }
            }
        }
        if problems.is_empty() {
            return Ok(());
        }
        problems.sort();
        Err(CdmError::new(
            ErrorKind::Internal,
            format!(
                "counter expectation failed against the {source}:\n{}",
                problems.join("\n")
            ),
        ))
    }

    /// Every way `actual` differs from this expectation, or `None` if it does not.
    fn discrepancies(
        &self,
        actual: &BTreeMap<CounterKind, u64>,
        counters: &JobCounters,
    ) -> Option<String> {
        let mut problems = Vec::new();
        for (kind, expected) in &self.expected {
            let found = actual.get(kind).copied().unwrap_or_default();
            if found != *expected {
                problems.push(format!("  {kind}: expected {expected}, found {found}"));
            }
        }
        if self.exhaustive {
            for kind in counters.registered() {
                if self.expected.contains_key(kind) {
                    continue;
                }
                let found = actual.get(kind).copied().unwrap_or_default();
                if found != 0 {
                    problems.push(format!(
                        "  {kind}: expected 0 (unstated counters must be zero), found {found}"
                    ));
                }
            }
        }
        if problems.is_empty() {
            return None;
        }
        problems.sort();
        Some(problems.join("\n"))
    }
}

/// The per-counter builder methods, one per counter of `MET-001`.
///
/// Written out rather than generated by a macro so that each carries the requirement it belongs
/// to, and so that `cargo doc` lists them.
impl CounterExpectation {
    /// Expects `READ` rows read from the origin.
    #[must_use]
    pub fn read(self, value: u64) -> Self {
        self.expect(CounterKind::Read, value)
    }

    /// Expects `WRITE` rows written and flushed (`MIG-005`).
    #[must_use]
    pub fn write(self, value: u64) -> Self {
        self.expect(CounterKind::Write, value)
    }

    /// Expects `MISMATCH` differing rows (`VAL-002`).
    #[must_use]
    pub fn mismatch(self, value: u64) -> Self {
        self.expect(CounterKind::Mismatch, value)
    }

    /// Expects `CORRECTED_MISMATCH` rows autocorrect rewrote (`VAL-005`).
    #[must_use]
    pub fn corrected_mismatch(self, value: u64) -> Self {
        self.expect(CounterKind::CorrectedMismatch, value)
    }

    /// Expects `MISSING` rows absent from the target (`VAL-002`).
    #[must_use]
    pub fn missing(self, value: u64) -> Self {
        self.expect(CounterKind::Missing, value)
    }

    /// Expects `CORRECTED_MISSING` rows autocorrect inserted (`VAL-005`).
    #[must_use]
    pub fn corrected_missing(self, value: u64) -> Self {
        self.expect(CounterKind::CorrectedMissing, value)
    }

    /// Expects `VALID` rows that compared equal or passed the guardrail.
    #[must_use]
    pub fn valid(self, value: u64) -> Self {
        self.expect(CounterKind::Valid, value)
    }

    /// Expects `SKIPPED` rows a filter rejected (`MIG-002`, `MIG-003`).
    #[must_use]
    pub fn skipped(self, value: u64) -> Self {
        self.expect(CounterKind::Skipped, value)
    }

    /// Expects `LARGE` rows over a guardrail threshold (`GRD-001`).
    #[must_use]
    pub fn large(self, value: u64) -> Self {
        self.expect(CounterKind::Large, value)
    }

    /// Expects `ERROR` rows a failed range could not account for (`ENG-008`).
    #[must_use]
    pub fn error(self, value: u64) -> Self {
        self.expect(CounterKind::Error, value)
    }

    /// Expects `UNFLUSHED` buffered writes (`MIG-004`).
    ///
    /// Almost always an *interim* assertion: the committed value is reset before each flush and
    /// is what Java's unreachable threshold compares against.
    #[must_use]
    pub fn unflushed(self, value: u64) -> Self {
        self.expect(CounterKind::Unflushed, value)
    }

    /// Expects `PARTITIONS_PASSED` completed ranges.
    #[must_use]
    pub fn partitions_passed(self, value: u64) -> Self {
        self.expect(CounterKind::PartitionsPassed, value)
    }

    /// Expects `PARTITIONS_FAILED` failed ranges (`ENG-008`).
    #[must_use]
    pub fn partitions_failed(self, value: u64) -> Self {
        self.expect(CounterKind::PartitionsFailed, value)
    }
}

/// Every registered counter's value at one accounting level.
pub fn counts(counters: &JobCounters, view: CounterView) -> BTreeMap<CounterKind, u64> {
    counters
        .registered()
        .iter()
        .map(|&kind| (kind, counters.count_of(kind, view)))
        .collect()
}

/// The name of an accounting level, for a message.
const fn view_name(view: CounterView) -> &'static str {
    match view {
        CounterView::Interim => "interim",
        CounterView::Committed => "committed",
    }
}

/// Parses a Java-format final block (`MET-006`) back into counts.
///
/// The inverse of [`JobCounters::final_block`], and the Rust equivalent of what `cdm-assert.sh`
/// does with `grep` and `awk`. Lines that are not counter lines — the rule, the `RunId` — are
/// ignored, so a whole log can be passed in.
///
/// # Errors
///
/// [`ErrorKind::Internal`] if a counter line's value is not a number, or if the text contains no
/// counter lines at all — which almost always means the block was never printed, and silently
/// asserting "all zero" against nothing is how a test passes for the wrong reason.
pub fn parse_final_block(text: &str) -> Result<BTreeMap<CounterKind, u64>, CdmError> {
    let mut counts = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((label, value)) = line.rsplit_once(':') else {
            continue;
        };
        let Some(name) = label
            .trim()
            .strip_prefix("Final ")
            .map(|rest| rest.trim_end_matches(" Record Count").trim())
        else {
            continue;
        };
        let Some(kind) = CounterKind::ALL
            .into_iter()
            .find(|kind| kind.title_case() == name)
        else {
            continue;
        };
        let value: u64 = value.trim().parse().map_err(|e| {
            CdmError::new(
                ErrorKind::Internal,
                format!("`{line}` does not carry a counter value: {e}"),
            )
        })?;
        counts.insert(kind, value);
    }
    if counts.is_empty() {
        return Err(CdmError::new(
            ErrorKind::Internal,
            "no `Final … Count:` lines found; was the final block (MET-006) printed at all?"
                .to_owned(),
        ));
    }
    Ok(counts)
}

/// Parses a metrics string such as `Read: 10; Write: 9; Skipped: 1` (`MET-005`).
///
/// # Errors
///
/// [`ErrorKind::Internal`] if a segment is not `<Title Case Name>: <number>`. Unlike
/// [`parse_final_block`], nothing is skipped: the metrics string is a whole value, not a line in
/// a log, so an unrecognised segment means the format changed and `COMPAT-004` is broken.
pub fn parse_metrics_string(metrics: &str) -> Result<BTreeMap<CounterKind, u64>, CdmError> {
    let mut counts = BTreeMap::new();
    for segment in metrics.split(';') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let (name, value) = segment.split_once(':').unwrap_or((segment, ""));
        let kind = CounterKind::ALL
            .into_iter()
            .find(|kind| kind.title_case() == name.trim())
            .ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Internal,
                    format!("`{name}` is not one of the counters of MET-001"),
                )
            })?;
        let value: u64 = value.trim().parse().map_err(|e| {
            CdmError::new(
                ErrorKind::Internal,
                format!("`{segment}` does not carry a counter value: {e}"),
            )
        })?;
        counts.insert(kind, value);
    }
    Ok(counts)
}

/// Asserts counter values at an explicit accounting level (`TST-100`, `MET-004`).
///
/// The [`CounterView`] is the second argument and has no default, for the reason spelled out in
/// the module documentation: a helper that guesses the level reproduces `MIG-004` and `ENG-008`
/// in the tests meant to catch them.
///
/// ```
/// use cdm_core::JobKind;
/// use cdm_metrics::{CounterKind, CounterView, JobCounters};
/// use cdm_testkit::assert_counters;
///
/// let range = JobCounters::new(JobKind::Migrate);
/// let read = range.counter(CounterKind::Read)?;
/// range.increment_by(read, 3);
/// range.flush();
///
/// assert_counters!(&range, CounterView::Committed, { Read => 3 });
/// # Ok::<(), cdm_core::CdmError>(())
/// ```
#[macro_export]
macro_rules! assert_counters {
    ($counters:expr, $view:expr, { $($kind:ident => $value:expr),* $(,)? }) => {{
        let expectation = $crate::CounterExpectation::new()
            $(.expect($crate::reexport::CounterKind::$kind, $value))*;
        let outcome = expectation.check($counters, $view).err().map(|e| e.to_string());
        assert!(outcome.is_none(), "{}", outcome.unwrap_or_default());
    }};
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
    use cdm_core::JobKind;

    use super::*;

    fn migrate_range(read: u64, write: u64) -> JobCounters {
        let counters = JobCounters::new(JobKind::Migrate);
        let read_token = counters.counter(CounterKind::Read).unwrap();
        let write_token = counters.counter(CounterKind::Write).unwrap();
        counters.increment_by(read_token, read);
        counters.increment_by(write_token, write);
        counters
    }

    #[test]
    fn tst_100_an_expectation_holds_at_the_level_the_work_is_at() {
        let range = migrate_range(10, 9);

        CounterExpectation::new()
            .read(10)
            .write(9)
            .check(&range, CounterView::Interim)
            .unwrap();
        // Nothing has been folded into the totals yet.
        CounterExpectation::new()
            .check(&range, CounterView::Committed)
            .unwrap();

        range.flush();
        CounterExpectation::new()
            .read(10)
            .write(9)
            .check(&range, CounterView::Committed)
            .unwrap();
    }

    #[test]
    fn mig_004_asserting_the_wrong_level_says_so_and_names_the_defect() {
        // The shape of both Java defects: work exists at the interim level, and the assertion
        // reads the committed one.
        let range = migrate_range(10, 9);
        let err = CounterExpectation::new()
            .read(10)
            .write(9)
            .check(&range, CounterView::Committed)
            .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("committed level"), "{message}");
        assert!(message.contains("READ: expected 10, found 0"), "{message}");
        assert!(message.contains("MIG-004"), "{message}");
        assert!(message.contains("ENG-008"), "{message}");
        assert!(message.contains("holds at the interim level"), "{message}");
    }

    #[test]
    fn eng_008_the_note_appears_in_the_other_direction_too() {
        let range = migrate_range(10, 9);
        range.flush();
        let err = CounterExpectation::new()
            .read(10)
            .write(9)
            .check(&range, CounterView::Interim)
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("interim level"), "{message}");
        assert!(
            message.contains("holds at the committed level"),
            "{message}"
        );
    }

    #[test]
    fn tst_100_a_genuine_discrepancy_is_reported_without_the_level_note() {
        let range = migrate_range(10, 9);
        range.flush();
        let err = CounterExpectation::new()
            .read(11)
            .write(9)
            .check(&range, CounterView::Committed)
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("READ: expected 11, found 10"), "{message}");
        assert!(
            !message.contains("NOTE:"),
            "the note must not fire when neither level matches: {message}"
        );
    }

    #[test]
    fn tst_100_every_discrepancy_is_reported_not_just_the_first() {
        let range = migrate_range(10, 9);
        range.flush();
        let message = CounterExpectation::new()
            .read(1)
            .write(2)
            .skipped(3)
            .check(&range, CounterView::Committed)
            .unwrap_err()
            .to_string();
        assert!(message.contains("READ: expected 1"), "{message}");
        assert!(message.contains("WRITE: expected 2"), "{message}");
        assert!(message.contains("SKIPPED: expected 3"), "{message}");
        // And the actual metrics string is quoted for context.
        assert!(message.contains("Read: 10; Write: 9"), "{message}");
    }

    #[test]
    fn tst_100_unstated_counters_must_be_zero_unless_that_is_relaxed() {
        let range = migrate_range(10, 9);
        range.flush();

        let err = CounterExpectation::new()
            .read(10)
            .check(&range, CounterView::Committed)
            .unwrap_err();
        assert!(
            err.to_string().contains("WRITE: expected 0"),
            "{}",
            err.to_string()
        );

        CounterExpectation::new()
            .read(10)
            .ignoring_unstated()
            .check(&range, CounterView::Committed)
            .unwrap();
    }

    #[test]
    fn met_002_an_expectation_about_an_unregistered_counter_is_rejected() {
        let range = JobCounters::new(JobKind::Migrate);
        let err = CounterExpectation::new()
            .mismatch(0)
            .check(&range, CounterView::Committed)
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("MISMATCH"), "{message}");
        assert!(message.contains("MET-002"), "{message}");
        assert!(message.contains("migrate"), "{message}");
    }

    #[test]
    fn tst_100_stating_a_counter_twice_replaces_rather_than_accumulates() {
        let expectation = CounterExpectation::new().read(1).read(5);
        assert_eq!(expectation.stated().get(&CounterKind::Read), Some(&5));
        assert_eq!(expectation.stated().len(), 1);
    }

    #[test]
    fn tst_100_the_macro_requires_a_view_and_passes_when_the_counts_agree() {
        let range = migrate_range(3, 3);
        assert_counters!(&range, CounterView::Interim, { Read => 3, Write => 3 });
        range.flush();
        assert_counters!(&range, CounterView::Committed, { Read => 3, Write => 3 });
    }

    #[test]
    fn tst_100_the_macro_fails_loudly_with_the_full_report() {
        let range = migrate_range(3, 3);
        let failure = std::panic::catch_unwind(|| {
            assert_counters!(&range, CounterView::Committed, { Read => 3, Write => 3 });
        })
        .unwrap_err();
        let message = failure
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(message.contains("MIG-004"), "{message}");
    }

    #[test]
    fn met_006_a_final_block_parses_back_into_the_counts_it_renders() {
        let range = migrate_range(10, 9);
        range.flush();
        let block = range.final_block(None);

        let parsed = parse_final_block(&block).unwrap();
        assert_eq!(parsed.get(&CounterKind::Read), Some(&10));
        assert_eq!(parsed.get(&CounterKind::Write), Some(&9));
        assert_eq!(parsed.get(&CounterKind::PartitionsPassed), Some(&0));
        // UNFLUSHED is never rendered in a committed block (MET-005).
        assert_eq!(parsed.get(&CounterKind::Unflushed), None);

        CounterExpectation::new()
            .read(10)
            .write(9)
            .check_final_block(&block)
            .unwrap();

        let err = CounterExpectation::new()
            .read(11)
            .check_final_block(&block)
            .unwrap_err();
        assert!(err.to_string().contains("final block"), "{err}");
    }

    #[test]
    fn met_006_an_absent_final_block_is_an_error_not_an_all_zero_pass() {
        // The failure mode this guards: a test greps a log that never contained a block, parses
        // nothing, and "passes" against an empty map.
        let err = parse_final_block("nothing to see here\nRunId: 5").unwrap_err();
        assert!(err.to_string().contains("MET-006"), "{err}");

        let err = parse_final_block("Final Read Record Count: banana").unwrap_err();
        assert!(err.to_string().contains("banana"), "{err}");
    }

    #[test]
    fn met_005_a_metrics_string_parses_back_into_the_counts_it_renders() {
        let range = migrate_range(10, 9);
        range.flush();
        let metrics = range.metrics(CounterView::Committed);

        let parsed = parse_metrics_string(&metrics).unwrap();
        assert_eq!(parsed.get(&CounterKind::Read), Some(&10));
        assert_eq!(parsed.get(&CounterKind::Write), Some(&9));

        CounterExpectation::new()
            .read(10)
            .write(9)
            .check_metrics_string(&metrics)
            .unwrap();

        // The interim rendering carries Unflushed, which the committed one never does.
        let interim = range.metrics(CounterView::Interim);
        assert!(parse_metrics_string(&interim)
            .unwrap()
            .contains_key(&CounterKind::Unflushed));
    }

    #[test]
    fn met_005_an_unrecognised_segment_is_an_error() {
        let err = parse_metrics_string("Read: 1; Frobnicated: 2").unwrap_err();
        assert!(err.to_string().contains("MET-001"), "{err}");
        let err = parse_metrics_string("Read: banana").unwrap_err();
        assert!(err.to_string().contains("banana"), "{err}");
    }

    #[test]
    fn tst_100_check_parsed_reports_a_counter_the_rendering_omits() {
        let block = JobCounters::new(JobKind::Migrate).final_block(None);
        let err = CounterExpectation::new()
            .unflushed(3)
            .check_final_block(&block)
            .unwrap_err();
        assert!(err.to_string().contains("does not report it"), "{err}");
    }

    #[test]
    fn tst_100_counts_reports_every_registered_counter() {
        let range = JobCounters::new(JobKind::Guardrail);
        let map = counts(&range, CounterView::Committed);
        assert_eq!(map.len(), range.registered().len());
        assert!(map.values().all(|v| *v == 0));
        assert_eq!(view_name(CounterView::Interim), "interim");
        assert_eq!(view_name(CounterView::Committed), "committed");
    }
}
