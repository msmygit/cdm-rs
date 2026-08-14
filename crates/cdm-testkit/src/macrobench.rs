//! The tier-2 macro-benchmark: rows per second, end to end, against real nodes (`TST-060`,
//! `NFR-004`).
//!
//! `docs/BENCHMARKS.md` describes three tiers that answer three different questions. Tier 1 —
//! the `criterion` benchmarks in `crates/*/benches` — answers "did this commit make the hot path
//! slower?" and needs nothing but a compiler. This module is tier 2: **how many rows per second,
//! end to end?** It starts a real origin and a real target, seeds a deterministic dataset into
//! the origin, runs a full migration through [`MigrateJob`] and the real [`Scheduler`], and
//! reports the throughput.
//!
//! # It drives the product, not a copy of it
//!
//! Every component between the two nodes is the shipping one: `cdm-cql` prepares and issues the
//! statements, `cdm-engine`'s planner splits the ring, its scheduler claims the ranges and its
//! migrate job reads, converts, binds and writes. The only thing this module contributes is the
//! containers, the dataset and the stopwatch. A macro-benchmark built on a hand-rolled copy loop
//! would measure the copy loop, and would keep reporting a number long after the product it was
//! supposed to be measuring had regressed.
//!
//! # It is not a CI gate, and must not become one
//!
//! Container throughput on a shared runner is far noisier than the regressions worth catching —
//! `docs/BENCHMARKS.md` §3 records tier 1 moving by 5–7× between an idle and a loaded machine,
//! and tier 2 adds two JVMs and a container runtime on top of that. The result is recorded and
//! trended. Failing a build on it would produce a gate that fires on noise, which is a gate
//! nobody looks at.
//!
//! # What is deterministic, and what is not
//!
//! The *input* is fixed: the dataset is a pure function of [`MacroBenchSpec::seed`] via
//! [`DataGen`], so two runs of the same spec migrate the same bytes in the same order
//! (`TST-101`). The *output* — throughput — is a wall-clock measurement and varies with the
//! machine, which is the entire reason the previous paragraph exists.
//!
//! # `TST-102`
//!
//! [`run_macro_bench`] needs a container runtime and returns [`ErrorKind::Connect`] when there is
//! none. Callers that must not fail — `cargo xtask bench` and this crate's own suite — use
//! [`run_macro_bench_or_skip`], which detects first and returns `Ok(None)` with an explanation.
//!
//! # Where the dependency edge went
//!
//! This module is the only thing in `cdm-testkit` that needs `cdm-engine`, `cdm-cql` and
//! `cdm-config`, and `ARCHITECTURE.md` §3 does not grant this crate those edges. It is therefore
//! behind the off-by-default `macrobench` feature, so the graph every other crate's tests build
//! against is unchanged. See the note above `[features]` in `Cargo.toml` and `ARCHITECTURE.md`
//! §3.3.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cdm_codec::{CodecRegistry, CqlTypeInfo, Planner as CodecPlanner, PlannerOptions};
use cdm_config::model::CdmConfig;
use cdm_config::types::BatchGrouping;
use cdm_core::observe::{Operation, RequestObserver, RetryCause};
use cdm_core::{CdmError, ErrorKind, RunId, Side, TableRef};
use cdm_cql::connect::{connect, ClusterSession};
use cdm_cql::exec::{PreparedSetOptions, RunExecutor, TokenWidth};
use cdm_cql::schema::introspect::fetch_table;
use cdm_cql::statement::{
    ColumnMapping, MappingOptions, MissingKeyPolicy, OriginProjection, OriginRangeSelect,
    OriginSelectByPk, StatementOptions, StatementSet, TargetSelectByPk, TargetUpsert,
};
use cdm_engine::jobs::migrate::{MigrateFeatures, MigrateJob, MigratePlan, MigrateSettings};
use cdm_engine::planner::{Partitioner, Planner, PlannerSettings};
use cdm_engine::scheduler::{NoopObserver, Scheduler, SchedulerSettings};
use cdm_metrics::{CounterKind, CounterView};

use crate::containers::{Engine, OriginTarget};
use crate::data::DataGen;
use crate::runtime::{ContainerRuntime, NoContainerRuntime};
use crate::schema::{create_keyspace_statement, TableSpec};
use crate::seed::Seed;

/// The keyspace the macro-benchmark works in.
pub const KEYSPACE: &str = "cdm_macrobench";

/// The origin table: the side the benchmark seeds and reads.
pub const ORIGIN_TABLE: &str = "macro_src";

/// The target table: the side the migration writes.
pub const TARGET_TABLE: &str = "macro_dst";

/// The benchmark name in the `bencher` output, and therefore in every trend line drawn from it.
///
/// Character-identical between runs on purpose: it is the join key for anything that compares one
/// run against the next.
pub const BENCHER_NAME: &str = "nfr_004_macro_migrate";

/// The seed [`MacroBenchSpec::default`] uses.
///
/// Fixed rather than drawn from entropy, because a benchmark whose input changes between runs
/// cannot attribute a change in throughput to a change in the code.
pub const DEFAULT_SEED: Seed = Seed::new(60_004);

/// How many rows are written per `INSERT` round trip while seeding, and how many of those are in
/// flight at once.
///
/// Seeding is not measured, but it is on the critical path of every run, and one statement at a
/// time against a containerised node is minutes rather than seconds for a default spec.
const SEED_CONCURRENCY: usize = 64;

/// How often the resident-set sampler reads its own process size.
///
/// Fine enough to catch the plateau a bounded pipeline settles at (`NFR-003`), coarse enough that
/// the sampler itself does not show up in the throughput it is sampling.
const RSS_SAMPLE_INTERVAL: Duration = Duration::from_millis(25);

/// What to measure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroBenchSpec {
    /// Rows to seed into origin before the run.
    pub rows: u64,
    /// Columns per row, excluding the primary key.
    pub columns: usize,
    /// Deterministic data seed, so a rerun measures the same bytes.
    pub seed: Seed,
    /// Container image, e.g. `cassandra:5.0`.
    pub image: String,
}

impl Default for MacroBenchSpec {
    fn default() -> Self {
        Self {
            rows: 100_000,
            columns: 16,
            seed: DEFAULT_SEED,
            image: "cassandra:5.0".to_owned(),
        }
    }
}

impl MacroBenchSpec {
    /// The table both sides use: a `bigint` partition key and [`MacroBenchSpec::columns`] `text`
    /// columns.
    ///
    /// Narrow `text` rather than the full type zoo of [`SchemaGen::all_types`](crate::SchemaGen)
    /// deliberately. Tier 2 measures *throughput*, and a schema of collections and UDTs would
    /// mean the headline figure moved with the per-element conversion cost that tier 1 already
    /// measures directly and far more precisely. Widening the table is `columns`' job; changing
    /// what a column *costs* is tier 1's.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if `columns` is zero — a table with a key and nothing else has
    /// nothing to migrate.
    pub fn table(&self, table: &str) -> Result<TableSpec, CdmError> {
        if self.columns == 0 {
            return Err(CdmError::new(
                ErrorKind::Config,
                "a macro-benchmark table needs at least one non-key column",
            ));
        }
        let mut builder =
            TableSpec::builder(KEYSPACE, table).partition_key("id", CqlTypeInfo::BigInt);
        for index in 0..self.columns {
            builder = builder.column(format!("c{index}"), CqlTypeInfo::Text);
        }
        builder.build()
    }

    /// The engine named by [`MacroBenchSpec::image`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] if the image is not an `image:tag` pair this fixture can start.
    pub fn engine(&self) -> Result<Engine, CdmError> {
        Engine::parse(&self.image)
    }

    /// Every `INSERT` that seeds the origin, in a fixed order.
    ///
    /// The partition key is the row's index rather than a generated value, so a spec asking for
    /// `rows` rows seeds exactly that many — two random keys that collided would silently shorten
    /// the dataset and make the throughput figure a measurement of a different workload. Every
    /// other column comes from [`DataGen`] and is therefore a pure function of the seed
    /// (`TST-101`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Config`] as [`MacroBenchSpec::table`], or whatever [`DataGen`] reports.
    pub fn insert_statements(&self) -> Result<Vec<String>, CdmError> {
        let table = self.table(ORIGIN_TABLE)?;
        let names: Vec<&str> = table
            .columns()
            .iter()
            .map(crate::ColumnSpec::name)
            .collect();
        let header = format!(
            "INSERT INTO {} ({}) VALUES ",
            table.qualified_name(),
            names.join(", ")
        );

        let mut generator = DataGen::new(self.seed);
        let mut statements = Vec::with_capacity(usize::try_from(self.rows).unwrap_or(usize::MAX));
        for index in 0..self.rows {
            let mut literals = Vec::with_capacity(self.columns + 1);
            literals.push(index.to_string());
            for _ in 0..self.columns {
                literals.push(generator.literal(&CqlTypeInfo::Text)?);
            }
            statements.push(format!("{header}({})", literals.join(", ")));
        }
        Ok(statements)
    }
}

/// What one run measured.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroBenchResult {
    /// The row count the spec asked for.
    pub spec_rows: u64,
    /// Rows the migration actually wrote, as the job's own `WRITE` counter reports them
    /// (`MET-004`, committed view).
    pub rows_migrated: u64,
    /// How long the migration took: from the scheduler being handed the token plan to the last
    /// range completing. Excludes container startup and seeding, which are setup.
    pub wall_clock: Duration,
    /// [`MacroBenchResult::rows_migrated`] over [`MacroBenchResult::wall_clock`].
    pub rows_per_second: f64,
    /// Peak resident set size in bytes, if the platform can report it; `None` otherwise.
    ///
    /// **Read the caveats before quoting this number.** It is the largest value seen by sampling
    /// this process's own resident set every 25 ms — `RSS_SAMPLE_INTERVAL`, which is private and so
    /// is named rather than linked — *during the migration only*. Therefore:
    ///
    /// * it is the **harness process**, which is the migration pipeline plus the container
    ///   runtime client and the test binary — not a `cdm` binary, and not a lower bound on one;
    /// * it **excludes the two database containers entirely**. They are separate processes in a
    ///   separate VM on macOS, and they dominate the machine's actual memory use;
    /// * it is **sampled**, so a spike shorter than the interval is invisible. It is a peak of
    ///   observations, not a peak;
    /// * it is `None` on every platform except Linux. `/proc/self/status` is the only way to read
    ///   this without `libc`, and the workspace is `#![forbid(unsafe_code)]`, which rules out the
    ///   `getrusage` that macOS and Windows would need.
    ///
    /// It is reported because a *change* in it between two runs on the same machine is
    /// informative about `NFR-003`. It is not a measurement of cdm-rs's memory footprint.
    pub peak_rss_bytes: Option<u64>,
    /// Time from the start of the migration phase to the first row read (`NFR-002` asserts < 2s).
    ///
    /// **What this includes**: building both sessions, introspecting both schemas, resolving the
    /// column mapping, preparing every statement, resolving the [`MigratePlan`], planning the
    /// token ranges, starting the scheduler, and the first origin range-read returning.
    ///
    /// **What it excludes, and cannot include**: process spawn, dynamic linking, and
    /// configuration loading. The harness runs in-process against nodes that are already up, so
    /// there is no `cdm` process to start. It is therefore a **lower bound** on what `NFR-002`
    /// actually asks about, and a run under 2s here does not by itself discharge that
    /// requirement.
    ///
    /// It is measured from the observer seam (`MET-010`), on the first
    /// [`Operation::RangeRead`] against the origin to come back — which is the first page of
    /// rows, i.e. the first row read.
    pub cold_start: Duration,
}

impl MacroBenchResult {
    /// A one-line summary for logs.
    pub fn summary(&self) -> String {
        let rss = self.peak_rss_bytes.map_or_else(
            || "peak rss unavailable on this platform".to_owned(),
            |bytes| {
                format!(
                    "peak rss {:.1} MiB (sampled, harness only)",
                    bytes_to_mib(bytes)
                )
            },
        );
        format!(
            "{BENCHER_NAME}: {} of {} rows in {:.2}s = {:.0} rows/s, cold start {:.3}s, {rss}",
            self.rows_migrated,
            self.spec_rows,
            self.wall_clock.as_secs_f64(),
            self.rows_per_second,
            self.cold_start.as_secs_f64(),
        )
    }

    /// The `--output-format bencher` line, so tier-2 results feed the same tooling tier 1 does.
    ///
    /// The format carries exactly one number and treats lower as better, so the figure reported
    /// is **nanoseconds per migrated row** — the reciprocal of
    /// [`MacroBenchResult::rows_per_second`], which is what makes a throughput regression show up
    /// as a rising line alongside the micro-benchmarks rather than an inverted one.
    ///
    /// The deviation is zero because a tier-2 run is a single sample. That is honest rather than
    /// tidy: there is no variance to report from one observation, and inventing one would let a
    /// consumer draw error bars that mean nothing.
    pub fn to_bencher_line(&self) -> String {
        let nanos_per_row = if self.rows_migrated == 0 {
            0
        } else {
            // A `u128` divided by a non-zero `u64`, clamped: the result cannot exceed the elapsed
            // nanoseconds, which is a `u64` for any run shorter than 584 years.
            u64::try_from(self.wall_clock.as_nanos() / u128::from(self.rows_migrated))
                .unwrap_or(u64::MAX)
        };
        format!("test {BENCHER_NAME} ... bench: {nanos_per_row} ns/iter (+/- 0)")
    }
}

/// Bytes as mebibytes, for a log line.
fn bytes_to_mib(bytes: u64) -> f64 {
    // Precision loss above 2^53 bytes is 8 petabytes of resident set; the cast is safe in
    // practice and the value is only ever printed.
    #[allow(clippy::cast_precision_loss)]
    {
        bytes as f64 / (1024.0 * 1024.0)
    }
}

/// Runs one macro-benchmark end to end, or explains why it could not (`TST-102`).
///
/// Returns `Ok(None)` — never an error — when no container runtime answers, having printed the
/// reason to stderr. This is the entry point `cargo xtask bench` and any scheduled job should
/// use: a benchmark is not a correctness gate, and a laptop without Docker must not turn one red.
///
/// # Errors
///
/// Whatever [`run_macro_bench`] reports, once a runtime *has* been found. At that point a failure
/// is a real one — the image would not start, the migration lost rows — and is worth surfacing.
pub async fn run_macro_bench_or_skip(
    spec: &MacroBenchSpec,
) -> Result<Option<MacroBenchResult>, CdmError> {
    match ContainerRuntime::detect() {
        Ok(runtime) => {
            eprintln!("macro-benchmark: using container runtime {runtime}");
            run_macro_bench(spec).await.map(Some)
        }
        Err(reason) => {
            eprintln!("{}", skip_message(&reason));
            Ok(None)
        }
    }
}

/// The message [`run_macro_bench_or_skip`] prints when there is nothing to run against.
fn skip_message(reason: &NoContainerRuntime) -> String {
    format!("macro-benchmark ({BENCHER_NAME}) not run.\n{reason}")
}

/// Runs one macro-benchmark end to end. Requires a container runtime.
///
/// The phases, in order: start the two nodes, apply the schema to both, seed the origin, then —
/// and only then — start the clock and migrate. Setup is outside the measurement because the
/// question tier 2 asks is about the migration.
///
/// # Errors
///
/// * [`ErrorKind::Connect`] if no container runtime is available (call
///   [`ContainerRuntime::detect`] first and skip per `TST-102`, or use
///   [`run_macro_bench_or_skip`]), or if a node never becomes queryable;
/// * [`ErrorKind::Config`] if the spec is unusable — a zero column count, an unknown image;
/// * [`ErrorKind::Internal`] if the run did not migrate what it seeded. A throughput figure from
///   a migration that dropped rows describes nothing, so it is refused rather than reported.
// The driver's own `SessionBuilder::build()` future is large, and `connect` awaits it; the lint
// fires on the caller, where nothing can be done about it.
#[allow(clippy::large_futures)]
pub async fn run_macro_bench(spec: &MacroBenchSpec) -> Result<MacroBenchResult, CdmError> {
    let engine = spec.engine()?;
    let origin_table = spec.table(ORIGIN_TABLE)?;
    let target_table = spec.table(TARGET_TABLE)?;

    let clusters = OriginTarget::start(&engine).await?;
    let (origin, target) = sessions(&clusters).await?;

    for (session, table) in [(&origin, &origin_table), (&target, &target_table)] {
        ddl(session, &create_keyspace_statement(KEYSPACE)).await?;
        for statement in table.create_statements() {
            ddl(session, &statement).await?;
        }
    }

    seed_origin(&origin, &spec.insert_statements()?).await?;

    // A second pair of sessions, so the cold-start measurement includes connecting. The seeding
    // pair is deliberately not reused: it has a warm pool, a resolved topology and a populated
    // schema cache, none of which a cold start gets for free.
    drop(origin);
    let (origin, target) = sessions(&clusters).await?;

    let probe = Arc::new(ColdStartProbe::new());
    let observer = Arc::clone(&probe) as Arc<dyn RequestObserver>;

    let plan = resolve_plan(
        &origin,
        &target,
        &origin_table,
        &target_table,
        Arc::clone(&observer),
    )
    .await?;

    let peak_rss = Arc::new(AtomicU64::new(0));
    let sampler = tokio::spawn(sample_resident_set(Arc::clone(&peak_rss)));

    let started = Instant::now();
    let report =
        Scheduler::observing(SchedulerSettings::default().with_workers(8), Some(observer))?
            .run(
                &Planner::new(PlannerSettings::new(Partitioner::Murmur3).with_num_parts(64))
                    .plan(RunId::from_raw(1), None)?,
                Arc::new(MigrateJob::new(Arc::new(plan))),
                Arc::new(NoopObserver),
            )
            .await?;
    let wall_clock = started.elapsed();
    sampler.abort();

    if report.ranges_failed() > 0 {
        return Err(CdmError::new(
            ErrorKind::Internal,
            format!(
                "{} of {} ranges failed, so this run measured a broken migration \
                 rather than a fast one",
                report.ranges_failed(),
                report.outcomes().len()
            ),
        ));
    }

    let rows_migrated = report
        .counters()
        .count_of(CounterKind::Write, CounterView::Committed);

    // The counters are the job's own account of itself, so they cannot corroborate it. Counting
    // the target independently is what distinguishes "migrated 100,000 rows quickly" from
    // "counted to 100,000 quickly".
    let in_target = count_rows(&target, TARGET_TABLE).await?;
    if rows_migrated != spec.rows || in_target != spec.rows {
        return Err(CdmError::new(
            ErrorKind::Internal,
            format!(
                "seeded {} rows, the job counted {rows_migrated} written and the target holds \
                 {in_target}; a throughput figure from an incomplete migration is meaningless",
                spec.rows
            ),
        ));
    }

    Ok(MacroBenchResult {
        spec_rows: spec.rows,
        rows_migrated,
        wall_clock,
        rows_per_second: rows_per_second(rows_migrated, wall_clock),
        peak_rss_bytes: match peak_rss.load(Ordering::Relaxed) {
            0 => None,
            bytes => Some(bytes),
        },
        cold_start: probe.first_range_read().unwrap_or(wall_clock),
    })
}

/// Rows over seconds, guarding the degenerate cases the division has.
fn rows_per_second(rows: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if rows == 0 || seconds <= 0.0 {
        return 0.0;
    }
    // Row counts above 2^53 are not reachable in a benchmark that had to write every one of them.
    #[allow(clippy::cast_precision_loss)]
    {
        rows as f64 / seconds
    }
}

/// A configuration pointing each side at its own node.
fn config_for(clusters: &OriginTarget) -> CdmConfig {
    let mut config = CdmConfig::default();
    let (host, port) = split_contact_point(&clusters.origin().contact_point());
    config.connect.origin.host = host;
    config.connect.origin.port = port;
    let (host, port) = split_contact_point(&clusters.target().contact_point());
    config.connect.target.host = host;
    config.connect.target.port = port;
    config
}

/// `127.0.0.1:9042` into its parts, falling back to the well-known port.
fn split_contact_point(contact_point: &str) -> (String, u16) {
    contact_point.rsplit_once(':').map_or_else(
        || (contact_point.to_owned(), crate::DEFAULT_NATIVE_PORT),
        |(host, port)| {
            (
                host.to_owned(),
                port.parse().unwrap_or(crate::DEFAULT_NATIVE_PORT),
            )
        },
    )
}

/// Connects to both sides.
#[allow(clippy::large_futures)]
async fn sessions(clusters: &OriginTarget) -> Result<(ClusterSession, ClusterSession), CdmError> {
    let config = config_for(clusters);
    let origin = connect(&config, Side::Origin).await?;
    let target = connect(&config, Side::Target).await?;
    Ok((origin, target))
}

/// Applies one DDL statement and waits for agreement.
async fn ddl(session: &ClusterSession, cql: &str) -> Result<(), CdmError> {
    session
        .session()
        .query_unpaged(cql, &[])
        .await
        .map_err(|e| CdmError::new(ErrorKind::SchemaMismatch, format!("`{cql}` failed: {e}")))?;
    session
        .session()
        .await_schema_agreement()
        .await
        .map_err(|e| {
            CdmError::new(
                ErrorKind::SchemaMismatch,
                format!("no schema agreement after `{cql}`: {e}"),
            )
        })?;
    Ok(())
}

/// Writes every seed statement, [`SEED_CONCURRENCY`] at a time.
///
/// Unprepared statements with inlined literals, because that is what [`DataGen`] produces and
/// this crate has no way to bind a typed value (`ARCHITECTURE.md` §3). Seeding is setup and is
/// not part of the measurement, so its own throughput matters only in that it must not dominate
/// the wall-clock cost of running the benchmark at all — hence the concurrency.
async fn seed_origin(origin: &ClusterSession, statements: &[String]) -> Result<(), CdmError> {
    let mut inflight = tokio::task::JoinSet::new();
    let session = Arc::new(origin.session().clone());

    for statement in statements {
        if inflight.len() >= SEED_CONCURRENCY {
            join_one(&mut inflight).await?;
        }
        let session = Arc::clone(&session);
        let statement = statement.clone();
        inflight.spawn(async move {
            session
                .query_unpaged(statement.as_str(), &[])
                .await
                .map(|_| ())
                .map_err(|e| CdmError::new(ErrorKind::Write, format!("seeding failed: {e}")))
        });
    }
    while !inflight.is_empty() {
        join_one(&mut inflight).await?;
    }
    Ok(())
}

/// Awaits one seeding task, flattening the join error into a [`CdmError`].
async fn join_one(
    inflight: &mut tokio::task::JoinSet<Result<(), CdmError>>,
) -> Result<(), CdmError> {
    match inflight.join_next().await {
        None => Ok(()),
        Some(Ok(result)) => result,
        Some(Err(e)) => Err(CdmError::new(
            ErrorKind::Internal,
            format!("a seeding task did not complete: {e}"),
        )),
    }
}

/// `SELECT COUNT(*)`, the independent check on what the migration claims it wrote.
async fn count_rows(session: &ClusterSession, table: &str) -> Result<u64, CdmError> {
    let result = session
        .session()
        .query_unpaged(format!("SELECT COUNT(*) FROM {KEYSPACE}.{table}"), &[])
        .await
        .map_err(|e| CdmError::new(ErrorKind::Read, format!("counting {table} failed: {e}")))?
        .into_rows_result()
        .map_err(|e| {
            CdmError::new(
                ErrorKind::Read,
                format!("counting {table} returned no rows: {e}"),
            )
        })?;
    let (count,) = result.single_row::<(i64,)>().map_err(|e| {
        CdmError::new(
            ErrorKind::Read,
            format!("counting {table} returned no count: {e}"),
        )
    })?;
    u64::try_from(count).map_err(|_| {
        CdmError::new(
            ErrorKind::Read,
            format!("{table} reported a negative row count"),
        )
    })
}

/// Everything `MigratePlan::resolve` needs, derived from the two live tables.
///
/// A transcription of what `crates/cdm-engine/tests/migrate_it.rs` does, for the same reason it
/// does it: this is the sequence a real run performs, and a benchmark that shortcut any of it
/// would be measuring a pipeline nobody ships.
async fn resolve_plan(
    origin: &ClusterSession,
    target: &ClusterSession,
    origin_table: &TableSpec,
    target_table: &TableSpec,
    observer: Arc<dyn RequestObserver>,
) -> Result<MigratePlan, CdmError> {
    let origin_schema = fetch_schema(Side::Origin, origin, origin_table.table()).await?;
    let target_schema = fetch_schema(Side::Target, target, target_table.table()).await?;

    let mapping =
        ColumnMapping::resolve(&origin_schema, &target_schema, &MappingOptions::default())?;
    let projection = OriginProjection::new(mapping.origin_columns(), &[]);
    let statements = StatementSet {
        origin_range_select: OriginRangeSelect::new(&origin_schema, &projection, None, false)
            .cql()
            .to_owned(),
        origin_select_by_pk: OriginSelectByPk::new(&origin_schema, &projection)
            .cql()
            .to_owned(),
        target_select_by_pk: TargetSelectByPk::new(&mapping)?.cql().to_owned(),
        target_upsert: TargetUpsert::new(&mapping, StatementOptions::default())?
            .cql()
            .to_owned(),
    };

    // The settings a throughput run wants: batched writes, a full page of rows per round trip,
    // no dry run. They are the shape of the reference workload, not tuning — anything cleverer
    // belongs in the write-up, where the configuration can be stated alongside the number.
    let settings = MigrateSettings::new(10, 1_000, BatchGrouping::Strict, false, false, false);

    let executor = RunExecutor::prepare(
        origin,
        target,
        &statements,
        PreparedSetOptions {
            fetch_size: settings.fetch_size(),
            counter_target: target_schema.is_counter_table(),
            ..PreparedSetOptions::default()
        },
        settings.batch_size(),
        TokenWidth::Murmur3,
    )
    .await?
    .observing(cdm_cql::observe::RequestMetrics::from_option(Some(
        observer,
    )));

    let codecs = CodecPlanner::new(
        CodecRegistry::with_builtins(&[], None)?,
        PlannerOptions::default(),
    );
    MigratePlan::resolve(
        executor,
        &mapping,
        &projection,
        &codecs,
        settings,
        MissingKeyPolicy::default(),
        false,
        MigrateFeatures::default(),
    )
}

/// Introspects one side's table, turning "no such table" into an error rather than a `None`.
async fn fetch_schema(
    side: Side,
    session: &ClusterSession,
    table: &str,
) -> Result<cdm_cql::schema::TableSchema, CdmError> {
    fetch_table(side, session.session(), &TableRef::new(KEYSPACE, table))
        .await?
        .ok_or_else(|| {
            CdmError::new(
                ErrorKind::SchemaMismatch,
                format!("{side} has no table {KEYSPACE}.{table} to benchmark"),
            )
        })
}

/// Records when the first origin range read came back (`NFR-002`, via the `MET-010` seam).
///
/// Everything else the observer is told is discarded: tier 2 reports one throughput figure, and
/// the per-operation histograms that would justify a fuller implementation are tier 1's subject
/// and `cdm-metrics`' job.
#[derive(Debug)]
struct ColdStartProbe {
    created: Instant,
    /// Nanoseconds from [`ColdStartProbe::created`] to the first range read, or zero if none has
    /// come back yet. Zero doubles as "unset" because a range read that returned in literally no
    /// time is not a case any clock can produce.
    first_range_read_nanos: AtomicU64,
}

impl ColdStartProbe {
    fn new() -> Self {
        Self {
            created: Instant::now(),
            first_range_read_nanos: AtomicU64::new(0),
        }
    }

    /// How long the first origin range read took to come back, if one has.
    fn first_range_read(&self) -> Option<Duration> {
        match self.first_range_read_nanos.load(Ordering::Relaxed) {
            0 => None,
            nanos => Some(Duration::from_nanos(nanos)),
        }
    }
}

impl RequestObserver for ColdStartProbe {
    fn request_started(&self, _side: Side) {}

    fn request_finished(&self, side: Side, operation: Operation, _elapsed: Duration) {
        if side != Side::Origin || operation != Operation::RangeRead {
            return;
        }
        let elapsed = u64::try_from(self.created.elapsed().as_nanos()).unwrap_or(u64::MAX);
        // Only the first writer wins; every later range read is a warm one and says nothing about
        // cold start. `max(1)` keeps zero meaning "unset".
        let _ = self.first_range_read_nanos.compare_exchange(
            0,
            elapsed.max(1),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }

    fn request_retried(&self, _cause: RetryCause) {}

    fn batch_executed(&self, _statements: u64) {}

    fn bytes_transferred(&self, _side: Side, _bytes: u64) {}

    fn ratelimit_waited(&self, _side: Side, _waited: Duration) {}
}

/// Samples this process's resident set until aborted, keeping the largest reading.
///
/// See [`MacroBenchResult::peak_rss_bytes`] for what the resulting number does and does not
/// include. On a platform where [`resident_set_bytes`] has no answer this loop reads `None`
/// forever and the peak stays zero, which the caller reports as `None`.
async fn sample_resident_set(peak: Arc<AtomicU64>) {
    loop {
        if let Some(bytes) = resident_set_bytes() {
            peak.fetch_max(bytes, Ordering::Relaxed);
        }
        tokio::time::sleep(RSS_SAMPLE_INTERVAL).await;
    }
}

/// This process's current resident set in bytes, on the one platform that can be asked without
/// `unsafe`.
///
/// `/proc/self/status`' `VmRSS` line, parsed. The alternative everywhere else is `getrusage`,
/// which needs `libc` and therefore `unsafe`, which the workspace forbids
/// (`#![forbid(unsafe_code)]`). Reporting `None` is the honest answer, and a great deal more
/// useful than a plausible-looking figure nobody can interpret.
#[cfg(target_os = "linux")]
fn resident_set_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    kib.checked_mul(1024)
}

/// No `unsafe`-free way to read this on macOS or Windows; see the Linux implementation.
#[cfg(not(target_os = "linux"))]
fn resident_set_bytes() -> Option<u64> {
    None
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

    #[test]
    fn tst_060_the_default_spec_is_the_one_the_documentation_names() {
        let spec = MacroBenchSpec::default();
        assert_eq!(spec.rows, 100_000);
        assert_eq!(spec.columns, 16);
        assert_eq!(spec.seed, DEFAULT_SEED);
        assert_eq!(spec.image, "cassandra:5.0");
        assert_eq!(spec.engine().unwrap(), Engine::cassandra("5.0"));
    }

    #[test]
    fn tst_060_the_table_has_one_key_and_the_requested_columns() {
        let spec = MacroBenchSpec {
            columns: 3,
            ..MacroBenchSpec::default()
        };
        let table = spec.table(ORIGIN_TABLE).unwrap();
        assert_eq!(table.qualified_name(), format!("{KEYSPACE}.{ORIGIN_TABLE}"));
        assert_eq!(table.columns().len(), 4);
        let ddl = table.create_table_statement();
        assert!(ddl.contains("id bigint"), "{ddl}");
        assert!(ddl.contains("c2 text"), "{ddl}");
        assert!(!ddl.contains("c3 "), "{ddl}");
    }

    #[test]
    fn tst_060_a_table_with_no_payload_column_is_refused() {
        let spec = MacroBenchSpec {
            columns: 0,
            ..MacroBenchSpec::default()
        };
        assert_eq!(
            spec.table(ORIGIN_TABLE).unwrap_err().kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn tst_101_the_same_seed_generates_the_same_dataset() {
        let spec = MacroBenchSpec {
            rows: 50,
            columns: 4,
            ..MacroBenchSpec::default()
        };
        let first = spec.insert_statements().unwrap();
        let second = spec.insert_statements().unwrap();
        assert_eq!(
            first, second,
            "TST-101: a rerun must migrate the same bytes"
        );

        let elsewhere = MacroBenchSpec {
            seed: Seed::new(DEFAULT_SEED.value() + 1),
            ..spec.clone()
        };
        assert_ne!(
            first,
            elsewhere.insert_statements().unwrap(),
            "a different seed must produce different data, or the seed is not doing anything"
        );
    }

    #[test]
    fn tst_060_every_seeded_row_has_a_distinct_key() {
        let spec = MacroBenchSpec {
            rows: 500,
            columns: 2,
            ..MacroBenchSpec::default()
        };
        let statements = spec.insert_statements().unwrap();
        assert_eq!(statements.len(), 500);
        // The key is the first literal after `VALUES (`, and it is the row index. A collision
        // here would silently shorten the dataset and change the workload being measured.
        let keys: std::collections::BTreeSet<&str> = statements
            .iter()
            .filter_map(|s| s.rsplit_once("VALUES (").map(|(_, rest)| rest))
            .filter_map(|rest| rest.split(',').next())
            .collect();
        assert_eq!(keys.len(), 500);
        assert!(statements[0].contains("VALUES (0, "), "{}", statements[0]);
    }

    #[test]
    fn nfr_004_the_bencher_line_reports_nanoseconds_per_row() {
        let result = MacroBenchResult {
            spec_rows: 1_000,
            rows_migrated: 1_000,
            wall_clock: Duration::from_secs(2),
            rows_per_second: 500.0,
            peak_rss_bytes: Some(64 * 1024 * 1024),
            cold_start: Duration::from_millis(900),
        };
        // 2s / 1000 rows = 2 ms per row.
        assert_eq!(
            result.to_bencher_line(),
            "test nfr_004_macro_migrate ... bench: 2000000 ns/iter (+/- 0)"
        );
        let summary = result.summary();
        assert!(summary.contains("500 rows/s"), "{summary}");
        assert!(summary.contains("1000 of 1000 rows"), "{summary}");
        assert!(summary.contains("peak rss 64.0 MiB"), "{summary}");
    }

    #[test]
    fn nfr_004_a_run_that_migrated_nothing_reports_zero_rather_than_dividing_by_it() {
        let result = MacroBenchResult {
            spec_rows: 10,
            rows_migrated: 0,
            wall_clock: Duration::ZERO,
            rows_per_second: rows_per_second(0, Duration::ZERO),
            peak_rss_bytes: None,
            cold_start: Duration::ZERO,
        };
        assert!((result.rows_per_second - 0.0).abs() < f64::EPSILON);
        assert!(result.to_bencher_line().contains("0 ns/iter"));
        assert!(
            result.summary().contains("peak rss unavailable"),
            "{}",
            result.summary()
        );
    }

    #[test]
    fn nfr_004_throughput_is_rows_over_seconds() {
        let rate = rows_per_second(100_000, Duration::from_secs(4));
        assert!((rate - 25_000.0).abs() < 0.001, "{rate}");
    }

    #[test]
    fn nfr_002_the_probe_records_only_the_first_origin_range_read() {
        let probe = ColdStartProbe::new();
        assert_eq!(probe.first_range_read(), None);

        // Neither of these is the first row read.
        probe.request_finished(Side::Target, Operation::Write, Duration::from_secs(1));
        probe.request_finished(Side::Origin, Operation::KeyRead, Duration::from_secs(1));
        assert_eq!(probe.first_range_read(), None);

        probe.request_finished(Side::Origin, Operation::RangeRead, Duration::from_secs(1));
        let first = probe
            .first_range_read()
            .expect("the range read was recorded");
        probe.request_finished(Side::Origin, Operation::RangeRead, Duration::from_secs(1));
        assert_eq!(
            probe.first_range_read(),
            Some(first),
            "a later, warm range read must not overwrite the cold-start measurement"
        );
    }

    #[test]
    fn tst_060_the_contact_point_splits_into_a_host_and_a_port() {
        assert_eq!(
            split_contact_point("127.0.0.1:19042"),
            ("127.0.0.1".to_owned(), 19042)
        );
        // A malformed endpoint falls back rather than failing: the fixture always produces a
        // well-formed one, and a benchmark is not the place to invent a new error path.
        assert_eq!(
            split_contact_point("127.0.0.1"),
            ("127.0.0.1".to_owned(), crate::DEFAULT_NATIVE_PORT)
        );
    }

    #[test]
    fn tst_060_peak_rss_is_reported_only_where_it_can_be_measured() {
        // The point of the assertion is that the two arms of the `cfg` agree with what
        // `MacroBenchResult::peak_rss_bytes` promises, so nobody reads a `None` on macOS as a
        // measurement of zero.
        let sample = resident_set_bytes();
        if cfg!(target_os = "linux") {
            assert!(sample.is_some_and(|bytes| bytes > 0), "{sample:?}");
        } else {
            assert_eq!(sample, None);
        }
    }

    #[test]
    fn tst_102_the_skip_message_names_the_benchmark_and_the_requirement() {
        // Reconstructed through `detect` so the message is the real one: on a machine with a
        // runtime this asserts nothing about the skip, which is why the branch is explicit.
        match ContainerRuntime::detect() {
            Ok(runtime) => assert!(!runtime.endpoint().is_empty()),
            Err(reason) => {
                let message = skip_message(&reason);
                assert!(message.contains(BENCHER_NAME), "{message}");
                assert!(message.contains("TST-102"), "{message}");
            }
        }
    }
}
