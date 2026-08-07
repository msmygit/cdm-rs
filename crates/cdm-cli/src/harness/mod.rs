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
mod report;
mod session;

use std::io::Write;
use std::sync::Arc;

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind, JobKind, RunId};
use cdm_engine::planner::{Planner, PlannerSettings, TokenPlan};
use cdm_engine::scheduler::{NoopObserver, RunReport, Scheduler, SchedulerSettings};

pub use build::{BuiltJob, JobBuilder, ResolvedTables};
pub use report::{PlanSummary, RunSummary};
pub use session::Sessions;

use crate::cli::JobArgs;
use crate::loader::load;

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
pub fn execute(args: &JobArgs, kind: JobKind, options: JobOptions) -> Result<JobOutcome, CdmError> {
    let config = resolve(args, options)?;
    // The whole run happens inside one runtime, built here rather than in `main`, so that a
    // command which never touches a cluster — `cdm config validate`, `cdm completions` — does not
    // pay for a thread pool it will not use.
    runtime()?.block_on(async {
        let sessions = Sessions::open(&config).await?;
        let tables = ResolvedTables::introspect(&sessions, &config).await?;
        let job = build::job(kind, &sessions, &tables, &config, args).await?;
        let plan = token_plan(&config, &tables)?;
        let report = run(&config, plan, Arc::clone(&job.processor)).await?;
        Ok(finish(kind, args, &config, &report, &job))
    })
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
        let (plan, planner) = token_plan_with_planner(&config, &tables)?;
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
fn token_plan(config: &EffectiveConfig, tables: &ResolvedTables) -> Result<TokenPlan, CdmError> {
    token_plan_with_planner(config, tables).map(|(plan, _)| plan)
}

/// The plan and the planner that produced it, which `cdm plan` needs in order to report on it.
fn token_plan_with_planner(
    config: &EffectiveConfig,
    tables: &ResolvedTables,
) -> Result<(TokenPlan, Planner), CdmError> {
    let settings = PlannerSettings::from_config(config.config(), tables.partitioner());
    let planner = Planner::new(settings);
    let plan = planner.plan(RunId::from_raw(0), None)?;
    Ok((plan, planner))
}

/// Runs the plan through the scheduler.
async fn run(
    config: &EffectiveConfig,
    plan: TokenPlan,
    job: Arc<dyn cdm_engine::scheduler::RangeProcessor>,
) -> Result<RunReport, CdmError> {
    let settings = SchedulerSettings::from_config(config);
    Scheduler::new(settings)?
        .run(&plan, job, Arc::new(NoopObserver))
        .await
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
