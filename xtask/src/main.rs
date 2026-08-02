//! Repository automation for cdm-rs.
//!
//! Implements the CI gates described in [`docs/SPEC.md`] `OPS-011` (traceability) and
//! `OPS-012` (generated-artefact freshness), plus the one-command entry points of `OPS-024`.

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
    CheckTraceability {
        /// Fail on requirements that are marked done but lack a citing test.
        #[arg(long, default_value_t = true)]
        require_tests: bool,
    },
    /// Regenerate api/openapi.yaml and schema/*.json (`API-002`, `OPS-012`).
    Openapi {
        /// Fail instead of writing when the checked-in files are stale.
        #[arg(long)]
        check: bool,
    },
    /// Regenerate docs/generated/*.md from the config, metric and CLI models (`OPS-012`).
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    anyhow::bail!(
        "xtask task {:?} is not implemented yet; see docs/ROADMAP.md PR #1",
        cli.task
    )
}
