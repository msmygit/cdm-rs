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
        text.contains("GENERATED FILE"),
        "{} must carry the generated-file banner",
        path.display()
    );

    // Parse rather than grep. A prose description inside a YAML flow mapping silently
    // becomes a second key if it contains a comma, which is invalid OpenAPI that a
    // substring check would happily wave through.
    let doc: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("{} is not valid YAML: {e}", path.display()))?;

    let version = doc.get("openapi").and_then(serde_yaml::Value::as_str);
    anyhow::ensure!(
        version.is_some_and(|v| v.starts_with("3.1")),
        "{} declares `openapi: {:?}`, expected 3.1.x",
        path.display(),
        version.unwrap_or("<missing>")
    );

    let paths = doc
        .get("paths")
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or_else(|| anyhow::anyhow!("{} has no `paths` object", path.display()))?;
    anyhow::ensure!(!paths.is_empty(), "{} declares no paths", path.display());

    anyhow::ensure!(
        check,
        "the OpenAPI generator is delivered by PR #42; see docs/ROADMAP.md"
    );
    println!(
        "openapi --check: {} parses, declares OpenAPI 3.1 and defines {} paths.\n\
         note: byte-for-byte regeneration checking arrives with the generator in PR #42.",
        path.display(),
        paths.len()
    );
    Ok(())
}

/// `OPS-012`, generated-documentation half (`CFG-001`, `CFG-003`).
///
/// Both artefacts are projections of `cdm_config::CdmConfig`: the JSON Schema the web UI and
/// editors consume, and the property reference table. Generating them from the same registry the
/// loaders use is what makes the "no hand-maintained parallel list" half of `CFG-001` mechanical
/// rather than aspirational.
///
/// The metric and CLI tables named by `OPS-012` join this list when their models land, in PR #19
/// and PR #10 respectively.
fn docs(check: bool) -> anyhow::Result<()> {
    let root = repo_root()?;
    let artefacts = [
        (
            root.join("schema/cdm-config.schema.json"),
            cdm_config::json_schema_document(),
        ),
        (
            root.join("docs/generated/PROPERTIES.md"),
            cdm_config::properties_markdown(),
        ),
    ];

    let mut stale = Vec::new();
    for (path, generated) in &artefacts {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", path.display()))?;
        anyhow::ensure!(parent.is_dir(), "{} is missing", parent.display());

        let current = std::fs::read_to_string(path).unwrap_or_default();
        // Line-ending policy is git's business, not a staleness signal: a Windows checkout may
        // materialise an LF artefact with CRLF.
        if cdm_config::generate::is_current(&current, generated) {
            continue;
        }
        if check {
            stale.push(
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        } else {
            std::fs::write(path, generated)
                .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))?;
            println!(
                "docs: wrote {}",
                path.strip_prefix(&root).unwrap_or(path).display()
            );
        }
    }

    anyhow::ensure!(
        stale.is_empty(),
        "{} generated artefact(s) are stale; run `cargo xtask docs`:\n{}",
        stale.len(),
        stale
            .iter()
            .map(|path| format!("  - {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    if check {
        println!(
            "docs --check: {} generated artefact(s) are up to date.",
            artefacts.len()
        );
    }
    Ok(())
}
