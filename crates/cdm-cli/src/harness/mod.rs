//! The shared job harness: connect, introspect, plan, run (`CLI-001`).
//!
//! Every job command does the same five things in the same order, and only the fourth differs:
//!
//! 1. load and validate the configuration (`CFG-020`);
//! 2. open a session to each side (`CON-001`);
//! 3. introspect both tables and resolve the column mapping (`SCH-001`..`SCH-007`);
//! 4. build the job — the one step migrate, validate and guardrail do differently;
//! 5. plan the ring and run it through the scheduler, then report (`TOK-001`, `ENG-001`,
//!    `MET-005`).
//!
//! Those steps existed in the integration tests of three separate crates before they existed
//! here, which is why `cdm migrate` answered "not yet" while the migrate engine was finished and
//! passing against real nodes. The engines were never the missing piece.
//!
//! # Why the shape is "one harness, three job builders" rather than three commands
//!
//! The four steps that are not the job are where a run goes wrong: a keyspace that does not exist,
//! a token a partitioner cannot represent, a schema that changed since the last run. Writing them
//! once means an operator gets the same diagnostic from `cdm validate` as from `cdm migrate`, and
//! it means [`JobBuilder`] is the whole of what a *new* job — a third-party `JobPlugin`
//! (`PLG-004`) — has to supply.

mod build;
pub mod observe;
mod report;
mod resume;
mod session;
mod tracking;

use std::io::Write;
use std::sync::Arc;

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind, JobKind, RunId};
use cdm_engine::planner::{Partitioner, Planner, PlannerSettings, TokenPlan};
use cdm_engine::scheduler::{RangeObserver, RunReport, Scheduler, SchedulerSettings};
use cdm_metrics::{EventBus, Instruments};

pub use build::{BuiltJob, JobBuilder, ResolvedOrigin, ResolvedTables};
pub use observe::{LiveRun, Observers};
pub use report::{PlanSummary, RunSummary};
pub use resume::{resume, QuarantineEntry, ResumeOptions, ResumeOutcome, ResumedRun};
pub use session::Sessions;
pub use tracking::Tracking;

use crate::cli::JobArgs;
use crate::loader::load;
use crate::tui::{LiveDisplay, NodeProvider, Presentation};

/// The run identifier a run that is not being recorded plans and publishes under.
///
/// Zero, which is `RunId::UNSET`: run identifiers exist to key the tracking tables (`TRK-010`), and
/// a run with `track_run.enabled` off has none. Naming the constant is what keeps the plan, the
/// event stream (`MET-030`) and the tracking rows (`TRK-020`) agreeing about which run they
/// describe — a tracked run replaces it with its allocated id in all three at once, and an
/// untracked one keeps the zero its shuffle has always been seeded with (`TOK-007`).
const UNTRACKED_RUN_ID: RunId = RunId::UNSET;

/// The job flags that are spellings of configuration rather than of behaviour (`VAL-015`).
///
/// Both are applied to the loaded `CdmConfig` *before* validation runs, which is what makes them
/// sugar in the strict sense: a `--sample 5` run and a `filter.token_coverage_percent = 5` run are
/// the same run, the same Tier-1 range check rejects a bad value, and Tier 2 gets to warn about a
/// keys-only comparison exactly as it would for the property.
#[derive(Debug, Clone, Copy, Default)]
pub struct JobOptions {
    /// `--sample <percent>`, which sets `filter.token_coverage_percent`.
    pub sample: Option<u8>,
    /// `--keys-only`, which sets `validate.keys_only`.
    pub keys_only: bool,
}

/// A finished run, in the two shapes the CLI needs it in.
///
/// The terminal wants Java's counter block and a sentence; `--summary-out` wants the `MET-033`
/// document, which is a different audience and a different amount of detail. They are produced
/// from one `RunReport` so they cannot disagree about what happened.
#[derive(Debug)]
pub struct JobOutcome {
    /// What the terminal prints, and what `--output json` renders.
    pub summary: RunSummary,
    /// The `MET-033` run summary, for `--summary-out`.
    pub record: cdm_metrics::RunSummary,
}

/// Runs a job end to end and returns its summary (`CLI-001`, `MET-033`).
///
/// # Errors
///
/// Anything that stops the run before it starts: an invalid configuration (`ErrorKind::Config`), a
/// cluster that cannot be reached (`ErrorKind::Connect`), a table that does not exist or cannot be
/// mapped (`ErrorKind::SchemaMismatch`). A run that *starts* and then fails ranges returns `Ok`
/// with a summary saying so — that is a completed command reporting a bad result, which
/// `CLI-004` distinguishes from the command itself failing.
pub fn execute(
    args: &JobArgs,
    kind: JobKind,
    options: JobOptions,
    presentation: Presentation,
) -> Result<JobOutcome, CdmError> {
    let config = resolve(args, options)?;
    // The whole run happens inside one runtime, built here rather than in `main`, so that a
    // command which never touches a cluster — `cdm config validate`, `cdm completions` — does not
    // pay for a thread pool it will not use.
    runtime()?.block_on(async {
        // TOK-007, TRK-020, MET-030: the run id is allocated before anything that names it. The
        // plan is shuffled by it, the tracking rows are keyed by it and the event stream reports
        // it, and a run whose three artefacts disagreed about its own identity would be
        // unreadable afterwards. A guardrail is never recorded (see below), so it keeps the zero.
        let run_id = if kind == JobKind::Guardrail {
            UNTRACKED_RUN_ID
        } else {
            tracking::run_id(&config)?
        };
        // MET-030: one bus per run, created before the job so that a validate job can be handed it
        // (`VAL-002`'s findings are published through it) and before the scheduler so that the
        // display has subscribed by the time the first range starts.
        let node_id = SchedulerSettings::from_config(&config).node_id().to_owned();
        let bus = Arc::new(EventBus::new(run_id, node_id));

        // MET-010: the instruments have to exist before the job does, because the executors
        // *inside* the job are what record a request's latency — there is nowhere else a request
        // is visible. They are built on the same condition as the bus: a silent run pays nothing,
        // and `RequestMetrics::unobserved` then costs one null check per request rather than a
        // clock read.
        let started = std::time::Instant::now();
        let instruments = presentation
            .is_live()
            .then(|| Arc::new(Instruments::new(started)));

        // GRD-001: a guardrail run opens the origin and nothing else, so it takes its own four
        // steps rather than the two-sided ones. The two paths meet again at the plan, because the
        // ring is split the same way whatever is going to read it.
        if kind == JobKind::Guardrail {
            let sessions = Arc::new(Sessions::open_origin(&config).await?);
            let origin = ResolvedOrigin::introspect(&sessions.origin, &config).await?;
            let job =
                build::guardrail(&sessions.origin, &origin, &config, instruments.clone()).await?;
            // A guardrail is untracked by construction: the tracking tables live in the target
            // keyspace (`TRK-010`) and `GRD-001` gives this path no target session to reach them
            // through. There is also nothing to resume — it writes nothing.
            let tracking = Tracking::disabled();
            let plan = token_plan(&config, origin.partitioner(), run_id)?;
            let report = run(
                &config,
                &plan,
                Arc::clone(&job.processor),
                Watchers {
                    kind,
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
            return Ok(finish(kind, args, &config, &report, &job));
        }

        let sessions = Arc::new(Sessions::open(&config).await?);
        let tables = ResolvedTables::introspect(&sessions, &config).await?;
        let job = build::job(
            kind,
            &sessions,
            &tables,
            &config,
            args,
            presentation.is_live().then(|| Arc::clone(&bus)),
            instruments.clone(),
        )
        .await?;
        let plan = token_plan(&config, tables.partitioner(), run_id)?;
        // TRK-020: the run row and every range row exist before the first range is claimed. A
        // crash between the two would leave ranges no resume could know about.
        let tracking = Tracking::start(&config, &sessions, kind, &plan, None).await?;
        let report = run(
            &config,
            &plan,
            Arc::clone(&job.processor),
            Watchers {
                kind,
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
        // TRK-022: the terminal status, whatever it is. `ENDED` only for a run that finished.
        // After `run`, so the display has already been torn down and the terminal handed back:
        // this write can fail, and its warning belongs on the operator's real screen rather than
        // on an alternate one that is about to be discarded.
        tracking.finish(&report).await?;
        Ok(finish(kind, args, &config, &report, &job))
    })
}

/// Splits `ks.tbl` into its two halves for the `RunStarted` event (`MET-030`).
///
/// `schema.origin.keyspace_table` is one string in the configuration model because that is how
/// Java spells the property (`CLI-002`), and the event carries the two parts separately. A value
/// with no dot is taken as a keyspace, which is what an operator who typed half of it meant.
fn split_keyspace_table(value: Option<&str>) -> (Option<String>, Option<String>) {
    match value.map(|value| value.split_once('.')) {
        Some(Some((keyspace, table))) => (Some(keyspace.to_owned()), Some(table.to_owned())),
        Some(None) => (value.map(str::to_owned), None),
        None => (None, None),
    }
}

/// A callback the display polls for the driver's current view of the cluster (`MET-031`).
///
/// Polled rather than snapshotted once, because a node going away mid-run is exactly the thing an
/// operator watching a display wants to see. The driver keeps this metadata refreshed from its own
/// topology events, so reading it costs no query.
fn node_provider(sessions: &Arc<Sessions>) -> NodeProvider {
    let sessions = Arc::clone(sessions);
    Arc::new(move || sessions.node_status())
}

/// Closes the run's artefacts and assembles both summaries (`MET-033`, `VAL-013`, `CFG-023`).
fn finish(
    kind: JobKind,
    args: &JobArgs,
    config: &EffectiveConfig,
    report: &RunReport,
    job: &BuiltJob,
) -> JobOutcome {
    // VAL-013 is explicit that a report which cannot be *written* does not fail a run: moving data
    // is the job, and the file is a by-product. Closing it can only fail for the same reasons
    // writing to it could, so it is said once, loudly, on stderr, and the run still reports what
    // it did.
    if let Some(discrepancies) = &job.discrepancies {
        if let Err(error) = discrepancies.finish() {
            let _ = writeln!(std::io::stderr(), "warning: {}", error.message());
        }
    }

    // Everything the scheduler knows, plus the two things it cannot: the configuration digest of
    // `CFG-023` and the pointer to the `VAL-013` report.
    let mut record = report
        .summary(chrono::Utc::now())
        .with_config_hash(config.config_hash());
    if let Some(reference) = job.discrepancies.as_ref().and_then(|r| r.reference()) {
        record = record.with_discrepancy_report(reference);
    }

    JobOutcome {
        summary: RunSummary::from_report(kind, report, args.dry_run),
        record,
    }
}

/// Computes the token plan and reports it without touching data (`TOK-001`, `CLI-001`).
///
/// `cdm plan` exists so that the two questions an operator asks before a migration — "how will this
/// be divided" and "how much memory will it take" — can be answered without starting one. It still
/// connects and introspects, because a plan that ignores the partitioner and the schema would
/// answer a different question from the one the run will ask.
///
/// # Errors
///
/// As [`execute`], minus anything that can only fail once rows move.
pub fn plan(args: &JobArgs) -> Result<PlanSummary, CdmError> {
    let config = resolve(args, JobOptions::default())?;
    runtime()?.block_on(async {
        let sessions = Sessions::open(&config).await?;
        let tables = ResolvedTables::introspect(&sessions, &config).await?;
        let (plan, planner) = token_plan_with_planner(&config, tables.partitioner(), RunId::UNSET)?;
        let report = planner.report(&plan, None)?;
        Ok(PlanSummary::new(&report, &tables, &config))
    })
}

/// Connects and introspects, stopping there (`SCH-001`..`SCH-008`).
///
/// The harness's steps one to three, without four or five. `cdm schema show` and `cdm schema diff`
/// are exactly those three steps and nothing else, and going through this rather than opening their
/// own sessions is what makes a mapping `cdm schema diff` accepts a mapping the run accepts: it is
/// the same [`ResolvedTables`] the job would have been built from.
///
/// # Errors
///
/// As [`execute`], minus anything that can only fail once a job exists.
pub fn resolve_tables(args: &JobArgs) -> Result<ResolvedTables, CdmError> {
    let config = resolve(args, JobOptions::default())?;
    runtime()?.block_on(async {
        let sessions = Sessions::open(&config).await?;
        ResolvedTables::introspect(&sessions, &config).await
    })
}

/// Loads the configuration and refuses to go further if it is invalid (`CFG-020`, `CFG-021`).
///
/// Tier 1 and tier 2 run here. Tier 3 needs the live schema and therefore runs inside
/// [`ResolvedTables::introspect`], once there is a session to ask.
fn resolve(args: &JobArgs, options: JobOptions) -> Result<EffectiveConfig, CdmError> {
    let outcome = load(&args.config)?;
    let Some(mut config) = outcome.config else {
        return Err(CdmError::new(
            ErrorKind::Config,
            "the configuration could not be assembled; run `cdm config validate` to see why",
        ));
    };

    // VAL-015: the two flags are folded into the configuration here, before either validator runs,
    // so that they are checked by the same rules the properties are checked by and are visible to
    // `config_hash` — two runs that sampled differently must not hash the same.
    if let Some(percent) = options.sample {
        cdm_engine::jobs::validate::sample_percent(&mut config, percent)?;
    }
    if options.keys_only {
        config.validate.keys_only = true;
    }

    // Every error at once, not the first: an operator fixing a configuration by trial and error,
    // one round trip per mistake, is the complaint `CFG-021` exists to answer.
    let config = config;
    let validator = cdm_config::Validator::new();
    let mut diagnostics = validator.tier1(&config);
    diagnostics.extend(validator.tier2(&config));
    let blocking: Vec<_> = diagnostics
        .into_iter()
        .filter(cdm_core::Diagnostic::is_blocking)
        .collect();
    if !blocking.is_empty() {
        use std::fmt::Write as _;

        let mut rendered = String::new();
        for diagnostic in &blocking {
            rendered.push_str("\n  ");
            rendered.push_str(&diagnostic.title);
            if let Some(location) = &diagnostic.location {
                // Ignoring the result is correct here: writing to a `String` cannot fail, and the
                // alternative is an error path that can never be taken.
                let _ = write!(rendered, " [{location}]");
            }
        }
        return Err(CdmError::new(
            ErrorKind::Config,
            format!(
                "the configuration has {} problem(s) that would stop the run:{rendered}\n\
                 Run `cdm config validate` for the detail and the suggested fixes.",
                blocking.len()
            ),
        ));
    }

    Ok(EffectiveConfig::resolve(config))
}

/// The Tokio runtime a run executes on.
fn runtime() -> Result<tokio::runtime::Runtime, CdmError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            CdmError::new(
                ErrorKind::Internal,
                format!("cannot start the async runtime: {error}"),
            )
        })
}

/// Splits the ring, using the partitioner the origin actually reports (`TOK-001`).
///
/// Reading the partitioner from `system.local` rather than assuming Murmur3 is what makes a
/// RandomPartitioner cluster work at all: its tokens do not fit in an `i64`, and a plan built for
/// the wrong partitioner covers the wrong ring.
fn token_plan(
    config: &EffectiveConfig,
    partitioner: Partitioner,
    run_id: RunId,
) -> Result<TokenPlan, CdmError> {
    token_plan_with_planner(config, partitioner, run_id).map(|(plan, _)| plan)
}

/// The plan and the planner that produced it, which `cdm plan` needs in order to report on it.
fn token_plan_with_planner(
    config: &EffectiveConfig,
    partitioner: Partitioner,
    run_id: RunId,
) -> Result<(TokenPlan, Planner), CdmError> {
    let settings = PlannerSettings::from_config(config.config(), partitioner);
    let planner = Planner::new(settings);
    let plan = planner.plan(run_id, None)?;
    Ok((plan, planner))
}

/// Everything that watches one run (`MET-031`, `TRK-021`).
///
/// Bundled rather than passed one by one because [`run`] needs all of it and a seven-argument
/// function is where an argument gets silently transposed. It is also the honest shape: these are
/// not independent settings, they are the two audiences a run has — the operator watching it now,
/// and the resume that will read it later.
struct Watchers<'a> {
    /// Which job is running, for the display's header and the `RunStarted` event.
    kind: JobKind,
    /// The run's identifier: the tracking key, and what the event stream reports.
    run_id: RunId,
    /// The structured event stream (`MET-030`).
    bus: &'a Arc<EventBus>,
    /// The per-request instruments of `MET-010`, when a run is being watched.
    ///
    /// Built before the job, because the executors *inside* the job are what record a request's
    /// latency — there is nowhere else a request is visible. `None` on a silent run, which is what
    /// makes `RequestMetrics::unobserved` cost a null check per request rather than a clock read.
    instruments: Option<Arc<Instruments>>,
    /// When the run began, the origin for every rate and window the display reports.
    started: std::time::Instant,
    /// What, if anything, is drawn while the run runs (`MET-031`).
    presentation: Presentation,
    /// The driver's current view of the cluster, polled by the display.
    nodes: NodeProvider,
    /// The durable record a resume is planned from (`TRK-020`..`TRK-022`).
    tracking: &'a Tracking,
}

/// Runs the plan through the scheduler, with whatever is watching it (`ENG-001`, `MET-031`).
///
/// # Both watchers, or neither is noticed
///
/// A run can be displayed and recorded at once, and until they were composed here each was written
/// as *the* observer — so whichever landed second would have silently replaced the first. Losing
/// the display is a cosmetic regression; losing the tracking is a run that cannot be resumed, which
/// is the defect `TRK-038` exists to prevent and which nothing would report at the time.
/// [`Observers`] fans out to both, and collapses to exactly what each path used to hand over when
/// only one of them is present — a `NoopObserver` for a silent, untracked run.
///
/// # The display is started and stopped around the run, never inside it
///
/// The terminal is taken before the first range and handed back after the last, on every path out
/// including the failing one — the `?` is deliberately *after* `finish`. A run that returned early
/// while the alternate screen was still up would print its error onto a screen that is about to be
/// discarded, and leave the operator's shell in raw mode. See `crate::tui::terminal`.
async fn run(
    config: &EffectiveConfig,
    plan: &TokenPlan,
    job: Arc<dyn cdm_engine::scheduler::RangeProcessor>,
    watchers: Watchers<'_>,
) -> Result<RunReport, CdmError> {
    let Watchers {
        kind,
        run_id,
        bus,
        instruments,
        started,
        presentation,
        nodes,
        tracking,
    } = watchers;
    let settings = SchedulerSettings::from_config(config);
    // MET-010: the rate-limiter wait of `ENG-005` is measured where the limiter is, which is the
    // one instrument `cdm-cql` cannot see.
    let scheduler = Scheduler::observing(
        settings,
        instruments
            .clone()
            .map(|i| i as Arc<dyn cdm_core::RequestObserver>),
    )?;

    // Tracking first: see `Observers`, where the ordering is a safety property rather than a
    // preference. A run with tracking off contributes nothing here and pays nothing.
    let observers = Observers::new().and(tracking.observer());

    // A silent run keeps the observer it has always had. `LiveRun` is cheap — two calls per range,
    // none per row — but "cheap" is not "free", and a run nobody is watching should pay nothing.
    let Presentation::Silent = presentation else {
        let live = Arc::new(LiveRun::new(
            kind,
            run_id,
            config.node_id(),
            Arc::clone(bus),
            instruments.unwrap_or_else(|| Arc::new(Instruments::new(started))),
            &plan.token_ranges(),
            started,
        ));
        let display = LiveDisplay::start(
            presentation,
            live.dashboard(),
            bus.subscribe(),
            scheduler.control(),
            Some(nodes),
        );
        // MET-030: the run is announced only once there is somebody to hear it.
        let (keyspace, table) =
            split_keyspace_table(config.config().schema.origin.keyspace_table.as_deref());
        bus.run_started(
            chrono::Utc::now(),
            kind,
            keyspace,
            table,
            u64::try_from(plan.len()).unwrap_or(u64::MAX),
        );

        let report = scheduler
            .run(
                plan,
                job,
                observers
                    .and(Some(live as Arc<dyn RangeObserver>))
                    .into_observer(),
            )
            .await;
        display.finish().await;
        return report;
    };

    scheduler.run(plan, job, observers.into_observer()).await
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
    use crate::cli::ConfigArgs;

    /// A job's arguments carrying only `--set` overrides, which is all these tests need.
    fn args(overrides: &[&str]) -> JobArgs {
        JobArgs {
            config: ConfigArgs {
                set: overrides.iter().map(|s| (*s).to_owned()).collect(),
                ..ConfigArgs::default()
            },
            dry_run: false,
            summary_out: None,
            tui: false,
        }
    }

    const TABLE: &str = "schema.origin.keyspace_table=ks.tbl";

    #[test]
    fn val_015_sample_sets_the_token_coverage_the_planner_reads() {
        // The whole content of "`--sample` is sugar": there is no second implementation of range
        // shrinking, only `TOK-005`'s, reached through the property the flag writes.
        let config = resolve(
            &args(&[TABLE]),
            JobOptions {
                sample: Some(5),
                keys_only: false,
            },
        )
        .unwrap();
        assert_eq!(config.config().filter.token_coverage_percent, 5);
    }

    #[test]
    fn val_015_sample_is_checked_by_the_propertys_own_tier_one_rule() {
        // `--sample 0` plans a run that reads nothing and reports everything it did not look at as
        // fine. Both ends of the range are refused, and refused *before* a session is opened.
        for percent in [0, 101] {
            let error = resolve(
                &args(&[TABLE]),
                JobOptions {
                    sample: Some(percent),
                    keys_only: false,
                },
            )
            .expect_err("an out-of-range sample must not reach a cluster");
            assert_eq!(error.kind(), cdm_core::ErrorKind::Config);
        }
    }

    #[test]
    fn val_015_keys_only_sets_the_property_the_comparison_plan_reads() {
        let config = resolve(
            &args(&[TABLE]),
            JobOptions {
                sample: None,
                keys_only: true,
            },
        )
        .unwrap();
        assert!(config.config().validate.keys_only);
    }

    #[test]
    fn val_015_neither_flag_disturbs_a_run_that_did_not_ask_for_it() {
        let config = resolve(&args(&[TABLE]), JobOptions::default()).unwrap();
        assert_eq!(config.config().filter.token_coverage_percent, 100);
        assert!(!config.config().validate.keys_only);
    }

    #[test]
    fn cfg_023_sampling_changes_the_config_hash() {
        // Two runs that sampled differently did different work. A digest that could not tell them
        // apart would make two run summaries look like reports on the same job, and would let
        // `DST-003`'s consistency check pass across a fleet that disagreed.
        let full = resolve(&args(&[TABLE]), JobOptions::default()).unwrap();
        let sampled = resolve(
            &args(&[TABLE]),
            JobOptions {
                sample: Some(5),
                keys_only: false,
            },
        )
        .unwrap();
        assert_ne!(full.config_hash(), sampled.config_hash());
    }
}
