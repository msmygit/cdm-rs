//! Introspection and job construction — step three and four of the harness.
//!
//! [`ResolvedTables`] is everything the two live schemas imply and every job needs: the column
//! mapping, the origin projection, the statement set and the partitioner. [`job`] is the only
//! place migrate and validate differ, and [`guardrail`] is the third job, built from
//! [`ResolvedOrigin`] because `GRD-001` forbids it a target.
//!
//! # This is where the configuration becomes a plan
//!
//! Everything an operator wrote under `feature.*`, `schema.origin.column.*` and `transform.*` is
//! parsed and validated long before it reaches this file, and every one of those settings is
//! *implemented* somewhere else — in `cdm-feature`, `cdm-codec` or `cdm-cql`. What happens here is
//! the join: the validated configuration is resolved, against the two live schemas, into the plans
//! the jobs hold. There is no second place that reads a `feature.*` property.
//!
//! That makes this file the one place where a setting can be lost without anything failing. It is
//! not a hypothetical: every job here was once built with `MappingOptions::default()`,
//! `MigrateFeatures::default()` and `MissingKeyPolicy::default()`, which meant a run configured
//! with TTL and writetime preservation started, wrote every row with the *write's* timestamp, and
//! exited 0 reporting success. A missing feature announces itself; a discarded one does not.

use std::sync::Arc;

use cdm_codec::{
    CodecRegistry, Codecset, Planner as CodecPlanner, PlannerOptions, TimestampFormat,
};
use cdm_config::EffectiveConfig;
use cdm_core::{CdmError, ErrorKind, JobKind, Row, RunId, Side, TableRef};
use cdm_cql::connect::ClusterSession;
use cdm_cql::exec::{OriginReadOptions, OriginReader, PreparedSetOptions, RunExecutor, TokenWidth};
use cdm_cql::observe::RequestMetrics;
use cdm_cql::rows::{CqlRowSink, CqlRowSource, RowTimestamps};
use cdm_cql::schema::introspect::fetch_table;
use cdm_cql::schema::table::{ColumnMeta, TableSchema};
use cdm_cql::statement::{
    Binder, ColumnMapping, MappingOptions, MissingKeyPolicy, OriginProjection, OriginRangeSelect,
    OriginSelectByPk, StatementOptions, StatementSet, TargetSelectByPk, TargetUpsert,
};
use cdm_engine::jobs::guardrail::{CqlOriginRows, GuardrailJob};
use cdm_engine::jobs::migrate::{MigrateFeatures, MigrateJob, MigratePlan, MigrateSettings};
use cdm_engine::jobs::validate::{
    ComparisonPlan, DiffLog, DiscrepancyReport, ValidateExplode, ValidateJob, ValidateSettings,
};
use cdm_engine::planner::Partitioner;
use cdm_engine::scheduler::RangeProcessor;
use cdm_feature::{
    table_view, ColumnValueFilter, ConstantColumns, ExplodeMap, ExtractJson, FeatureSchema,
    FilterChain, Guardrail, TableFacts, WritetimeFilter, WritetimeTtl, WritetimeTtlPlan,
};

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
    /// The columns the origin scan selects, before any feature appends a virtual one (`SCH-007`).
    pub projection: OriginProjection,
    /// The two tables as `cdm-feature` sees them, for the plans the jobs resolve from them.
    ///
    /// The origin side is built from the *mapped* origin columns rather than from the whole table,
    /// because every plan `cdm-feature` resolves addresses cells by their position in the origin
    /// projection: `WritetimeTtlPlan::resolve` places its first `TTL(…)` at
    /// `origin.columns().len()`, and `ExplodePlan` and `ColumnValueFilter` index into the row.
    /// Building it from the full table would put every one of those indices out by the number of
    /// columns `schema.origin.column.skip` removed, and nothing downstream could notice.
    features: FeatureSchema,
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

        let origin = fetch(&sessions.origin, Side::Origin, &origin_ref).await?;
        let target = fetch(sessions.target()?, Side::Target, &target_ref).await?;

        // SCH-010: a view cannot be written to, and its rows are a projection of a base table that
        // is being migrated separately. Failing here beats failing on the first write.
        origin.reject_if_materialized_view(Side::Origin)?;
        target.reject_if_materialized_view(Side::Target)?;

        // FEA-010, FEA-011: the constants are resolved against the live target before the mapping
        // is, because resolution is what type-checks each literal against the column it will be
        // written into. The mapping only needs the resulting `(column, literal)` pairs.
        let target_facts = facts(&target, &target.columns)?;
        let mapping =
            ColumnMapping::resolve(&origin, &target, &mapping_options(config, &target_facts)?)?;
        let projection = OriginProjection::new(mapping.origin_columns(), &[]);
        let features = FeatureSchema::new(facts(&origin, mapping.origin_columns())?, target_facts);
        let partitioner = Partitioner::detect(&sessions.origin.capabilities().partitioner)?;

        Ok(Self {
            origin,
            target,
            mapping,
            projection,
            features,
            partitioner,
        })
    }

    /// The partitioner the origin reports (`TOK-001`).
    #[must_use]
    pub const fn partitioner(&self) -> Partitioner {
        self.partitioner
    }

    /// The four statements a job draws from, over the projection that job reads (`SCH-004`..`SCH-007`).
    ///
    /// The projection is a parameter rather than [`ResolvedTables::projection`] because migrate and
    /// validate each append their own `TTL(…)` and `WRITETIME(…)` expressions to it
    /// (`FEA-040`, `VAL-018`), and a run whose statements and whose plan disagreed about the width
    /// of a row would mis-read every cell after the first virtual one.
    fn statements(
        &self,
        projection: &OriginProjection,
        using: StatementOptions,
        where_clause: Option<&str>,
    ) -> Result<StatementSet, CdmError> {
        Ok(StatementSet {
            origin_range_select: OriginRangeSelect::new(
                &self.origin,
                projection,
                where_clause,
                false,
            )
            .cql()
            .to_owned(),
            origin_select_by_pk: OriginSelectByPk::new(&self.origin, projection)
                .cql()
                .to_owned(),
            target_select_by_pk: TargetSelectByPk::new(&self.mapping)?.cql().to_owned(),
            target_upsert: TargetUpsert::new(&self.mapping, using)?.cql().to_owned(),
        })
    }
}

/// The origin table alone, for the one job that must not reach a target (`GRD-001`).
///
/// A deliberate second type rather than an `Option`-ridden [`ResolvedTables`]: a guardrail run has
/// no target table, no column mapping and no write statement, and modelling those as absent would
/// leave every migrate and validate call site unwrapping something that is always present.
#[derive(Debug)]
pub struct ResolvedOrigin {
    /// The origin table as the cluster reports it.
    pub origin: TableSchema,
    /// The columns the range scan selects (`SCH-007`).
    pub projection: OriginProjection,
    facts: TableFacts,
    partitioner: Partitioner,
}

impl ResolvedOrigin {
    /// Introspects the origin table and nothing else.
    ///
    /// # Errors
    ///
    /// As [`ResolvedTables::introspect`], for the origin alone.
    pub async fn introspect(
        session: &ClusterSession,
        config: &EffectiveConfig,
    ) -> Result<Self, CdmError> {
        let origin_ref = config.origin_table().cloned().ok_or_else(|| {
            CdmError::new(
                ErrorKind::Config,
                "no origin table is configured; set `schema.origin.keyspace_table`",
            )
            .with_context(|c| c.with_config_key("schema.origin.keyspace_table"))
        })?;
        let origin = fetch(session, Side::Origin, &origin_ref).await?;

        // A guardrail measures the table as it is, so the projection is every column: `skip` is a
        // statement about what a *migration* carries across, and a column left behind is still a
        // column whose size an operator asked about.
        let projection = OriginProjection::new(&origin.columns, &[]);
        let facts = facts(&origin, &origin.columns)?;
        let partitioner = Partitioner::detect(&session.capabilities().partitioner)?;
        Ok(Self {
            origin,
            projection,
            facts,
            partitioner,
        })
    }

    /// The partitioner the origin reports (`TOK-001`).
    #[must_use]
    pub const fn partitioner(&self) -> Partitioner {
        self.partitioner
    }
}

/// Fetches one side's table, turning "not found" into a diagnostic that names it.
async fn fetch(
    session: &ClusterSession,
    side: Side,
    table: &TableRef,
) -> Result<TableSchema, CdmError> {
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

/// A table as `cdm-feature` sees it: `columns` in row order, keyed by the table's primary key.
fn facts(table: &TableSchema, columns: &[ColumnMeta]) -> Result<TableFacts, CdmError> {
    let pairs: Vec<(&str, &str)> = columns
        .iter()
        .map(|column| (column.name.as_str(), column.cql_type.as_str()))
        .collect();
    let key: Vec<&str> = table
        .primary_key()
        .iter()
        .map(|column| column.name.as_str())
        .collect();
    TableFacts::from_view(&table_view(table.table_ref(), &pairs), &key)
}

/// The mapping inputs `feature.*` and `schema.origin.column.*` supply (`SCH-003`, `SCH-004`,
/// `FEA-010`, `FEA-020`, `FEA-030`).
fn mapping_options(
    config: &EffectiveConfig,
    target: &TableFacts,
) -> Result<MappingOptions, CdmError> {
    let core = config.to_core();
    let constants = ConstantColumns::load(&core)?.resolve(target)?;
    let explode = ExplodeMap::load(&core);
    let extract = ExtractJson::load(&core);
    Ok(MappingOptions {
        rename: config.config().schema.origin.column.rename.clone(),
        skip: config.config().schema.origin.column.skip.clone(),
        constants: constants
            .iter()
            .map(|constant| (constant.name().to_owned(), constant.literal().to_owned()))
            .collect(),
        explode_map: explode.is_enabled().then(|| {
            (
                explode.origin_column().to_owned(),
                explode.key_column().to_owned(),
                explode.value_column().to_owned(),
            )
        }),
        extract_json: extract.is_enabled().then(|| {
            (
                extract.origin_column().to_owned(),
                extract.target_column().to_owned(),
            )
        }),
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

// `RangeProcessor` is not `Debug` — a job holds prepared statements and live sessions, and there
// is no useful rendering of those — so the derive cannot apply. Written out rather than dropped
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
/// [`ErrorKind::Config`] for a feature this schema cannot satisfy.
///
/// `requests` is where every driver request the job issues is timed (`MET-010`); `None` records
/// nothing and reads no clock.
pub(super) async fn job(
    kind: JobKind,
    sessions: &Sessions,
    tables: &ResolvedTables,
    config: &EffectiveConfig,
    args: &JobArgs,
    events: Option<Arc<cdm_metrics::EventBus>>,
    requests: Option<Arc<cdm_metrics::Instruments>>,
) -> Result<BuiltJob, CdmError> {
    let requests = request_metrics(requests);
    match kind {
        JobKind::Migrate => migrate(sessions, tables, config, args, requests)
            .await
            .map(BuiltJob::bare),
        JobKind::Validate => validate(sessions, tables, config, events, requests).await,
        // The guardrail is built from `ResolvedOrigin` and the origin session alone, so it cannot
        // be reached through a value that holds a target (`GRD-001`). `super::execute` routes it.
        JobKind::Guardrail => Err(CdmError::new(
            ErrorKind::Internal,
            "the guardrail job is built by `build::guardrail` from an origin-only session, and \
             must not be routed through the two-sided builder",
        )),
    }
}

/// Builds the migrate job (`MIG-001`).
async fn migrate(
    sessions: &Sessions,
    tables: &ResolvedTables,
    config: &EffectiveConfig,
    args: &JobArgs,
    requests: RequestMetrics,
) -> Result<Arc<dyn RangeProcessor>, CdmError> {
    let counter_target = tables.target.is_counter_table();
    let writetime_filter = config.config().filter.writetime.min.is_some()
        || config.config().filter.writetime.max.is_some();
    let settings =
        MigrateSettings::from_config(config, counter_target, writetime_filter, args.dry_run);

    let codecs = codec_planner(config)?;
    let features = migrate_features(config, tables, &codecs, counter_target, writetime_filter)?;

    // SCH-007: `TTL(…)` and `WRITETIME(…)` occupy positions in the result row exactly as columns
    // do, so they are part of the projection the statements are generated from and part of the
    // width the plan is resolved against. The two must be built from the same value.
    let projection = OriginProjection::new(
        tables.mapping.origin_columns(),
        features.writetime.projection(),
    );
    let using = StatementOptions {
        using: using_clause(&features.writetime),
    };
    let statements = tables.statements(
        &projection,
        using,
        config.config().filter.cql_where.as_deref(),
    )?;
    let executor = RunExecutor::prepare(
        &sessions.origin,
        sessions.target()?,
        &statements,
        PreparedSetOptions {
            fetch_size: settings.fetch_size(),
            counter_target,
            ..PreparedSetOptions::default()
        },
        settings.batch_size(),
        token_width(tables.partitioner()),
    )
    .await?
    // MET-010: every origin page, target write and unlogged batch this run issues is timed here,
    // because here is the only place a request exists.
    .observing(requests);

    let plan = MigratePlan::resolve(
        executor,
        &tables.mapping,
        &projection,
        &codecs,
        settings,
        missing_key_policy(config),
        config.config().transform.map_remove_null_value,
        features,
    )?;
    Ok(Arc::new(MigrateJob::new(Arc::new(plan))))
}

/// Builds the guardrail job (`GRD-001`..`GRD-003`).
///
/// Takes the origin [`ClusterSession`] rather than [`Sessions`], which is the whole of `GRD-001`'s
/// structural claim as it applies to this crate: from here down there is no value in scope through
/// which a target could be reached, and the reader this hands to [`GuardrailJob`] holds an origin
/// session and a range select and nothing else.
///
/// # Errors
///
/// [`ErrorKind::Config`] when `feature.guardrail.column_size_kb` is unset or zero — a clean report
/// from a run that was never looking is indistinguishable from a clean report from one that was —
/// and [`ErrorKind::SchemaMismatch`] if the range select does not prepare.
pub(super) async fn guardrail(
    session: &ClusterSession,
    origin: &ResolvedOrigin,
    config: &EffectiveConfig,
    requests: Option<Arc<cdm_metrics::Instruments>>,
) -> Result<BuiltJob, CdmError> {
    let select = OriginRangeSelect::new(
        &origin.origin,
        &origin.projection,
        config.config().filter.cql_where.as_deref(),
        false,
    );
    let reader = OriginReader::prepare(
        session,
        &select,
        OriginReadOptions::default(),
        token_width(origin.partitioner()),
    )
    .await?
    // MET-010: a guardrail run issues origin range reads and nothing else, so its target
    // histograms stay empty and are not exported — which is what `GRD-001` implies.
    .observing(request_metrics(requests));
    let rows = CqlOriginRows::resolve(Arc::new(reader), &origin.origin, &origin.projection)?;
    let guardrail = Guardrail::load(&config.to_core())?.resolve(&origin.facts)?;
    Ok(BuiltJob::bare(Arc::new(GuardrailJob::new(
        Arc::new(rows),
        guardrail,
    )?)))
}

/// The features a migrate run switches on (`FEA-020`, `FEA-030`, `FEA-040`, `FEA-050`).
fn migrate_features(
    config: &EffectiveConfig,
    tables: &ResolvedTables,
    codecs: &CodecPlanner,
    counter_target: bool,
    writetime_filter: bool,
) -> Result<MigrateFeatures, CdmError> {
    let core = config.to_core();
    let origin = &tables.features.origin;

    let writetime = writetime_ttl(config, origin, counter_target)?;

    let column_filter = ColumnValueFilter::load(&core, origin);
    let row_writetime = WritetimeFilter::load(&core, writetime.clone())?;
    let filters = FilterChain::new()
        .with_enabled(row_writetime.is_enabled(), Arc::new(row_writetime))
        .with_enabled(column_filter.is_enabled(), Arc::new(column_filter));

    let explode = ExplodeMap::load(&core);
    let explode = explode
        .is_enabled()
        .then(|| explode.resolve(&tables.features, codecs))
        .transpose()?;
    let extract_json = ExtractJson::load(&core);
    let extract_json = extract_json
        .is_enabled()
        .then(|| extract_json.resolve(&tables.features))
        .transpose()?;

    Ok(MigrateFeatures {
        filters,
        writetime,
        explode,
        extract_json,
        writetime_filter,
    })
}

/// The TTL and writetime plan a run resolves, whichever job is running (`FEA-040`..`FEA-046`).
///
/// Shared by [`migrate_features`] and [`validate`] rather than written out twice, because
/// `VAL-018` is precisely the requirement that the two jobs resolve *the same* plan: a validate
/// run's autocorrect writes through the same `USING` clause a migrate run's insert does, over the
/// same `TTL(…)`/`WRITETIME(…)` projection. Two copies of this could disagree, and the disagreement
/// would be invisible — a corrected row is written either way.
///
/// # Errors
///
/// [`ErrorKind::Config`] when `feature.writetime_ttl.*` names a column this origin does not have,
/// or names one whose type cannot carry a TTL.
fn writetime_ttl(
    config: &EffectiveConfig,
    origin: &TableFacts,
    counter_target: bool,
) -> Result<WritetimeTtlPlan, CdmError> {
    // FEA-045: a counter column on *either* side disables TTL and writetime, because neither side
    // can accept a timestamp or a TTL on a counter write. `WritetimeTtlPlan::resolve` only knows
    // about the origin, so the target's half of the rule is applied here.
    if counter_target || origin.is_counter_table() {
        return Ok(WritetimeTtlPlan::disabled());
    }
    WritetimeTtl::load(&config.to_core())?.resolve(origin)
}

/// A resolved [`WritetimeTtlPlan`] as the row sink's per-row stamp (`VAL-018`).
///
/// The plan lives in `cdm-feature`, which depends on `cdm-cql` and not the other way round
/// (`ARCHITECTURE.md` §3), so the sink is written against `cdm-cql`'s [`RowTimestamps`] and the two
/// are joined here — the same seam, and for the same reason, as [`using_clause`] below. There is no
/// computation in this type: both methods delegate, so a corrected row and a migrated row are
/// stamped by the identical code.
#[derive(Debug)]
struct PlanTimestamps(WritetimeTtlPlan);

impl RowTimestamps for PlanTimestamps {
    fn ttl(&self, row: &Row) -> Result<Option<i32>, CdmError> {
        self.0.ttl(row)
    }

    fn writetime(&self, row: &Row) -> Result<Option<i64>, CdmError> {
        self.0.writetime(row)
    }
}

/// The `USING` clause a resolved TTL/writetime plan implies (`FEA-046`).
///
/// `cdm-feature` and `cdm-cql` each own a `UsingClause` on opposite sides of the dependency edge,
/// so the two booleans cross as data — the same seam `cdm-engine`'s migrate plan uses.
fn using_clause(plan: &WritetimeTtlPlan) -> cdm_cql::statement::UsingClause {
    let feature = plan.using_clause();
    cdm_cql::statement::UsingClause {
        ttl: feature.ttl,
        timestamp: feature.timestamp,
    }
}

/// The request-timing seam `cdm-cql` records through, from the run's instruments (`MET-010`).
///
/// `cdm-metrics` is on the far side of the dependency edge from `cdm-cql` (`ARCHITECTURE.md` §3),
/// so the two meet through `cdm_core::RequestObserver`, which `Instruments` implements. This is
/// the one line that joins them, and it is in the crate that builds both.
fn request_metrics(instruments: Option<Arc<cdm_metrics::Instruments>>) -> RequestMetrics {
    RequestMetrics::from_option(instruments.map(|i| i as Arc<dyn cdm_core::RequestObserver>))
}

/// What to substitute for a null in a target key column (`MIG-013`).
fn missing_key_policy(config: &EffectiveConfig) -> MissingKeyPolicy {
    MissingKeyPolicy {
        missing_key_ts_replace: config.config().transform.missing_key_ts_replace,
    }
}

/// Builds the validate job (`VAL-001`, `VAL-013`, `VAL-015`).
async fn validate(
    sessions: &Sessions,
    tables: &ResolvedTables,
    config: &EffectiveConfig,
    events: Option<Arc<cdm_metrics::EventBus>>,
    requests: RequestMetrics,
) -> Result<BuiltJob, CdmError> {
    let counter_target = tables.target.is_counter_table();

    // VAL-018: a corrected row carries the origin's TTL and writetime, resolved exactly as a
    // migrate write resolves them. That is three things and they only work together — the
    // projection must select the `TTL(…)`/`WRITETIME(…)` cells, the upsert must carry the `USING`
    // clause they imply, and the sink must bind them — so all three are driven off this one plan.
    // `FEA-045` disables it for a counter table on either side, which is `writetime_ttl`'s job.
    let writetime = writetime_ttl(config, &tables.features.origin, counter_target)?;

    let target_select = TargetSelectByPk::new(&tables.mapping)?;
    // SCH-007: the projected `TTL(…)`/`WRITETIME(…)` expressions occupy row positions after every
    // mapped column, so the comparison, the key plan and the column filter — all of which index by
    // mapped-column position — are unaffected by their presence.
    let projection = OriginProjection::new(tables.mapping.origin_columns(), writetime.projection());
    let range_select = OriginRangeSelect::new(
        &tables.origin,
        &projection,
        config.config().filter.cql_where.as_deref(),
        false,
    );

    let source = CqlRowSource::prepare(
        Arc::clone(sessions.origin.session()),
        &range_select,
        &tables.mapping,
        &target_select,
        token_kind(tables.partitioner()),
        // MIG-013: the same policy the binder below writes with, so the key this source derives is
        // the key the migration wrote the row under.
        missing_key_policy(config),
    )
    .await?
    // MET-010: validate's origin scan, per page.
    .observing(requests.clone());

    let codecs = codec_planner(config)?;
    let using = StatementOptions {
        using: using_clause(&writetime),
    };
    let binder = Binder::new(
        &tables.mapping,
        TargetUpsert::new(&tables.mapping, using)?,
        &codecs,
        missing_key_policy(config),
        config.config().transform.map_remove_null_value,
    )?;
    let sink = CqlRowSink::prepare(
        Arc::clone(sessions.target()?.session()),
        &target_select,
        binder,
        &tables.mapping,
        // The same plan the `USING` clause above was generated from and the projection above
        // selects for: a clause with no values bound into it writes `UNSET` (`VAL-018`).
        Some(Arc::new(PlanTimestamps(writetime))),
    )
    .await?
    // MET-010: validate's target lookup (`VAL-001`) and its autocorrect write (`VAL-003`).
    .observing(requests);

    // FEA-031, FEA-032: the extracted property is a target column like any other, and whether it
    // overwrites decides what a *comparison* of that column even means. A validate run that did
    // not know about the extraction would report every extracted column as a mismatch.
    let core = config.to_core();
    let extract_json = ExtractJson::load(&core);
    let extract_json_overwrites = extract_json.overwrites();
    let extract_json = extract_json
        .is_enabled()
        .then(|| extract_json.resolve(&tables.features))
        .transpose()?;

    // VAL-015: `--keys-only` arrived here as `validate.keys_only`, because the flag is a spelling
    // of the property and nothing else. A keys-only plan compares existence, so `MISMATCH` is
    // structurally zero in the run that follows.
    let plan = ComparisonPlan::resolve(
        &tables.mapping,
        &codecs,
        extract_json,
        extract_json_overwrites,
    )?
    .with_keys_only(config.config().validate.keys_only);
    let mut settings = ValidateSettings::read_only();
    settings.autocorrect = config.config().autocorrect.clone();
    settings.target_is_counter = counter_target;

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

    // FEA-052: the column-value filter reads a cell of the origin row by position, which validate's
    // projection supplies. `filter.writetime.*` is deliberately not installed here, and no longer
    // because the cell is missing — `VAL-018` now selects `WRITETIME(…)` on this side too. It is
    // not installed because skipping a row is not the same act on the two sides: a migrate run that
    // skips one leaves the target alone, while a validate run that skipped one would report a
    // target it never looked at as `VALID`. Making that a filter on validate is a change to what
    // `VAL-016`'s verdict means and needs a requirement of its own.
    let column_filter = ColumnValueFilter::load(&core, &tables.features.origin);
    let filters =
        FilterChain::new().with_enabled(column_filter.is_enabled(), Arc::new(column_filter));

    // FEA-020, FEA-022: the migration wrote one target row per map entry, so validate must ask
    // about one target row per map entry. The plan is resolved exactly as `migrate_features`
    // resolves it, and the key plan is the origin source's — the same one the records it emits were
    // keyed with, minus the exploded components it cannot fill.
    let explode = ExplodeMap::load(&core);
    let explode = explode
        .is_enabled()
        .then(|| explode.resolve(&tables.features, &codecs))
        .transpose()?
        .map(|plan| ValidateExplode::new(plan, source.key_plan().clone()));

    let mut job = ValidateJob::new(
        Arc::new(source),
        Arc::new(sink),
        Arc::new(plan),
        settings,
        Arc::new(diff_log),
    )
    .with_filters(filters)
    .with_report(Arc::clone(&report));
    if let Some(explode) = explode {
        job = job.with_explode(explode);
    }

    // MET-030: a `Discrepancy` event per finding, but only when something is listening. The bus is
    // handed over exactly when a live display (`MET-031`) has subscribed; on a silent run the
    // events would be constructed — key fingerprint and all, per differing row — and then dropped
    // for want of a subscriber, which is real work on the one job that can produce a finding per
    // row. `SEC-002`'s redaction is applied by the bus at construction either way.
    if let Some(events) = events {
        job = job.with_events(events);
    }

    Ok(BuiltJob {
        processor: Arc::new(job),
        discrepancies: report.is_enabled().then_some(report),
    })
}

/// The conversion planner, with the configured codecs registered (`CDC-001`, `CDC-021`).
fn codec_planner(config: &EffectiveConfig) -> Result<CodecPlanner, CdmError> {
    // An unrecognised codec name is a configuration error rather than a silently ignored one:
    // a run that quietly skips the conversion an operator asked for writes the wrong bytes
    // (`CDC-002`).
    let transform = &config.config().transform;
    let enabled = transform
        .codecs
        .iter()
        .map(|name| Codecset::parse(name))
        .collect::<Result<Vec<_>, _>>()?;

    // CDC-021: `TIMESTAMP_STRING_FORMAT` is the one codec that cannot be built from its name alone,
    // and the registry refuses to build it without a pattern and a zone rather than inventing one.
    // Both properties have defaults, so the only way this used to fail was by not being read.
    // Built only when the codec is enabled, so that a stale pattern left in a configuration cannot
    // stop a run that does not use it.
    let timestamp_format = enabled
        .contains(&Codecset::TimestampStringFormat)
        .then(|| {
            TimestampFormat::new(
                &transform.codec_timestamp_format,
                &transform.codec_timestamp_zone,
            )
        })
        .transpose()?;

    Ok(CodecPlanner::new(
        CodecRegistry::with_builtins(&enabled, timestamp_format)?,
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
    //! What these cover, and what they cannot.
    //!
    //! Everything below is the *first* half of each wiring: the configuration an operator wrote
    //! reaching the options the plan is resolved from. The second half — that the plan then writes
    //! the right bytes — is `cdm-feature`'s and `cdm-engine`'s, and is already covered there. The
    //! seam between them is exactly where the defect these tests exist to prevent lived, because a
    //! `::default()` at a call site is invisible to both sides' tests.

    use cdm_core::TableRef;
    use cdm_feature::{table_view, TableFacts};

    use super::*;
    use crate::cli::{ConfigArgs, JobArgs};

    /// A validated configuration carrying only the `--set` overrides a case needs.
    fn config(overrides: &[&str]) -> EffectiveConfig {
        super::super::resolve(
            &JobArgs {
                config: ConfigArgs {
                    set: overrides.iter().map(|s| (*s).to_owned()).collect(),
                    ..ConfigArgs::default()
                },
                dry_run: false,
                summary_out: None,
                tui: false,
            },
            super::super::JobOptions::default(),
        )
        .expect("the overrides in these tests are all valid")
    }

    /// A target table with the columns the constant-column cases write into.
    fn target_facts() -> TableFacts {
        TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "dst"),
                &[("id", "int"), ("tenant", "text"), ("data", "text")],
            ),
            &["id"],
        )
        .unwrap()
    }

    const TABLE: &str = "schema.origin.keyspace_table=ks.tbl";

    #[test]
    fn met_010_the_run_instruments_are_what_the_cql_executors_record_into() {
        // The one line that joins the two halves. `cdm-cql` records into `dyn RequestObserver` and
        // cannot name `Instruments`; `cdm-metrics` implements the trait and cannot name a driver.
        // If this conversion produced a *different* set of instruments from the one `LiveRun`
        // reports, every crate's own tests would still pass and every percentile in a real run
        // would still be zero — which is exactly the shape of the defect this fixes.
        let instruments = Arc::new(cdm_metrics::Instruments::new(std::time::Instant::now()));
        let metrics = request_metrics(Some(Arc::clone(&instruments)));
        assert!(metrics.is_observed());

        // Recorded the way the executors record, through the seam and not through `Instruments`.
        let observer: &dyn cdm_core::RequestObserver = instruments.as_ref();
        observer.request_started(cdm_core::Side::Origin);
        observer.request_finished(
            cdm_core::Side::Origin,
            cdm_metrics::Operation::RangeRead,
            std::time::Duration::from_millis(9),
        );

        let snapshot = instruments.snapshot();
        assert_eq!(
            snapshot
                .origin
                .latency_for(cdm_metrics::Operation::RangeRead)
                .count,
            1
        );

        // And a run nobody is watching hands the executors nothing to record into, which is what
        // keeps the unobserved path free of a clock read.
        assert!(!request_metrics(None).is_observed());
    }

    #[test]
    fn fea_010_constant_columns_reach_the_column_mapping() {
        let options = mapping_options(
            &config(&[
                TABLE,
                "feature.constant_columns.names=tenant",
                "feature.constant_columns.values='acme'",
            ]),
            &target_facts(),
        )
        .unwrap();
        assert_eq!(
            options.constants,
            vec![("tenant".to_owned(), "'acme'".to_owned())]
        );
    }

    #[test]
    fn fea_010_a_run_that_configures_no_constant_writes_none() {
        let options = mapping_options(&config(&[TABLE]), &target_facts()).unwrap();
        assert!(options.constants.is_empty());
        assert!(options.explode_map.is_none());
        assert!(options.extract_json.is_none());
    }

    #[test]
    fn fea_020_the_explode_map_reaches_the_column_mapping() {
        let options = mapping_options(
            &config(&[
                TABLE,
                "feature.explode_map.origin_column=fruits",
                "feature.explode_map.target_key_column=fruit",
                "feature.explode_map.target_value_column=price",
            ]),
            &target_facts(),
        )
        .unwrap();
        assert_eq!(
            options.explode_map,
            Some(("fruits".to_owned(), "fruit".to_owned(), "price".to_owned()))
        );
    }

    #[test]
    fn fea_030_extract_json_reaches_the_column_mapping() {
        let options = mapping_options(
            &config(&[
                TABLE,
                "feature.extract_json.origin_column=doc",
                "feature.extract_json.property_mapping=city:town",
            ]),
            &target_facts(),
        )
        .unwrap();
        assert_eq!(
            options.extract_json,
            Some(("doc".to_owned(), "town".to_owned()))
        );
    }

    #[test]
    fn sch_003_renames_and_skips_reach_the_column_mapping() {
        let options = mapping_options(
            &config(&[
                TABLE,
                "schema.origin.column.rename=a:b",
                "schema.origin.column.skip=c",
            ]),
            &target_facts(),
        )
        .unwrap();
        assert_eq!(options.rename, vec!["a:b".to_owned()]);
        assert_eq!(options.skip, vec!["c".to_owned()]);
    }

    #[test]
    fn mig_013_the_missing_key_timestamp_replacement_reaches_the_binder() {
        // The whole of the defect: the property parsed, validated, hashed — and then replaced by
        // `MissingKeyPolicy::default()`, which substitutes nothing, so the row that needed the
        // substitution was counted `ERROR` instead.
        let policy = missing_key_policy(&config(&[
            TABLE,
            "transform.missing_key_ts_replace=1087383600000",
        ]));
        assert_eq!(policy.missing_key_ts_replace, Some(1_087_383_600_000));
        assert_eq!(
            missing_key_policy(&config(&[TABLE])).missing_key_ts_replace,
            None
        );
    }

    /// The origin of SIT `smoke/03_ttl_writetime`, as `cdm-feature` sees it.
    fn ttl_origin_facts() -> TableFacts {
        TableFacts::from_view(
            &table_view(
                TableRef::new("origin", "smoke_ttl_writetime"),
                &[
                    ("key", "text"),
                    ("t_col1", "text"),
                    ("tw_col2", "text"),
                    ("w_col3", "text"),
                    ("col4", "text"),
                ],
            ),
            &["key"],
        )
        .unwrap()
    }

    /// That case's `fix.properties`, which is also its `migrate.properties`.
    const TTL_CONFIG: &[&str] = &[
        TABLE,
        "schema.origin.ttl.names=t_col1,tw_col2",
        "schema.origin.writetime.names=tw_col2,w_col3",
    ];

    #[test]
    fn val_018_the_validate_builder_resolves_the_same_writetime_plan_as_migrate() {
        // The root cause: `validate` resolved no plan at all, so its projection selected no
        // `TTL(…)`/`WRITETIME(…)`, there was nothing for a `USING` clause to bind, and every
        // corrected row went out at the coordinator's wall clock. Both jobs now call this one
        // function, so the two cannot resolve differently.
        let plan = writetime_ttl(&config(TTL_CONFIG), &ttl_origin_facts(), false).unwrap();
        assert!(plan.has_ttl(), "the projection must select TTL(…)");
        assert!(
            plan.has_writetime(),
            "the projection must select WRITETIME(…)"
        );
        assert_eq!(
            plan.projection(),
            [
                "TTL(t_col1)".to_owned(),
                "TTL(tw_col2)".to_owned(),
                "WRITETIME(tw_col2)".to_owned(),
                "WRITETIME(w_col3)".to_owned(),
            ],
            "the virtual columns follow the mapped ones, so no mapped position moves (SCH-007)"
        );

        // FEA-046: the clause the target upsert is generated with, from that same plan.
        let clause = using_clause(&plan);
        assert!(clause.ttl && clause.timestamp);
    }

    #[test]
    fn val_018_a_counter_target_still_resolves_no_ttl_or_writetime() {
        // FEA-045: neither side can accept a timestamp or a TTL on a counter write, so a counter
        // correction is stamped by the server — and `FEA-046` omits the clause rather than binding
        // a synthetic value.
        let plan = writetime_ttl(&config(TTL_CONFIG), &ttl_origin_facts(), true).unwrap();
        assert!(plan.projection().is_empty());
        let clause = using_clause(&plan);
        assert!(!clause.ttl && !clause.timestamp);
    }

    #[test]
    fn val_018_the_validate_builder_does_not_build_its_upsert_from_the_default_options() {
        // Asserted against the source for the reason `grd_001_*` below is: the defect was a
        // `StatementOptions::default()` at exactly one call site, it is invisible to every test on
        // either side of the seam, and restoring it would leave the run green.
        let source = include_str!("build.rs");
        let body = source
            .split("async fn validate(")
            .nth(1)
            .and_then(|rest| rest.split("\n/// ").next())
            .expect("the function is defined in this file");
        assert!(
            !body.contains("StatementOptions::default()"),
            "validate's upsert must carry the USING clause its writetime plan implies (VAL-018)"
        );
        assert!(body.contains("using: using_clause(&writetime)"), "{body}");
        assert!(body.contains("PlanTimestamps(writetime)"), "{body}");
    }

    #[test]
    fn cdc_021_the_timestamp_format_options_reach_the_registry() {
        // `CodecRegistry::with_builtins(&enabled, None)` refused this codec outright, however the
        // format and the zone were configured — including at their defaults, which are valid, so
        // the codec could not be used at all from the command line.
        codec_planner(&config(&[
            TABLE,
            "transform.codecs=TIMESTAMP_STRING_FORMAT",
        ]))
        .expect("a configured timestamp format must build its codec");
        codec_planner(&config(&[
            TABLE,
            "transform.codecs=TIMESTAMP_STRING_FORMAT",
            "transform.codec_timestamp_format=yyMMddHHmmss",
            "transform.codec_timestamp_zone=Europe/Dublin",
        ]))
        .expect("a non-default timestamp format must build its codec");
    }

    #[test]
    fn cdc_002_a_codec_name_the_registry_does_not_know_still_stops_the_run() {
        let error = codec_planner(&config(&[TABLE, "transform.codecs=NOT_A_CODEC"]))
            .expect_err("a conversion that was asked for and skipped writes the wrong bytes");
        assert_eq!(error.kind(), ErrorKind::Config);
    }

    #[test]
    fn grd_001_the_guardrail_builder_is_handed_no_target_to_be_careful_with() {
        // `GRD-001` requires the read-only property to be structural rather than observed, and the
        // structure is a signature: `guardrail` takes the origin `ClusterSession`, not `Sessions`.
        // Asserted against the source because that is where the claim lives — a reviewer changing
        // the parameter to `&Sessions` would restore exactly the reachability the requirement
        // forbids, and nothing else in the crate would notice.
        let source = include_str!("build.rs");
        let signature = source
            .split("pub(super) async fn guardrail(")
            .nth(1)
            .and_then(|rest| rest.split(") -> ").next())
            .expect("the function is defined in this file");
        assert!(
            signature.contains("session: &ClusterSession"),
            "{signature}"
        );
        assert!(!signature.contains("Sessions"), "{signature}");
    }

    #[test]
    fn cdc_021_a_run_that_converts_no_timestamps_registers_no_format() {
        // The other half of gating on the codec set: reading the two format properties must not
        // become a new way for a run that never converts a timestamp to fail to start.
        codec_planner(&config(&[
            TABLE,
            "transform.codec_timestamp_format=yyMMddHHmmss",
        ]))
        .expect("a format matters only to the codec that reads it");
    }
}
