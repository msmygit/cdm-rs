//! What the migrate job is configured with, resolved once (`MIG-004`, `MIG-021`, `MIG-022`,
//! `MIG-041`).
//!
//! # The flush threshold, and the Java bug that made it unreachable
//!
//! `MIG-004` gives the formula:
//!
//! ```text
//! flush_threshold = min(fetch_size, max(batch_size * 10, 100))
//! ```
//!
//! cdm-rs computes it exactly as Java does. What differs is the comparison Java makes against it.
//! `CopyJobSession` reads
//!
//! ```java
//! jobCounter.getCount(CounterType.UNFLUSHED) >= flushThreshold
//! ```
//!
//! and the single-argument `getCount` is `getCount(type, false)` — the **committed** value.
//! `UNFLUSHED` is only ever incremented at the *interim* level and is reset before it could be
//! flushed, so its committed value is permanently `0` and that condition is never true. Java
//! therefore flushes exactly once, at the end of each range, having buffered every write for the
//! whole range in memory. It is a large part of why Java CDM's documentation says
//! `--driver-memory 25G`.
//!
//! [`MigrateSettings::should_flush`] takes the **interim** count, so the threshold does what it
//! says. `--compat-java` does not restore the unreachable comparison: reproducing an
//! unbounded-memory bug has no legitimate use, and `NFR-003` requires peak memory to be bounded by
//! configuration rather than by range size. This divergence is recorded in
//! `docs/MIGRATION_FROM_JAVA.md`.

use cdm_config::types::BatchGrouping;
use cdm_config::EffectiveConfig;

/// Why `perfops.batch_size` was coerced to 1 (`MIG-021`).
///
/// Carried rather than discarded because the coercion changes throughput by up to an order of
/// magnitude, and an operator who set `batch_size = 100` and got `1` deserves to be told which of
/// the two rules did it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BatchCoercion {
    /// The target is a counter table: a counter update is not idempotent, and a batch that is
    /// partially applied and then retried by the coordinator double-counts (`MIG-032`,
    /// `CON-012`).
    CounterTable,
    /// A writetime filter is active. Java's rule; the filter reads the origin's writetime per
    /// row, and Java's batching path does not carry the per-row `USING TIMESTAMP` a batch would
    /// need for the filtered rows to be written correctly.
    WritetimeFilter,
    /// The configured value was less than 1, which is not a batch size.
    NotABatch,
}

impl BatchCoercion {
    /// The sentence a notice or a log line uses.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::CounterTable => {
                "the target is a counter table, and a counter update must never be batched \
                 (MIG-021, MIG-032)"
            }
            Self::WritetimeFilter => {
                "a writetime filter is active, which Java also coerces to a batch size of 1 \
                 (MIG-021)"
            }
            Self::NotABatch => "the configured batch size is less than 1 (MIG-021)",
        }
    }
}

/// The migrate job's resolved settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrateSettings {
    configured_batch_size: u32,
    batch_size: u32,
    coercion: Option<BatchCoercion>,
    fetch_size: u32,
    flush_threshold: u64,
    grouping: BatchGrouping,
    dry_run: bool,
}

impl Default for MigrateSettings {
    /// The `CFG-160` defaults, uncoerced: `batch_size = 5`, `fetch_size = 1000`.
    fn default() -> Self {
        Self::new(5, 1_000, BatchGrouping::Strict, false, false, false)
    }
}

impl MigrateSettings {
    /// Resolves the settings, applying the coercion of `MIG-021`.
    ///
    /// `counter_target` and `writetime_filter` are facts about the run that the configuration
    /// cannot override, which is why they are arguments rather than properties: Tier-2 and Tier-3
    /// validation already emit the notice (`CFG-040`), and this is where the notice becomes true.
    #[must_use]
    pub fn new(
        batch_size: u32,
        fetch_size: u32,
        grouping: BatchGrouping,
        counter_target: bool,
        writetime_filter: bool,
        dry_run: bool,
    ) -> Self {
        // The order matches Java's `CqlTable.getBatchSize`, and it matters only for which reason
        // is reported: a counter table with a writetime filter is coerced either way.
        let coercion = if counter_target {
            Some(BatchCoercion::CounterTable)
        } else if writetime_filter {
            Some(BatchCoercion::WritetimeFilter)
        } else if batch_size < 1 {
            Some(BatchCoercion::NotABatch)
        } else {
            None
        };
        let effective = if coercion.is_some() { 1 } else { batch_size };
        Self {
            configured_batch_size: batch_size,
            batch_size: effective,
            coercion,
            fetch_size,
            flush_threshold: flush_threshold(fetch_size, effective),
            grouping,
            dry_run,
        }
    }

    /// Resolves the settings from a validated configuration.
    #[must_use]
    pub fn from_config(
        config: &EffectiveConfig,
        counter_target: bool,
        writetime_filter: bool,
        dry_run: bool,
    ) -> Self {
        let perfops = &config.config().perfops;
        Self::new(
            perfops.batch_size,
            perfops.fetch_size,
            perfops.batch_grouping,
            counter_target,
            writetime_filter,
            dry_run,
        )
    }

    /// The batch size actually in force, after `MIG-021`. Never zero.
    #[must_use]
    pub const fn batch_size(&self) -> u32 {
        self.batch_size
    }

    /// The batch size the operator configured, before coercion.
    #[must_use]
    pub const fn configured_batch_size(&self) -> u32 {
        self.configured_batch_size
    }

    /// Why the batch size was coerced, if it was (`MIG-021`).
    #[must_use]
    pub const fn coercion(&self) -> Option<BatchCoercion> {
        self.coercion
    }

    /// Whether writes are batched at all (`MIG-020`).
    #[must_use]
    pub const fn is_batching(&self) -> bool {
        self.batch_size > 1
    }

    /// The origin page size, in rows (`ENG-003`).
    #[must_use]
    pub const fn fetch_size(&self) -> u32 {
        self.fetch_size
    }

    /// How rows are grouped into a batch (`MIG-022`).
    #[must_use]
    pub const fn grouping(&self) -> BatchGrouping {
        self.grouping
    }

    /// Whether this run issues no target writes (`MIG-041`).
    #[must_use]
    pub const fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// The flush threshold of `MIG-004`: `min(fetch_size, max(batch_size * 10, 100))`.
    #[must_use]
    pub const fn flush_threshold(&self) -> u64 {
        self.flush_threshold
    }

    /// Whether `unflushed` buffered writes are enough to flush (`MIG-004`).
    ///
    /// **The argument must be the interim count.** The whole of the Java defect this requirement
    /// documents is that the committed count is read here instead, where it is structurally always
    /// zero. `mig_004_the_threshold_fires_on_the_interim_count_not_the_committed_one` asserts the
    /// difference rather than merely asserting that a flush happens.
    #[must_use]
    pub const fn should_flush(&self, unflushed_interim: u64) -> bool {
        unflushed_interim >= self.flush_threshold
    }

    /// Logs the resolved parameters once, in the shape Java's `PARAM --` line has (`FEA-062`).
    pub fn log(&self) {
        tracing::info!(
            target: "cdm::engine::migrate",
            flush_threshold = self.flush_threshold,
            fetch_size = self.fetch_size,
            batch_size = self.batch_size,
            configured_batch_size = self.configured_batch_size,
            batch_grouping = self.grouping.as_str(),
            dry_run = self.dry_run,
            "migrate parameters resolved"
        );
        if let Some(coercion) = self.coercion {
            tracing::warn!(
                target: "cdm::engine::migrate",
                configured = self.configured_batch_size,
                effective = self.batch_size,
                "perfops.batch_size was coerced to 1: {}",
                coercion.reason()
            );
        }
    }
}

/// `MIG-004`'s formula, in `u64` so that a large `fetch_size` cannot overflow the multiplication.
const fn flush_threshold(fetch_size: u32, batch_size: u32) -> u64 {
    let batched = (batch_size as u64).saturating_mul(10);
    let floor = if batched > 100 { batched } else { 100 };
    let fetch = fetch_size as u64;
    if fetch < floor {
        fetch
    } else {
        floor
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
    use super::*;

    #[test]
    fn mig_004_the_threshold_is_javas_formula_exactly() {
        // min(fetch_size, max(batch_size * 10, 100))
        assert_eq!(flush_threshold(1_000, 5), 100, "5*10=50, floored at 100");
        assert_eq!(flush_threshold(1_000, 50), 500, "50*10=500");
        assert_eq!(flush_threshold(200, 50), 200, "the fetch size wins");
        assert_eq!(flush_threshold(10, 1), 10, "a small page flushes per page");
        assert_eq!(flush_threshold(1_000, 1), 100);
        // The multiplication cannot overflow the way `int` does in Java, where
        // `batchSize * 10` wraps and `Math.max(..., 100)` then picks 100 from a negative number.
        assert_eq!(
            flush_threshold(u32::MAX, u32::MAX),
            u64::from(u32::MAX),
            "the fetch size still bounds it"
        );
        assert_eq!(flush_threshold(u32::MAX, 100_000_000), 1_000_000_000);
    }

    #[test]
    fn mig_004_the_threshold_fires_on_the_interim_count_not_the_committed_one() {
        // The Java defect this requirement documents is a comparison against a value that is
        // structurally always zero. The threshold must therefore be reachable, and the test that
        // proves it is one that would pass trivially against a permanently-zero committed count —
        // so it asserts both directions.
        let settings = MigrateSettings::default();
        assert_eq!(settings.flush_threshold(), 100);
        assert!(!settings.should_flush(0), "a zero count never flushes");
        assert!(!settings.should_flush(99));
        assert!(settings.should_flush(100), "the threshold is reachable");
        assert!(settings.should_flush(101));
    }

    #[test]
    fn mig_021_a_counter_target_coerces_the_batch_size_to_one() {
        let settings = MigrateSettings::new(100, 1_000, BatchGrouping::Strict, true, false, false);
        assert_eq!(settings.batch_size(), 1);
        assert_eq!(settings.configured_batch_size(), 100);
        assert!(!settings.is_batching());
        assert_eq!(settings.coercion(), Some(BatchCoercion::CounterTable));
        // The threshold follows the *coerced* size, as Java's does.
        assert_eq!(settings.flush_threshold(), 100);
        settings.log();
    }

    #[test]
    fn mig_021_a_writetime_filter_coerces_the_batch_size_to_one() {
        let settings = MigrateSettings::new(20, 1_000, BatchGrouping::Strict, false, true, false);
        assert_eq!(settings.batch_size(), 1);
        assert_eq!(settings.coercion(), Some(BatchCoercion::WritetimeFilter));
        assert!(settings.coercion().unwrap().reason().contains("MIG-021"));
    }

    #[test]
    fn mig_021_a_batch_size_below_one_is_coerced_and_says_so() {
        let settings = MigrateSettings::new(0, 1_000, BatchGrouping::Strict, false, false, false);
        assert_eq!(settings.batch_size(), 1);
        assert_eq!(settings.coercion(), Some(BatchCoercion::NotABatch));

        let ordinary = MigrateSettings::new(20, 1_000, BatchGrouping::Strict, false, false, false);
        assert_eq!(ordinary.batch_size(), 20);
        assert_eq!(ordinary.coercion(), None);
        assert!(ordinary.is_batching());
        assert_eq!(ordinary.flush_threshold(), 200);
    }

    #[test]
    fn mig_021_the_counter_rule_is_reported_ahead_of_the_writetime_one() {
        // Both apply; the operator is told about the one that is not negotiable.
        let settings = MigrateSettings::new(10, 1_000, BatchGrouping::Strict, true, true, false);
        assert_eq!(settings.coercion(), Some(BatchCoercion::CounterTable));
        assert!(settings
            .coercion()
            .unwrap()
            .reason()
            .contains("counter table"));
    }

    #[test]
    fn mig_022_the_default_grouping_is_strict() {
        assert_eq!(MigrateSettings::default().grouping(), BatchGrouping::Strict);
        let legacy = MigrateSettings::new(5, 1_000, BatchGrouping::Legacy, false, false, false);
        assert_eq!(legacy.grouping(), BatchGrouping::Legacy);
    }

    #[test]
    fn mig_041_a_dry_run_is_off_by_default_and_visible_when_on() {
        assert!(!MigrateSettings::default().is_dry_run());
        let dry = MigrateSettings::new(5, 1_000, BatchGrouping::Strict, false, false, true);
        assert!(dry.is_dry_run());
        dry.log();
    }

    #[test]
    fn mig_021_settings_resolve_from_a_configuration() {
        let mut raw = cdm_config::model::CdmConfig::default();
        raw.perfops.batch_size = 7;
        raw.perfops.fetch_size = 50;
        let config = EffectiveConfig::resolve(raw);
        let settings = MigrateSettings::from_config(&config, false, false, false);
        assert_eq!(settings.batch_size(), 7);
        assert_eq!(settings.fetch_size(), 50);
        assert_eq!(settings.flush_threshold(), 50, "the page bounds the buffer");

        let counter = MigrateSettings::from_config(&config, true, false, false);
        assert_eq!(counter.batch_size(), 1);
    }
}
