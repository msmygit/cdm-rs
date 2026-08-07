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

use std::sync::Arc;

use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind, JobKind, RunId};
use cdm_engine::planner::{Planner, PlannerSettings, TokenPlan};
use cdm_engine::scheduler::{NoopObserver, RunReport, Scheduler, SchedulerSettings};

pub use build::{JobBuilder, ResolvedTables};
pub use report::{PlanSummary, RunSummary};
pub use session::Sessions;

use crate::cli::JobArgs;
use crate::loader::load;

/// Runs a job end to end and returns its summary (`CLI-001`).
///
/// # Errors
///
/// Anything that stops the run before it starts: an invalid configuration (`ErrorKind::Config`), a
/// cluster that cannot be reached (`ErrorKind::Connect`), a table that does not exist or cannot be
/// mapped (`ErrorKind::SchemaMismatch`). A run that *starts* and then fails ranges returns `Ok`
/// with a summary saying so — that is a completed command reporting a bad result, which
/// `CLI-004` distinguishes from the command itself failing.
pub fn execute(args: &JobArgs, kind: JobKind) -> Result<RunSummary, CdmError> {
    let config = resolve(args)?;
    // The whole run happens inside one runtime, built here rather than in `main`, so that a
    // command which never touches a cluster — `cdm config validate`, `cdm completions` — does not
    // pay for a thread pool it will not use.
    runtime()?.block_on(async {
        let sessions = Sessions::open(&config).await?;
        let tables = ResolvedTables::introspect(&sessions, &config).await?;
        let job = build::job(kind, &sessions, &tables, &config, args).await?;
        let plan = token_plan(&config, &tables)?;
        let report = run(&config, plan, job).await?;
        Ok(RunSummary::from_report(kind, &report, args.dry_run))
    })
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
    let config = resolve(args)?;
    runtime()?.block_on(async {
        let sessions = Sessions::open(&config).await?;
        let tables = ResolvedTables::introspect(&sessions, &config).await?;
        let (plan, planner) = token_plan_with_planner(&config, &tables)?;
        let report = planner.report(&plan, None)?;
        Ok(PlanSummary::new(&report, &tables, &config))
    })
}

/// Loads the configuration and refuses to go further if it is invalid (`CFG-020`, `CFG-021`).
///
/// Tier 1 and tier 2 run here. Tier 3 needs the live schema and therefore runs inside
/// [`ResolvedTables::introspect`], once there is a session to ask.
fn resolve(args: &JobArgs) -> Result<EffectiveConfig, CdmError> {
    let outcome = load(&args.config)?;
    let Some(config) = outcome.config else {
        return Err(CdmError::new(
            ErrorKind::Config,
            "the configuration could not be assembled; run `cdm config validate` to see why",
        ));
    };

    // Every error at once, not the first: an operator fixing a configuration by trial and error,
    // one round trip per mistake, is the complaint `CFG-021` exists to answer.
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
