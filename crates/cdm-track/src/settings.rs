//! Whether tracking is on, and which run id it uses (`TRK-001`, `TRK-002`, `TRK-003`).
//!
//! Java decides both of these in `BasePartitionJob`, spread over a null check on
//! `trackRunFeature` and a `runId == 0` test. Gathering them into one value has a practical
//! consequence: the decision is taken once, before anything connects, and every later question
//! ("do I write a details row?", "am I resuming?") is answered by reading a field rather than by
//! re-deriving the condition — which is how Java ends up with `trackRun` enabled and `runId` still
//! zero in some code paths.

use cdm_config::CdmConfig;
use cdm_core::{CdmError, RunId, RunIdGenerator};
use chrono::{DateTime, Utc};

/// The `track_run.*` settings, resolved into the decisions the rest of the crate needs.
///
/// Built with [`TrackingSettings::from_config`]. The individual fields map one-for-one onto the
/// properties of `SPEC.md` §3.5 and their `spark.cdm.trackRun*` aliases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackingSettings {
    /// `track_run.enabled` / `spark.cdm.trackRun`.
    pub enabled: bool,
    /// `track_run.run_id` / `spark.cdm.trackRun.runId`. [`RunId::UNSET`] means "allocate one".
    pub run_id: RunId,
    /// `track_run.previous_run_id` / `spark.cdm.trackRun.previousRunId`.
    pub previous_run_id: RunId,
    /// `track_run.auto_rerun` / `spark.cdm.trackRun.autoRerun`.
    pub auto_rerun: bool,
    /// `track_run.rerun_multiplier` / `spark.cdm.trackRun.rerunMultiplier`.
    pub rerun_multiplier: u32,
}

impl TrackingSettings {
    /// Reads the `track_run` section of a validated configuration.
    pub fn from_config(config: &CdmConfig) -> Self {
        Self {
            enabled: config.track_run.enabled,
            run_id: RunId::from_raw(config.track_run.run_id),
            previous_run_id: RunId::from_raw(config.track_run.previous_run_id),
            auto_rerun: config.track_run.auto_rerun,
            rerun_multiplier: config.track_run.rerun_multiplier,
        }
    }

    /// Settings with tracking off, which is the default (`CFG-150`).
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            run_id: RunId::UNSET,
            previous_run_id: RunId::UNSET,
            auto_rerun: false,
            rerun_multiplier: 1,
        }
    }

    /// Whether tracking is on (`TRK-001`).
    ///
    /// Four independent switches turn it on, and any one of them is enough: setting a run id, or
    /// naming a previous run, or asking for an automatic rerun all *imply* `track_run.enabled`,
    /// because none of them means anything without a tracking table to read and write. Java
    /// derives the same disjunction in `PropertyHelper`; reproducing it exactly matters because
    /// an operator who sets only `spark.cdm.trackRun.previousRunId` expects the resume to happen.
    pub const fn is_enabled(&self) -> bool {
        self.enabled
            || !self.run_id.is_unset()
            || !self.previous_run_id.is_unset()
            || self.auto_rerun
    }

    /// The run id to use, allocating one if the operator did not supply it (`TRK-002`).
    ///
    /// `now` is a parameter rather than a call to `Utc::now()` so that the allocation is a pure
    /// function of its inputs and can be tested without a clock. `generator` is the process-wide
    /// [`RunIdGenerator`]; sharing one is what makes ids unique when several runs start together.
    ///
    /// Returns `None` when tracking is off: allocating an id for a run nobody will record is how
    /// a "tracking disabled" run ends up printing a `RunId:` line that resolves to nothing.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](cdm_core::ErrorKind::Tracking) if the clock is outside the
    /// representable range — see [`RunId::from_parts`].
    pub fn resolve_run_id(
        &self,
        generator: &RunIdGenerator,
        now: DateTime<Utc>,
    ) -> Result<Option<RunId>, CdmError> {
        if !self.is_enabled() {
            return Ok(None);
        }
        if !self.run_id.is_unset() {
            return Ok(Some(self.run_id));
        }
        Ok(Some(generator.next(now.timestamp_micros())?))
    }

    /// The rerun multiplier, floored at one (`TRK-033`).
    ///
    /// Java compares `rerunMultiplier > 1` and otherwise ignores the value, so a configured `0`
    /// behaves as `1` there. Tier-1 validation already rejects `0`, but the floor is applied here
    /// too: a multiplier of zero reaching the subdivider would produce *no* ranges, which is the
    /// one outcome a resume must never have.
    pub const fn effective_rerun_multiplier(&self) -> u32 {
        if self.rerun_multiplier < 1 {
            1
        } else {
            self.rerun_multiplier
        }
    }
}

impl Default for TrackingSettings {
    fn default() -> Self {
        Self::disabled()
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

    fn at(micros: i64) -> DateTime<Utc> {
        DateTime::from_timestamp_micros(micros).unwrap()
    }

    #[test]
    fn trk_001_any_one_of_the_four_switches_enables_tracking() {
        assert!(!TrackingSettings::disabled().is_enabled());

        let mut only_enabled = TrackingSettings::disabled();
        only_enabled.enabled = true;
        assert!(only_enabled.is_enabled());

        let mut only_run_id = TrackingSettings::disabled();
        only_run_id.run_id = RunId::from_raw(7);
        assert!(only_run_id.is_enabled());

        let mut only_previous = TrackingSettings::disabled();
        only_previous.previous_run_id = RunId::from_raw(7);
        assert!(only_previous.is_enabled());

        let mut only_auto = TrackingSettings::disabled();
        only_auto.auto_rerun = true;
        assert!(only_auto.is_enabled());
    }

    #[test]
    fn trk_001_the_default_configuration_does_not_track() {
        let settings = TrackingSettings::from_config(&CdmConfig::default());
        assert_eq!(settings, TrackingSettings::disabled());
        assert!(!settings.is_enabled());
    }

    #[test]
    fn trk_001_settings_are_read_from_the_track_run_section() {
        let mut config = CdmConfig::default();
        config.track_run.enabled = true;
        config.track_run.run_id = 11;
        config.track_run.previous_run_id = 10;
        config.track_run.auto_rerun = true;
        config.track_run.rerun_multiplier = 4;

        let settings = TrackingSettings::from_config(&config);
        assert!(settings.enabled);
        assert_eq!(settings.run_id, RunId::from_raw(11));
        assert_eq!(settings.previous_run_id, RunId::from_raw(10));
        assert!(settings.auto_rerun);
        assert_eq!(settings.effective_rerun_multiplier(), 4);
    }

    #[test]
    fn trk_002_an_unset_run_id_is_allocated_and_a_set_one_is_honoured() {
        let generator = RunIdGenerator::new();
        let mut settings = TrackingSettings::disabled();
        settings.enabled = true;

        let allocated = settings
            .resolve_run_id(&generator, at(1_000_000))
            .unwrap()
            .unwrap();
        assert!(!allocated.is_unset());
        assert_eq!(allocated.unix_micros(), 1_000_000);

        settings.run_id = RunId::from_raw(42);
        assert_eq!(
            settings.resolve_run_id(&generator, at(2_000_000)).unwrap(),
            Some(RunId::from_raw(42)),
            "an operator-supplied id must be used verbatim, including a Java nanoTime value"
        );
    }

    #[test]
    fn trk_002_no_run_id_is_allocated_when_tracking_is_off() {
        let generator = RunIdGenerator::new();
        assert_eq!(
            TrackingSettings::disabled()
                .resolve_run_id(&generator, at(1_000_000))
                .unwrap(),
            None
        );
    }

    #[test]
    fn trk_003_allocated_ids_increase_even_within_one_microsecond() {
        let generator = RunIdGenerator::new();
        let mut settings = TrackingSettings::disabled();
        settings.auto_rerun = true;

        let first = settings
            .resolve_run_id(&generator, at(5_000_000))
            .unwrap()
            .unwrap();
        let second = settings
            .resolve_run_id(&generator, at(5_000_000))
            .unwrap()
            .unwrap();
        assert!(second > first);
    }

    #[test]
    fn trk_033_the_multiplier_never_falls_below_one() {
        let mut settings = TrackingSettings::disabled();
        settings.rerun_multiplier = 0;
        assert_eq!(settings.effective_rerun_multiplier(), 1);
        settings.rerun_multiplier = 1;
        assert_eq!(settings.effective_rerun_multiplier(), 1);
        settings.rerun_multiplier = 9;
        assert_eq!(settings.effective_rerun_multiplier(), 9);
    }
}
