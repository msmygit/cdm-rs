//! The `cdm` command-line interface.
//!
//! The CLI is a thin client: it parses arguments, loads configuration, calls into the library
//! crates, and renders the result. No behaviour lives here that the HTTP API, the MCP server or
//! the web UI could not also reach — that is what keeps the four interfaces from drifting
//! (`docs/ARCHITECTURE.md` §10, `TST-050`).
//!
//! # Specification
//!
//! - `CLI-001` — the subcommand tree
//! - `CLI-002` — Java invocation shapes are accepted
//! - `CLI-003` — properties-to-canonical conversion
//! - `CLI-004` — meaningful exit codes
//! - `CLI-005` — machine-readable output
//! - `CLI-006` — the `cdm config init` wizard
//! - `CLI-007` — shell completions
//! - `CON-008`, `CON-029` — [`commands::connect`]
//! - `SCH-008` — [`commands::schema`]
//! - `CDC-031` — [`commands::codecs`]
//! - `TRK-034` — [`commands::runs`]
//! - `VAL-013`, `VAL-015`, `MET-033` — the validate flags and the run summary, via [`harness`]

pub mod cli;
pub mod commands;
pub mod exit;
pub mod harness;
pub mod loader;
pub mod output;

use std::io::Write;

use cdm_core::CdmError;
use cdm_core::JobKind;
use clap::{CommandFactory, Parser};

use crate::cli::{Cli, Command, ConfigCommand, RunsCommand};
use crate::exit::Exit;
use crate::harness::JobOptions;
use crate::output::{emit, Report};

/// Parses arguments and runs the requested command.
pub fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let mut stdout = std::io::stdout().lock();

    match run(&cli, &mut stdout) {
        Ok(exit) => exit.code(),
        Err(error) => {
            let diagnostic = error.to_diagnostic();
            let _ = writeln!(std::io::stderr(), "error: {}", diagnostic.title);
            if let Some(detail) = &diagnostic.detail {
                let _ = writeln!(std::io::stderr(), "  {detail}");
            }
            if let Some(suggestion) = &diagnostic.suggestion {
                let _ = writeln!(std::io::stderr(), "  try: {suggestion}");
            }
            Exit::for_error(&error).code()
        }
    }
}

/// Runs one command, writing its output to `out`.
///
/// Separated from [`main`] so tests can drive it with a buffer instead of a terminal.
pub fn run(cli: &Cli, out: &mut dyn Write) -> Result<Exit, CdmError> {
    match &cli.command {
        Command::Config { command } => run_config(cli, command, out),

        Command::Version => {
            let report = commands::misc::version();
            finish(&report, cli, out)
        }
        Command::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), "cdm", out);
            Ok(Exit::Success)
        }

        Command::Migrate(args) => run_job(cli, args, JobKind::Migrate, JobOptions::default(), out),
        Command::Validate(args) => run_job(
            cli,
            &args.job,
            JobKind::Validate,
            JobOptions {
                sample: args.sample,
                keys_only: args.keys_only,
            },
            out,
        ),
        Command::Guardrail(args) => {
            run_job(cli, args, JobKind::Guardrail, JobOptions::default(), out)
        }
        Command::Plan(args) => {
            let report = harness::plan(args)?;
            finish(&report, cli, out)
        }

        Command::Runs { command } => run_runs(cli, command, out),
        Command::Schema { command } => match command {
            crate::cli::SchemaCommand::Show(args) => {
                let report = commands::schema::show(args)?;
                finish(&report, cli, out)
            }
            crate::cli::SchemaCommand::Diff(args) => {
                let report = commands::schema::diff(args)?;
                finish(&report, cli, out)
            }
        },
        Command::Connect { command } => {
            let crate::cli::ConnectCommand::Test { side, config } = command;
            let report = commands::connect::test(config, *side)?;
            finish(&report, cli, out)
        }
        Command::Codecs => {
            let report = commands::codecs::list()?;
            finish(&report, cli, out)
        }

        // Specified, scheduled, not yet built. Each says what is missing and where it comes from.
        Command::Cluster => Err(commands::misc::not_yet(
            "cdm cluster",
            "it reports live nodes, their leases and their per-node counters (DST-018), all of \
             which are written by the `Coordinator` in `cdm-cluster` — a crate that is still a \
             stub, so there is no membership table to read",
        )),
        Command::Serve(_) => Err(commands::misc::not_yet(
            "cdm serve",
            "the HTTP control plane, the web UI and the metrics endpoint live in `cdm-service`, \
             `cdm-api` and `cdm-ui`, which are Phase 6 and not yet built",
        )),
        Command::Mcp => Err(commands::misc::not_yet(
            "cdm mcp",
            "the MCP tool surface is generated from the OpenAPI document in `cdm-api` and served \
             by `cdm-mcp`, both of which are Phase 6 and not yet built",
        )),
    }
}

/// Runs one of the three jobs through the shared harness (`CLI-001`, `CLI-004`, `MET-033`).
fn run_job(
    cli: &Cli,
    args: &crate::cli::JobArgs,
    kind: JobKind,
    options: JobOptions,
    out: &mut dyn Write,
) -> Result<Exit, CdmError> {
    let outcome = harness::execute(args, kind, options)?;

    // A summary is written before the exit code is decided, so a run that ends badly still
    // reports what it did. Java prints its counter block and then throws; the numbers an operator
    // needs are exactly the ones an early return would swallow.
    if let Some(path) = &args.summary_out {
        write_summary(&outcome.record, path)?;
    }

    // `finish` maps findings onto `Completed`, which is right for a validate discrepancy and
    // wrong for an interruption: `CLI-004` reserves `4` for the one outcome a supervisor may
    // retry unchanged.
    let exit = outcome.summary.exit();
    finish(&outcome.summary, cli, out)?;
    Ok(exit)
}

/// Writes the `MET-033` run summary to `path`.
///
/// The document is `cdm-metrics`' own, not a second model of the run assembled here: it carries the
/// config hash of `CFG-023`, the plan, every counter, the timings, the per-node breakdown and — for
/// a validate run — the discrepancy totals and a pointer to the `VAL-013` report.
fn write_summary(
    summary: &cdm_metrics::RunSummary,
    path: &std::path::Path,
) -> Result<(), CdmError> {
    // A summary whose directory does not exist is a summary an operator asked for and did not get,
    // discovered at the end of the run. Creating it is cheaper than explaining that.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|error| {
            CdmError::new(
                cdm_core::ErrorKind::Internal,
                format!("cannot create {}: {error}", parent.display()),
            )
        })?;
    }
    std::fs::write(path, summary.to_json()?).map_err(|error| {
        CdmError::new(
            cdm_core::ErrorKind::Internal,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}

/// `cdm runs …` (`TRK-034`).
fn run_runs(cli: &Cli, command: &RunsCommand, out: &mut dyn Write) -> Result<Exit, CdmError> {
    match command {
        RunsCommand::List(config) => {
            let report = commands::runs::list(config)?;
            finish(&report, cli, out)
        }
        RunsCommand::Show { run_id, config } => {
            let report = commands::runs::show(config, *run_id)?;
            finish(&report, cli, out)
        }
        RunsCommand::Cancel { run_id, config } => {
            let report = commands::runs::cancel(config, *run_id)?;
            finish(&report, cli, out)
        }
        // `cdm runs resume` is the one operation here that starts a run rather than reading one.
        // `RunManager::resume` already produces the work list (`TRK-030`..`TRK-033`), but the
        // harness plans a ring from `Planner::plan` and has no way to be handed a pre-computed set
        // of ranges instead; until `TokenPlan` can be built from a `ResumePlan`, wiring this would
        // mean re-planning the whole ring and calling it a resume.
        RunsCommand::Resume { .. } => Err(commands::misc::not_yet(
            "cdm runs resume",
            "`cdm-track` computes the resume work list already, but the harness cannot yet hand a \
             pre-computed range set to the scheduler in place of a freshly planned ring; \
             `cdm runs list` and `cdm runs show` will tell you what is outstanding",
        )),
    }
}

fn run_config(cli: &Cli, command: &ConfigCommand, out: &mut dyn Write) -> Result<Exit, CdmError> {
    match command {
        ConfigCommand::Validate { config, tier } => {
            let report = commands::config::validate(config, *tier)?;
            finish(&report, cli, out)
        }
        ConfigCommand::Explain { key, config } => {
            let report = commands::config::explain(key, config)?;
            finish(&report, cli, out)
        }
        ConfigCommand::Diff { left, right } => {
            let report = commands::config::diff(left, right)?;
            finish(&report, cli, out)
        }
        ConfigCommand::Schema => {
            let report = commands::config::schema()?;
            finish(&report, cli, out)
        }
        ConfigCommand::Convert { from, to } => {
            let report = commands::config::convert(from, to.as_deref())?;
            finish(&report, cli, out)
        }
        ConfigCommand::Init {
            config,
            out: path,
            non_interactive,
        } => {
            let report = commands::init::init(config, path.as_deref(), *non_interactive)?;
            finish(&report, cli, out)
        }
    }
}

/// Renders a report and maps it to an exit code (`CLI-004`, `CLI-005`).
fn finish<R: Report>(report: &R, cli: &Cli, out: &mut dyn Write) -> Result<Exit, CdmError> {
    emit(report, cli.output, out).map_err(|e| {
        CdmError::new(
            cdm_core::ErrorKind::Internal,
            format!("cannot write output: {e}"),
        )
    })?;

    Ok(if report.has_findings() {
        Exit::Completed
    } else {
        Exit::Success
    })
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
    use std::path::PathBuf;

    use cdm_core::{JobKind, RunStatus};
    use cdm_metrics::{DiscrepancyReportRef, RunSummary};

    use super::*;

    fn summary() -> RunSummary {
        RunSummary::new(
            JobKind::Validate,
            RunStatus::Ended,
            "node-a",
            chrono::Utc::now(),
            std::time::Duration::from_secs(90),
        )
        .with_config_hash("0123456789abcdef")
    }

    #[test]
    fn met_033_the_summary_is_written_as_one_json_document() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        write_summary(&summary(), &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(value["job"], "validate");
        assert_eq!(value["config_hash"], "0123456789abcdef");
        assert!(value["schema"].is_string(), "{text}");
    }

    #[test]
    fn met_033_a_summary_directory_that_does_not_exist_is_created() {
        // `--summary-out reports/2026-08-07/run.json` is how people actually spell it, and
        // discovering at the *end* of a six-hour run that the directory was missing is the one
        // failure mode this file exists to prevent.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reports").join("nested").join("run.json");
        write_summary(&summary(), &path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn cfg_023_the_summary_carries_the_config_digest_and_never_the_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        write_summary(&summary(), &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        // Two runs that disagree about a result must be comparable for "did they run the same
        // job?" without either party sending the other their credentials (`SEC-001`).
        assert!(text.contains("0123456789abcdef"), "{text}");
        assert!(!text.contains("password"), "{text}");
        assert!(!text.contains("connect"), "{text}");
    }

    #[test]
    fn val_013_the_summary_points_at_the_discrepancy_report_rather_than_inlining_it() {
        // A large validate run has millions of findings. Inlining them would turn the one artefact
        // that must stay attachable to a ticket into one that cannot be, so the summary carries a
        // pointer -- and, crucially, whether the file it points at was redacted (`SEC-002`).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");

        let counters = cdm_metrics::JobCounters::new(JobKind::Validate);
        let record =
            summary()
                .with_counters(&counters)
                .with_discrepancy_report(DiscrepancyReportRef {
                    path: PathBuf::from("cdm_logs/cdm_discrepancies.ndjson"),
                    format: "ndjson".to_owned(),
                    records: 17,
                    values_redacted: true,
                });
        write_summary(&record, &path).unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let report = &value["discrepancies"]["report"];
        assert_eq!(report["records"], 17);
        assert_eq!(report["values_redacted"], true);
        assert_eq!(report["format"], "ndjson");
    }
}
