//! Recording a run so that it can be resumed (`TRK-010`, `TRK-020`..`TRK-022`, `TRK-038`).
//!
//! The harness plans a ring and runs it. Everything here is what makes that run *recoverable*:
//! the `cdm_run_info` row, one `cdm_run_details` row per planned range, and the terminal status
//! written when the scheduler stops. Without it a run that is interrupted on its fourth day has
//! left nothing behind for `cdm runs resume` to read, which is the same as having no resume.
//!
//! # Which counters are recorded, and whose
//!
//! Only the **committed** level, and only this run's (`MET-004`, `TRK-038`). A resumed run opens
//! its own `cdm_run_info` row with its own counters starting at zero, and names the run it
//! continues in `previous_run_id`. It does not inherit the previous run's totals: those totals
//! describe rows the previous run moved, they are recorded against the run that moved them, and
//! adding them here would report the same rows twice to anyone summing the history — while
//! `Partitions Failed`, which `TRK-030` reads to decide whether a run is worth resuming, would
//! describe an interruption that has already been dealt with.
//!
//! # What is not tracked
//!
//! A guardrail run. Tracking lives in the target keyspace (`TRK-010`) and `GRD-001` requires a
//! guardrail to open no target connection at all, so there is nowhere to write it. It is a
//! read-only report over the origin; there is nothing to resume.

use std::sync::Arc;

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind, JobKind, RunId, RunIdGenerator, TableRef, TrackingStore};
use cdm_engine::planner::TokenPlan;
use cdm_engine::scheduler::{RangeObserver, RunReport};
use cdm_track::tracker::{committed_run_info, new_run_record, RunTracker, TrackerConfig};
use cdm_track::CassandraStore;

use super::session::Sessions;

/// A run's tracking, or the absence of it.
///
/// `None` inside is not an error state: `track_run.enabled` is off by default, exactly as in Java,
/// and an untracked run is a supported way to use the tool. It simply cannot be resumed.
#[derive(Debug)]
pub struct Tracking {
    tracker: Option<Arc<RunTracker>>,
}

impl Tracking {
    /// A run that records nothing.
    pub const fn disabled() -> Self {
        Self { tracker: None }
    }

    /// Opens the tracking tables and records the plan (`TRK-020`).
    ///
    /// Returns [`Tracking::disabled`] when `track_run.enabled` is false, so callers have one path
    /// rather than two.
    ///
    /// `previous` is the run this one continues, recorded in `cdm_run_info.previous_run_id` so a
    /// chain of resumes can be traced back (`TRK-038`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Tracking`] if the tracking tables cannot be created or the run id already
    /// exists (`TRK-020`), and [`ErrorKind::Internal`] if the run has no target session.
    pub async fn start(
        config: &EffectiveConfig,
        sessions: &Sessions,
        job: JobKind,
        plan: &TokenPlan,
        previous: Option<RunId>,
    ) -> Result<Self, CdmError> {
        if !config.config().track_run.enabled {
            return Ok(Self::disabled());
        }
        let table = tracked_table(config)?;
        let store = open_store(sessions, &table)?;
        let record = new_run_record(plan.run_id(), previous, table, job);
        let tracker = RunTracker::start(
            store as Arc<dyn TrackingStore>,
            &record,
            &plan.token_ranges(),
            TrackerConfig::default(),
        )
        .await?;
        Ok(Self {
            tracker: Some(Arc::new(tracker)),
        })
    }

    /// Wraps an already-started tracker, for the resume path, which has to create the run row
    /// before it can plan against the store it opened.
    pub const fn started(tracker: Arc<RunTracker>) -> Self {
        Self {
            tracker: Some(tracker),
        }
    }

    /// The observer the scheduler reports ranges to (`TRK-021`), if this run is recorded.
    ///
    /// `None` rather than a `NoopObserver`, because the caller composes this with the live
    /// display's observer (`super::Observers`) and a no-op in that list is a call per range that
    /// does nothing. It also lets `Observers` recognise the "only one watcher" case and hand it to
    /// the scheduler directly.
    pub fn observer(&self) -> Option<Arc<dyn RangeObserver>> {
        self.tracker
            .as_ref()
            .map(|tracker| Arc::clone(tracker) as Arc<dyn RangeObserver>)
    }

    /// Whether this run is being recorded.
    pub const fn is_enabled(&self) -> bool {
        self.tracker.is_some()
    }

    /// Records the run's terminal status and its committed aggregate (`TRK-022`).
    ///
    /// The status is the scheduler's, never `ENDED` unconditionally: a run stopped by a signal
    /// (`ENG-010`) or by the error limit (`ENG-009`) must stay resumable, and `TRK-030` decides
    /// that by reading this column.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Tracking`] if the final write fails. A failure leaves the run short of
    /// `ENDED`, which a later resume reads as unfinished — the safe direction.
    pub async fn finish(&self, report: &RunReport) -> Result<(), CdmError> {
        let Some(tracker) = &self.tracker else {
            return Ok(());
        };
        tracker
            .finish(report.status(), committed_run_info(report.counters()))
            .await
    }
}

/// The run id this run records under (`TRK-001`, `TOK-007`).
///
/// `track_run.run_id` is honoured when set, because an operator who supplied one is co-ordinating
/// with something outside cdm-rs. Otherwise one is generated. An untracked run keeps
/// [`super::UNTRACKED_RUN_ID`], which is what it has always used to seed its shuffle: changing that
/// would change the range order of every existing untracked run for no benefit. Referring to the
/// harness's constant rather than restating the zero is what keeps the plan, the event stream and
/// the tracking rows unable to drift apart about which run they describe.
///
/// # Errors
///
/// [`ErrorKind::Tracking`] if the clock is outside the representable range (`TRK-001`).
pub(super) fn run_id(config: &EffectiveConfig) -> Result<RunId, CdmError> {
    if !config.config().track_run.enabled {
        return Ok(super::UNTRACKED_RUN_ID);
    }
    allocate_run_id(config.config().track_run.run_id)
}

/// One generator per process (`TRK-003`).
///
/// It has to be shared, because that is what makes the ids strictly increasing: a fresh generator
/// has issued nothing, so two runs starting inside the same microsecond would be handed the same
/// id and `TRK-020` would reject the second for a reason nobody could reproduce.
static RUN_IDS: RunIdGenerator = RunIdGenerator::new();

/// Generates a run id, or wraps the configured one.
///
/// # Errors
///
/// [`ErrorKind::Tracking`] if the clock is outside the representable range.
pub(super) fn allocate_run_id(configured: i64) -> Result<RunId, CdmError> {
    if configured != 0 {
        return Ok(RunId::from_raw(configured));
    }
    RUN_IDS.next(chrono::Utc::now().timestamp_micros())
}

/// The table the run history is keyed by (`TRK-010`, `CFG-023`).
///
/// The target, falling back to the origin when no target table is configured — the same fallback
/// `cdm runs list` uses, so the two commands read and write the same history.
///
/// # Errors
///
/// [`ErrorKind::Config`] when neither is configured.
pub(super) fn tracked_table(config: &EffectiveConfig) -> Result<TableRef, CdmError> {
    config
        .target_table()
        .or_else(|| config.origin_table())
        .cloned()
        .ok_or_else(|| {
            CdmError::new(
                ErrorKind::Config,
                "run tracking is keyed by the target table, and none is configured; set \
                 `schema.origin.keyspace_table` (or `schema.target.keyspace_table`)",
            )
            .with_context(|c| c.with_config_key("schema.target.keyspace_table"))
        })
}

/// Opens the Cassandra tracking store on the target session (`TRK-010`).
///
/// # Errors
///
/// [`ErrorKind::Internal`] if this run opened the origin only, and whatever the store reports.
pub(super) fn open_store(
    sessions: &Sessions,
    table: &TableRef,
) -> Result<Arc<CassandraStore>, CdmError> {
    Ok(Arc::new(CassandraStore::for_target(
        sessions.target()?,
        table,
    )?))
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

    fn config(overrides: &[&str]) -> EffectiveConfig {
        let mut config = cdm_config::model::CdmConfig::default();
        for override_ in overrides {
            let (key, value) = override_.split_once('=').unwrap();
            match key {
                "track_run.enabled" => config.track_run.enabled = value == "true",
                "track_run.run_id" => config.track_run.run_id = value.parse().unwrap(),
                "schema.origin.keyspace_table" => {
                    config.schema.origin.keyspace_table = Some(value.to_owned());
                }
                other => panic!("unhandled override {other}"),
            }
        }
        EffectiveConfig::resolve(config)
    }

    #[test]
    fn trk_001_an_untracked_run_keeps_the_unset_run_id_it_has_always_used() {
        let id = run_id(&config(&[])).unwrap();
        assert!(id.is_unset());
        // TOK-007: and it is the same zero the planner has always been given, so an untracked
        // run's range order does not change under anyone.
        assert_eq!(id, super::super::UNTRACKED_RUN_ID);
        assert_eq!(id.as_i64(), 0);
    }

    #[test]
    fn met_030_the_event_stream_and_the_tracking_rows_name_the_same_run() {
        // `MET-031`'s display header, `MET-030`'s events and `cdm_run_info` all carry a run id,
        // and they are all fed from this one function — so a tracked run cannot show `run 0` on
        // the dashboard while recording itself under a different number, which is what happened
        // when the display's constant and the tracking store's allocation were separate.
        let tracked = run_id(&config(&["track_run.enabled=true", "track_run.run_id=99"])).unwrap();
        assert_eq!(tracked, RunId::from_raw(99));
        assert_ne!(tracked, super::super::UNTRACKED_RUN_ID);
    }

    #[test]
    fn trk_001_a_tracked_run_allocates_a_new_id_unless_one_is_configured() {
        let generated = run_id(&config(&["track_run.enabled=true"])).unwrap();
        assert!(!generated.is_unset());

        let configured = run_id(&config(&[
            "track_run.enabled=true",
            "track_run.run_id=4242",
        ]))
        .unwrap();
        assert_eq!(configured, RunId::from_raw(4242));
    }

    #[test]
    fn trk_001_two_allocations_never_collide() {
        // Two runs started in the same microsecond must not share a run id: `TRK-020` rejects the
        // second, which would abort a run for a reason nobody could reproduce.
        let first = allocate_run_id(0).unwrap();
        let second = allocate_run_id(0).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn trk_010_the_tracked_table_is_the_target_falling_back_to_the_origin() {
        let table = tracked_table(&config(&["schema.origin.keyspace_table=ks.tbl"])).unwrap();
        assert_eq!(table, TableRef::new("ks", "tbl"));

        let error = tracked_table(&config(&[])).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
    }

    #[tokio::test]
    async fn trk_020_tracking_that_is_off_records_nothing_and_needs_no_session() {
        let tracking = Tracking::disabled();
        assert!(!tracking.is_enabled());
        // It contributes no observer, so a silent untracked run hands the scheduler exactly the
        // `NoopObserver` it always has, with no per-range call that does nothing.
        assert!(tracking.observer().is_none());
    }
}
