//! Run identity and lifecycle: [`RunId`], [`RunStatus`], [`JobKind`] and [`Side`].

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicI64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{CdmError, ErrorKind};

/// Which cluster an operation, error or metric concerns.
///
/// Defined here rather than in `cdm-cql` (where `ARCHITECTURE.md` §3.1 lists it) because
/// [`ErrorContext`](crate::ErrorContext) has to name a side and `cdm-core` cannot depend on
/// `cdm-cql`. `cdm-cql` re-exports this type rather than defining a second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// The source cluster data is read from.
    Origin,
    /// The destination cluster data is written to.
    Target,
}

impl Side {
    /// The stable lowercase string form, as used in property names and metric labels.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Origin => "origin",
            Self::Target => "target",
        }
    }

    /// The other side.
    #[must_use]
    pub const fn opposite(&self) -> Self {
        match self {
            Self::Origin => Self::Target,
            Self::Target => Self::Origin,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One of the built-in job types (`SPEC.md` §2; extensible via `PLG-004`).
///
/// The set is closed: a job contributed by a [`JobPlugin`](crate::JobPlugin) is identified by its
/// plugin name, not by a `JobKind`, so third-party jobs cannot collide with the parity three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    /// Copy rows from origin to target.
    Migrate,
    /// Compare origin and target, optionally correcting differences.
    Validate,
    /// Inspect origin rows against configured limits without writing anything.
    Guardrail,
}

impl JobKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 3] = [Self::Migrate, Self::Validate, Self::Guardrail];

    /// The stable lowercase string form used by the CLI (`cdm migrate`), the REST API and the
    /// `run_type` column of `cdm_run_info`.
    ///
    /// Note that the exact spelling Java writes into `run_type` is not pinned by `SPEC.md`;
    /// establishing that byte-for-byte is `COMPAT-003`'s job in `cdm-track` (PR #25). Until then
    /// this is the cdm-rs spelling only, and no compatibility claim is made for it.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Migrate => "migrate",
            Self::Validate => "validate",
            Self::Guardrail => "guardrail",
        }
    }
}

impl fmt::Display for JobKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for JobKind {
    type Err = CdmError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| {
                CdmError::new(ErrorKind::Config, format!("unknown job type `{s}`"))
                    .with_context(|c| c.with_config_key("job"))
            })
    }
}

/// The status of a run, or of one range within a run (`TRK-012`).
///
/// The first seven variants are Java's, and their string forms are a compatibility contract: a
/// Java run must be resumable by cdm-rs and vice versa (`COMPAT-003`), and both tools read and
/// write these exact strings in the `status` column of `cdm_run_info` and `cdm_run_details`
/// (`TRK-010`). [`RunStatus::Interrupted`] and [`RunStatus::Aborted`] are cdm-rs additions and,
/// per `TRK-012`, appear on the run row only — Java ignores statuses it does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunStatus {
    /// Planned but not yet claimed by a worker.
    NotStarted,
    /// Claimed by a worker and in progress.
    Started,
    /// Completed successfully: migrated, or validated with no differences.
    Pass,
    /// Failed with an error, a panic, or a lost lease.
    Fail,
    /// Validation found differences that were not corrected.
    Diff,
    /// Validation found differences and corrected all of them.
    DiffCorrected,
    /// The run as a whole finished. Terminal for the run row only.
    Ended,
    /// **cdm-rs only.** The run was stopped by a signal and can be resumed.
    Interrupted,
    /// **cdm-rs only.** The run was stopped deliberately, for example because the error limit was
    /// exceeded.
    Aborted,
}

impl RunStatus {
    /// Every status, in declaration order.
    pub const ALL: [Self; 9] = [
        Self::NotStarted,
        Self::Started,
        Self::Pass,
        Self::Fail,
        Self::Diff,
        Self::DiffCorrected,
        Self::Ended,
        Self::Interrupted,
        Self::Aborted,
    ];

    /// The string written to the `status` column — the exact spelling Java uses (`TRK-012`).
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotStarted => "NOT_STARTED",
            Self::Started => "STARTED",
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Diff => "DIFF",
            Self::DiffCorrected => "DIFF_CORRECTED",
            Self::Ended => "ENDED",
            Self::Interrupted => "INTERRUPTED",
            Self::Aborted => "ABORTED",
        }
    }

    /// Whether Java CDM knows this status. The two cdm-rs additions do not round-trip through a
    /// Java reader, which is why `TRK-012` confines them to the run row.
    pub const fn is_java_compatible(&self) -> bool {
        !matches!(self, Self::Interrupted | Self::Aborted)
    }

    /// Whether a range in this status still has work outstanding, i.e. whether resuming a run
    /// must re-plan it. `TRK-031` defines the set as `{NOT_STARTED, STARTED, FAIL, DIFF}`.
    pub const fn is_pending(&self) -> bool {
        matches!(
            self,
            Self::NotStarted | Self::Started | Self::Fail | Self::Diff
        )
    }
}

impl fmt::Display for RunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RunStatus {
    type Err = CdmError;

    /// Parses the Java spelling. Case-insensitive, because operators type these by hand into
    /// `cdm runs` filters (`TRK-034`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| CdmError::new(ErrorKind::Tracking, format!("unknown run status `{s}`")))
    }
}

/// Number of low bits of a [`RunId`] given over to the collision counter (`TRK-003`).
const RUN_ID_COUNTER_BITS: u32 = 12;

/// The largest counter value that fits in [`RUN_ID_COUNTER_BITS`].
const RUN_ID_COUNTER_MASK: i64 = (1 << RUN_ID_COUNTER_BITS) - 1;

/// The identifier of a single execution of a job (`TRK-002`, `TRK-003`).
///
/// Stored as a CQL `bigint` (`TRK-010`), so the representation is `i64`. The layout is
/// `unix_micros << 12 | counter`:
///
/// * **time-sortable** — comparing ids compares wall-clock time, which is what Java's
///   "the latest run is the highest run id" logic in `TRK-030` relies on;
/// * **monotonic** — [`RunIdGenerator`] never emits the same id twice, even when several runs
///   start within the same microsecond or the clock steps backwards;
/// * **collision-free across nodes** in practice, unlike Java's `System.nanoTime()`, whose value
///   is per-JVM and unrelated to wall-clock time.
///
/// The microsecond field holds 51 usable bits, which runs out in the year 2255.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(i64);

impl RunId {
    /// The sentinel Java uses for "no run id supplied" (`TRK-001`, `TRK-002`).
    pub const UNSET: Self = Self(0);

    /// Builds an id from an explicit timestamp and counter.
    ///
    /// Taking the timestamp as a parameter is deliberate: it keeps this constructor pure and
    /// therefore deterministically testable. Reading the clock is [`RunIdGenerator`]'s job, and
    /// even there it is the caller who supplies the microseconds.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if `unix_micros` is negative or too large to fit in the
    /// 51 bits above the counter, or if `counter` exceeds 4095.
    pub fn from_parts(unix_micros: i64, counter: u16) -> Result<Self, CdmError> {
        if unix_micros < 0 {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                format!("run id timestamp {unix_micros} is before the unix epoch"),
            ));
        }
        if i64::from(counter) > RUN_ID_COUNTER_MASK {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                format!("run id counter {counter} exceeds the {RUN_ID_COUNTER_BITS}-bit field"),
            ));
        }
        let shifted = unix_micros.checked_shl(RUN_ID_COUNTER_BITS).filter(|v| {
            // `checked_shl` only guards the shift amount, not the loss of high bits.
            v >> RUN_ID_COUNTER_BITS == unix_micros && *v >= 0
        });
        let Some(shifted) = shifted else {
            return Err(CdmError::new(
                ErrorKind::Tracking,
                format!("run id timestamp {unix_micros} does not fit in 51 bits"),
            ));
        };
        Ok(Self(shifted | i64::from(counter)))
    }

    /// Wraps an id read from `cdm_run_info`, or supplied by the operator via `track_run.run_id`.
    ///
    /// No validation: Java-generated ids are `System.nanoTime()` values with no internal
    /// structure, and cdm-rs must be able to resume them (`COMPAT-003`). Consequently
    /// [`RunId::unix_micros`] is meaningful only for ids cdm-rs generated itself.
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    /// The `bigint` written to `cdm_run_info.run_id`.
    pub const fn as_i64(&self) -> i64 {
        self.0
    }

    /// Whether this is the "no run id supplied" sentinel.
    pub const fn is_unset(&self) -> bool {
        self.0 == 0
    }

    /// The timestamp component. Meaningless for ids not generated by cdm-rs.
    pub const fn unix_micros(&self) -> i64 {
        self.0 >> RUN_ID_COUNTER_BITS
    }

    /// The collision-counter component. Meaningless for ids not generated by cdm-rs.
    pub const fn counter(&self) -> u16 {
        // Masked to 12 bits, so the truncation is exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            (self.0 & RUN_ID_COUNTER_MASK) as u16
        }
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Issues strictly increasing [`RunId`]s (`TRK-003`).
///
/// The generator is pure: the caller supplies the current time, which keeps it deterministically
/// testable and keeps `cdm-core` free of clock access. `cdm-track` owns the single instance and
/// feeds it `Utc::now()`.
///
/// Strict monotonicity is preserved even when the clock repeats or steps backwards, by advancing
/// into the counter field. Sharing is lock-free, so several workers may generate ids at once.
#[derive(Debug)]
pub struct RunIdGenerator {
    last: AtomicI64,
}

impl RunIdGenerator {
    /// A generator that has not yet issued an id.
    pub const fn new() -> Self {
        Self {
            last: AtomicI64::new(0),
        }
    }

    /// Issues the next id for the given wall-clock time.
    ///
    /// The result is `max(unix_micros << 12, previous + 1)`, so ids are strictly increasing and,
    /// as long as the clock behaves, time-sortable.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`] if `unix_micros` is out of range (see
    /// [`RunId::from_parts`]), or if more than 4096 ids are requested within one microsecond and
    /// the counter field is exhausted for that microsecond *and* every subsequent one up to
    /// `i64::MAX`, which cannot occur in practice.
    pub fn next(&self, unix_micros: i64) -> Result<RunId, CdmError> {
        let floor = RunId::from_parts(unix_micros, 0)?.as_i64();
        loop {
            let last = self.last.load(Ordering::Acquire);
            let candidate = if floor > last {
                floor
            } else {
                last.checked_add(1).ok_or_else(|| {
                    CdmError::new(ErrorKind::Tracking, "run id space is exhausted")
                })?
            };
            if self
                .last
                .compare_exchange_weak(last, candidate, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(RunId::from_raw(candidate));
            }
        }
    }
}

impl Default for RunIdGenerator {
    fn default() -> Self {
        Self::new()
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
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn trk_012_statuses_use_the_exact_java_strings() {
        assert_eq!(
            RunStatus::ALL.map(|s| s.as_str()),
            [
                "NOT_STARTED",
                "STARTED",
                "PASS",
                "FAIL",
                "DIFF",
                "DIFF_CORRECTED",
                "ENDED",
                "INTERRUPTED",
                "ABORTED",
            ]
        );
    }

    #[test]
    fn trk_012_statuses_round_trip_through_their_java_strings() {
        for status in RunStatus::ALL {
            assert_eq!(RunStatus::from_str(status.as_str()).unwrap(), status);
            assert_eq!(status.to_string(), status.as_str());
            // Serde must agree with the hand-written spelling: the same value goes to the
            // tracking table and to the REST API.
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{}\"", status.as_str())
            );
        }
    }

    #[test]
    fn trk_012_status_parsing_is_case_insensitive_and_rejects_unknown_values() {
        assert_eq!(
            RunStatus::from_str("diff_corrected").unwrap(),
            RunStatus::DiffCorrected
        );
        let err = RunStatus::from_str("SORT_OF_OK").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Tracking);
        assert!(err.to_string().contains("SORT_OF_OK"));
    }

    #[test]
    fn trk_012_the_two_cdm_rs_additions_are_flagged_as_java_incompatible() {
        assert!(!RunStatus::Interrupted.is_java_compatible());
        assert!(!RunStatus::Aborted.is_java_compatible());
        assert_eq!(
            RunStatus::ALL
                .iter()
                .filter(|s| s.is_java_compatible())
                .count(),
            7
        );
    }

    #[test]
    fn trk_012_pending_statuses_are_the_ones_a_resume_replans() {
        let pending: Vec<RunStatus> = RunStatus::ALL
            .into_iter()
            .filter(RunStatus::is_pending)
            .collect();
        assert_eq!(
            pending,
            vec![
                RunStatus::NotStarted,
                RunStatus::Started,
                RunStatus::Fail,
                RunStatus::Diff,
            ]
        );
    }

    #[test]
    fn trk_003_run_id_packs_micros_above_a_twelve_bit_counter() {
        let id = RunId::from_parts(1_712_345_678_901_234, 5).unwrap();
        assert_eq!(id.unix_micros(), 1_712_345_678_901_234);
        assert_eq!(id.counter(), 5);
        assert_eq!(id.as_i64(), (1_712_345_678_901_234 << 12) | 5);
        assert_eq!(id.to_string(), id.as_i64().to_string());
    }

    #[test]
    fn trk_003_run_ids_sort_by_time_then_counter() {
        let earlier = RunId::from_parts(1_000, 4095).unwrap();
        let later = RunId::from_parts(1_001, 0).unwrap();
        assert!(earlier < later, "the later microsecond must win");
        assert!(RunId::from_parts(1_000, 0).unwrap() < earlier);

        let mut ids: Vec<RunId> = vec![later, earlier];
        ids.sort_unstable();
        assert_eq!(ids, vec![earlier, later]);
    }

    #[test]
    fn trk_003_out_of_range_parts_are_rejected() {
        assert_eq!(
            RunId::from_parts(-1, 0).unwrap_err().kind(),
            ErrorKind::Tracking
        );
        assert!(RunId::from_parts(1, 4096).is_err());
        assert!(RunId::from_parts(1, 4095).is_ok());
        // 52 bits of microseconds no longer fit above the counter.
        assert!(RunId::from_parts(1 << 51, 0).is_err());
        assert!(RunId::from_parts((1 << 51) - 1, 0).is_ok());
    }

    #[test]
    fn trk_003_raw_ids_from_java_are_accepted_unvalidated() {
        // A `System.nanoTime()` value, which has no internal structure.
        let java = RunId::from_raw(9_223_372_036_854_775_807);
        assert_eq!(java.as_i64(), i64::MAX);
        assert!(!java.is_unset());
        assert!(RunId::UNSET.is_unset());
        assert_eq!(RunId::UNSET.as_i64(), 0);
    }

    #[test]
    fn trk_003_generator_is_strictly_monotonic_within_one_microsecond() {
        let generator = RunIdGenerator::new();
        let ids: Vec<RunId> = (0..5)
            .map(|_| generator.next(1_000_000).unwrap())
            .collect::<Vec<_>>();
        for pair in ids.windows(2) {
            assert!(pair[0] < pair[1], "{pair:?} is not increasing");
        }
        assert_eq!(ids[0].counter(), 0);
        assert_eq!(ids[4].counter(), 4);
        assert_eq!(
            ids.iter().collect::<BTreeSet<_>>().len(),
            ids.len(),
            "ids must be unique"
        );
    }

    #[test]
    fn trk_003_generator_survives_a_clock_that_steps_backwards() {
        let generator = RunIdGenerator::default();
        let first = generator.next(2_000_000).unwrap();
        let second = generator.next(1_000_000).unwrap();
        assert!(second > first, "monotonicity outranks the clock");
        let third = generator.next(3_000_000).unwrap();
        assert_eq!(third.unix_micros(), 3_000_000);
        assert_eq!(third.counter(), 0);
    }

    #[test]
    fn trk_003_generator_rejects_an_out_of_range_clock() {
        assert!(RunIdGenerator::new().next(-1).is_err());
    }

    #[test]
    fn trk_003_run_id_serialises_as_a_bare_integer() {
        let id = RunId::from_parts(42, 1).unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, id.as_i64().to_string());
        assert_eq!(serde_json::from_str::<RunId>(&json).unwrap(), id);
    }

    #[test]
    fn plg_004_job_kinds_are_the_three_built_ins() {
        assert_eq!(
            JobKind::ALL.map(|k| k.as_str()),
            ["migrate", "validate", "guardrail"]
        );
        for kind in JobKind::ALL {
            assert_eq!(JobKind::from_str(kind.as_str()).unwrap(), kind);
            assert_eq!(kind.to_string(), kind.as_str());
            assert_eq!(
                serde_json::to_string(&kind).unwrap(),
                format!("\"{}\"", kind.as_str())
            );
        }
        assert_eq!(JobKind::from_str("MIGRATE").unwrap(), JobKind::Migrate);
        let err = JobKind::from_str("backup").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert_eq!(err.context().config_key.as_deref(), Some("job"));
    }

    #[test]
    fn err_001_sides_render_lowercase_and_can_be_flipped() {
        assert_eq!(Side::Origin.to_string(), "origin");
        assert_eq!(Side::Target.as_str(), "target");
        assert_eq!(Side::Origin.opposite(), Side::Target);
        assert_eq!(Side::Target.opposite(), Side::Origin);
        assert_eq!(serde_json::to_string(&Side::Origin).unwrap(), "\"origin\"");
    }
}
