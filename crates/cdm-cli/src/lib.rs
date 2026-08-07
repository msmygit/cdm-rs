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
//! - `CLI-007` — shell completions

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

use crate::cli::{Cli, Command, ConfigCommand};
use crate::exit::Exit;
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

        // Specified, scheduled, not yet built. Each names the pull request that delivers it.
        Command::Migrate(args) => run_job(cli, args, JobKind::Migrate, out),
        Command::Validate(args) => run_job(cli, args, JobKind::Validate, out),
        Command::Guardrail(args) => run_job(cli, args, JobKind::Guardrail, out),
        Command::Plan(args) => {
            let report = harness::plan(args)?;
            finish(&report, cli, out)
        }
        Command::Runs { .. } => Err(commands::misc::not_yet("cdm runs", "#25")),
        Command::Schema { .. } => Err(commands::misc::not_yet("cdm schema", "#9")),
        Command::Connect { .. } => Err(commands::misc::not_yet("cdm connect", "#7")),
        Command::Codecs => Err(commands::misc::not_yet("cdm codecs", "#12")),
        Command::Cluster => Err(commands::misc::not_yet("cdm cluster", "#50")),
        Command::Serve(_) => Err(commands::misc::not_yet("cdm serve", "#42")),
        Command::Mcp => Err(commands::misc::not_yet("cdm mcp", "#45")),
    }
}

/// Runs one of the three jobs through the shared harness (`CLI-001`, `CLI-004`).
fn run_job(
    cli: &Cli,
    args: &crate::cli::JobArgs,
    kind: JobKind,
    out: &mut dyn Write,
) -> Result<Exit, CdmError> {
    let summary = harness::execute(args, kind)?;

    // A summary is written before the exit code is decided, so a run that ends badly still
    // reports what it did. Java prints its counter block and then throws; the numbers an operator
    // needs are exactly the ones an early return would swallow.
    if let Some(path) = &args.summary_out {
        let json = serde_json::to_string_pretty(&summary).map_err(|error| {
            CdmError::new(
                cdm_core::ErrorKind::Internal,
                format!("cannot render the run summary: {error}"),
            )
        })?;
        std::fs::write(path, json).map_err(|error| {
            CdmError::new(
                cdm_core::ErrorKind::Internal,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    }

    // `finish` maps findings onto `Completed`, which is right for a validate discrepancy and
    // wrong for an interruption: `CLI-004` reserves `4` for the one outcome a supervisor may
    // retry unchanged.
    let exit = summary.exit();
    finish(&summary, cli, out)?;
    Ok(exit)
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
        ConfigCommand::Init { .. } => {
            Err(commands::misc::not_yet("cdm config init", "#10 follow-up"))
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
