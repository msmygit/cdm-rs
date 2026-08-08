//! `cdm runs resume`: executing the ranges a previous run did not finish
//! (`TRK-030`..`TRK-033`, `TRK-038`, `TRK-039`, `TOK-011`).
//!
//! # What makes this different from starting a job
//!
//! Everything up to the plan is the same — configuration, sessions, introspection, the job — and
//! then the plan is not a split of the ring but the outstanding ranges of a recorded run. That one
//! substitution is the whole feature, and it is also the whole risk: a "resume" that re-planned
//! the ring would migrate every range again, which on a counter table is not a duplicate write but
//! a wrong number (`DST-015`). So this module never falls back to a full plan silently. A previous
//! run that cannot be resumed is an error naming the reason, and the operator runs `cdm migrate`
//! if a full run is what they meant.
//!
//! # Counters (`MET-004`, `TRK-038`)
//!
//! A resumed run's counters start at zero and are recorded against its own `cdm_run_info` row,
//! which names the run it continues in `previous_run_id`. They are this run's numbers: the rows
//! *it* read and wrote. The alternative — seeding them from the previous run's totals — would
//! report the same rows twice to anyone summing a history, and would carry a `Partitions Failed`
//! count describing an interruption that has by then been dealt with, which `TRK-030` reads to
//! decide whether there is still work left.
//!
//! # Counter tables (`DST-015`, `TRK-039`)
//!
//! `RerunPolicy::for_job` narrows the pending set to `NOT_STARTED` for a job that writes counters,
//! because a `STARTED`, `FAIL` or `DIFF` counter range may have applied some of its updates and
//! re-applying them adds to a counter that already moved. The ranges that policy withholds are
//! *reported*, never quietly dropped: they need a human, and a resume that hid them would look
//! like a clean recovery.

use std::sync::Arc;

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind, JobKind, RunId};
use cdm_metrics::EventBus;
use cdm_track::manage::RunManager;
use cdm_track::resume::{QuarantinedRange, RerunPolicy, ResumePlan};
use cdm_track::tracker::{new_run_record, RunTracker, TrackerConfig};
use cdm_track::CassandraStore;
use serde::Serialize;

use super::build::{self, ResolvedTables};
use super::session::Sessions;
use super::tracking::{self, Tracking};
use super::{finish, node_provider, resolve, run, runtime, JobOptions, JobOutcome, Watchers};
use crate::cli::JobArgs;
use crate::tui::Presentation;

/// How the run to resume is chosen (`TRK-030`).
#[derive(Debug, Clone, Copy)]
pub struct ResumeOptions {
    /// The run named on the command line, if any.
    pub previous_run_id: Option<i64>,
    /// Whether `--auto` was given: adopt the most recent unfinished run.
    pub auto: bool,
    /// Which job's history to search when adopting automatically.
    ///
    /// Only a tie-breaker. Once a run is chosen, the job it recorded is what gets rebuilt — a
    /// `VALIDATE` run resumed as a migrate would write the whole table.
    pub job: JobKind,
}

impl Default for ResumeOptions {
    fn default() -> Self {
        Self {
            previous_run_id: None,
            auto: false,
            job: JobKind::Migrate,
        }
    }
}

/// One range a resume withheld, rendered for a report (`DST-015`, `SEC-002`).
///
/// Token bounds, the recorded status and the reason. There is no field a row could occupy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuarantineEntry {
    /// The token bounds, as `(min, max]`.
    pub range: String,
    /// The status the previous run left the range in.
    pub status: Option<String>,
    /// Why it was withheld, in the words an operator needs.
    pub reason: &'static str,
}

impl QuarantineEntry {
    fn from_range(quarantined: &QuarantinedRange) -> Self {
        Self {
            range: quarantined.range.to_string(),
            status: quarantined.status.map(|status| status.as_str().to_owned()),
            reason: quarantined.reason.message(),
        }
    }
}

/// What a resume did (`TRK-038`).
#[derive(Debug)]
pub struct ResumedRun {
    /// The run that was resumed.
    pub previous_run_id: i64,
    /// The new run's id, or `None` when there was nothing outstanding and no run was started.
    pub run_id: Option<i64>,
    /// The job that was rebuilt, taken from the previous run's `run_type` (`TRK-013`).
    pub job: JobKind,
    /// How many `cdm_run_details` rows the resume considered.
    pub ranges_considered: usize,
    /// How many ranges it re-planned.
    pub ranges_replanned: usize,
    /// The ranges it refused to replay (`DST-015`).
    pub quarantined: Vec<QuarantineEntry>,
    /// The run itself, absent when nothing was outstanding.
    pub outcome: Option<JobOutcome>,
}

/// The result of `cdm runs resume`, for the command to render.
pub type ResumeOutcome = ResumedRun;

/// Re-runs the ranges a previous run did not finish (`TRK-034`, `TRK-038`).
///
/// # Errors
///
/// Everything [`super::execute`] can return, plus [`ErrorKind::Tracking`] when there is no run to
/// resume, when the named run is not recorded, or when the previous run cannot be resumed at all
/// and a full plan — which this command must never start on its own — would be the only option
/// (`TRK-032`). [`ErrorKind::Config`] when run tracking is off, because then nothing was recorded
/// to resume from.
pub fn resume(
    args: &JobArgs,
    options: ResumeOptions,
    presentation: Presentation,
) -> Result<ResumedRun, CdmError> {
    let config = resolve(args, JobOptions::default())?;
    // Both preconditions are decided before a session is opened. An operator who has not turned
    // tracking on, or who has not said which run they mean, should be told so in the second it
    // takes to parse the configuration rather than after two cluster handshakes.
    require_tracking(&config)?;
    let requested = choose_requested(&config, options)?;
    runtime()?.block_on(async {
        let sessions = Arc::new(Sessions::open(&config).await?);
        let tables = ResolvedTables::introspect(&sessions, &config).await?;
        let table = tracking::tracked_table(&config)?;
        let store = tracking::open_store(&sessions, &table)?;
        let manager = RunManager::new(Arc::clone(&store), table.clone());

        let previous_run_id = resolve_previous(&manager, requested, options.job, &table).await?;
        let job = previous_job(&manager, previous_run_id, &table).await?;

        // DST-015: the policy is a fact about this table and this job, decided once, here.
        let policy = RerunPolicy::for_job(job, tables.target.is_counter_table(), corrects(&config));
        let run_id = tracking::allocate_run_id(config.config().track_run.run_id)?;
        let plan = manager
            .resume(
                Some(previous_run_id),
                job,
                policy,
                config.config().track_run.rerun_multiplier,
                run_id,
            )
            .await?
            .ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Tracking,
                    format!("run {previous_run_id} has nothing left to resume"),
                )
            })?;
        refuse_fallback(&plan, previous_run_id)?;

        let quarantined: Vec<QuarantineEntry> = plan
            .quarantined()
            .iter()
            .map(QuarantineEntry::from_range)
            .collect();
        let summary = ResumedRun {
            previous_run_id: previous_run_id.as_i64(),
            run_id: None,
            job,
            ranges_considered: plan.considered(),
            ranges_replanned: plan.ranges().len(),
            quarantined,
            outcome: None,
        };
        if plan.ranges().is_empty() {
            // Nothing outstanding is a real answer, and not a reason to start a run. It is also
            // not the same as a fallback, which `refuse_fallback` has already ruled out.
            return Ok(summary);
        }

        // TOK-011: the seam. The scheduler runs *these* ranges, not a fresh split of the ring.
        let token_plan = plan.token_plan(run_id, tables.partitioner())?;

        // MET-030: the bus carries the *new* run's id, which is the one the tracking rows are
        // keyed by, so a display and the run history name the same run. The progress bar it feeds
        // is weighted over the outstanding ranges alone — a resume that showed itself as a
        // fraction of the whole ring would sit at 60% and never move.
        let bus = Arc::new(EventBus::new(
            run_id,
            super::SchedulerSettings::from_config(&config)
                .node_id()
                .to_owned(),
        ));
        // MET-010: on the same condition as the bus, and before the job, because the executors
        // inside the job are what record a request. A resumed run is watched exactly as a fresh
        // one is.
        let started = std::time::Instant::now();
        let instruments = presentation
            .is_live()
            .then(|| Arc::new(cdm_metrics::Instruments::new(started)));
        let built = build::job(
            job,
            &sessions,
            &tables,
            &config,
            args,
            presentation.is_live().then(|| Arc::clone(&bus)),
            instruments.clone(),
        )
        .await?;

        // TRK-020, TRK-038: a new run row, naming the run it continues, with its own counters.
        let tracker = Arc::new(
            RunTracker::start(
                store as Arc<dyn cdm_core::TrackingStore>,
                &new_run_record(run_id, Some(previous_run_id), table, job),
                &token_plan.token_ranges(),
                TrackerConfig::default(),
            )
            .await?,
        );
        let tracking = Tracking::started(tracker);
        let report = run(
            &config,
            &token_plan,
            Arc::clone(&built.processor),
            Watchers {
                kind: job,
                run_id,
                bus: &bus,
                instruments,
                started,
                presentation,
                nodes: node_provider(&sessions),
                tracking: &tracking,
            },
        )
        .await?;
        // TRK-022, ENG-009, ENG-010: a resumed run that is itself interrupted records the status
        // that says so, and is therefore resumable in its turn.
        tracking.finish(&report).await?;

        Ok(ResumedRun {
            run_id: Some(run_id.as_i64()),
            outcome: Some(finish(job, args, &config, &report, &built)),
            ..summary
        })
    })
}

/// Turns the operator's request into the id of a run that exists (`TRK-030`).
async fn resolve_previous(
    manager: &RunManager<CassandraStore>,
    requested: Requested,
    job: JobKind,
    table: &cdm_core::TableRef,
) -> Result<RunId, CdmError> {
    match requested {
        Requested::Run(id) => Ok(id),
        Requested::Adopt => manager.previous_run_id(None, job).await?.ok_or_else(|| {
            CdmError::new(
                ErrorKind::Tracking,
                format!(
                    "no unfinished {} run is recorded for {table}; `cdm runs list` shows the \
                     history",
                    job.as_str()
                ),
            )
        }),
    }
}

/// The job the run being resumed recorded (`TRK-013`).
///
/// Authoritative over anything the command line said: re-running a recorded `VALIDATE` as a
/// migrate would write the whole table.
async fn previous_job(
    manager: &RunManager<CassandraStore>,
    previous_run_id: RunId,
    table: &cdm_core::TableRef,
) -> Result<JobKind, CdmError> {
    let previous = manager.record(previous_run_id).await?.ok_or_else(|| {
        // Distinct from `plan_resume`'s fallback, and deliberately so: there, a missing run is a
        // reason to plan the ring; here, the operator named a run that does not exist.
        CdmError::new(
            ErrorKind::Tracking,
            format!(
                "no run {previous_run_id} is recorded for {table}; `cdm runs list` shows what is"
            ),
        )
    })?;
    if previous.job == JobKind::Guardrail {
        return Err(CdmError::new(
            ErrorKind::Tracking,
            "a guardrail run reads the origin and writes nothing, so there is nothing to resume; \
             re-run `cdm guardrail`",
        ));
    }
    Ok(previous.job)
}

/// Whether a validate run would write to the target (`DST-015`).
///
/// The counter restriction applies to a job that *writes*, and a validate run writes only when
/// autocorrect is on. `FEA-045` and `MIG-032` are why `missing_counter` counts: it is the flag
/// that lets validate write a counter row at all.
fn corrects(config: &EffectiveConfig) -> bool {
    let autocorrect = &config.config().autocorrect;
    autocorrect.missing || autocorrect.mismatch || autocorrect.missing_counter
}

/// Which run the operator asked for, before the store has been consulted (`TRK-030`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Requested {
    /// This one, named on the command line or in `track_run.previous_run_id`.
    Run(RunId),
    /// Whichever `auto_rerun` adopts.
    Adopt,
}

/// Refuses a resume on a configuration that recorded nothing (`TRK-038`).
///
/// `track_run.enabled` is what wrote the rows a resume reads, and it is also what will record the
/// resumed run so that *it* can be resumed in turn. Without it there is neither a history to read
/// nor a way to save the one being made, and a "resume" would be an untracked full-plan run under
/// another name.
fn require_tracking(config: &EffectiveConfig) -> Result<(), CdmError> {
    if config.config().track_run.enabled {
        return Ok(());
    }
    Err(CdmError::new(
        ErrorKind::Config,
        "run tracking is off, so no run was recorded to resume from; set `track_run.enabled` \
         before the run you will want to resume, not after it",
    )
    .with_context(|c| c.with_config_key("track_run.enabled")))
}

/// The run to resume, from the command line or the configuration (`TRK-030`).
///
/// An explicit id wins over `auto_rerun`, and neither being present is an error rather than an
/// implicit adoption: `cdm runs resume` typed by mistake must not start a run over a table the
/// operator was only asking about.
fn choose_requested(
    config: &EffectiveConfig,
    options: ResumeOptions,
) -> Result<Requested, CdmError> {
    let track_run = &config.config().track_run;
    let requested = options.previous_run_id.unwrap_or(track_run.previous_run_id);
    let requested = RunId::from_raw(requested);
    if !requested.is_unset() {
        return Ok(Requested::Run(requested));
    }
    if options.auto || track_run.auto_rerun {
        return Ok(Requested::Adopt);
    }
    Err(CdmError::new(
        ErrorKind::Tracking,
        "name the run to resume (`cdm runs resume <run-id>`), pass `--auto` to adopt the most \
         recent unfinished one, or set `track_run.auto_rerun`. `cdm runs list` shows the history \
         and `cdm runs show <run-id>` what one run left outstanding.",
    ))
}

/// Turns `TRK-032`'s fallback into an error rather than a full run.
///
/// `plan_resume` reports "I could not read this run, plan the whole ring instead", which is the
/// right answer for `auto_rerun` on a job that was going to run anyway. It is the wrong answer
/// here: an operator who typed `cdm runs resume` and got a full migration of a petabyte has been
/// badly served, and on a counter table they have been given wrong data.
fn refuse_fallback(plan: &ResumePlan, previous_run_id: RunId) -> Result<(), CdmError> {
    let Some(reason) = plan.fallback_reason() else {
        return Ok(());
    };
    Err(CdmError::new(
        ErrorKind::Tracking,
        format!(
            "run {previous_run_id} cannot be resumed: {}. Resuming would mean re-planning the \
             whole ring, which is a full run — start one with `cdm migrate` or `cdm validate` if \
             that is what you want.",
            reason.message()
        ),
    ))
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
    use cdm_core::{RunStatus, TokenRange};
    use cdm_track::resume::{FallbackReason, QuarantineReason};

    use super::*;

    fn config(autocorrect: &[&str]) -> EffectiveConfig {
        let mut config = cdm_config::model::CdmConfig::default();
        for flag in autocorrect {
            match *flag {
                "missing" => config.autocorrect.missing = true,
                "mismatch" => config.autocorrect.mismatch = true,
                "missing_counter" => config.autocorrect.missing_counter = true,
                other => panic!("unhandled flag {other}"),
            }
        }
        EffectiveConfig::resolve(config)
    }

    /// A configuration with tracking on, and the `track_run` fields the test needs.
    fn tracked(previous: i64, auto_rerun: bool) -> EffectiveConfig {
        let mut config = cdm_config::model::CdmConfig::default();
        config.track_run.enabled = true;
        config.track_run.previous_run_id = previous;
        config.track_run.auto_rerun = auto_rerun;
        EffectiveConfig::resolve(config)
    }

    #[test]
    fn trk_038_a_resume_is_refused_when_nothing_was_ever_recorded() {
        let error = require_tracking(&config(&[]))
            .expect_err("without tracking there is no history to resume");
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(error.message().contains("track_run.enabled"), "{error}");
        require_tracking(&tracked(0, true)).unwrap();
    }

    #[test]
    fn trk_030_the_named_run_wins_over_auto_rerun_and_over_the_configured_one() {
        // The command line, then `track_run.previous_run_id`, then adoption. An operator who named
        // a run and was handed a different one has been actively misled.
        assert_eq!(
            choose_requested(
                &tracked(55, true),
                ResumeOptions {
                    previous_run_id: Some(42),
                    auto: true,
                    job: JobKind::Migrate,
                }
            )
            .unwrap(),
            Requested::Run(RunId::from_raw(42))
        );
        assert_eq!(
            choose_requested(&tracked(55, true), ResumeOptions::default()).unwrap(),
            Requested::Run(RunId::from_raw(55))
        );
        assert_eq!(
            choose_requested(&tracked(0, true), ResumeOptions::default()).unwrap(),
            Requested::Adopt
        );
        assert_eq!(
            choose_requested(
                &tracked(0, false),
                ResumeOptions {
                    auto: true,
                    ..ResumeOptions::default()
                }
            )
            .unwrap(),
            Requested::Adopt
        );
    }

    #[test]
    fn trk_030_a_resume_that_names_no_run_starts_nothing() {
        // `cdm runs resume` typed while looking around must not start a migration over whichever
        // run happened to be last.
        let error = choose_requested(&tracked(0, false), ResumeOptions::default())
            .expect_err("a resume with no run named must not adopt one by default");
        assert_eq!(error.kind(), ErrorKind::Tracking);
        assert!(error.message().contains("--auto"), "{error}");
        assert!(error.message().contains("cdm runs show"), "{error}");
    }

    #[test]
    fn dst_015_a_validate_run_counts_as_a_writer_exactly_when_autocorrect_is_on() {
        assert!(!corrects(&config(&[])));
        for flag in ["missing", "mismatch", "missing_counter"] {
            assert!(corrects(&config(&[flag])), "{flag} makes validate write");
        }
        // And that is what decides the counter restriction on a counter table.
        assert!(!RerunPolicy::for_job(JobKind::Validate, true, false).is_counter_restricted());
        assert!(RerunPolicy::for_job(JobKind::Validate, true, true).is_counter_restricted());
    }

    #[test]
    fn trk_032_a_fallback_is_refused_rather_than_turned_into_a_full_run() {
        let plan = ResumePlan::fallback(
            RunId::from_raw(7),
            FallbackReason::RunNotFound(RunId::from_raw(7)),
        );
        let error = refuse_fallback(&plan, RunId::from_raw(7))
            .expect_err("a resume must never silently plan the whole ring");
        assert_eq!(error.kind(), ErrorKind::Tracking);
        assert!(error.message().contains("cdm migrate"), "{error}");
    }

    #[test]
    fn dst_015_a_quarantine_entry_carries_bounds_a_status_and_a_reason_and_nothing_else() {
        let entry = QuarantineEntry::from_range(&QuarantinedRange {
            range: TokenRange::new(10, 19).unwrap(),
            status: Some(RunStatus::Started),
            reason: QuarantineReason::CounterPartiallyApplied,
        });
        assert_eq!(entry.status.as_deref(), Some("STARTED"));
        assert!(entry.reason.contains("double-count"));

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("10"), "{json}");
        for forbidden in ["password", "value", "row"] {
            assert!(!json.to_lowercase().contains(forbidden), "{json}");
        }
    }
}
