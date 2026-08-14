//! Repository automation for cdm-rs.
//!
//! Implements the CI gates described in `docs/SPEC.md`: `OPS-011` (requirement traceability) and
//! `OPS-012` (generated-artefact freshness), plus the one-command entry points of `OPS-024`.

mod traceability;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand, ValueEnum};

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
    /// Run the containerised integration suite (`TST-002`, `TST-102`).
    It {
        /// Which engines to run against: `cassandra`, `scylla`, `all`, or `image:tag` pairs.
        ///
        /// Sets `CDM_IT_ENGINES` for the test process. Defaults to whatever is already in the
        /// environment, and thence to the newest Cassandra.
        #[arg(long)]
        engines: Option<String>,
    },
    /// Run the ported SIT parity suite (`TST-003`).
    Sit,
    /// Run the tier-2 macro-benchmark: rows per second, end to end (`TST-060`, `NFR-004`).
    Bench {
        /// Rows to migrate. Defaults to the spec's reference workload.
        ///
        /// Every flag here is optional so that the defaults live in exactly one place —
        /// `MacroBenchSpec::default()`. A duplicated default in the CLI is a default that
        /// silently stops matching the one the harness documents.
        #[arg(long)]
        rows: Option<u64>,
        /// Non-key columns per row.
        #[arg(long)]
        columns: Option<usize>,
        /// Container image for both clusters, as `repository:tag`.
        #[arg(long)]
        image: Option<String>,
        /// Emit one machine-readable `bencher` line instead of the human summary.
        #[arg(long)]
        bencher: bool,
    },
    /// Run the differential suite against Java CDM (`TST-020`).
    Differential {
        /// Which generated corpus to run: the full CQL type matrix, or its smoke subset.
        #[arg(long, value_enum, default_value_t = CorpusChoice::Full)]
        corpus: CorpusChoice,
        /// The seed the corpus is generated from.
        ///
        /// Defaults to `CDM_TEST_SEED`, and thence to entropy. Every run prints the seed it used
        /// and writes it beside the report, so a nightly failure replays exactly with
        /// `--seed <n> --corpus <same>`. A differential failure that cannot be reproduced is a
        /// rumour, not a defect.
        #[arg(long)]
        seed: Option<u64>,
        /// Where the diff report and both runs' evidence are written.
        #[arg(long, default_value = "reports/differential")]
        out: PathBuf,
        /// Leave the second run's containers up afterwards, for inspection.
        #[arg(long)]
        keep_clusters: bool,
    },
}

/// Which generated corpus a differential run uses (`TST-020`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CorpusChoice {
    /// Every CQL type, nesting depth 3, nulls, empty collections and edge-case values.
    ///
    /// What `TST-020` actually requires, and what the nightly runs.
    Full,
    /// The same shapes over far fewer rows: for iterating on the harness itself.
    ///
    /// A green smoke run proves the plumbing, never parity — it does not cover the type matrix.
    Smoke,
}

impl std::fmt::Display for CorpusChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Full => "full",
            Self::Smoke => "smoke",
        })
    }
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
        Task::It { engines } => integration(engines.as_deref()),
        Task::Sit => sit(),
        Task::Bench {
            rows,
            columns,
            image,
            bencher,
        } => bench(*rows, *columns, image.as_deref(), *bencher),
        Task::Differential {
            corpus,
            seed,
            out,
            keep_clusters,
        } => differential(*corpus, *seed, out, *keep_clusters),
        Task::InstallHooks => {
            anyhow::bail!(not_yet(task))
        }
    }
}

/// The workspace root, derived from this crate's manifest directory.
fn repo_root() -> anyhow::Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
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
        Task::CheckTraceability
        | Task::Openapi { .. }
        | Task::Docs { .. }
        | Task::It { .. }
        | Task::Sit
        | Task::Bench { .. }
        | Task::Differential { .. } => "this build",
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

/// `TST-002` and `TST-102`: the one command that runs the containerised suite.
///
/// Every containerised test is marked `#[ignore]`, so `cargo test` leaves them alone and stays
/// fast and offline; this is the command that opts in. It runs single-threaded because the
/// container fixtures publish the CQL port on the host unchanged — a node advertises its own
/// address and port, so a mapped port would leave the driver unable to reach the pool — and two
/// fixtures therefore cannot share a port.
///
/// # Skipping is not failing
///
/// `TST-102` requires a clear message rather than a failure when no container runtime is
/// available. The check uses `cdm_testkit::ContainerRuntime::detect`, the same rule the tests
/// themselves apply, so this command and the suite can never disagree about whether a runtime is
/// there.
fn integration(engines: Option<&str>) -> anyhow::Result<()> {
    match cdm_testkit::ContainerRuntime::detect() {
        Ok(runtime) => println!("integration: using container runtime at {runtime}"),
        Err(reason) => {
            println!("{reason}");
            println!(
                "integration: skipped, not failed (TST-102). Nothing ran, and that is not an error."
            );
            return Ok(());
        }
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = std::process::Command::new(cargo);
    command.current_dir(repo_root()?).args([
        "test",
        "--workspace",
        "--all-features",
        "--",
        "--ignored",
        "--test-threads=1",
        "--nocapture",
    ]);
    if let Some(engines) = engines {
        command.env("CDM_IT_ENGINES", engines);
    }

    let status = command
        .status()
        .map_err(|e| anyhow::anyhow!("cannot run the integration suite: {e}"))?;
    anyhow::ensure!(status.success(), "the integration suite failed: {status}");
    Ok(())
}

/// `TST-003`: the ported Java SIT parity suite.
///
/// Two steps, and the order matters. The suite drives the `cdm` **binary** as a subprocess — as
/// Java's SIT drives `spark-submit` rather than `CopyJobSession` — so the binary has to exist
/// before the tests look for it beside themselves. Building it here rather than leaving it to the
/// caller is what makes `cargo xtask sit` the one command the workflow and a developer both run.
///
/// Single-threaded for the same reason as [`integration`], and one more: every case owns tables in
/// the shared `origin` and `target` keyspaces, exactly as the Java harness does, so two cases
/// running at once would drop each other's tables.
///
/// # Skipping is not failing
///
/// As `TST-102`: with no container runtime this reports why and returns success. A red suite that
/// only means "no Docker here" trains people to ignore it.
fn sit() -> anyhow::Result<()> {
    match cdm_testkit::ContainerRuntime::detect() {
        Ok(runtime) => println!("sit: using container runtime at {runtime}"),
        Err(reason) => {
            println!("{reason}");
            println!("sit: skipped, not failed (TST-102). Nothing ran, and that is not an error.");
            return Ok(());
        }
    }

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let root = repo_root()?;

    let status = std::process::Command::new(&cargo)
        .current_dir(&root)
        .args(["build", "-p", "cdm-cli", "--bin", "cdm"])
        .status()
        .map_err(|e| anyhow::anyhow!("cannot build the `cdm` binary: {e}"))?;
    anyhow::ensure!(
        status.success(),
        "the `cdm` binary did not build, so no SIT case could run: {status}"
    );

    remove_sit_node();
    let status = std::process::Command::new(&cargo)
        .current_dir(&root)
        .args([
            "test",
            "-p",
            "cdm-testkit",
            "--test",
            "sit_it",
            "--",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .status()
        .map_err(|e| anyhow::anyhow!("cannot run the SIT parity suite: {e}"))?;
    remove_sit_node();
    anyhow::ensure!(status.success(), "the SIT parity suite failed: {status}");
    Ok(())
}

/// `TST-060` tier 2, and the only measurement that speaks to `NFR-004`: rows per second for a
/// whole migration, origin container to target container.
///
/// The micro-benchmarks in `crates/*/benches` answer "did this commit make the hot path slower".
/// They cannot answer "how fast is a migration", because the answer is dominated by the driver,
/// the network and the two clusters — none of which a criterion harness contains. This does, at
/// the cost of needing a container runtime and minutes rather than seconds.
///
/// # Not a gate
///
/// This reports a number; it never decides whether a change is acceptable. Throughput here moves
/// with the runner's disk, its co-tenants and the container image's own tuning, so a threshold on
/// it would fire on the weather. See `docs/BENCHMARKS.md` §1. The workflow that runs this weekly
/// records the figure and stops there.
///
/// # Skipping is not failing
///
/// As [`integration`] and [`sit`], under `TST-102`. Informational output goes to stderr so that
/// stdout carries only the result line: `--bencher` output is piped into a parser, and a
/// "skipped, no runtime" sentence arriving on the same channel would either be parsed as a
/// benchmark or abort the parse.
fn bench(
    rows: Option<u64>,
    columns: Option<usize>,
    image: Option<&str>,
    bencher: bool,
) -> anyhow::Result<()> {
    match cdm_testkit::ContainerRuntime::detect() {
        Ok(runtime) => eprintln!("bench: using container runtime at {runtime}"),
        Err(reason) => {
            eprintln!("{reason}");
            eprintln!(
                "bench: skipped, not failed (TST-102). Nothing ran, and that is not an error."
            );
            return Ok(());
        }
    }

    let mut spec = cdm_testkit::macrobench::MacroBenchSpec::default();
    if let Some(rows) = rows {
        spec.rows = rows;
    }
    if let Some(columns) = columns {
        spec.columns = columns;
    }
    if let Some(image) = image {
        image.clone_into(&mut spec.image);
    }
    eprintln!(
        "bench: {} rows x {} columns on {}, seed {}",
        spec.rows, spec.columns, spec.image, spec.seed
    );

    // A current-thread runtime would serialise the concurrent writes the engine issues, and so
    // measure the harness rather than the migration.
    let runtime = tokio::runtime::Runtime::new().map_err(|e| {
        anyhow::anyhow!("cannot start a tokio runtime for the macro-benchmark: {e}")
    })?;
    let result = runtime
        .block_on(cdm_testkit::macrobench::run_macro_bench(&spec))
        .map_err(|e| anyhow::anyhow!("the macro-benchmark failed: {e}"))?;

    if bencher {
        println!("{}", result.to_bencher_line());
    } else {
        println!("{}", result.summary());
    }
    Ok(())
}

// --------------------------------------------------------------- TST-020: the differential suite

/// The pinned Java CDM environment this suite runs on, relative to the workspace root.
///
/// Reused rather than reinvented. That directory stands up Java CDM 6.0.1 on Spark 4.1.2 against
/// two containerised Cassandra 5.0 nodes, with both downloads SHA-512 checked, and it is
/// *verified*: its smoke test moved 200,000 rows with zero errors. Its README records eight
/// caveats, two of which are `spark-submit` workarounds — a generated `/etc/passwd` and
/// `-Duser.home` — that nobody would rediscover cheaply. Tier 3 of `docs/BENCHMARKS.md` uses that
/// environment to ask *how fast*; `TST-020` uses the same environment to ask *does it produce the
/// same bytes*.
const JAVA_ENV_DIR: &str = "bench/java-comparison/environment";

/// Statements sent to `cqlsh` in one invocation while seeding the origin.
///
/// `README-ENVIRONMENT.md` caveat 6: one `cqlsh` connection fed statements as fast as a process
/// can write them trips the coordinator — a 100,000-statement load died around statement 10,500
/// with `NoHostAvailable`. A correctness corpus is far smaller than a benchmark dataset, but the
/// failure mode is a function of the *rate*, not of the total, so the load is chunked anyway. Each
/// chunk is a fresh connection, which also bounds how much has to be replayed to diagnose a
/// rejected statement.
const CQLSH_STATEMENTS_PER_INVOCATION: usize = 250;

/// Ring splits both implementations are given (`spark.cdm.perfops.numParts`).
///
/// Java CDM's default is 5000, which on a single node is 5000 Spark tasks scheduling over a
/// handful of rows. 64 is what `submit-migrate.sh` defaults to and is enough splits that the token
/// planner's boundary handling (`TOK-003`) is genuinely exercised on both sides rather than being
/// one range that trivially covers the ring.
const DIFFERENTIAL_NUM_PARTS: &str = "64";

/// Rows per second each side is allowed, per cluster (`spark.cdm.perfops.ratelimit.*`).
///
/// Java CDM's rate limiter is also the only backpressure in its write path, and removing it cost
/// 135,264 error records in PR #88's testing. It is set identically on both sides, and well above
/// anything a correctness corpus can reach, so it is inert here and cannot be the reason the two
/// implementations disagree.
const DIFFERENTIAL_RATELIMIT: &str = "100000";

/// Writes per batch, both sides (`spark.cdm.perfops.batchSize`).
const DIFFERENTIAL_BATCH_SIZE: &str = "5";

/// Read page size, both sides (`spark.cdm.perfops.fetchSizeInRows`).
const DIFFERENTIAL_FETCH_SIZE: &str = "1000";

/// `TST-020`: run Java CDM and cdm-rs over one seeded corpus and prove they agree.
///
/// > a harness runs both implementations against the same seeded dataset and asserts
/// > byte-identical target state and identical counter blocks
///
/// # Why this is a gate when the benchmarks are not
///
/// `cargo xtask bench` reports a number and decides nothing, because throughput moves with the
/// runner's disk and its co-tenants. This decides. A difference between the two implementations is
/// not weather: it is either a defect in cdm-rs or a deliberate divergence that has not been
/// recorded in `docs/MIGRATION_FROM_JAVA.md`, and both need a human. So the diff is written to
/// `--out` — uploaded by `differential.yml` whatever the outcome — and a difference exits non-zero.
///
/// # The order of operations, and why it is this order
///
/// 1. **Both implementations are built before either runs.** A compile error discovered forty
///    minutes into a run has wasted forty minutes of cluster time.
/// 2. **Java runs first, into its own pair of fresh nodes**, and its target's state is captured
///    before those nodes are destroyed. Two live targets would be the stronger arrangement, but
///    `versions.env` declares exactly one origin/target pair and the environment's scripts are
///    reused rather than forked; capturing is what makes one pair enough.
/// 3. **cdm-rs then runs against a newly created pair**, seeded from the same [`Seed`] and so from
///    byte-identical statements. Nothing of the Java run survives into it: not the target's
///    SSTables, not its compaction backlog, not the origin's page cache.
/// 4. **Both halves are verified complete before anything is compared.** A partial migration
///    diffed against a complete one produces a large, confident and meaningless report.
///
/// # Skipping is not failing
///
/// As [`integration`], [`sit`] and [`bench()`], under `TST-102`: with no container runtime this
/// says so and returns success.
///
/// [`Seed`]: cdm_testkit::Seed
fn differential(
    corpus: CorpusChoice,
    seed: Option<u64>,
    out: &Path,
    keep_clusters: bool,
) -> anyhow::Result<()> {
    // The same rule the containerised tests themselves apply, so the command and the suite can
    // never disagree about whether a runtime is there.
    match cdm_testkit::ContainerRuntime::detect() {
        Ok(runtime) => println!("differential: using container runtime at {runtime}"),
        Err(reason) => {
            skipped(&reason.to_string());
            return Ok(());
        }
    }
    // And then Docker specifically. Every script under `bench/java-comparison/environment/` names
    // `docker` — `docker build`, `docker network`, `docker exec` — so a host with Podman alone
    // passes the check above and would fail on the first script. That is a missing runtime by any
    // useful definition, and `TST-102` says a missing runtime is a message, not a red build.
    if let Err(reason) = docker_answers() {
        skipped(&reason);
        return Ok(());
    }

    let root = repo_root()?;
    let env_dir = root.join(JAVA_ENV_DIR);
    let environment = JavaEnvironment::load(&env_dir)?;
    let out = if out.is_absolute() {
        out.to_path_buf()
    } else {
        root.join(out)
    };
    std::fs::create_dir_all(&out)
        .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", out.display()))?;

    let seed = seed.map_or_else(
        cdm_testkit::Seed::from_env_or_entropy,
        cdm_testkit::Seed::new,
    );
    let generated = generate_corpus(corpus, seed)?;
    println!(
        "differential: {corpus} corpus, seed {}, {} rows in {}",
        seed.value(),
        generated.row_count(),
        generated.table()
    );
    // Written before anything can fail, so that a run killed by a CI timeout still says how to
    // reproduce itself.
    write_file(
        &out.join("seed.txt"),
        &format!(
            "corpus = {corpus}\nseed = {}\nreplay with: cargo xtask differential --corpus \
             {corpus} --seed {}\n",
            seed.value(),
            seed.value()
        ),
    )?;

    let cdm_binary = build_cdm_binary(&root)?;
    run_environment_script(&env_dir, "build-image.sh", &[], &[])?;

    let java = java_half(&env_dir, &environment, &generated, &out)?;
    let rust = rust_half(&cdm_binary, &env_dir, &environment, &generated, &out)?;

    if keep_clusters {
        println!(
            "differential: --keep-clusters: the cdm-rs pair is still up. \
             `{JAVA_ENV_DIR}/clusters-down.sh` removes it."
        );
    } else {
        clusters_down(&env_dir);
    }

    compare_halves(&java, &rust, &out)
}

/// Prints why nothing ran and reports success (`TST-102`).
///
/// A red suite that only means "no Docker here" trains people to ignore it.
fn skipped(reason: &str) {
    println!("{reason}");
    println!("differential: skipped, not failed (TST-102). Nothing ran, and that is not an error.");
}

/// Whether Docker is installed *and* answering.
fn docker_answers() -> Result<(), String> {
    match Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "`docker info` exited {status}; the daemon is not answering. Every script in \
             {JAVA_ENV_DIR} needs it."
        )),
        Err(e) => Err(format!(
            "`docker` is not runnable ({e}). Every script in {JAVA_ENV_DIR} needs it, so the \
             Java half of this suite cannot run without it."
        )),
    }
}

/// Builds the seeded corpus (`TST-020`).
fn generate_corpus(
    corpus: CorpusChoice,
    seed: cdm_testkit::Seed,
) -> anyhow::Result<cdm_testkit::differential::Corpus> {
    let built = match corpus {
        CorpusChoice::Full => cdm_testkit::differential::Corpus::full(seed),
        CorpusChoice::Smoke => cdm_testkit::differential::Corpus::smoke(seed),
    };
    built.map_err(|e| {
        anyhow::anyhow!(
            "cannot generate the {corpus} corpus at seed {}: {e}",
            seed.value()
        )
    })
}

/// What one implementation left behind, and the evidence that it finished.
#[derive(Debug)]
struct Half {
    /// How this half is named in messages and in the report.
    name: &'static str,
    /// The target's contents, as `cqlsh` renders them.
    target_state: String,
    /// The `MET-006` final counter block, normalised to begin at `Final `.
    counter_block: String,
    /// Rows the target actually holds, counted independently of anything either job claimed.
    target_rows: u64,
}

/// Runs Java CDM against a fresh pair of nodes and captures what it produced.
fn java_half(
    env_dir: &Path,
    environment: &JavaEnvironment,
    corpus: &cdm_testkit::differential::Corpus,
    out: &Path,
) -> anyhow::Result<Half> {
    let dir = out.join("java-cdm");
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", dir.display()))?;
    prepare_clusters(env_dir, environment, corpus)?;

    println!(
        "differential: running Java CDM Migrate over {}",
        corpus.table()
    );
    let outdir = path_argument(&dir)?;
    // `submit-migrate.sh` owns the whole `spark-submit` line, including both workarounds. Every
    // knob it exposes is set here rather than left to its defaults, so that this file is the one
    // place a reader can see that the two implementations were configured alike.
    let submitted = run_environment_script(
        env_dir,
        "submit-migrate.sh",
        &[corpus.table(), outdir],
        &[
            ("CDM_NUM_PARTS", DIFFERENTIAL_NUM_PARTS),
            ("CDM_BATCH_SIZE", DIFFERENTIAL_BATCH_SIZE),
            ("CDM_FETCH_SIZE", DIFFERENTIAL_FETCH_SIZE),
            ("CDM_RATELIMIT", DIFFERENTIAL_RATELIMIT),
        ],
    );
    // The script's own output holds the counter block it greps out of Spark's log, so it is kept
    // whether or not the run succeeded — a failed differential run is diagnosed from this file.
    let log = dir.join("submit.log");
    if let Ok(text) = &submitted {
        write_file(&log, text)?;
    }
    let text = submitted.map_err(|e| {
        anyhow::anyhow!("Java CDM's spark-submit failed: {e}\nsee {}", log.display())
    })?;

    finish_half("java-cdm", &text, environment, corpus, &dir)
}

/// Runs cdm-rs against a fresh pair of nodes and captures what it produced.
fn rust_half(
    cdm_binary: &Path,
    env_dir: &Path,
    environment: &JavaEnvironment,
    corpus: &cdm_testkit::differential::Corpus,
    out: &Path,
) -> anyhow::Result<Half> {
    let dir = out.join("cdm-rs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", dir.display()))?;
    prepare_clusters(env_dir, environment, corpus)?;

    let properties = dir.join("cdm.properties");
    write_file(&properties, &rust_properties(environment, corpus.table()))?;

    println!(
        "differential: running cdm-rs migrate over {}",
        corpus.table()
    );
    // Run from the output directory: `VAL-012`'s diff log defaults to `cdm_logs/cdm_diff.log`
    // relative to the working directory, and a run started from the repository root would leave
    // one in the source tree.
    let output = Command::new(cdm_binary)
        .current_dir(&dir)
        .arg("migrate")
        .arg("--properties-file")
        .arg(&properties)
        .arg("--summary-out")
        .arg(dir.join("summary.json"))
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run `{} migrate`: {e}", cdm_binary.display()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = dir.join("migrate.log");
    write_file(&log, &text)?;
    // `CLI-004` reserves 2..5 for a run that never happened. Unlike Java CDM, cdm-rs's exit status
    // is trustworthy — but it is checked *as well as* the counter block, not instead of it.
    anyhow::ensure!(
        output.status.code() == Some(0),
        "cdm-rs migrate exited {:?}; see {}",
        output.status.code(),
        log.display()
    );

    finish_half("cdm-rs", &text, environment, corpus, &dir)
}

/// Verifies one half completed and captures its target, given the job's own output.
///
/// # Completion is proved three times over
///
/// A partial migration compared against a complete one produces a diff that is entirely about the
/// missing rows and says nothing about parity, so nothing is compared until both halves clear all
/// three of:
///
/// * **the error counter is zero.** `README-ENVIRONMENT.md` caveat 4: Java CDM exits **0** after
///   losing data. A run reporting `Final Error Record Count: 34454` with 1,465 rows missing on the
///   target still returned success, because nothing in its `src/main/scala` calls `System.exit`.
///   The exit status is therefore not evidence, and the counter block is.
/// * **the write counter equals the corpus.** The counter block can only report what the job
///   believes.
/// * **an independent `SELECT COUNT(*)` on the target equals the corpus.** This is the one check
///   that does not take either implementation's word for anything.
fn finish_half(
    name: &'static str,
    job_output: &str,
    environment: &JavaEnvironment,
    corpus: &cdm_testkit::differential::Corpus,
    dir: &Path,
) -> anyhow::Result<Half> {
    let counter_block = normalised_counter_block(job_output).ok_or_else(|| {
        anyhow::anyhow!(
            "{name} printed no `Final … Record Count` block (MET-006), so there is no evidence it \
             ran to completion; see {}",
            dir.display()
        )
    })?;
    write_file(&dir.join("counters.txt"), &format!("{counter_block}\n"))?;

    let expected = corpus.row_count();
    let errors = counter(&counter_block, "Error").unwrap_or(0);
    anyhow::ensure!(
        errors == 0,
        "{name} reported {errors} error records. It may well have exited 0 anyway — Java CDM \
         does — but a run that dropped rows cannot be compared against one that did not.\n{counter_block}"
    );
    let written = counter(&counter_block, "Write").ok_or_else(|| {
        anyhow::anyhow!(
            "{name}'s counter block has no `Final Write Record Count`:\n{counter_block}"
        )
    })?;
    anyhow::ensure!(
        written == expected,
        "{name} wrote {written} of the corpus's {expected} rows:\n{counter_block}"
    );

    let target_rows = count_target_rows(environment, corpus.table())?;
    anyhow::ensure!(
        target_rows == expected,
        "{name} claims {written} rows written, and the target holds {target_rows} of the \
         corpus's {expected}. The claim is not the evidence."
    );

    let target_state = capture_target_state(environment, corpus.table())?;
    write_file(&dir.join("target-state.txt"), &target_state)?;
    println!("differential: {name} completed, {target_rows} rows verified on the target");

    Ok(Half {
        name,
        target_state,
        counter_block,
        target_rows,
    })
}

/// Compares the two halves and writes the report that is this suite's product.
fn compare_halves(java: &Half, rust: &Half, out: &Path) -> anyhow::Result<()> {
    let report = out.join("report.txt");

    // The one call whose exact shape belongs to the comparison engine in
    // `crates/cdm-testkit/src/differential/compare.rs`. Everything above it produces the four
    // pieces of evidence `TST-020` names — two target states and two counter blocks — as plain
    // text on disk, so adapting to a different signature costs this statement and nothing else.
    match cdm_testkit::differential::compare(
        &java.target_state,
        &java.counter_block,
        &rust.target_state,
        &rust.counter_block,
    ) {
        Ok(()) => {
            write_file(
                &report,
                &format!(
                    "TST-020: {} and {} produced identical target state and identical counter \
                     blocks over {} rows.\n",
                    java.name, rust.name, java.target_rows
                ),
            )?;
            println!(
                "differential: identical. {} rows, byte for byte, counter block included.",
                java.target_rows
            );
            Ok(())
        }
        Err(diff) => {
            write_file(&report, &format!("{diff}\n"))?;
            anyhow::bail!(
                "TST-020: {} and {} disagree.\n{diff}\n\nThe report is {}, and it is this run's \
                 product: unlike the tier-2 and tier-3 benchmarks, this suite is a gate. \
                 Reproduce it with the flags in {}.",
                java.name,
                rust.name,
                report.display(),
                out.join("seed.txt").display()
            )
        }
    }
}

/// Destroys both nodes and recreates them, then seeds the origin from the corpus.
///
/// # Fresh nodes, not a truncated table
///
/// Reusing one pair across both implementations is the easiest way to fabricate a difference:
/// whichever ran second would write into a target holding the first run's SSTables and its
/// compaction backlog, and read an origin already in the page cache. For a throughput measurement
/// that is a bias; for a *correctness* comparison it is worse, because a target that was not empty
/// is a target whose final state is a function of two runs.
fn prepare_clusters(
    env_dir: &Path,
    environment: &JavaEnvironment,
    corpus: &cdm_testkit::differential::Corpus,
) -> anyhow::Result<()> {
    clusters_down(env_dir);
    run_environment_script(env_dir, "clusters-up.sh", &[], &[])?;

    // The schema goes on **both** sides. Java CDM does not create the target table — it reads the
    // target's schema to build its statements and fails if the table is not there — and cdm-rs
    // behaves the same way (`SCH-001`).
    let schema = corpus.schema_statements();
    println!(
        "differential: applying {} schema statement(s) to both clusters",
        schema.len()
    );
    for (container, host) in [
        (&environment.origin_container, &environment.origin_ip),
        (&environment.target_container, &environment.target_ip),
    ] {
        cqlsh(environment, container, host, &joined(schema))?;
    }

    let inserts = corpus.insert_statements();
    println!(
        "differential: seeding the origin with {} row(s)",
        corpus.row_count()
    );
    for chunk in inserts.chunks(CQLSH_STATEMENTS_PER_INVOCATION) {
        cqlsh(
            environment,
            &environment.origin_container,
            &environment.origin_ip,
            &joined(chunk),
        )?;
    }
    Ok(())
}

/// Joins statements into one `cqlsh` script, terminating any that is missing its semicolon.
fn joined(statements: &[String]) -> String {
    let mut script = String::new();
    for statement in statements {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        script.push_str(trimmed);
        if !trimmed.ends_with(';') {
            script.push(';');
        }
        script.push('\n');
    }
    script
}

/// Removes the environment's nodes and network, reporting nothing.
///
/// Teardown failures are not this suite's business: `clusters-down.sh` already succeeds when there
/// is nothing to remove, and a run that produced a valid diff must not be turned red by the
/// cleanup that follows it.
fn clusters_down(env_dir: &Path) {
    let _ = run_environment_script(env_dir, "clusters-down.sh", &[], &[]);
}

/// The properties cdm-rs is given, mirroring `environment/cdm.properties.template`.
///
/// Written out here rather than shared with the Java template because the two are not the same
/// file for a good reason: the template is expanded by `sed` inside a shell script, and cdm-rs
/// additionally needs `spark.cdm.schema.target.keyspaceTable`, which Java CDM defaults from the
/// origin. Every setting that changes what a job *does* is identical, and identical by
/// construction: both sides read the constants above.
fn rust_properties(environment: &JavaEnvironment, keyspace_table: &str) -> String {
    let JavaEnvironment {
        origin_ip,
        target_ip,
        native_port,
        ..
    } = environment;
    format!(
        "# Generated by `cargo xtask differential` (TST-020). Do not edit: it is regenerated per\n\
         # run and kept as the record of how cdm-rs was configured for the run beside it.\n\
         #\n\
         # Every value here matches the one `environment/cdm.properties.template` gives Java CDM.\n\
         # A differential run whose two sides were configured differently would be comparing\n\
         # configurations, not implementations.\n\
         spark.cdm.connect.origin.host                     {origin_ip}\n\
         spark.cdm.connect.origin.port                     {native_port}\n\
         spark.cdm.connect.target.host                     {target_ip}\n\
         spark.cdm.connect.target.port                     {native_port}\n\
         \n\
         spark.cdm.schema.origin.keyspaceTable             {keyspace_table}\n\
         # Java CDM defaults the target to the origin's value; cdm-rs is told explicitly so that\n\
         # the file states the whole arrangement.\n\
         spark.cdm.schema.target.keyspaceTable             {keyspace_table}\n\
         \n\
         # Off on both sides. Autocorrect belongs to `validate`, and a migrate run that repaired\n\
         # its own output would hide exactly the difference this suite exists to find.\n\
         spark.cdm.autocorrect.missing                     false\n\
         spark.cdm.autocorrect.mismatch                    false\n\
         # Off on both sides: run tracking writes rows to the target, and the target's state is\n\
         # the thing being compared.\n\
         spark.cdm.trackRun                                false\n\
         \n\
         spark.cdm.perfops.numParts                        {DIFFERENTIAL_NUM_PARTS}\n\
         spark.cdm.perfops.batchSize                       {DIFFERENTIAL_BATCH_SIZE}\n\
         spark.cdm.perfops.fetchSizeInRows                 {DIFFERENTIAL_FETCH_SIZE}\n\
         spark.cdm.perfops.ratelimit.origin                {DIFFERENTIAL_RATELIMIT}\n\
         spark.cdm.perfops.ratelimit.target                {DIFFERENTIAL_RATELIMIT}\n\
         spark.cdm.perfops.consistency.read                LOCAL_QUORUM\n\
         spark.cdm.perfops.consistency.write               LOCAL_QUORUM\n\
         # Zero, as Java CDM defaults: a row that fails to migrate aborts the run rather than\n\
         # quietly shortening the target.\n\
         spark.cdm.perfops.errorLimit                      0\n"
    )
}

/// Builds the `cdm` binary and returns its path.
///
/// The suite drives the binary as a black box, exactly as it drives `spark-submit` on the other
/// side. A harness that called `cdm-engine` directly would be comparing Java's whole product
/// against half of ours.
fn build_cdm_binary(root: &Path) -> anyhow::Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(&cargo)
        .current_dir(root)
        .args(["build", "-p", "cdm-cli", "--bin", "cdm"])
        .status()
        .map_err(|e| anyhow::anyhow!("cannot build the `cdm` binary: {e}"))?;
    anyhow::ensure!(
        status.success(),
        "the `cdm` binary did not build, so no differential run could happen: {status}"
    );

    let target =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let binary = target
        .join("debug")
        .join(if cfg!(windows) { "cdm.exe" } else { "cdm" });
    anyhow::ensure!(
        binary.is_file(),
        "the `cdm` binary is not at {} after a successful build",
        binary.display()
    );
    Ok(binary)
}

/// The pieces of `environment/versions.env` this runner needs.
///
/// Parsed, not restated. That file is the single place the container names, the network and the
/// two fixed addresses are declared, and a second copy here would drift the first time somebody
/// repinned an image — leaving this runner talking to a node the Java half never started. The
/// addresses are fixed rather than discovered because Cassandra advertises an address in
/// `system.local.rpc_address` and every driver honours it; see the environment README's
/// "Networking".
#[derive(Debug)]
struct JavaEnvironment {
    /// The origin node's container name.
    origin_container: String,
    /// The target node's container name.
    target_container: String,
    /// The origin node's fixed address on the bench network.
    origin_ip: String,
    /// The target node's fixed address on the bench network.
    target_ip: String,
    /// The CQL port both nodes serve on.
    native_port: String,
}

impl JavaEnvironment {
    /// Reads the five values this runner needs out of `versions.env`.
    fn load(env_dir: &Path) -> anyhow::Result<Self> {
        let path = env_dir.join("versions.env");
        let text = std::fs::read_to_string(&path).map_err(|e| {
            anyhow::anyhow!(
                "cannot read {}: {e}. The Java CDM environment is where this suite runs; \
                 without it there is nothing to be differential against.",
                path.display()
            )
        })?;

        // A deliberately small parser: `KEY=value` and comments. `versions.env` is sourced by
        // shell scripts and does interpolate (`${CDM_VERSION}`), but none of the five keys read
        // here does, and a parser that pretended to be a shell would be a lie the first time one
        // of them started to.
        let mut values = std::collections::BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                values.insert(key.trim().to_owned(), value.trim().to_owned());
            }
        }
        let take = |key: &str| -> anyhow::Result<String> {
            match values.get(key) {
                Some(value) if !value.contains('$') => Ok(value.clone()),
                Some(value) => anyhow::bail!(
                    "{} declares {key}={value}, which interpolates; this parser is not a shell",
                    path.display()
                ),
                None => anyhow::bail!("{} does not declare {key}", path.display()),
            }
        };
        Ok(Self {
            origin_container: take("ORIGIN_CONTAINER")?,
            target_container: take("TARGET_CONTAINER")?,
            origin_ip: take("ORIGIN_IP")?,
            target_ip: take("TARGET_IP")?,
            native_port: take("NATIVE_PORT")?,
        })
    }
}

/// Runs one of the environment's scripts, returning everything it printed.
///
/// Invoked through `bash` rather than executed directly so that a checkout whose execute bits did
/// not survive — a zip, a Windows worktree, some CI caches — still runs.
fn run_environment_script(
    env_dir: &Path,
    script: &str,
    args: &[&str],
    env: &[(&str, &str)],
) -> anyhow::Result<String> {
    let path = env_dir.join(script);
    anyhow::ensure!(
        path.is_file(),
        "{} is missing; the differential suite runs on the pinned Java CDM environment",
        path.display()
    );
    let mut command = Command::new("bash");
    command.arg(&path).args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|e| anyhow::anyhow!("cannot run {}: {e}", path.display()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    anyhow::ensure!(
        output.status.success(),
        "{} exited {:?}:\n{text}",
        path.display(),
        output.status.code()
    );
    Ok(text)
}

/// Runs a CQL script inside a node's own container.
///
/// `cqlsh` ships in the `cassandra:5.0` image, so this needs nothing installed on the host, and it
/// is guaranteed to match the server version. The script arrives on stdin rather than as `-e`
/// because a corpus statement can be long enough to embarrass a command line.
fn cqlsh(
    environment: &JavaEnvironment,
    container: &str,
    host: &str,
    script: &str,
) -> anyhow::Result<String> {
    let mut child = Command::new("docker")
        .args([
            "exec",
            "-i",
            container,
            "cqlsh",
            host,
            &environment.native_port,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot run cqlsh in {container}: {e}"))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| anyhow::anyhow!("cannot feed cqlsh in {container}: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| anyhow::anyhow!("cqlsh in {container} did not finish: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::ensure!(
        output.status.success(),
        "cqlsh in {container} exited {:?}:\n{stderr}",
        output.status.code()
    );
    // cqlsh reports a rejected statement on stderr and still exits 0 when it was fed a script, so
    // the status alone would let a half-applied schema through — and a half-applied schema shows
    // up much later as a difference that looks like a codec bug.
    anyhow::ensure!(
        stderr.trim().is_empty(),
        "cqlsh in {container} reported an error:\n{stderr}"
    );
    Ok(stdout)
}

/// Counts the rows the target actually holds, independently of either job's counters.
fn count_target_rows(environment: &JavaEnvironment, keyspace_table: &str) -> anyhow::Result<u64> {
    let text = cqlsh(
        environment,
        &environment.target_container,
        &environment.target_ip,
        &format!("SELECT COUNT(*) FROM {keyspace_table};\n"),
    )?;
    cdm_testkit::parse_cqlsh(&text)
        .first()
        .and_then(|table| table.rows.first())
        .and_then(|row| row.trim().parse::<u64>().ok())
        .ok_or_else(|| anyhow::anyhow!("cannot read a count out of cqlsh's output:\n{text}"))
}

/// Captures the target's contents as `cqlsh` renders them.
///
/// A full scan of a single node returns rows in token order, which is a function of the partition
/// keys and the partitioner alone — so two nodes holding the same rows render them in the same
/// order, whatever vnodes they happened to be assigned. `PAGING OFF` removes the one piece of
/// interactive furniture that would otherwise appear part way down a long result.
fn capture_target_state(
    environment: &JavaEnvironment,
    keyspace_table: &str,
) -> anyhow::Result<String> {
    cqlsh(
        environment,
        &environment.target_container,
        &environment.target_ip,
        &format!("PAGING OFF;\nSELECT * FROM {keyspace_table};\n"),
    )
}

/// Extracts the `MET-006` final counter block from a job's output.
///
/// Java CDM's lines arrive wrapped in log4j's furniture —
/// `26/08/14 13:15:00 INFO JobCounter: Final Read Record Count: 200000` — while cdm-rs prints the
/// bare line. Taking each line from `Final ` onwards is the whole normalisation, and it is
/// deliberately the *only* one: `COMPAT-004` requires the block itself to be character-identical,
/// so anything more would be this function hiding the difference it was written to expose.
fn normalised_counter_block(text: &str) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(index) = line.find("Final ") else {
            continue;
        };
        let Some(rest) = line.get(index..).map(str::trim_end) else {
            continue;
        };
        if !rest.contains("Record Count:") {
            continue;
        }
        let owned = rest.to_owned();
        // Spark's log is replayed by `submit-migrate.sh`, so the same line can appear twice.
        if !lines.contains(&owned) {
            lines.push(owned);
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Reads one counter out of a normalised block, e.g. `Read`, `Write`, `Error`.
fn counter(block: &str, label: &str) -> Option<u64> {
    let prefix = format!("Final {label} Record Count:");
    block
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix.as_str()))
        .and_then(|value| value.trim().parse().ok())
}

/// A path as a command-line argument, or an error naming it.
fn path_argument(path: &Path) -> anyhow::Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("{} is not valid UTF-8", path.display()))
}

/// Writes a file, naming it on failure.
fn write_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    std::fs::write(path, contents)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", path.display()))
}

/// The container name the SIT suite gives its shared node.
const SIT_CONTAINER: &str = "cdm-sit-node";

/// Stops and removes the SIT node, if one is left over.
///
/// The suite holds its node in a `static` so that nineteen cases share one container, and a
/// `static` is never dropped — so the container outlives the test process, and the next run cannot
/// bind the fixed CQL port. Java's harness has the same shape and the same answer: `environment.sh
/// -m teardown`. This runs before and after, names exactly one container, and never fails the
/// command: a runtime that cannot remove a container that is not there has nothing to report.
fn remove_sit_node() {
    for runtime in ["docker", "podman"] {
        let removed = std::process::Command::new(runtime)
            .args(["rm", "--force", SIT_CONTAINER])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if matches!(removed, Ok(status) if status.success()) {
            return;
        }
    }
}
