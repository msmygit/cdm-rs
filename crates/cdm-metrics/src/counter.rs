//! The counter vocabulary (`MET-001`) and the per-job registration sets (`MET-002`).

use std::fmt;
use std::str::FromStr;

use cdm_core::{CdmError, ErrorKind, JobKind};

/// One of the thirteen counters Java CDM maintains (`MET-001`).
///
/// The declaration order is Java's `JobCounter.CounterType` declaration order, and it is a
/// compatibility contract rather than a matter of taste: Java renders both the metrics string
/// (`MET-005`) and the final block (`MET-006`) by iterating `CounterType.values()`, so this order
/// *is* the order counters appear in `cdm_run_info.run_info` and in every SIT `.assert` file
/// (`COMPAT-004`). Never reorder these variants.
///
/// ```
/// use cdm_metrics::CounterKind;
///
/// assert_eq!(CounterKind::CorrectedMismatch.as_str(), "CORRECTED_MISMATCH");
/// assert_eq!(CounterKind::CorrectedMismatch.title_case(), "Corrected Mismatch");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CounterKind {
    /// Rows read from the origin.
    Read,
    /// Rows written to the target, incremented only once a write has been flushed (`MIG-005`).
    Write,
    /// Rows present on both sides whose values differ (`VAL-002`).
    Mismatch,
    /// Mismatched rows that autocorrect rewrote (`VAL-005`).
    CorrectedMismatch,
    /// Rows present on the origin and absent from the target (`VAL-002`).
    Missing,
    /// Missing rows that autocorrect inserted (`VAL-005`).
    CorrectedMissing,
    /// Rows that compared equal, or that passed the guardrail.
    Valid,
    /// Rows a filter rejected, or that produced no statement to execute (`MIG-002`, `MIG-003`).
    Skipped,
    /// Rows exceeding a guardrail threshold (`GRD-001`).
    Large,
    /// Rows a failed range could not account for (`ENG-008`).
    Error,
    /// Writes issued but not yet flushed. Bookkeeping for the flush threshold of `MIG-004`, not a
    /// result: it is omitted from every committed rendering (`MET-005`, `MET-006`).
    Unflushed,
    /// Token ranges that completed successfully.
    PartitionsPassed,
    /// Token ranges that failed.
    PartitionsFailed,
}

impl CounterKind {
    /// How many counters there are. The width of the array inside
    /// [`JobCounters`](crate::JobCounters).
    pub const COUNT: usize = 13;

    /// Every counter, in Java's declaration order — which is also the rendering order of
    /// `MET-005` and `MET-006`.
    pub const ALL: [Self; Self::COUNT] = [
        Self::Read,
        Self::Write,
        Self::Mismatch,
        Self::CorrectedMismatch,
        Self::Missing,
        Self::CorrectedMissing,
        Self::Valid,
        Self::Skipped,
        Self::Large,
        Self::Error,
        Self::Unflushed,
        Self::PartitionsPassed,
        Self::PartitionsFailed,
    ];

    /// The `SCREAMING_SNAKE_CASE` name, identical to the Java enum constant. This is the name
    /// exposed to Prometheus, OTLP and the REST API, and the key of
    /// [`MetricsSnapshot::counters`](cdm_core::MetricsSnapshot::counters).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "READ",
            Self::Write => "WRITE",
            Self::Mismatch => "MISMATCH",
            Self::CorrectedMismatch => "CORRECTED_MISMATCH",
            Self::Missing => "MISSING",
            Self::CorrectedMissing => "CORRECTED_MISSING",
            Self::Valid => "VALID",
            Self::Skipped => "SKIPPED",
            Self::Large => "LARGE",
            Self::Error => "ERROR",
            Self::Unflushed => "UNFLUSHED",
            Self::PartitionsPassed => "PARTITIONS_PASSED",
            Self::PartitionsFailed => "PARTITIONS_FAILED",
        }
    }

    /// The human-facing spelling used by the metrics string and the final block.
    ///
    /// Java computes this at render time with
    /// `StringUtils.capitalize` over the underscore-separated, lower-cased words
    /// (`JobCounter.printFriendlyCase`). cdm-rs writes the results down instead, because they are
    /// a parity contract that must not silently follow a change in a case-conversion helper.
    /// `met_005_title_case_matches_the_java_algorithm` re-derives every one of them.
    pub const fn title_case(self) -> &'static str {
        match self {
            Self::Read => "Read",
            Self::Write => "Write",
            Self::Mismatch => "Mismatch",
            Self::CorrectedMismatch => "Corrected Mismatch",
            Self::Missing => "Missing",
            Self::CorrectedMissing => "Corrected Missing",
            Self::Valid => "Valid",
            Self::Skipped => "Skipped",
            Self::Large => "Large",
            Self::Error => "Error",
            Self::Unflushed => "Unflushed",
            Self::PartitionsPassed => "Partitions Passed",
            Self::PartitionsFailed => "Partitions Failed",
        }
    }

    /// This counter's slot in [`CounterKind::ALL`], and therefore in the atomic array of
    /// [`JobCounters`](crate::JobCounters). Always less than [`CounterKind::COUNT`].
    pub const fn index(self) -> usize {
        match self {
            Self::Read => 0,
            Self::Write => 1,
            Self::Mismatch => 2,
            Self::CorrectedMismatch => 3,
            Self::Missing => 4,
            Self::CorrectedMissing => 5,
            Self::Valid => 6,
            Self::Skipped => 7,
            Self::Large => 8,
            Self::Error => 9,
            Self::Unflushed => 10,
            Self::PartitionsPassed => 11,
            Self::PartitionsFailed => 12,
        }
    }

    /// Whether this counter counts token ranges rather than records.
    ///
    /// The final block renders the two range counters as `Final Partitions Passed: N` and
    /// everything else as `Final <Name> Record Count: N` (`MET-006`).
    pub const fn is_partition_counter(self) -> bool {
        matches!(self, Self::PartitionsPassed | Self::PartitionsFailed)
    }

    /// Whether the given job registers this counter (`MET-002`).
    ///
    /// This is a `const fn`, which is how `MET-003` is met at *compile* time: a caller that knows
    /// its job statically can assert its counters in a `const` block, and a wrong pair fails the
    /// build rather than the run.
    ///
    /// ```compile_fail
    /// use cdm_core::JobKind;
    /// use cdm_metrics::CounterKind;
    ///
    /// // A migrate job has no MISMATCH counter, so this does not compile.
    /// const _: () = assert!(CounterKind::Mismatch.is_registered_for(JobKind::Migrate));
    /// ```
    ///
    /// ```
    /// use cdm_core::JobKind;
    /// use cdm_metrics::CounterKind;
    ///
    /// const _: () = assert!(CounterKind::Write.is_registered_for(JobKind::Migrate));
    /// ```
    pub const fn is_registered_for(self, job: JobKind) -> bool {
        match job {
            JobKind::Migrate => matches!(
                self,
                Self::Read
                    | Self::Write
                    | Self::Skipped
                    | Self::Error
                    | Self::Unflushed
                    | Self::PartitionsPassed
                    | Self::PartitionsFailed
            ),
            JobKind::Validate => matches!(
                self,
                Self::Read
                    | Self::Valid
                    | Self::Mismatch
                    | Self::CorrectedMismatch
                    | Self::Missing
                    | Self::CorrectedMissing
                    | Self::Skipped
                    | Self::Error
                    | Self::PartitionsPassed
                    | Self::PartitionsFailed
            ),
            JobKind::Guardrail => matches!(
                self,
                Self::Read
                    | Self::Valid
                    | Self::Skipped
                    | Self::Large
                    | Self::PartitionsPassed
                    | Self::PartitionsFailed
            ),
        }
    }
}

impl fmt::Display for CounterKind {
    /// The `SCREAMING_SNAKE_CASE` name. The title-cased rendering is
    /// [`CounterKind::title_case`], deliberately not a `Display` impl, so that no parity-critical
    /// string can be produced by an accidental `{}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CounterKind {
    type Err = CdmError;

    /// Parses the `SCREAMING_SNAKE_CASE` name, case-insensitively, so that a counter named on the
    /// command line or in an API filter round-trips.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| CdmError::new(ErrorKind::Internal, format!("unknown counter `{s}`")))
    }
}

/// The counters a job registers, in rendering order (`MET-002`).
///
/// ```
/// use cdm_core::JobKind;
/// use cdm_metrics::{registered_counters, CounterKind};
///
/// assert_eq!(
///     registered_counters(JobKind::Guardrail),
///     &[
///         CounterKind::Read,
///         CounterKind::Valid,
///         CounterKind::Skipped,
///         CounterKind::Large,
///         CounterKind::PartitionsPassed,
///         CounterKind::PartitionsFailed,
///     ],
/// );
/// ```
pub const fn registered_counters(job: JobKind) -> &'static [CounterKind] {
    match job {
        JobKind::Migrate => MIGRATE_COUNTERS,
        JobKind::Validate => VALIDATE_COUNTERS,
        JobKind::Guardrail => GUARDRAIL_COUNTERS,
    }
}

/// `MET-002`, migrate row. Kept in [`CounterKind::ALL`] order, which is the rendering order.
const MIGRATE_COUNTERS: &[CounterKind] = &[
    CounterKind::Read,
    CounterKind::Write,
    CounterKind::Skipped,
    CounterKind::Error,
    CounterKind::Unflushed,
    CounterKind::PartitionsPassed,
    CounterKind::PartitionsFailed,
];

/// `MET-002`, validate row. Also used by the fix (autocorrect) variants of the validate job,
/// which Java models as the same `JobType.VALIDATE`.
const VALIDATE_COUNTERS: &[CounterKind] = &[
    CounterKind::Read,
    CounterKind::Mismatch,
    CounterKind::CorrectedMismatch,
    CounterKind::Missing,
    CounterKind::CorrectedMissing,
    CounterKind::Valid,
    CounterKind::Skipped,
    CounterKind::Error,
    CounterKind::PartitionsPassed,
    CounterKind::PartitionsFailed,
];

/// `MET-002`, guardrail row.
const GUARDRAIL_COUNTERS: &[CounterKind] = &[
    CounterKind::Read,
    CounterKind::Valid,
    CounterKind::Skipped,
    CounterKind::Large,
    CounterKind::PartitionsPassed,
    CounterKind::PartitionsFailed,
];

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

    /// Java's `JobCounter.printFriendlyCase`, transliterated, so that
    /// [`CounterKind::title_case`] can be checked against the algorithm it replaces rather than
    /// against a second hand-written list.
    fn print_friendly_case(name: &str) -> String {
        name.to_lowercase()
            .split('_')
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn met_001_the_thirteen_counters_are_javas_in_javas_order() {
        assert_eq!(CounterKind::ALL.len(), CounterKind::COUNT);
        assert_eq!(
            CounterKind::ALL.map(CounterKind::as_str),
            [
                "READ",
                "WRITE",
                "MISMATCH",
                "CORRECTED_MISMATCH",
                "MISSING",
                "CORRECTED_MISSING",
                "VALID",
                "SKIPPED",
                "LARGE",
                "ERROR",
                "UNFLUSHED",
                "PARTITIONS_PASSED",
                "PARTITIONS_FAILED",
            ],
        );
    }

    #[test]
    fn met_001_every_kind_indexes_its_own_slot() {
        // The invariant the `indexing_slicing` allow in `JobCounters::unit` rests on.
        for (slot, kind) in CounterKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.index(), slot, "{kind} indexes the wrong slot");
            assert!(kind.index() < CounterKind::COUNT);
        }
    }

    #[test]
    fn met_001_names_round_trip_case_insensitively() {
        for kind in CounterKind::ALL {
            assert_eq!(CounterKind::from_str(kind.as_str()).unwrap(), kind);
            assert_eq!(
                CounterKind::from_str(&kind.as_str().to_lowercase()).unwrap(),
                kind
            );
            assert_eq!(kind.to_string(), kind.as_str());
        }
        let err = CounterKind::from_str("ROWS_READ").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert!(err.to_string().contains("ROWS_READ"));
    }

    #[test]
    fn met_005_title_case_matches_the_java_algorithm() {
        for kind in CounterKind::ALL {
            assert_eq!(kind.title_case(), print_friendly_case(kind.as_str()));
        }
    }

    #[test]
    fn met_002_registration_matches_java_for_every_job() {
        assert_eq!(
            registered_counters(JobKind::Migrate)
                .iter()
                .copied()
                .map(CounterKind::as_str)
                .collect::<Vec<_>>(),
            vec![
                "READ",
                "WRITE",
                "SKIPPED",
                "ERROR",
                "UNFLUSHED",
                "PARTITIONS_PASSED",
                "PARTITIONS_FAILED",
            ],
        );
        assert_eq!(
            registered_counters(JobKind::Validate)
                .iter()
                .copied()
                .map(CounterKind::as_str)
                .collect::<Vec<_>>(),
            vec![
                "READ",
                "MISMATCH",
                "CORRECTED_MISMATCH",
                "MISSING",
                "CORRECTED_MISSING",
                "VALID",
                "SKIPPED",
                "ERROR",
                "PARTITIONS_PASSED",
                "PARTITIONS_FAILED",
            ],
        );
        assert_eq!(
            registered_counters(JobKind::Guardrail)
                .iter()
                .copied()
                .map(CounterKind::as_str)
                .collect::<Vec<_>>(),
            vec![
                "READ",
                "VALID",
                "SKIPPED",
                "LARGE",
                "PARTITIONS_PASSED",
                "PARTITIONS_FAILED",
            ],
        );
    }

    #[test]
    fn met_002_the_registration_lists_agree_with_the_const_predicate() {
        for job in JobKind::ALL {
            let from_predicate: Vec<CounterKind> = CounterKind::ALL
                .into_iter()
                .filter(|kind| kind.is_registered_for(job))
                .collect();
            assert_eq!(
                from_predicate,
                registered_counters(job),
                "{job} disagrees with itself"
            );
        }
    }

    #[test]
    fn met_002_registration_lists_are_in_rendering_order() {
        for job in JobKind::ALL {
            let indices: Vec<usize> = registered_counters(job)
                .iter()
                .map(|kind| kind.index())
                .collect();
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            assert_eq!(indices, sorted, "{job} is not in CounterKind::ALL order");
        }
    }

    #[test]
    fn met_002_unflushed_belongs_to_migrate_alone() {
        // Only migrate buffers writes, so only migrate can have unflushed ones.
        assert!(CounterKind::Unflushed.is_registered_for(JobKind::Migrate));
        assert!(!CounterKind::Unflushed.is_registered_for(JobKind::Validate));
        assert!(!CounterKind::Unflushed.is_registered_for(JobKind::Guardrail));
        // LARGE is the mirror image: guardrail alone.
        assert!(CounterKind::Large.is_registered_for(JobKind::Guardrail));
        assert!(!CounterKind::Large.is_registered_for(JobKind::Migrate));
        assert!(!CounterKind::Large.is_registered_for(JobKind::Validate));
        // Java's guardrail job registers no ERROR counter.
        assert!(!CounterKind::Error.is_registered_for(JobKind::Guardrail));
    }

    #[test]
    fn met_006_only_the_two_partition_counters_count_ranges() {
        let partition: Vec<CounterKind> = CounterKind::ALL
            .into_iter()
            .filter(|kind| kind.is_partition_counter())
            .collect();
        assert_eq!(
            partition,
            vec![CounterKind::PartitionsPassed, CounterKind::PartitionsFailed]
        );
    }
}
