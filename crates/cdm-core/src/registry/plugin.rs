//! The plugin traits (`PLG-001`..`PLG-007`, `PLG-012`, `PLG-013`).
//!
//! Every trait here is **object-safe**, `Send + Sync + 'static`, and extends [`Plugin`], so every
//! plugin can name itself, name who provided it, and contribute configuration keys. The
//! [`Registry`](super::Registry) stores them as `Arc<dyn Trait>`, and built-in implementations use
//! exactly the same registration path as third-party ones (`PLG-010`).
//!
//! Traits whose implementations will do I/O — [`RowSource`], [`RowSink`], [`TrackingStore`],
//! [`MetricsExporter`], [`JobRunner`] — are `async` via `#[async_trait]`. `cdm-core` itself still
//! does none: `async-trait` is a signature transformation and brings no runtime with it.

use async_trait::async_trait;

use crate::domain::{JobKind, PrimaryKey, RawCell, Record, RunId, RunStatus, TokenRange};
use crate::error::{CdmError, Diagnostic};

use super::context::{
    BindingBuilder, CompareHook, EffectiveConfig, MetricsSnapshot, ProjectionBuilder, RangeOutcome,
    RangeRecord, RecordSink, RunRecord, SchemaPair, TypePair,
};

/// What every plugin can do (`PLG-012`, `PLG-013`).
///
/// # Identity
///
/// [`Plugin::name`] is the registration key within a plugin category, and [`Plugin::provider`]
/// says where the implementation came from, so that a conflicting registration can name *both*
/// sides (`PLG-010`) instead of just the key that clashed.
///
/// ```
/// use cdm_core::{Plugin, FilterPlugin, Record, CdmError};
///
/// struct DropEverything;
///
/// impl Plugin for DropEverything {
///     fn name(&self) -> &'static str { "drop-everything" }
///     fn provider(&self) -> &'static str { "example-plugin" }
/// }
///
/// impl FilterPlugin for DropEverything {
///     fn accepts(&self, _record: &Record) -> Result<bool, CdmError> { Ok(false) }
/// }
/// ```
pub trait Plugin: Send + Sync + 'static {
    /// The registration key, unique within the plugin's category. Lowercase kebab-case by
    /// convention, e.g. `explode-map`.
    fn name(&self) -> &'static str;

    /// Who supplies this plugin — a crate name for built-ins (`cdm-feature`), or the third
    /// party's own identifier. Only used in diagnostics.
    fn provider(&self) -> &'static str;

    /// Configuration keys this plugin contributes (`PLG-013`).
    ///
    /// The returned schema is merged into the generated JSON Schema, OpenAPI document, reference
    /// docs and Config Builder UI (`CFG-001`), so a plugin's settings are as discoverable as a
    /// built-in's. `None` means the plugin has no configuration.
    fn config_schema(&self) -> Option<schemars::Schema> {
        None
    }
}

/// Registers conversions between CQL type pairs (`PLG-001`).
///
/// The conversion planner resolves each column's origin/target type pair once at startup
/// (`ARCHITECTURE.md` §5.5) and caches the codec, so [`CodecPlugin::convert`] is on the hot path
/// and must not allocate more than the converted value requires.
pub trait CodecPlugin: Plugin {
    /// The type pairs this codec handles. A pair claimed by two codecs is a startup error
    /// (`PLG-010`).
    fn conversions(&self) -> Vec<TypePair>;

    /// Converts one cell from the origin type to the target type.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`](crate::ErrorKind::TypeConversion) when the value
    /// cannot be represented in the target type. This is a record-level failure: the engine counts
    /// `ERROR`, logs the primary key and column, and continues (`ARCHITECTURE.md` §13).
    fn convert(&self, pair: &TypePair, value: &RawCell) -> Result<RawCell, CdmError>;
}

/// Participates in config validation, statement construction, record transformation and
/// comparison (`PLG-002`).
///
/// All built-in features implement this trait and are registered through the public
/// [`RegistryBuilder::register_feature`](super::RegistryBuilder::register_feature) — there is no
/// privileged internal path (`PLG-010`).
pub trait FeaturePlugin: Plugin {
    /// Whether the feature is switched on for this run. A disabled feature contributes nothing:
    /// no projection, no binding, no transform.
    fn is_enabled(&self, config: &EffectiveConfig) -> bool;

    /// Tier-2 (cross-field) and Tier-3 (schema-bound) validation (`CFG-020`).
    ///
    /// Returns every finding rather than the first, so the operator fixes their configuration in
    /// one pass. An empty vector means the feature is happy.
    fn validate(&self, config: &EffectiveConfig, schema: &SchemaPair) -> Vec<Diagnostic>;

    /// Contributes extra origin projection expressions, e.g. `TTL(col)` or `WRITETIME(col)`.
    fn extend_origin_projection(&self, _projection: &mut ProjectionBuilder) {}

    /// Contributes extra target columns or literals.
    fn extend_target_binding(&self, _binding: &mut BindingBuilder) {}

    /// Transforms one origin record into zero or more output records.
    ///
    /// Emitting nothing drops the record; emitting several implements explode-map semantics
    /// (`FEA-020`). The default passes the record through unchanged.
    ///
    /// # Errors
    ///
    /// Returns a record-level error, which the engine counts and isolates.
    fn transform(&self, record: Record, out: &mut dyn RecordSink) -> Result<(), CdmError> {
        out.emit(record)
    }

    /// The feature's participation in validate comparison, if it has one.
    fn compare_hook(&self) -> Option<&dyn CompareHook> {
        None
    }
}

/// A row-level predicate (`PLG-003`).
///
/// Filters compose into a chain that evaluates in declaration order and short-circuits
/// (`FEA-054`); the chain itself lives in `cdm-feature`.
pub trait FilterPlugin: Plugin {
    /// Whether the record should be processed. `false` counts as `SKIPPED`, not as an error.
    ///
    /// # Errors
    ///
    /// Returns a record-level error if the predicate cannot be evaluated at all.
    fn accepts(&self, record: &Record) -> Result<bool, CdmError>;
}

/// A row-level check that reports rather than filters (`PLG-003`).
///
/// The guardrail job runs these over origin rows without writing anything (`GRD-001`).
pub trait GuardrailPlugin: Plugin {
    /// Checks one record, returning a diagnostic if it violates the guardrail.
    ///
    /// A violation is data to report, not a failure: the job counts `LARGE` and carries on.
    ///
    /// # Errors
    ///
    /// Returns a record-level error if the check cannot be evaluated at all.
    fn check(&self, record: &Record) -> Result<Option<Diagnostic>, CdmError>;
}

/// Registers a job type alongside migrate, validate and guardrail (`PLG-004`).
///
/// A plugin job is identified by its [`Plugin::name`]. [`JobPlugin::kind`] is `Some` only for the
/// three built-ins, which keeps [`JobKind`] closed and stops a third-party job from claiming
/// parity semantics it does not have.
pub trait JobPlugin: Plugin {
    /// The built-in kind this plugin implements, or `None` for a new job type.
    fn kind(&self) -> Option<JobKind> {
        None
    }

    /// Builds a runner for one run.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`](crate::ErrorKind::Config) if the configuration does not
    /// describe a runnable job. This happens at startup, before any data moves.
    fn create(&self, config: &EffectiveConfig) -> Result<Box<dyn JobRunner>, CdmError>;
}

/// The per-range work of a job (`PLG-004`).
#[async_trait]
pub trait JobRunner: Send + Sync + 'static {
    /// Processes one token range to completion.
    ///
    /// # Errors
    ///
    /// Returns an error only when the range as a whole failed. Record-level problems are counted
    /// and reported through the returned [`RangeOutcome`], because a single bad row must not fail
    /// a range (`ARCHITECTURE.md` §13).
    async fn run_range(&self, range: TokenRange) -> Result<RangeOutcome, CdmError>;
}

/// An origin of rows (`PLG-005`).
///
/// Abstracting the origin behind a trait is what makes an alternative backend possible without
/// touching the engine; the Cassandra implementation lives in `cdm-cql`.
#[async_trait]
pub trait RowSource: Plugin {
    /// Opens a stream of the rows in one token range.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Read`](crate::ErrorKind::Read), which the engine retries with backoff.
    async fn open(&self, range: TokenRange) -> Result<Box<dyn RowStream>, CdmError>;
}

/// A stream of records produced by a [`RowSource`] (`PLG-005`).
///
/// Pull-based and one row at a time, so the engine controls memory: `NFR-003` bounds steady-state
/// RSS by the in-flight page count, which is only enforceable if nothing buffers a whole range.
#[async_trait]
pub trait RowStream: Send + 'static {
    /// The next record, or `None` at the end of the range.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Read`](crate::ErrorKind::Read).
    async fn next_record(&mut self) -> Result<Option<Record>, CdmError>;
}

/// A destination for records (`PLG-005`).
#[async_trait]
pub trait RowSink: Plugin {
    /// Writes one record. Implementations may buffer; anything buffered counts as `UNFLUSHED`
    /// until [`RowSink::flush`] returns (`MET-001`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Write`](crate::ErrorKind::Write).
    async fn write(&self, record: &Record) -> Result<(), CdmError>;

    /// Flushes anything buffered. Called at least at the end of every range.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Write`](crate::ErrorKind::Write).
    async fn flush(&self) -> Result<(), CdmError>;

    /// Fetches the target row for a record, so a validate job can compare it (`VAL-002`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Read`](crate::ErrorKind::Read).
    async fn fetch(&self, key: &PrimaryKey) -> Result<Option<Record>, CdmError>;
}

/// An additional sink for metrics (`PLG-006`).
///
/// The built-in Prometheus and OTLP exporters register through this trait like any other; so does
/// the Java-format reporter that prints the parity block of `MET-006`.
#[async_trait]
pub trait MetricsExporter: Plugin {
    /// Publishes a snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error the engine logs and otherwise ignores: a failing exporter must never fail
    /// a run.
    async fn export(&self, snapshot: &MetricsSnapshot) -> Result<(), CdmError>;
}

/// A backend for run tracking (`PLG-007`, `TRK-036`).
///
/// The default implementation writes Java's `cdm_run_info` and `cdm_run_details` tables in the
/// target keyspace (`TRK-010`), which is what makes a run resumable across the two tools
/// (`COMPAT-003`). SQLite and in-memory implementations exist for targets that cannot host extra
/// tables and for tests.
#[async_trait]
pub trait TrackingStore: Plugin {
    /// Creates whatever storage the backend needs, if absent (`TRK-010`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](crate::ErrorKind::Tracking).
    async fn initialise(&self) -> Result<(), CdmError>;

    /// Records a new run and its planned ranges, all as `NOT_STARTED` (`TRK-020`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](crate::ErrorKind::Tracking), in particular when the run id
    /// already exists — which `TRK-020` requires to be rejected rather than overwritten.
    async fn create_run(&self, run: &RunRecord, ranges: &[RangeRecord]) -> Result<(), CdmError>;

    /// Updates the run row's status and, at the end, its metrics string (`TRK-021`, `TRK-022`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](crate::ErrorKind::Tracking).
    async fn update_run(
        &self,
        run_id: RunId,
        status: RunStatus,
        info: Option<&str>,
    ) -> Result<(), CdmError>;

    /// Updates one range's row (`TRK-021`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](crate::ErrorKind::Tracking).
    async fn update_range(&self, run_id: RunId, range: &RangeRecord) -> Result<(), CdmError>;

    /// The run row, if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](crate::ErrorKind::Tracking).
    async fn run(&self, run_id: RunId) -> Result<Option<RunRecord>, CdmError>;

    /// Every range row of a run, which is what a resume filters to the pending set (`TRK-031`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](crate::ErrorKind::Tracking).
    async fn ranges(&self, run_id: RunId) -> Result<Vec<RangeRecord>, CdmError>;

    /// The most recent run for a table and job, which `auto_rerun` adopts (`TRK-030`).
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Tracking`](crate::ErrorKind::Tracking).
    async fn latest_run(
        &self,
        table: &crate::domain::TableRef,
        job: JobKind,
    ) -> Result<Option<RunRecord>, CdmError>;
}
