//! Repository automation for cdm-rs.
//!
//! Implements the CI gates described in `docs/SPEC.md`: `OPS-011` (requirement traceability) and
//! `OPS-012` (generated-artefact freshness), plus the one-command entry points of `OPS-024`.

mod traceability;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// cdm-rs repository automation.
#[derive(Debug, Parser)]
#[command(name = "xtask", about, version)]
struct Cli {
    /// The task to run.
    #[command(subcommand)]
    task: Task,
}

/// Available automation tasks.
#[derive(Debug, Subcommand)]
enum Task {
    /// Verify every requirement ID in SPEC.md is traced, tested and not orphaned (`OPS-011`).
    CheckTraceability,
    /// Regenerate `api/openapi.yaml` and `schema/*.json` (`API-002`, `OPS-012`).
    Openapi {
        /// Fail instead of writing when the checked-in files are stale.
        #[arg(long)]
        check: bool,
    },
    /// Regenerate `docs/generated/*.md` from the config, metric and CLI models (`OPS-012`).
    Docs {
        /// Fail instead of writing when the checked-in files are stale.
        #[arg(long)]
        check: bool,
    },
    /// Install native git hooks for contributors who do not use pre-commit (`OPS-003`).
    InstallHooks,
    /// Run the containerised integration suite (`TST-002`).
    It,
    /// Run the ported SIT parity suite (`TST-003`).
    Sit,
    /// Run the differential suite against Java CDM (`TST-020`).
    Differential,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli.task) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(task: &Task) -> anyhow::Result<()> {
    match task {
        Task::CheckTraceability => traceability::check(&repo_root()?),
        Task::Openapi { check } => openapi(*check),
        Task::Docs { check } => docs(*check),
        Task::InstallHooks | Task::It | Task::Sit | Task::Differential => {
            anyhow::bail!(not_yet(task))
        }
    }
}

/// The workspace root, derived from this crate's manifest directory.
fn repo_root() -> anyhow::Result<std::path::PathBuf> {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "cannot determine the workspace root from {}",
                manifest.display()
            )
        })
}

/// The message emitted for tasks whose implementation has not landed yet.
///
/// Naming the pull request that delivers a task is more useful than a bare "unimplemented", and
/// keeps the roadmap and the tooling honest about each other.
fn not_yet(task: &Task) -> String {
    let pr = match task {
        Task::InstallHooks => "a #1 follow-up",
        Task::It => "#16 (cdm-testkit)",
        Task::Sit => "#32 (SIT parity suite)",
        Task::Differential => "#34 (differential harness)",
        Task::CheckTraceability | Task::Openapi { .. } | Task::Docs { .. } => "this build",
    };
    format!("`{task:?}` is delivered by PR {pr}; see docs/ROADMAP.md")
}

/// `OPS-012`, OpenAPI half.
///
/// The generator lands with the HTTP control plane in PR #42. Until then `--check` verifies what
/// can be honestly verified: that the checked-in contract exists, declares OpenAPI 3.1, and is
/// marked as generated so nobody hand-edits it.
fn openapi(check: bool) -> anyhow::Result<()> {
    let path = repo_root()?.join("api/openapi.yaml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;

    anyhow::ensure!(
        text.contains("openapi: 3.1"),
        "{} does not declare `openapi: 3.1`",
        path.display()
    );
    anyhow::ensure!(
        text.contains("GENERATED FILE"),
        "{} must carry the generated-file banner",
        path.display()
    );

    anyhow::ensure!(
        check,
        "the OpenAPI generator is delivered by PR #42; see docs/ROADMAP.md"
    );
    println!(
        "openapi --check: {} is present and declares OpenAPI 3.1.\n\
         note: byte-for-byte regeneration checking arrives with the generator in PR #42.",
        path.display()
    );
    Ok(())
}

/// `OPS-012`, generated-documentation half. Delivered with the config model in PR #4.
fn docs(check: bool) -> anyhow::Result<()> {
    let dir = repo_root()?.join("docs/generated");
    anyhow::ensure!(dir.is_dir(), "{} is missing", dir.display());

    anyhow::ensure!(
        check,
        "the documentation generator is delivered by PR #4; see docs/ROADMAP.md"
    );
    println!(
        "docs --check: {} exists.\n\
         note: generation from the config, metric and CLI models arrives in PR #4.",
        dir.display()
    );
    Ok(())
}
