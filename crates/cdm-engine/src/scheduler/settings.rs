//! What the scheduler needs to know before it starts (`ENG-001`, `ENG-004`, `ENG-007`,
//! `ENG-009`, `ENG-010`, `ENG-012`).
//!
//! [`SchedulerSettings`] is resolved once, from [`EffectiveConfig`], and then never consulted
//! again except through its getters — the same discipline `ARCHITECTURE.md` §5.5 applies to the
//! per-row hot path, for the same reason: a setting read on every range is a setting that can
//! change under a run.
//!
//! # One setting this crate reads but cannot apply
//!
//! **`cluster.ratelimit_is_global`** (`ENG-004`) is resolved here and exposed as
//! [`SchedulerSettings::ratelimit_is_global`], but the scheduler cannot act on it alone: the
//! effect it asks for — dividing the configured limit across live nodes — needs the live node
//! count, and only `cdm-cluster` knows that. The division itself is
//! [`SchedulerSettings::divided_across_nodes`], which the caller applies when the flag is set.
//! Splitting it this way keeps the scheduler free of a membership dependency while still making
//! the operator's choice visible to it.
//!
//! # One setting that has no configuration key
//!
//! [`SchedulerSettings::handle_signals`] (`ENG-010`) decides whether a run installs the
//! process-wide `SIGINT`/`SIGTERM` listener. It is deliberately *not* a configuration key: it is
//! not the operator's choice but the embedder's. A `cdm migrate` process is the run, so the run
//! owns the process's signals; a run driven by the HTTP control plane, or by a test, or by an
//! application that has embedded this crate, is one activity among several and must not seize
//! them. [`SchedulerSettings::from_config`] therefore turns it on — configuration is only ever
//! loaded by a process whose purpose is the run — and [`SchedulerSettings::default`] leaves it
//! off, so a programmatically constructed scheduler never has a side effect on the process that
//! built it.

use std::time::Duration;

use cdm_config::types::LogFormat;
use cdm_config::EffectiveConfig;

/// The default grace period a graceful shutdown gives in-flight ranges (`ENG-010`).
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(60);

/// Everything the scheduler is configured with, resolved once at startup.
///
/// `clippy::struct_excessive_bools` is allowed here for the same reason `cdm_properties!` allows
/// it on every configuration struct: these are not a state machine, they are five independent
/// operator switches, and folding them into two-variant enums would rename them without making
/// any combination of them any less possible.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerSettings {
    workers: u32,
    node_id: String,
    origin_rows_per_second: u32,
    target_rows_per_second: u32,
    max_inflight_reads: u32,
    max_inflight_writes: u32,
    fetch_size: u32,
    error_limit: u64,
    shutdown_grace: Duration,
    adaptive_ratelimit: bool,
    adaptive_ratelimit_min_percent: u8,
    ratelimit_is_global: bool,
    java_thread_label: bool,
    handle_signals: bool,
}

impl Default for SchedulerSettings {
    /// The defaults of `CFG-160`, with one worker.
    ///
    /// A default worker count of one rather than `num_cpus` keeps this impl pure: the number of
    /// CPUs is an environment fact, and a `Default` that reads the environment makes tests
    /// depend on the machine that runs them. [`SchedulerSettings::from_config`] resolves the real
    /// value, which `cdm-config` has already defaulted to `num_cpus`.
    fn default() -> Self {
        Self {
            workers: 1,
            node_id: "local".to_owned(),
            origin_rows_per_second: 20_000,
            target_rows_per_second: 20_000,
            max_inflight_reads: 256,
            max_inflight_writes: 2_000,
            fetch_size: 1_000,
            error_limit: 0,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
            adaptive_ratelimit: false,
            adaptive_ratelimit_min_percent: 10,
            ratelimit_is_global: false,
            java_thread_label: true,
            // ENG-010: off by default, because a `Default` that installs a process-wide signal
            // handler is a `Default` with a side effect on code that never asked for one.
            handle_signals: false,
        }
    }
}

impl SchedulerSettings {
    /// Resolves the settings from a validated, resolved configuration.
    #[must_use]
    pub fn from_config(config: &EffectiveConfig) -> Self {
        let perfops = &config.config().perfops;
        Self {
            workers: config.workers().max(1),
            node_id: config.node_id().to_owned(),
            origin_rows_per_second: perfops.ratelimit.origin,
            target_rows_per_second: perfops.ratelimit.target,
            max_inflight_reads: perfops.max_inflight_reads,
            max_inflight_writes: perfops.max_inflight_writes,
            fetch_size: perfops.fetch_size,
            error_limit: perfops.error_limit,
            shutdown_grace: perfops.shutdown_grace.get(),
            adaptive_ratelimit: perfops.adaptive_ratelimit,
            adaptive_ratelimit_min_percent: perfops.adaptive_ratelimit_min_percent,
            ratelimit_is_global: config.config().cluster.ratelimit_is_global,
            // ENG-012: the Java `min:max` label is a `pretty`-format affordance. In `compact`
            // and `json` the same facts are structured span fields, and repeating them as a
            // pre-formatted string would be noise.
            java_thread_label: config.config().logging.format == LogFormat::Pretty,
            // ENG-010: a process that has loaded a configuration is a process whose purpose is
            // this run, so the run owns its signals.
            handle_signals: true,
        }
    }

    /// How many range workers to run (`ENG-001`). Never zero.
    #[must_use]
    pub const fn workers(&self) -> u32 {
        self.workers
    }

    /// This node's identity, as carried by every range span (`ENG-011`).
    #[must_use]
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Origin rows read per second, `0` for unlimited (`ENG-004`).
    #[must_use]
    pub const fn origin_rows_per_second(&self) -> u32 {
        self.origin_rows_per_second
    }

    /// Target rows written per second, `0` for unlimited (`ENG-004`).
    #[must_use]
    pub const fn target_rows_per_second(&self) -> u32 {
        self.target_rows_per_second
    }

    /// Maximum concurrent origin reads (`ENG-007`).
    #[must_use]
    pub const fn max_inflight_reads(&self) -> u32 {
        self.max_inflight_reads
    }

    /// Maximum concurrent target writes (`ENG-007`).
    #[must_use]
    pub const fn max_inflight_writes(&self) -> u32 {
        self.max_inflight_writes
    }

    /// Origin page size, in rows (`ENG-003`).
    #[must_use]
    pub const fn fetch_size(&self) -> u32 {
        self.fetch_size
    }

    /// Row-level errors tolerated before the run aborts, `0` for no limit (`ENG-009`).
    #[must_use]
    pub const fn error_limit(&self) -> u64 {
        self.error_limit
    }

    /// How long in-flight ranges have to finish after a graceful stop (`ENG-010`).
    #[must_use]
    pub const fn shutdown_grace(&self) -> Duration {
        self.shutdown_grace
    }

    /// Whether the target write rate is handed to `ENG-006`'s AIMD controller.
    ///
    /// When set, `perfops.ratelimit.target` becomes the controller's *ceiling* rather than the
    /// rate. The origin read rate is never adaptive: `ENG-004` keeps the two sides independent,
    /// and it is the target that reports overload.
    #[must_use]
    pub const fn adaptive_ratelimit(&self) -> bool {
        self.adaptive_ratelimit
    }

    /// The adaptive controller's floor, as a percentage of the configured target rate
    /// (`ENG-006`).
    #[must_use]
    pub const fn adaptive_ratelimit_min_percent(&self) -> u8 {
        self.adaptive_ratelimit_min_percent
    }

    /// Whether the configured rate limits are a fleet-wide budget rather than per-node
    /// (`ENG-004`).
    ///
    /// The scheduler reports this rather than applying it; see the module documentation.
    #[must_use]
    pub const fn ratelimit_is_global(&self) -> bool {
        self.ratelimit_is_global
    }

    /// Whether the run installs the `SIGINT`/`SIGTERM` listener of `ENG-010`.
    ///
    /// See the module documentation for why this is the embedder's choice rather than the
    /// operator's.
    #[must_use]
    pub const fn handle_signals(&self) -> bool {
        self.handle_signals
    }

    /// Whether range spans carry the Java-compatible `min:max` label (`ENG-012`).
    #[must_use]
    pub const fn java_thread_label(&self) -> bool {
        self.java_thread_label
    }

    /// Sets the worker count, clamped to at least one.
    #[must_use]
    pub const fn with_workers(mut self, workers: u32) -> Self {
        self.workers = if workers == 0 { 1 } else { workers };
        self
    }

    /// Sets this node's identity.
    #[must_use]
    pub fn with_node_id(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = node_id.into();
        self
    }

    /// Sets both rate limits, in rows per second.
    #[must_use]
    pub const fn with_ratelimits(mut self, origin: u32, target: u32) -> Self {
        self.origin_rows_per_second = origin;
        self.target_rows_per_second = target;
        self
    }

    /// Sets the maximum concurrent origin reads.
    #[must_use]
    pub const fn with_max_inflight_reads(mut self, reads: u32) -> Self {
        self.max_inflight_reads = reads;
        self
    }

    /// Sets the maximum concurrent target writes.
    #[must_use]
    pub const fn with_max_inflight_writes(mut self, writes: u32) -> Self {
        self.max_inflight_writes = writes;
        self
    }

    /// Sets the origin page size.
    #[must_use]
    pub const fn with_fetch_size(mut self, fetch_size: u32) -> Self {
        self.fetch_size = fetch_size;
        self
    }

    /// Sets the error limit.
    #[must_use]
    pub const fn with_error_limit(mut self, error_limit: u64) -> Self {
        self.error_limit = error_limit;
        self
    }

    /// Sets the graceful-shutdown grace period.
    #[must_use]
    pub const fn with_shutdown_grace(mut self, grace: Duration) -> Self {
        self.shutdown_grace = grace;
        self
    }

    /// Sets whether the target rate is adaptive, and the floor it may fall to (`ENG-006`).
    #[must_use]
    pub const fn with_adaptive_ratelimit(mut self, enabled: bool, min_percent: u8) -> Self {
        self.adaptive_ratelimit = enabled;
        self.adaptive_ratelimit_min_percent = min_percent;
        self
    }

    /// Sets whether the rate limits are a fleet-wide budget.
    #[must_use]
    pub const fn with_ratelimit_is_global(mut self, global: bool) -> Self {
        self.ratelimit_is_global = global;
        self
    }

    /// Sets whether the run installs the `SIGINT`/`SIGTERM` listener (`ENG-010`).
    #[must_use]
    pub const fn with_signal_handling(mut self, enabled: bool) -> Self {
        self.handle_signals = enabled;
        self
    }

    /// Sets whether the Java-compatible range label is emitted.
    #[must_use]
    pub const fn with_java_thread_label(mut self, enabled: bool) -> Self {
        self.java_thread_label = enabled;
        self
    }

    /// Divides both rate limits across `live_nodes` (`ENG-004`, global mode).
    ///
    /// The configured limits are per node by default, matching Java's per-worker semantics. When
    /// an operator asks for a *fleet-wide* limit, every node applies its share instead. Integer
    /// division rounds down, but the result is clamped to at least one row per second: a
    /// hundred-node fleet with a limit of fifty must still make progress, and a node silently
    /// pinned at zero rows per second would look exactly like a hung run.
    ///
    /// A limit of `0` — unlimited — divides to `0`, because a share of "no limit" is "no limit".
    #[must_use]
    pub const fn divided_across_nodes(mut self, live_nodes: u32) -> Self {
        let nodes = if live_nodes == 0 { 1 } else { live_nodes };
        self.origin_rows_per_second = divide(self.origin_rows_per_second, nodes);
        self.target_rows_per_second = divide(self.target_rows_per_second, nodes);
        self
    }
}

/// One node's share of a fleet-wide limit.
const fn divide(limit: u32, nodes: u32) -> u32 {
    if limit == 0 {
        return 0;
    }
    let share = limit / nodes;
    if share == 0 {
        1
    } else {
        share
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
    use cdm_config::CdmConfig;

    use super::*;

    #[test]
    fn eng_001_settings_are_resolved_from_the_effective_configuration() {
        let mut config = CdmConfig::default();
        config.perfops.workers = Some(12);
        config.perfops.ratelimit.origin = 5_000;
        config.perfops.ratelimit.target = 7_000;
        config.perfops.max_inflight_reads = 64;
        config.perfops.max_inflight_writes = 128;
        config.perfops.fetch_size = 250;
        config.perfops.error_limit = 9;
        config.cluster.node_id = Some("node-a".to_owned());

        let settings = SchedulerSettings::from_config(&EffectiveConfig::resolve(config));

        assert_eq!(settings.workers(), 12);
        assert_eq!(settings.node_id(), "node-a");
        assert_eq!(settings.origin_rows_per_second(), 5_000);
        assert_eq!(settings.target_rows_per_second(), 7_000);
        assert_eq!(settings.max_inflight_reads(), 64);
        assert_eq!(settings.max_inflight_writes(), 128);
        assert_eq!(settings.fetch_size(), 250);
        assert_eq!(settings.error_limit(), 9);
        assert_eq!(settings.shutdown_grace(), DEFAULT_SHUTDOWN_GRACE);
    }

    #[test]
    fn eng_010_the_shutdown_grace_comes_from_the_configuration() {
        // Until `perfops.shutdown_grace` existed, this crate hard-coded the default and an
        // operator whose ranges take longer than a minute had no way to say so.
        let mut config = CdmConfig::default();
        config.perfops.shutdown_grace = cdm_config::types::DurationSetting::from_secs(300);

        let settings = SchedulerSettings::from_config(&EffectiveConfig::resolve(config));

        assert_eq!(settings.shutdown_grace(), Duration::from_secs(300));
    }

    #[test]
    fn eng_010_a_configured_run_owns_the_process_signals_and_a_built_one_does_not() {
        // The asymmetry is the point: `cdm migrate` *is* the process, so it takes SIGINT; a
        // scheduler built in-process by the API server, or by a test, must not.
        assert!(
            SchedulerSettings::from_config(&EffectiveConfig::resolve(CdmConfig::default()))
                .handle_signals()
        );
        assert!(!SchedulerSettings::default().handle_signals());
        assert!(SchedulerSettings::default()
            .with_signal_handling(true)
            .handle_signals());
    }

    #[test]
    fn eng_004_a_fleet_wide_ratelimit_is_read_from_the_cluster_section() {
        let mut config = CdmConfig::default();
        config.cluster.ratelimit_is_global = true;

        let settings = SchedulerSettings::from_config(&EffectiveConfig::resolve(config));

        // Read, not applied: dividing needs the live node count, which only `cdm-cluster` has.
        assert!(settings.ratelimit_is_global());
        assert_eq!(settings.origin_rows_per_second(), 20_000);
    }

    #[test]
    fn eng_004_the_default_ratelimit_is_per_node() {
        // Java's limits are per worker. Defaulting to a fleet-wide budget would silently slow
        // every existing multi-node invocation by a factor of the node count.
        let settings =
            SchedulerSettings::from_config(&EffectiveConfig::resolve(CdmConfig::default()));
        assert!(!settings.ratelimit_is_global());
    }

    #[test]
    fn eng_010_the_shutdown_grace_defaults_to_sixty_seconds() {
        assert_eq!(DEFAULT_SHUTDOWN_GRACE, Duration::from_secs(60));
        assert_eq!(
            SchedulerSettings::default().shutdown_grace(),
            Duration::from_secs(60)
        );
        assert_eq!(
            SchedulerSettings::default()
                .with_shutdown_grace(Duration::from_secs(5))
                .shutdown_grace(),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn eng_012_the_java_label_follows_the_log_format() {
        let mut config = CdmConfig::default();
        config.logging.format = LogFormat::Pretty;
        assert!(
            SchedulerSettings::from_config(&EffectiveConfig::resolve(config.clone()))
                .java_thread_label()
        );

        config.logging.format = LogFormat::Json;
        assert!(
            !SchedulerSettings::from_config(&EffectiveConfig::resolve(config)).java_thread_label()
        );
    }

    #[test]
    fn eng_006_the_adaptive_settings_are_read_from_the_configuration() {
        // `perfops.adaptive_ratelimit` was parsed and then ignored before PR #53; the point of
        // this test is that the flag now reaches the object that can act on it.
        let mut config = CdmConfig::default();
        config.perfops.adaptive_ratelimit = true;
        config.perfops.adaptive_ratelimit_min_percent = 25;

        let settings = SchedulerSettings::from_config(&EffectiveConfig::resolve(config));
        assert!(settings.adaptive_ratelimit());
        assert_eq!(settings.adaptive_ratelimit_min_percent(), 25);
    }

    #[test]
    fn eng_006_the_rate_is_static_unless_the_operator_says_otherwise() {
        let settings =
            SchedulerSettings::from_config(&EffectiveConfig::resolve(CdmConfig::default()));
        assert!(!settings.adaptive_ratelimit());
        assert!(!SchedulerSettings::default().adaptive_ratelimit());
        assert_eq!(
            SchedulerSettings::default().adaptive_ratelimit_min_percent(),
            10
        );
    }

    #[test]
    fn eng_001_the_worker_count_is_never_zero() {
        assert_eq!(SchedulerSettings::default().with_workers(0).workers(), 1);
        assert_eq!(SchedulerSettings::default().with_workers(7).workers(), 7);
    }

    #[test]
    fn eng_004_a_global_rate_limit_is_divided_across_live_nodes() {
        let settings = SchedulerSettings::default()
            .with_ratelimits(1_000, 500)
            .divided_across_nodes(4);
        assert_eq!(settings.origin_rows_per_second(), 250);
        assert_eq!(settings.target_rows_per_second(), 125);
    }

    #[test]
    fn eng_004_a_nodes_share_of_a_global_limit_is_never_zero() {
        let settings = SchedulerSettings::default()
            .with_ratelimits(50, 50)
            .divided_across_nodes(100);
        assert_eq!(settings.origin_rows_per_second(), 1);
        assert_eq!(settings.target_rows_per_second(), 1);
    }

    #[test]
    fn eng_004_dividing_an_unlimited_rate_leaves_it_unlimited() {
        let settings = SchedulerSettings::default()
            .with_ratelimits(0, 0)
            .divided_across_nodes(8);
        assert_eq!(settings.origin_rows_per_second(), 0);
        assert_eq!(settings.target_rows_per_second(), 0);
    }

    #[test]
    fn eng_004_dividing_across_no_live_nodes_divides_across_one() {
        let settings = SchedulerSettings::default()
            .with_ratelimits(30, 30)
            .divided_across_nodes(0);
        assert_eq!(settings.origin_rows_per_second(), 30);
    }

    #[test]
    fn eng_001_every_setting_has_a_builder() {
        let settings = SchedulerSettings::default()
            .with_workers(3)
            .with_node_id("n1")
            .with_ratelimits(1, 2)
            .with_max_inflight_reads(4)
            .with_max_inflight_writes(5)
            .with_fetch_size(6)
            .with_error_limit(7)
            .with_shutdown_grace(Duration::from_millis(8))
            .with_adaptive_ratelimit(true, 30)
            .with_signal_handling(true)
            .with_java_thread_label(false);

        assert_eq!(settings.workers(), 3);
        assert_eq!(settings.node_id(), "n1");
        assert_eq!(settings.origin_rows_per_second(), 1);
        assert_eq!(settings.target_rows_per_second(), 2);
        assert_eq!(settings.max_inflight_reads(), 4);
        assert_eq!(settings.max_inflight_writes(), 5);
        assert_eq!(settings.fetch_size(), 6);
        assert_eq!(settings.error_limit(), 7);
        assert_eq!(settings.shutdown_grace(), Duration::from_millis(8));
        assert!(settings.adaptive_ratelimit());
        assert_eq!(settings.adaptive_ratelimit_min_percent(), 30);
        assert!(settings.handle_signals());
        assert!(!settings.java_thread_label());
    }
}
