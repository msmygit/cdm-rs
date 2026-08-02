//! The single plugin registry (`PLG-010`) and the traits it stores (`PLG-001`..`PLG-007`).
//!
//! Every plugin — built-in or third-party — enters the system through
//! [`Registry::builder`] and the `register_*` methods on [`RegistryBuilder`]. There is no
//! privileged internal path: the built-in codecs, features and jobs call exactly the API a
//! third-party crate calls, which is the only way to keep that API honest.
//!
//! ```
//! use std::sync::Arc;
//! use cdm_core::{CdmError, FilterPlugin, Plugin, Record, Registry};
//!
//! struct SkipNothing;
//! impl Plugin for SkipNothing {
//!     fn name(&self) -> &'static str { "skip-nothing" }
//!     fn provider(&self) -> &'static str { "example-plugin" }
//! }
//! impl FilterPlugin for SkipNothing {
//!     fn accepts(&self, _record: &Record) -> Result<bool, CdmError> { Ok(true) }
//! }
//!
//! let registry = Registry::builder()
//!     .register_filter(Arc::new(SkipNothing))
//!     .build()?;
//!
//! assert!(registry.filter("skip-nothing").is_some());
//! # Ok::<(), CdmError>(())
//! ```

pub mod context;
pub mod plugin;

pub use context::{
    BindingBuilder, CompareHook, EffectiveConfig, MetricsSnapshot, ProjectionBuilder, RangeOutcome,
    RangeRecord, RecordSink, RunRecord, SchemaPair, TableView, TypePair,
};
pub use plugin::{
    CodecPlugin, FeaturePlugin, FilterPlugin, GuardrailPlugin, JobPlugin, JobRunner,
    MetricsExporter, Plugin, RowSink, RowSource, RowStream, TrackingStore,
};

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::error::{CdmError, ErrorKind};

/// Declares the registry's plugin categories once, so the nine of them cannot drift apart.
///
/// For each category this generates the field on [`Registry`] and [`RegistryBuilder`], the
/// `register_*` builder method with its conflict detection, and the lookup and iteration
/// accessors on the registry.
macro_rules! categories {
    ($(
        $kind:literal, $trait:ident, $field:ident, $one:ident, $register:ident;
    )*) => {
        /// The immutable set of plugins a run uses (`PLG-010`).
        ///
        /// Built once at startup and shared by every worker. Plugins are keyed by
        /// [`Plugin::name`] within their category, so a codec and a feature may share a name.
        #[derive(Clone, Default)]
        pub struct Registry {
            $( $field: BTreeMap<&'static str, Arc<dyn $trait>>, )*
        }

        /// Accumulates registrations and reports every conflict at once (`PLG-010`).
        #[derive(Default)]
        pub struct RegistryBuilder {
            $( $field: BTreeMap<&'static str, Arc<dyn $trait>>, )*
            conflicts: Vec<String>,
        }

        impl Registry {
            $(
                #[doc = concat!("The registered ", $kind, " named `name`, if any.")]
                pub fn $one(&self, name: &str) -> Option<&Arc<dyn $trait>> {
                    self.$field.get(name)
                }

                #[doc = concat!("Every registered ", $kind, ", in name order.")]
                pub fn $field(&self) -> impl ExactSizeIterator<Item = &Arc<dyn $trait>> {
                    self.$field.values()
                }
            )*

            /// The configuration schemas contributed by every registered plugin (`PLG-013`),
            /// as `(plugin name, schema)` pairs in category then name order.
            ///
            /// These are merged into the generated JSON Schema, OpenAPI document, reference docs
            /// and Config Builder UI, so plugin settings are as discoverable as built-in ones.
            pub fn config_schemas(&self) -> Vec<(&'static str, schemars::Schema)> {
                let mut out = Vec::new();
                $(
                    for plugin in self.$field.values() {
                        if let Some(schema) = plugin.config_schema() {
                            out.push((plugin.name(), schema));
                        }
                    }
                )*
                out
            }

            /// The total number of registered plugins across every category.
            pub fn len(&self) -> usize {
                0 $( + self.$field.len() )*
            }

            /// Whether nothing at all is registered.
            pub fn is_empty(&self) -> bool {
                self.len() == 0
            }
        }

        impl RegistryBuilder {
            $(
                #[doc = concat!("Registers a ", $kind, ".")]
                ///
                /// Registering two plugins with the same name in the same category is a startup
                /// error, reported by [`RegistryBuilder::build`] and naming both providers
                /// (`PLG-010`). Registration is recorded here and validated there so that one
                /// `build` reports every conflict rather than the first.
                #[must_use]
                pub fn $register(mut self, plugin: Arc<dyn $trait>) -> Self {
                    let name = plugin.name();
                    if let Some(existing) = self.$field.get(name) {
                        self.conflicts.push(format!(
                            "{} plugin `{name}` is registered by both `{}` and `{}`",
                            $kind,
                            existing.provider(),
                            plugin.provider(),
                        ));
                    } else {
                        self.$field.insert(name, plugin);
                    }
                    self
                }
            )*

            /// Finalises the registry.
            ///
            /// # Errors
            ///
            /// Returns [`ErrorKind::Config`] listing every conflicting registration, each naming
            /// the plugin and both providers (`PLG-010`).
            pub fn build(self) -> Result<Registry, CdmError> {
                if !self.conflicts.is_empty() {
                    return Err(CdmError::new(
                        ErrorKind::Config,
                        format!(
                            "conflicting plugin registrations: {}",
                            self.conflicts.join("; ")
                        ),
                    )
                    .with_context(|c| c.with_config_key("plugins")));
                }
                Ok(Registry {
                    $( $field: self.$field, )*
                })
            }
        }

        impl fmt::Debug for Registry {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct("Registry")
                    $( .field($kind, &self.$field.keys().collect::<Vec<_>>()) )*
                    .finish()
            }
        }

        impl fmt::Debug for RegistryBuilder {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct("RegistryBuilder")
                    $( .field($kind, &self.$field.keys().collect::<Vec<_>>()) )*
                    .field("conflicts", &self.conflicts)
                    .finish()
            }
        }
    };
}

categories! {
    "codec", CodecPlugin, codecs, codec, register_codec;
    "feature", FeaturePlugin, features, feature, register_feature;
    "filter", FilterPlugin, filters, filter, register_filter;
    "guardrail", GuardrailPlugin, guardrails, guardrail, register_guardrail;
    "job", JobPlugin, jobs, job, register_job;
    "row source", RowSource, sources, source, register_source;
    "row sink", RowSink, sinks, sink, register_sink;
    "metrics exporter", MetricsExporter, exporters, exporter, register_metrics_exporter;
    "tracking store", TrackingStore, tracking_stores, tracking_store, register_tracking_store;
}

impl Registry {
    /// Starts building a registry.
    pub fn builder() -> RegistryBuilder {
        RegistryBuilder::default()
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
    use async_trait::async_trait;

    use super::*;
    use crate::domain::{
        JobKind, PrimaryKey, RawCell, Record, Row, RunId, RunStatus, TableRef, TokenRange,
    };
    use crate::error::Diagnostic;

    /// One type implementing every plugin trait, so the tests can register the same object in
    /// every category and prove the categories are independent.
    struct Everything {
        name: &'static str,
        provider: &'static str,
    }

    impl Everything {
        fn new(name: &'static str, provider: &'static str) -> Arc<Self> {
            Arc::new(Self { name, provider })
        }
    }

    impl Plugin for Everything {
        fn name(&self) -> &'static str {
            self.name
        }

        fn provider(&self) -> &'static str {
            self.provider
        }

        fn config_schema(&self) -> Option<schemars::Schema> {
            Some(schemars::json_schema!({ "type": "object" }))
        }
    }

    impl CodecPlugin for Everything {
        fn conversions(&self) -> Vec<TypePair> {
            vec![TypePair::new("text", "int")]
        }

        fn convert(&self, _pair: &TypePair, value: &RawCell) -> Result<RawCell, CdmError> {
            Ok(value.clone())
        }
    }

    impl FeaturePlugin for Everything {
        fn is_enabled(&self, config: &EffectiveConfig) -> bool {
            config.contains("feature.enabled")
        }

        fn validate(&self, _config: &EffectiveConfig, _schema: &SchemaPair) -> Vec<Diagnostic> {
            Vec::new()
        }
    }

    impl FilterPlugin for Everything {
        fn accepts(&self, _record: &Record) -> Result<bool, CdmError> {
            Ok(true)
        }
    }

    impl GuardrailPlugin for Everything {
        fn check(&self, _record: &Record) -> Result<Option<Diagnostic>, CdmError> {
            Ok(None)
        }
    }

    impl JobPlugin for Everything {
        fn kind(&self) -> Option<JobKind> {
            Some(JobKind::Migrate)
        }

        fn create(&self, _config: &EffectiveConfig) -> Result<Box<dyn JobRunner>, CdmError> {
            Ok(Box::new(NoopRunner))
        }
    }

    struct NoopRunner;

    #[async_trait]
    impl JobRunner for NoopRunner {
        async fn run_range(&self, range: TokenRange) -> Result<RangeOutcome, CdmError> {
            Ok(RangeOutcome {
                range,
                status: RunStatus::Pass,
                info: "Read: 0".to_owned(),
            })
        }
    }

    struct EmptyStream;

    #[async_trait]
    impl RowStream for EmptyStream {
        async fn next_record(&mut self) -> Result<Option<Record>, CdmError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl RowSource for Everything {
        async fn open(&self, _range: TokenRange) -> Result<Box<dyn RowStream>, CdmError> {
            Ok(Box::new(EmptyStream))
        }
    }

    #[async_trait]
    impl RowSink for Everything {
        async fn write(&self, _record: &Record) -> Result<(), CdmError> {
            Ok(())
        }

        async fn flush(&self) -> Result<(), CdmError> {
            Ok(())
        }

        async fn fetch(&self, _key: &PrimaryKey) -> Result<Option<Record>, CdmError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl MetricsExporter for Everything {
        async fn export(&self, _snapshot: &MetricsSnapshot) -> Result<(), CdmError> {
            Ok(())
        }
    }

    #[async_trait]
    impl TrackingStore for Everything {
        async fn initialise(&self) -> Result<(), CdmError> {
            Ok(())
        }

        async fn create_run(
            &self,
            _run: &RunRecord,
            _ranges: &[RangeRecord],
        ) -> Result<(), CdmError> {
            Ok(())
        }

        async fn update_run(
            &self,
            _run_id: RunId,
            _status: RunStatus,
            _info: Option<&str>,
        ) -> Result<(), CdmError> {
            Ok(())
        }

        async fn update_range(&self, _run_id: RunId, _range: &RangeRecord) -> Result<(), CdmError> {
            Ok(())
        }

        async fn run(&self, _run_id: RunId) -> Result<Option<RunRecord>, CdmError> {
            Ok(None)
        }

        async fn ranges(&self, _run_id: RunId) -> Result<Vec<RangeRecord>, CdmError> {
            Ok(Vec::new())
        }

        async fn latest_run(
            &self,
            _table: &TableRef,
            _job: JobKind,
        ) -> Result<Option<RunRecord>, CdmError> {
            Ok(None)
        }
    }

    /// A plugin that contributes no configuration, exercising the `config_schema` default.
    struct Silent;

    impl Plugin for Silent {
        fn name(&self) -> &'static str {
            "silent"
        }

        fn provider(&self) -> &'static str {
            "cdm-core-tests"
        }
    }

    impl FilterPlugin for Silent {
        fn accepts(&self, _record: &Record) -> Result<bool, CdmError> {
            Ok(true)
        }
    }

    fn full_registry() -> Registry {
        let plugin = Everything::new("everything", "cdm-core-tests");
        Registry::builder()
            .register_codec(plugin.clone())
            .register_feature(plugin.clone())
            .register_filter(plugin.clone())
            .register_guardrail(plugin.clone())
            .register_job(plugin.clone())
            .register_source(plugin.clone())
            .register_sink(plugin.clone())
            .register_metrics_exporter(plugin.clone())
            .register_tracking_store(plugin)
            .build()
            .unwrap()
    }

    #[test]
    fn plg_010_every_category_registers_through_the_same_public_path() {
        let registry = full_registry();
        assert_eq!(
            registry.len(),
            9,
            "one plugin in each of the nine categories"
        );
        assert!(!registry.is_empty());
        assert!(registry.codec("everything").is_some());
        assert!(registry.feature("everything").is_some());
        assert!(registry.filter("everything").is_some());
        assert!(registry.guardrail("everything").is_some());
        assert!(registry.job("everything").is_some());
        assert!(registry.source("everything").is_some());
        assert!(registry.sink("everything").is_some());
        assert!(registry.exporter("everything").is_some());
        assert!(registry.tracking_store("everything").is_some());
        assert!(registry.codec("absent").is_none());
    }

    #[test]
    fn plg_010_an_empty_registry_is_valid() {
        let registry = Registry::builder().build().unwrap();
        assert!(registry.is_empty());
        assert_eq!(registry.codecs().len(), 0);
        assert!(registry.config_schemas().is_empty());
        assert_eq!(Registry::default().len(), 0);
    }

    #[test]
    fn plg_010_a_conflicting_registration_is_a_startup_error_naming_both_providers() {
        let err = Registry::builder()
            .register_codec(Everything::new("text-to-int", "cdm-codec"))
            .register_codec(Everything::new("text-to-int", "acme-plugins"))
            .build()
            .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::Config);
        let message = err.to_string();
        assert!(message.contains("codec plugin `text-to-int`"), "{message}");
        assert!(message.contains("`cdm-codec`"), "{message}");
        assert!(message.contains("`acme-plugins`"), "{message}");
        assert_eq!(err.context().config_key.as_deref(), Some("plugins"));
    }

    #[test]
    fn plg_010_every_conflict_is_reported_not_just_the_first() {
        let err = Registry::builder()
            .register_codec(Everything::new("a", "one"))
            .register_codec(Everything::new("a", "two"))
            .register_filter(Everything::new("b", "three"))
            .register_filter(Everything::new("b", "four"))
            .build()
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("codec plugin `a`"), "{message}");
        assert!(message.contains("filter plugin `b`"), "{message}");
    }

    #[test]
    fn plg_010_the_same_name_in_two_categories_is_not_a_conflict() {
        let registry = Registry::builder()
            .register_codec(Everything::new("shared", "one"))
            .register_filter(Everything::new("shared", "two"))
            .build()
            .unwrap();
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn plg_010_iteration_is_deterministic_in_name_order() {
        let registry = Registry::builder()
            .register_codec(Everything::new("zeta", "p"))
            .register_codec(Everything::new("alpha", "p"))
            .register_codec(Everything::new("mu", "p"))
            .build()
            .unwrap();
        let names: Vec<&str> = registry.codecs().map(|c| c.name()).collect();
        assert_eq!(names, ["alpha", "mu", "zeta"]);
        assert_eq!(registry.codecs().len(), 3);
    }

    #[test]
    fn plg_010_registries_are_debuggable_without_leaking_plugin_internals() {
        let rendered = format!("{:?}", full_registry());
        assert!(rendered.starts_with("Registry {"), "{rendered}");
        assert!(rendered.contains("codec: [\"everything\"]"), "{rendered}");
        assert!(
            rendered.contains("tracking store: [\"everything\"]"),
            "{rendered}"
        );

        let builder = format!("{:?}", Registry::builder());
        assert!(builder.contains("conflicts: []"), "{builder}");
    }

    #[test]
    fn plg_013_every_plugin_can_contribute_config_keys() {
        let schemas = full_registry().config_schemas();
        assert_eq!(schemas.len(), 9, "one schema per registered plugin");
        assert!(schemas.iter().all(|(name, _)| *name == "everything"));

        // The default implementation opts out, so a plugin without configuration is silent.
        assert!(Silent.config_schema().is_none());
        let registry = Registry::builder()
            .register_filter(Arc::new(Silent))
            .build()
            .unwrap();
        assert!(registry.config_schemas().is_empty());
    }

    #[test]
    fn plg_012_every_plugin_trait_is_object_safe_and_shareable() {
        fn assert_shareable<T: Send + Sync + 'static + ?Sized>(_: &Arc<T>) {}

        let plugin = Everything::new("everything", "cdm-core-tests");
        // Each of these coercions only compiles if the trait is object-safe.
        let codec: Arc<dyn CodecPlugin> = plugin.clone();
        let feature: Arc<dyn FeaturePlugin> = plugin.clone();
        let filter: Arc<dyn FilterPlugin> = plugin.clone();
        let guardrail: Arc<dyn GuardrailPlugin> = plugin.clone();
        let job: Arc<dyn JobPlugin> = plugin.clone();
        let source: Arc<dyn RowSource> = plugin.clone();
        let sink: Arc<dyn RowSink> = plugin.clone();
        let exporter: Arc<dyn MetricsExporter> = plugin.clone();
        let store: Arc<dyn TrackingStore> = plugin;

        assert_shareable(&codec);
        assert_shareable(&feature);
        assert_shareable(&filter);
        assert_shareable(&guardrail);
        assert_shareable(&job);
        assert_shareable(&source);
        assert_shareable(&sink);
        assert_shareable(&exporter);
        assert_shareable(&store);
        // A registry full of trait objects is itself shareable across workers.
        assert_shareable(&Arc::new(full_registry()));
    }

    #[test]
    fn plg_001_a_codec_declares_and_performs_its_conversions() {
        let registry = full_registry();
        let codec = registry.codec("everything").unwrap();
        assert_eq!(codec.conversions(), vec![TypePair::new("text", "int")]);
        let converted = codec
            .convert(&TypePair::new("text", "int"), &RawCell::from_static(b"1"))
            .unwrap();
        assert_eq!(converted, RawCell::from_static(b"1"));
    }

    #[test]
    fn plg_002_a_feature_validates_and_transforms() {
        let registry = full_registry();
        let feature = registry.feature("everything").unwrap();
        let mut config = EffectiveConfig::new();
        assert!(!feature.is_enabled(&config));
        config.insert("feature.enabled", "true");
        assert!(feature.is_enabled(&config));

        let schema = SchemaPair::new(
            TableView::new(TableRef::new("ks", "a"), Vec::new()),
            TableView::new(TableRef::new("ks", "b"), Vec::new()),
        );
        assert!(feature.validate(&config, &schema).is_empty());

        // The default `transform` passes the record through, and the default `compare_hook`
        // declines to participate.
        let mut out: Vec<Record> = Vec::new();
        let record = Record::new(PrimaryKey::default(), Row::default());
        feature.transform(record.clone(), &mut out).unwrap();
        assert_eq!(out, vec![record]);
        assert!(feature.compare_hook().is_none());

        // The default projection and binding hooks contribute nothing.
        let mut projection = ProjectionBuilder::new();
        let mut binding = BindingBuilder::new();
        feature.extend_origin_projection(&mut projection);
        feature.extend_target_binding(&mut binding);
        assert!(projection.expressions().is_empty());
        assert!(binding.bindings().is_empty());
    }

    #[test]
    fn plg_003_filters_and_guardrails_inspect_a_record() {
        let registry = full_registry();
        let record = Record::new(PrimaryKey::default(), Row::default());
        assert!(registry
            .filter("everything")
            .unwrap()
            .accepts(&record)
            .unwrap());
        assert!(registry
            .guardrail("everything")
            .unwrap()
            .check(&record)
            .unwrap()
            .is_none());
    }

    #[test]
    fn plg_004_a_job_plugin_creates_a_runner_for_a_range() {
        let registry = full_registry();
        let job = registry.job("everything").unwrap();
        assert_eq!(job.kind(), Some(JobKind::Migrate));
        let runner = job.create(&EffectiveConfig::new()).unwrap();
        let range = TokenRange::new(0, 9).unwrap();
        let outcome = block_on(runner.run_range(range));
        assert_eq!(outcome.unwrap().status, RunStatus::Pass);
    }

    #[test]
    fn plg_005_a_source_streams_records_and_a_sink_consumes_them() {
        let registry = full_registry();
        let source = registry.source("everything").unwrap();
        let mut stream = block_on(source.open(TokenRange::new(0, 9).unwrap())).unwrap();
        assert!(block_on(stream.next_record()).unwrap().is_none());

        let sink = registry.sink("everything").unwrap();
        let record = Record::new(PrimaryKey::default(), Row::default());
        block_on(sink.write(&record)).unwrap();
        block_on(sink.flush()).unwrap();
        assert!(block_on(sink.fetch(&PrimaryKey::default()))
            .unwrap()
            .is_none());
    }

    #[test]
    fn plg_006_an_exporter_receives_a_snapshot() {
        let registry = full_registry();
        let snapshot = MetricsSnapshot {
            run_id: RunId::from_raw(1),
            job: JobKind::Migrate,
            taken_at: chrono::DateTime::UNIX_EPOCH,
            counters: BTreeMap::new(),
        };
        block_on(registry.exporter("everything").unwrap().export(&snapshot)).unwrap();
    }

    #[test]
    fn plg_007_a_tracking_store_covers_the_run_and_range_lifecycle() {
        let registry = full_registry();
        let store = registry.tracking_store("everything").unwrap();
        let run = RunRecord {
            run_id: RunId::from_raw(1),
            previous_run_id: None,
            table: TableRef::new("ks", "tbl"),
            job: JobKind::Migrate,
            status: RunStatus::NotStarted,
            started_at: None,
            ended_at: None,
            info: None,
        };
        let range = RangeRecord {
            range: TokenRange::new(0, 9).unwrap(),
            status: RunStatus::NotStarted,
            started_at: None,
            info: None,
        };

        block_on(store.initialise()).unwrap();
        block_on(store.create_run(&run, std::slice::from_ref(&range))).unwrap();
        block_on(store.update_run(run.run_id, RunStatus::Started, None)).unwrap();
        block_on(store.update_range(run.run_id, &range)).unwrap();
        assert!(block_on(store.run(run.run_id)).unwrap().is_none());
        assert!(block_on(store.ranges(run.run_id)).unwrap().is_empty());
        assert!(block_on(store.latest_run(&run.table, JobKind::Migrate))
            .unwrap()
            .is_none());
    }

    /// Drives a future to completion on the current thread.
    ///
    /// `cdm-core` has no runtime dependency and must not acquire one just to exercise its own
    /// async signatures. Every future here completes without ever yielding, so polling once is
    /// enough; anything that did yield would panic, which is exactly the signal we want.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};

        let mut context = Context::from_waker(Waker::noop());
        match pin!(future).poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("cdm-core test futures must never yield"),
        }
    }
}
