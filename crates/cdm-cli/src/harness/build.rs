//! Introspection and job construction — step three and four of the harness.
//!
//! [`ResolvedTables`] is everything the two live schemas imply and every job needs: the column
//! mapping, the origin projection, the statement set and the partitioner. [`job`] is the only
//! place the three jobs differ.

use std::sync::Arc;

use cdm_codec::{CodecRegistry, Codecset, Planner as CodecPlanner, PlannerOptions};
use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind, JobKind, RunId, Side, TableRef};
use cdm_cql::exec::{PreparedSetOptions, RunExecutor, TokenWidth};
use cdm_cql::rows::{CqlRowSink, CqlRowSource};
use cdm_cql::schema::introspect::fetch_table;
use cdm_cql::schema::table::TableSchema;
use cdm_cql::statement::{
    Binder, ColumnMapping, MappingOptions, MissingKeyPolicy, OriginProjection, OriginRangeSelect,
    OriginSelectByPk, StatementOptions, StatementSet, TargetSelectByPk, TargetUpsert,
};
use cdm_engine::jobs::migrate::{MigrateFeatures, MigrateJob, MigratePlan, MigrateSettings};
use cdm_engine::jobs::validate::{
    ComparisonPlan, DiffLog, DiscrepancyReport, ValidateJob, ValidateSettings,
};
use cdm_engine::planner::Partitioner;
use cdm_engine::scheduler::RangeProcessor;

use super::Sessions;
use crate::cli::JobArgs;

/// Both schemas and everything derived from them.
#[derive(Debug)]
pub struct ResolvedTables {
    /// The origin table as the cluster reports it.
    pub origin: TableSchema,
    /// The target table as the cluster reports it.
    pub target: TableSchema,
    /// Which origin column feeds which target column (`SCH-003`).
    pub mapping: ColumnMapping,
    /// The columns the origin scan selects (`SCH-007`).
    pub projection: OriginProjection,
    partitioner: Partitioner,
}

impl ResolvedTables {
    /// Introspects both tables and resolves the mapping (`SCH-001`..`SCH-007`).
    ///
    /// This is also where tier-3 validation happens: the rules that need a live schema — a
    /// writetime column that must resolve, a rename whose target must exist — cannot run earlier,
    /// and running them here means they run before a single row is read rather than on the row
    /// that happens to break them (`CFG-020`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::SchemaMismatch`] if either table is missing, is a materialized view
    /// (`SCH-010`), or cannot be mapped; [`ErrorKind::Config`] for a tier-3 violation.
    pub async fn introspect(
        sessions: &Sessions,
        config: &EffectiveConfig,
    ) -> Result<Self, CdmError> {
        // `CFG-022` makes the origin table mandatory and `CFG-023` defaults the target to it, so
        // an absent origin here means validation was skipped rather than that it passed.
        let origin_ref = config.origin_table().cloned().ok_or_else(|| {
            CdmError::new(
                ErrorKind::Config,
                "no origin table is configured; set `schema.origin.keyspace_table`",
            )
            .with_context(|c| c.with_config_key("schema.origin.keyspace_table"))
        })?;
        let target_ref = config
            .target_table()
            .cloned()
            .unwrap_or_else(|| origin_ref.clone());

        let origin = fetch(sessions, Side::Origin, &origin_ref).await?;
        let target = fetch(sessions, Side::Target, &target_ref).await?;

        // SCH-010: a view cannot be written to, and its rows are a projection of a base table that
        // is being migrated separately. Failing here beats failing on the first write.
        origin.reject_if_materialized_view(Side::Origin)?;
        target.reject_if_materialized_view(Side::Target)?;

        let mapping = ColumnMapping::resolve(&origin, &target, &MappingOptions::default())?;
        let projection = OriginProjection::new(mapping.origin_columns(), &[]);
        let partitioner = Partitioner::detect(&sessions.origin.capabilities().partitioner)?;

        Ok(Self {
            origin,
            target,
            mapping,
            projection,
            partitioner,
        })
    }

    /// The partitioner the origin reports (`TOK-001`).
    #[must_use]
    pub const fn partitioner(&self) -> Partitioner {
        self.partitioner
    }

    /// The four statements every job draws from (`SCH-004`..`SCH-007`).
    fn statements(&self, where_clause: Option<&str>) -> Result<StatementSet, CdmError> {
        Ok(StatementSet {
            origin_range_select: OriginRangeSelect::new(
                &self.origin,
                &self.projection,
                where_clause,
                false,
            )
            .cql()
            .to_owned(),
            origin_select_by_pk: OriginSelectByPk::new(&self.origin, &self.projection)
                .cql()
                .to_owned(),
            target_select_by_pk: TargetSelectByPk::new(&self.mapping)?.cql().to_owned(),
            target_upsert: TargetUpsert::new(&self.mapping, StatementOptions::default())?
                .cql()
                .to_owned(),
        })
    }
}

/// Fetches one side's table, turning "not found" into a diagnostic that names it.
async fn fetch(sessions: &Sessions, side: Side, table: &TableRef) -> Result<TableSchema, CdmError> {
    let session = match side {
        Side::Origin => &sessions.origin,
        Side::Target => &sessions.target,
    };
    fetch_table(side, session.session(), table)
        .await?
        .ok_or_else(|| {
            CdmError::new(
                ErrorKind::SchemaMismatch,
                format!(
                    "the {} table {} does not exist on the cluster this run connected to",
                    side.as_str(),
                    table
                ),
            )
            .with_context(|c| c.with_side(side).with_table(table.clone()))
        })
}

/// What a job needs to build itself — the one step that differs between the three.
///
/// A third-party `JobPlugin` (`PLG-004`) implements this and gets the other four steps unchanged,
/// which is the reason the harness is shaped this way rather than as three commands.
pub trait JobBuilder {
    /// Builds the range processor.
    ///
    /// # Errors
    ///
    /// Whatever the job's own construction can reject — a counter table with no counter column
    /// mapped, a guardrail with no threshold.
    fn build(&self) -> Result<Arc<dyn RangeProcessor>, CdmError>;
}

/// A built job, plus the artefacts the harness has to close and report on afterwards.
///
/// The discrepancy report is the reason this is a struct rather than a bare processor: `VAL-013`'s
/// file has to be *finished* when the run ends — a `json` report is an unterminated array until
/// somebody writes the closing bracket — and `MET-033` wants a pointer to it in the run summary.
/// Both need a handle that outlives the builder, and reaching back into the job to find one would
/// mean downcasting a `dyn RangeProcessor`.
pub struct BuiltJob {
    /// The processor the scheduler runs.
    pub processor: Arc<dyn RangeProcessor>,
    /// The discrepancy report, for a validate run that was asked for one (`VAL-013`).
    pub discrepancies: Option<Arc<DiscrepancyReport>>,
}

// `RangeProcessor` is not `Debug` — a job holds prepared statements and live sessions, and there is
// no useful rendering of those — so the derive cannot apply. Written out rather than dropped
// because `missing_debug_implementations` is a workspace lint and a public type without `Debug`
// poisons every struct that contains one.
impl std::fmt::Debug for BuiltJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltJob")
            .field(
                "discrepancies",
                &self.discrepancies.as_ref().map(|r| r.format()),
            )
            .finish_non_exhaustive()
    }
}

impl BuiltJob {
    /// A job with nothing to close: everything but validate.
    fn bare(processor: Arc<dyn RangeProcessor>) -> Self {
        Self {
            processor,
            discrepancies: None,
        }
    }
}

/// Builds the job for `kind` (`CLI-001`).
///
/// # Errors
///
/// [`ErrorKind::SchemaMismatch`] for a mapping the job cannot execute, and
/// [`ErrorKind::Config`] for a job that is not yet reachable from the CLI.
pub(super) async fn job(
    kind: JobKind,
    sessions: &Sessions,
    tables: &ResolvedTables,
    config: &EffectiveConfig,
    args: &JobArgs,
) -> Result<BuiltJob, CdmError> {
    match kind {
        JobKind::Migrate => migrate(sessions, tables, config, args)
            .await
            .map(BuiltJob::bare),
        JobKind::Validate => validate(sessions, tables, config).await,
        // The guardrail job is implemented and tested, but its production row reader is not: the
        // only `OriginRows` that exists reads a range unpaged, which is fine for a test fixture
        // and wrong for a table this job exists to find wide rows in. Naming that is more useful
        // than wiring a reader that would fall over on the first large partition.
        JobKind::Guardrail => Err(CdmError::new(
            ErrorKind::Config,
            "`cdm guardrail` needs a paged origin reader, which lands with the guardrail row \
             source; the job itself is implemented and covered by `guardrail_it`",
        )),
    }
}

/// Builds the migrate job (`MIG-001`).
async fn migrate(
    sessions: &Sessions,
    tables: &ResolvedTables,
    config: &EffectiveConfig,
    args: &JobArgs,
) -> Result<Arc<dyn RangeProcessor>, CdmError> {
    let counter_target = tables.target.is_counter_table();
    let writetime_filter = config.config().filter.writetime.min.is_some()
        || config.config().filter.writetime.max.is_some();
    let settings =
        MigrateSettings::from_config(config, counter_target, writetime_filter, args.dry_run);

    let statements = tables.statements(config.config().filter.cql_where.as_deref())?;
    let executor = RunExecutor::prepare(
        &sessions.origin,
        &sessions.target,
        &statements,
        PreparedSetOptions {
            fetch_size: settings.fetch_size(),
            counter_target,
            ..PreparedSetOptions::default()
        },
        settings.batch_size(),
        token_width(tables.partitioner()),
    )
    .await?;

    let codecs = codec_planner(config)?;
    let plan = MigratePlan::resolve(
        executor,
        &tables.mapping,
        &tables.projection,
        &codecs,
        settings,
        MissingKeyPolicy::default(),
        config.config().transform.map_remove_null_value,
        MigrateFeatures::default(),
    )?;
    Ok(Arc::new(MigrateJob::new(Arc::new(plan))))
}

/// Builds the validate job (`VAL-001`, `VAL-013`, `VAL-015`).
async fn validate(
    sessions: &Sessions,
    tables: &ResolvedTables,
    config: &EffectiveConfig,
) -> Result<BuiltJob, CdmError> {
    let target_select = TargetSelectByPk::new(&tables.mapping)?;
    let range_select = OriginRangeSelect::new(
        &tables.origin,
        &tables.projection,
        config.config().filter.cql_where.as_deref(),
        false,
    );

    let source = CqlRowSource::prepare(
        Arc::clone(sessions.origin.session()),
        &range_select,
        &tables.mapping,
        &target_select,
        token_kind(tables.partitioner()),
    )
    .await?;

    let codecs = codec_planner(config)?;
    let binder = Binder::new(
        &tables.mapping,
        TargetUpsert::new(&tables.mapping, StatementOptions::default())?,
        &codecs,
        MissingKeyPolicy::default(),
        config.config().transform.map_remove_null_value,
    )?;
    let sink = CqlRowSink::prepare(
        Arc::clone(sessions.target.session()),
        &target_select,
        binder,
        &tables.mapping,
    )
    .await?;

    // VAL-015: `--keys-only` arrived here as `validate.keys_only`, because the flag is a spelling
    // of the property and nothing else. A keys-only plan compares existence, so `MISMATCH` is
    // structurally zero in the run that follows.
    let plan = ComparisonPlan::resolve(&tables.mapping, &codecs, None, false)?
        .with_keys_only(config.config().validate.keys_only);
    let mut settings = ValidateSettings::read_only();
    settings.autocorrect = config.config().autocorrect.clone();
    settings.target_is_counter = tables.target.is_counter_table();

    let diff_log = DiffLog::open(&config.config().logging.diff_file)?;

    // VAL-013. Opened before a row is read, because a report that cannot be created must be
    // discovered now rather than after six hours; `format = none` — the default — creates no file
    // and touches the filesystem not at all.
    let reporting = &config.config().validate.report;
    let report = Arc::new(DiscrepancyReport::open(
        RunId::from_raw(0),
        reporting.format,
        &reporting.path,
        reporting.redact_values,
    )?);

    let job = ValidateJob::new(
        Arc::new(source),
        Arc::new(sink),
        Arc::new(plan),
        settings,
        Arc::new(diff_log),
    )
    .with_report(Arc::clone(&report));

    Ok(BuiltJob {
        processor: Arc::new(job),
        discrepancies: report.is_enabled().then_some(report),
    })
}

/// The conversion planner, with the configured codecs registered (`CDC-001`).
fn codec_planner(config: &EffectiveConfig) -> Result<CodecPlanner, CdmError> {
    // An unrecognised codec name is a configuration error rather than a silently ignored one:
    // a run that quietly skips the conversion an operator asked for writes the wrong bytes
    // (`CDC-002`).
    let enabled = config
        .config()
        .transform
        .codecs
        .iter()
        .map(|name| Codecset::parse(name))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CodecPlanner::new(
        CodecRegistry::with_builtins(&enabled, None)?,
        PlannerOptions::default(),
    ))
}

/// The token width the prepared statements bind (`TOK-001`).
const fn token_width(partitioner: Partitioner) -> TokenWidth {
    match partitioner {
        Partitioner::Murmur3 | Partitioner::ByteOrdered => TokenWidth::Murmur3,
        Partitioner::Random => TokenWidth::Random,
    }
}

/// The same distinction, as the row source spells it.
const fn token_kind(partitioner: Partitioner) -> cdm_cql::rows::TokenKind {
    match partitioner {
        Partitioner::Murmur3 | Partitioner::ByteOrdered => cdm_cql::rows::TokenKind::Murmur3,
        Partitioner::Random => cdm_cql::rows::TokenKind::Random,
    }
}
