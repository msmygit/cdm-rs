//! The Java-format reporter: the metrics string (`MET-005`) and the final block (`MET-006`).
//!
//! Everything in this module is a byte-for-byte compatibility contract (`COMPAT-004`). Users'
//! scripts parse these strings, Java CDM's `SIT/cdm-assert.sh` greps them, and the same strings
//! are written into `cdm_run_info.run_info` and `cdm_run_details.run_info`, from where a Java run
//! may read them back (`TRK-021`, `TRK-022`, `TRK-030`). A separator or a capital letter changed
//! here breaks other people's tooling silently.

use cdm_core::RunId;

use crate::counter::CounterKind;
use crate::registry::{CounterView, JobCounters};

/// The separator between two entries of the metrics string (`MET-005`).
///
/// Java appends `"; "` after every entry and then removes the last two characters, which makes it
/// a separator rather than a terminator.
pub const METRIC_SEPARATOR: &str = "; ";

/// The `#` rule that opens and closes the final block (`MET-006`): ninety-six `#` characters,
/// as emitted by Java's `JobCounter.printMetrics`.
pub const FINAL_BLOCK_RULE: &str =
    "################################################################################################";

impl JobCounters {
    /// The metrics string (`MET-005`), e.g. `Read: 10; Write: 9; Skipped: 1`.
    ///
    /// Entries appear in [`CounterKind::ALL`] order, restricted to the counters this job
    /// registers (`MET-002`), title-cased and separated by [`METRIC_SEPARATOR`]. `UNFLUSHED` is
    /// omitted from the [`CounterView::Committed`] rendering and included in the
    /// [`CounterView::Interim`] one, because Java's `getMetrics(boolean interim)` does exactly
    /// that: the interim rendering is a debugging aid logged on the range-failure path, where the
    /// number of writes still in flight is the interesting quantity.
    ///
    /// ```
    /// use cdm_core::JobKind;
    /// use cdm_metrics::{CounterKind, CounterView, JobCounters};
    ///
    /// let counters = JobCounters::new(JobKind::Migrate);
    /// counters.increment_by(counters.counter(CounterKind::Read)?, 10);
    /// counters.increment_by(counters.counter(CounterKind::Write)?, 9);
    /// counters.increment_by(counters.counter(CounterKind::Skipped)?, 1);
    /// counters.increment_by(counters.counter(CounterKind::PartitionsPassed)?, 1);
    /// counters.flush();
    ///
    /// assert_eq!(
    ///     counters.metrics(CounterView::Committed),
    ///     "Read: 10; Write: 9; Skipped: 1; Error: 0; Partitions Passed: 1; Partitions Failed: 0",
    /// );
    /// # Ok::<(), cdm_core::CdmError>(())
    /// ```
    #[must_use]
    pub fn metrics(&self, view: CounterView) -> String {
        self.registered()
            .iter()
            .filter(|kind| view == CounterView::Interim || **kind != CounterKind::Unflushed)
            .map(|&kind| format!("{}: {}", kind.title_case(), self.count_of(kind, view)))
            .collect::<Vec<_>>()
            .join(METRIC_SEPARATOR)
    }

    /// The string written to `cdm_run_details.run_info` for a range, and to
    /// `cdm_run_info.run_info` for the run (`TRK-021`, `TRK-022`).
    ///
    /// This is the committed rendering, matching Java, which calls `getMetrics()` — the
    /// no-argument, non-interim overload — *after* flushing the range. For a per-range
    /// `JobCounters` the two levels hold the same numbers at that point, so the range's
    /// contribution is what is recorded either way; the distinction that matters is that
    /// `UNFLUSHED` is absent from the stored string.
    #[must_use]
    pub fn run_info(&self) -> String {
        self.metrics(CounterView::Committed)
    }

    /// The lines of the final block (`MET-006`), without their line terminators.
    ///
    /// `run_id` is `Some` when run tracking is enabled, which is precisely when Java emits the
    /// `RunId:` line — it prints it inside the `null != trackRunFeature` branch of
    /// `printMetrics`.
    #[must_use]
    pub fn final_block_lines(&self, run_id: Option<RunId>) -> Vec<String> {
        let mut lines = Vec::with_capacity(self.registered().len() + 3);
        lines.push(FINAL_BLOCK_RULE.to_owned());
        if let Some(run_id) = run_id {
            lines.push(format!("RunId: {run_id}"));
        }
        for &kind in self.registered() {
            if kind == CounterKind::Unflushed {
                continue;
            }
            let count = self.count_of(kind, CounterView::Committed);
            let name = kind.title_case();
            if kind.is_partition_counter() {
                lines.push(format!("Final {name}: {count}"));
            } else {
                lines.push(format!("Final {name} Record Count: {count}"));
            }
        }
        lines.push(FINAL_BLOCK_RULE.to_owned());
        lines
    }

    /// The final block (`MET-006`) as one newline-separated string, without a trailing newline.
    ///
    /// ```
    /// use cdm_core::{JobKind, RunId};
    /// use cdm_metrics::{CounterKind, JobCounters};
    ///
    /// let counters = JobCounters::new(JobKind::Guardrail);
    /// counters.increment_by(counters.counter(CounterKind::Read)?, 4);
    /// counters.increment_by(counters.counter(CounterKind::Valid)?, 1);
    /// counters.increment_by(counters.counter(CounterKind::Large)?, 3);
    /// counters.increment_by(counters.counter(CounterKind::PartitionsPassed)?, 1);
    /// counters.flush();
    ///
    /// let block = counters.final_block(Some(RunId::from_raw(1_712_345_678_901_234)));
    /// assert!(block.contains("RunId: 1712345678901234"));
    /// assert!(block.contains("Final Large Record Count: 3"));
    /// assert!(block.contains("Final Partitions Passed: 1"));
    /// # Ok::<(), cdm_core::CdmError>(())
    /// ```
    #[must_use]
    pub fn final_block(&self, run_id: Option<RunId>) -> String {
        self.final_block_lines(run_id).join("\n")
    }

    /// Emits the final block (`MET-006`) to `tracing`, one event per line, at `INFO`.
    ///
    /// One event per line rather than one multi-line event because that is what Java's log4j
    /// output looks like, and `cdm-assert.sh` greps the log line by line.
    pub fn log_final_block(&self, run_id: Option<RunId>) {
        for line in self.final_block_lines(run_id) {
            tracing::info!(target: "cdm::metrics", "{line}");
        }
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
    use cdm_core::JobKind;

    use super::*;

    /// Applies `SIT/cdm-assert.sh`'s transformation to a final block: keep the `Final ` lines,
    /// strip everything up to and including `Final `. The shell does it with
    /// `egrep 'JobCounter.* Final ' | sed 's/^.*Final //'`; the result is what a `.assert` file
    /// contains, so a byte-for-byte comparison against a checked-in Java fixture is the strongest
    /// parity evidence available short of running the Java jar.
    fn as_assert_file(block: &str) -> String {
        let mut out = String::new();
        for line in block.lines() {
            if let Some((_, tail)) = line.split_once("Final ") {
                out.push_str(tail);
                out.push('\n');
            }
        }
        out
    }

    /// Builds a set of counters with the given values already committed.
    fn committed(job: JobKind, values: &[(CounterKind, u64)]) -> JobCounters {
        let counters = JobCounters::new(job);
        for &(kind, value) in values {
            counters.increment_by(counters.counter(kind).unwrap(), value);
        }
        counters.flush();
        counters
    }

    #[test]
    fn met_005_renders_the_example_from_the_specification() {
        let counters = committed(
            JobKind::Migrate,
            &[
                (CounterKind::Read, 10),
                (CounterKind::Write, 9),
                (CounterKind::Skipped, 1),
            ],
        );
        // The SPEC example `Read: 10; Write: 9; Skipped: 1` shows the shape, not a whole
        // migrate rendering: every registered counter is always present, zero or not.
        let metrics = counters.metrics(CounterView::Committed);
        assert!(
            metrics.starts_with("Read: 10; Write: 9; Skipped: 1"),
            "{metrics}"
        );
        insta::assert_snapshot!(
            metrics,
            @"Read: 10; Write: 9; Skipped: 1; Error: 0; Partitions Passed: 0; Partitions Failed: 0"
        );
    }

    #[test]
    fn met_005_entries_are_separated_by_semicolon_space_and_never_terminated_by_one() {
        let counters = committed(JobKind::Migrate, &[(CounterKind::Read, 1)]);
        let metrics = counters.metrics(CounterView::Committed);

        assert!(!metrics.ends_with(';'), "{metrics}");
        assert!(!metrics.ends_with(' '), "{metrics}");
        assert_eq!(metrics.matches("; ").count(), 5, "{metrics}");
        assert_eq!(metrics.matches(';').count(), 5, "{metrics}");
        // The name/value separator is a colon and a single space.
        assert!(metrics.starts_with("Read: 1;"), "{metrics}");
        assert!(!metrics.contains(" :"), "{metrics}");
        assert!(!metrics.contains("  "), "{metrics}");
    }

    #[test]
    fn met_005_names_are_title_cased_from_screaming_snake_case() {
        let counters = JobCounters::new(JobKind::Validate);
        let metrics = counters.metrics(CounterView::Committed);
        insta::assert_snapshot!(
            metrics,
            @"Read: 0; Mismatch: 0; Corrected Mismatch: 0; Missing: 0; Corrected Missing: 0; Valid: 0; Skipped: 0; Error: 0; Partitions Passed: 0; Partitions Failed: 0"
        );
        assert!(!metrics.contains('_'), "{metrics}");
        assert!(!metrics.contains("CORRECTED"), "{metrics}");
    }

    #[test]
    fn met_005_unflushed_is_omitted_from_the_committed_rendering() {
        let counters = JobCounters::new(JobKind::Migrate);
        counters.increment_by(counters.counter(CounterKind::Unflushed).unwrap(), 7);

        let interim = counters.metrics(CounterView::Interim);
        assert!(interim.contains("Unflushed: 7"), "{interim}");
        insta::assert_snapshot!(
            interim,
            @"Read: 0; Write: 0; Skipped: 0; Error: 0; Unflushed: 7; Partitions Passed: 0; Partitions Failed: 0"
        );

        let committed = counters.metrics(CounterView::Committed);
        assert!(!committed.contains("Unflushed"), "{committed}");
        assert!(!counters.run_info().contains("Unflushed"));
    }

    #[test]
    fn met_005_only_migrate_can_show_unflushed_at_all() {
        for job in [JobKind::Validate, JobKind::Guardrail] {
            let counters = JobCounters::new(job);
            assert!(!counters.metrics(CounterView::Interim).contains("Unflushed"));
        }
    }

    #[test]
    fn met_005_entries_follow_the_java_enum_declaration_order() {
        let metrics = JobCounters::new(JobKind::Validate).metrics(CounterView::Committed);
        let names: Vec<&str> = metrics
            .split(METRIC_SEPARATOR)
            .filter_map(|entry| entry.split(':').next())
            .collect();
        assert_eq!(
            names,
            vec![
                "Read",
                "Mismatch",
                "Corrected Mismatch",
                "Missing",
                "Corrected Missing",
                "Valid",
                "Skipped",
                "Error",
                "Partitions Passed",
                "Partitions Failed",
            ],
        );
    }

    #[test]
    fn met_004_the_interim_rendering_reports_the_range_and_the_committed_one_the_totals() {
        let run = JobCounters::new(JobKind::Migrate);
        let range = JobCounters::new(JobKind::Migrate);
        range.increment_by(range.counter(CounterKind::Read).unwrap(), 4);

        // Before the range completes, the run has nothing and the range has interim work.
        assert!(range.metrics(CounterView::Interim).starts_with("Read: 4"));
        assert!(run.run_info().starts_with("Read: 0"));

        range.flush();
        run.add(&range).unwrap();
        assert_eq!(range.run_info(), run.run_info());
        assert!(run.run_info().starts_with("Read: 4"));
    }

    #[test]
    fn met_006_reproduces_the_java_banner_for_a_migrate_run() {
        let counters = committed(
            JobKind::Migrate,
            &[
                (CounterKind::Read, 1_000_000),
                (CounterKind::Write, 999_998),
                (CounterKind::Skipped, 2),
                (CounterKind::PartitionsPassed, 5_000),
            ],
        );
        insta::assert_snapshot!(
            counters.final_block(Some(RunId::from_raw(1_712_345_678_901_234))),
            @r"
        ################################################################################################
        RunId: 1712345678901234
        Final Read Record Count: 1000000
        Final Write Record Count: 999998
        Final Skipped Record Count: 2
        Final Error Record Count: 0
        Final Partitions Passed: 5000
        Final Partitions Failed: 0
        ################################################################################################
        "
        );
    }

    #[test]
    fn met_006_the_rule_is_ninety_six_hashes_and_opens_and_closes_the_block() {
        assert_eq!(FINAL_BLOCK_RULE.len(), 96);
        assert!(FINAL_BLOCK_RULE.chars().all(|c| c == '#'));

        let lines = JobCounters::new(JobKind::Migrate).final_block_lines(None);
        assert_eq!(lines.first().map(String::as_str), Some(FINAL_BLOCK_RULE));
        assert_eq!(lines.last().map(String::as_str), Some(FINAL_BLOCK_RULE));
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.as_str() == FINAL_BLOCK_RULE)
                .count(),
            2
        );
    }

    #[test]
    fn met_006_the_run_id_line_appears_only_when_run_tracking_is_enabled() {
        let counters = JobCounters::new(JobKind::Migrate);
        let untracked = counters.final_block(None);
        assert!(!untracked.contains("RunId"), "{untracked}");

        let tracked = counters.final_block(Some(RunId::from_raw(42)));
        let lines: Vec<&str> = tracked.lines().collect();
        // Immediately after the opening rule, as in `printMetrics`.
        assert_eq!(lines.get(1).copied(), Some("RunId: 42"));
        assert_eq!(lines.len(), untracked.lines().count() + 1);
    }

    #[test]
    fn met_006_partition_counters_drop_the_record_count_suffix() {
        let block = JobCounters::new(JobKind::Guardrail).final_block(None);
        assert!(block.contains("Final Partitions Passed: 0"), "{block}");
        assert!(block.contains("Final Partitions Failed: 0"), "{block}");
        assert!(!block.contains("Partitions Passed Record Count"), "{block}");
        assert!(block.contains("Final Read Record Count: 0"), "{block}");
    }

    #[test]
    fn met_006_unflushed_never_reaches_the_final_block() {
        let counters = JobCounters::new(JobKind::Migrate);
        counters.increment_by(counters.counter(CounterKind::Unflushed).unwrap(), 3);
        counters.flush();
        let block = counters.final_block(Some(RunId::from_raw(1)));
        assert!(!block.contains("Unflushed"), "{block}");
    }

    #[test]
    fn met_006_renders_the_validate_and_guardrail_blocks() {
        let validate = committed(
            JobKind::Validate,
            &[
                (CounterKind::Read, 7),
                (CounterKind::Valid, 7),
                (CounterKind::PartitionsPassed, 1),
            ],
        );
        insta::assert_snapshot!(
            "final_block_validate",
            validate.final_block(Some(RunId::from_raw(1_712_345_678_901_234)))
        );

        let guardrail = committed(
            JobKind::Guardrail,
            &[
                (CounterKind::Read, 4),
                (CounterKind::Valid, 1),
                (CounterKind::Large, 3),
                (CounterKind::PartitionsPassed, 1),
            ],
        );
        insta::assert_snapshot!(
            "final_block_guardrail",
            guardrail.final_block(Some(RunId::from_raw(1_712_345_678_901_234)))
        );
    }

    /// `SIT/smoke/04_counters/cdm.validateData.assert`, byte for byte.
    const SIT_SMOKE_04_VALIDATE_DATA: &str = "\
Read Record Count: 7
Mismatch Record Count: 0
Corrected Mismatch Record Count: 0
Missing Record Count: 0
Corrected Missing Record Count: 0
Valid Record Count: 7
Skipped Record Count: 0
Error Record Count: 0
Partitions Passed: 1
Partitions Failed: 0
";

    /// `SIT/features/05_guardrail/cdm.guardrailCheck.assert`, byte for byte.
    const SIT_FEATURES_05_GUARDRAIL: &str = "\
Read Record Count: 4
Valid Record Count: 1
Skipped Record Count: 0
Large Record Count: 3
Partitions Passed: 1
Partitions Failed: 0
";

    /// `SIT/features/08_map_columns_origin_target/cdm.migrateData.assert`, byte for byte.
    const SIT_FEATURES_08_MIGRATE_DATA: &str = "\
Read Record Count: 3
Write Record Count: 3
Skipped Record Count: 0
Error Record Count: 0
Partitions Passed: 1
Partitions Failed: 0
";

    #[test]
    fn met_006_matches_the_java_sit_assertion_for_validate() {
        let counters = committed(
            JobKind::Validate,
            &[
                (CounterKind::Read, 7),
                (CounterKind::Valid, 7),
                (CounterKind::PartitionsPassed, 1),
            ],
        );
        assert_eq!(
            as_assert_file(&counters.final_block(Some(RunId::from_raw(1)))),
            SIT_SMOKE_04_VALIDATE_DATA,
        );
    }

    #[test]
    fn met_006_matches_the_java_sit_assertion_for_a_correcting_validate() {
        // SIT/regression/03_performance/cdm.fixData.assert.
        let counters = committed(
            JobKind::Validate,
            &[
                (CounterKind::Read, 4_000),
                (CounterKind::Mismatch, 100),
                (CounterKind::CorrectedMismatch, 100),
                (CounterKind::Missing, 100),
                (CounterKind::CorrectedMissing, 100),
                (CounterKind::Valid, 3_800),
                (CounterKind::PartitionsPassed, 32),
            ],
        );
        assert_eq!(
            as_assert_file(&counters.final_block(None)),
            "\
Read Record Count: 4000
Mismatch Record Count: 100
Corrected Mismatch Record Count: 100
Missing Record Count: 100
Corrected Missing Record Count: 100
Valid Record Count: 3800
Skipped Record Count: 0
Error Record Count: 0
Partitions Passed: 32
Partitions Failed: 0
",
        );
    }

    #[test]
    fn met_006_matches_the_java_sit_assertion_for_guardrail() {
        let counters = committed(
            JobKind::Guardrail,
            &[
                (CounterKind::Read, 4),
                (CounterKind::Valid, 1),
                (CounterKind::Large, 3),
                (CounterKind::PartitionsPassed, 1),
            ],
        );
        assert_eq!(
            as_assert_file(&counters.final_block(None)),
            SIT_FEATURES_05_GUARDRAIL,
        );
    }

    #[test]
    fn met_006_matches_the_java_sit_assertion_for_migrate() {
        let counters = committed(
            JobKind::Migrate,
            &[
                (CounterKind::Read, 3),
                (CounterKind::Write, 3),
                (CounterKind::PartitionsPassed, 1),
            ],
        );
        assert_eq!(
            as_assert_file(&counters.final_block(None)),
            SIT_FEATURES_08_MIGRATE_DATA,
        );
    }

    #[test]
    fn eng_008_a_failed_range_reports_its_error_accounting_in_the_interim_string() {
        // The shape `CopyJobSession` logs on the failure path: ERROR is read − write − skipped
        // over the *interim* counts, then PARTITIONS_FAILED, then the interim metrics.
        let range = JobCounters::new(JobKind::Migrate);
        let read = range.counter(CounterKind::Read).unwrap();
        let write = range.counter(CounterKind::Write).unwrap();
        let skipped = range.counter(CounterKind::Skipped).unwrap();
        let error = range.counter(CounterKind::Error).unwrap();
        let failed = range.counter(CounterKind::PartitionsFailed).unwrap();

        range.increment_by(read, 10);
        range.increment_by(write, 6);
        range.increment_by(skipped, 1);

        let unaccounted = range.count(read, CounterView::Interim)
            - range.count(write, CounterView::Interim)
            - range.count(skipped, CounterView::Interim);
        range.increment_by(error, unaccounted);
        range.increment(failed);
        range.flush();

        assert_eq!(range.count(error, CounterView::Committed), 3);
        insta::assert_snapshot!(
            range.run_info(),
            @"Read: 10; Write: 6; Skipped: 1; Error: 3; Partitions Passed: 0; Partitions Failed: 1"
        );
    }

    #[test]
    fn met_006_logging_the_block_emits_one_event_per_line() {
        // The reporter must not fail or panic when no subscriber is installed.
        let counters = JobCounters::new(JobKind::Migrate);
        counters.log_final_block(Some(RunId::from_raw(1)));
        counters.log_final_block(None);
    }
}
