//! The token-range planner (`TOK-001`..`TOK-010`).
//!
//! Planning is the first thing a run does and the last thing anyone can change their mind about:
//! the list of ranges it produces is the unit of scheduling, tracking, resume, leasing and
//! failure isolation (`ARCHITECTURE.md` §5.2, principle P5). It is also pure — given a
//! partitioner, a configuration and a [`RunId`], the plan is a deterministic function of its
//! inputs, with no I/O anywhere in this module.
//!
//! # The three strategies
//!
//! | `strategy` | What it does | Requirement |
//! |---|---|---|
//! | [`Fixed`](PlanStrategy::Fixed) | Reproduces Java CDM's splitter exactly. The default. | `TOK-003` |
//! | [`RingAware`](PlanStrategy::RingAware) | Splits along ring-ownership boundaries so each range maps to one replica set. | `TOK-008` |
//! | [`Adaptive`](PlanStrategy::Adaptive) | Starts from `Fixed` and subdivides ranges whose estimated row count exceeds `max_rows_per_range`. | `TOK-010` |
//!
//! `RingAware` and `Adaptive` need facts about the cluster, which arrive through the
//! [`ClusterTopology`] trait rather than a driver session — see [`topology`].
//!
//! # Layout
//!
//! * [`partitioner`] — detection and default bounds (`TOK-001`, `TOK-002`);
//! * [`split`] — the Java-parity splitter (`TOK-003`, `TOK-004`, `TOK-005`), including the two
//!   places where the Java source and `SPEC.md` §6's pseudocode disagree;
//! * [`shuffle`] — the run-seeded double shuffle (`TOK-006`, `TOK-007`);
//! * [`topology`] — the cluster-metadata trait and its in-memory implementation;
//! * [`report`] — what `cdm plan` prints (`TOK-009`, `NFR-003`).

pub mod partitioner;
pub mod report;
pub mod shuffle;
pub mod split;
pub mod topology;

use std::fmt;
use std::str::FromStr;

use cdm_config::types::TokenBound;
use cdm_config::CdmConfig;
use cdm_core::{CdmError, ErrorKind, RunId, TableRef, TokenRange};
use serde::{Deserialize, Serialize};

pub use partitioner::Partitioner;
pub use report::{MemoryEnvelope, PlanReport, SpanBucket};
pub use shuffle::shuffle_for_run;
pub use split::{split_ring, FALLBACK_PARTITION_SIZE, MAX_PLANNED_RANGES};
pub use topology::{ClusterTopology, InMemoryTopology, RingSegment, SizeEstimate};

/// How the ring is divided into ranges (`TOK-003`, `TOK-008`, `TOK-010`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum PlanStrategy {
    /// Java CDM's splitter, reproduced exactly. The default, and the only `[P]` strategy.
    #[default]
    Fixed,
    /// Split along ring-ownership boundaries, so every range maps to a single replica set and
    /// the reads for it can be routed with no coordinator hop (`TOK-008`).
    RingAware,
    /// Start from [`Fixed`](PlanStrategy::Fixed) and subdivide any range whose estimated row
    /// count exceeds `max_rows_per_range`, so a hot range does not become the straggler that
    /// sets the wall clock (`TOK-010`).
    Adaptive,
}

impl PlanStrategy {
    /// Every strategy, in declaration order.
    pub const ALL: [Self; 3] = [Self::Fixed, Self::RingAware, Self::Adaptive];

    /// The stable configuration spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::RingAware => "ring_aware",
            Self::Adaptive => "adaptive",
        }
    }

    /// Whether this strategy needs a [`ClusterTopology`] to plan.
    pub const fn needs_topology(self) -> bool {
        !matches!(self, Self::Fixed)
    }
}

impl FromStr for PlanStrategy {
    type Err = CdmError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalised = value.trim().to_ascii_lowercase().replace('-', "_");
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == normalised)
            .ok_or_else(|| {
                CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "unknown plan strategy `{value}`; expected one of fixed, ring_aware, \
                         adaptive"
                    ),
                )
                .with_context(|ctx| ctx.with_config_key("plan.strategy"))
            })
    }
}

impl fmt::Display for PlanStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything the planner needs from configuration.
///
/// Built with [`PlannerSettings::from_config`] in production and with the builder methods in
/// tests. The partitioner is a separate argument because it is *detected* from the origin
/// (`TOK-001`), not configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerSettings {
    /// The detected origin partitioner (`TOK-001`).
    pub partitioner: Partitioner,
    /// `perfops.num_parts` — how many ranges the ring is split into (`TOK-003`).
    pub num_parts: u64,
    /// `filter.token_coverage_percent` (`TOK-005`).
    pub coverage_percent: u8,
    /// `filter.token.min`, or the partitioner default (`TOK-002`).
    pub token_min: Option<i128>,
    /// `filter.token.max`, or the partitioner default (`TOK-002`).
    pub token_max: Option<i128>,
    /// Which splitter to use (`TOK-008`, `TOK-010`).
    pub strategy: PlanStrategy,
    /// The row count above which [`PlanStrategy::Adaptive`] subdivides a range (`TOK-010`).
    pub max_rows_per_range: u64,
    /// The rate limit `cdm plan` estimates the duration against (`TOK-009`), in rows per second.
    pub rate_limit_rows_per_second: u32,
    /// `perfops.max_inflight_reads`, for the memory envelope (`NFR-003`).
    pub max_inflight_reads: u32,
    /// `perfops.max_inflight_writes`, for the memory envelope (`NFR-003`).
    pub max_inflight_writes: u32,
    /// The table being planned, if known.
    ///
    /// `system.size_estimates` is per table, so both the adaptive strategy (`TOK-010`) and the
    /// row estimate `cdm plan` prints (`TOK-009`) need it. It is optional because a plan is
    /// perfectly computable without it — the geometry does not depend on the data.
    pub table: Option<TableRef>,
}

/// The default `max_rows_per_range` for [`PlanStrategy::Adaptive`].
///
/// A million rows is roughly a minute of work at the default rate limit — small enough that a
/// straggler cannot dominate a run, large enough that subdivision does not multiply the tracking
/// write rate.
pub const DEFAULT_MAX_ROWS_PER_RANGE: u64 = 1_000_000;

impl PlannerSettings {
    /// Settings for `partitioner` with cdm-rs's defaults everywhere else.
    pub const fn new(partitioner: Partitioner) -> Self {
        Self {
            partitioner,
            num_parts: 5_000,
            coverage_percent: 100,
            token_min: None,
            token_max: None,
            strategy: PlanStrategy::Fixed,
            max_rows_per_range: DEFAULT_MAX_ROWS_PER_RANGE,
            rate_limit_rows_per_second: 20_000,
            max_inflight_reads: 256,
            max_inflight_writes: 2_000,
            table: None,
        }
    }

    /// Reads the settings out of a validated configuration.
    ///
    /// The duration estimate uses the *lower* of the two rate limits, because a run cannot go
    /// faster than its slower side.
    ///
    /// `plan.strategy` and `plan.max_rows_per_range` are not yet part of the configuration model:
    /// `docs/TRACEABILITY.md` books `TOK-008` and `TOK-010` into PR #53, which adds the
    /// `SPEC.md` §3.5 rows the property registry is checked against. Until then the strategy is
    /// selected programmatically with [`PlannerSettings::with_strategy`] and this method always
    /// returns [`PlanStrategy::Fixed`], which is the specified default.
    pub fn from_config(config: &CdmConfig, partitioner: Partitioner) -> Self {
        Self {
            partitioner,
            num_parts: config.perfops.num_parts,
            coverage_percent: config.filter.token_coverage_percent,
            token_min: config.filter.token.min.map(TokenBound::get),
            token_max: config.filter.token.max.map(TokenBound::get),
            strategy: PlanStrategy::Fixed,
            max_rows_per_range: DEFAULT_MAX_ROWS_PER_RANGE,
            rate_limit_rows_per_second: config
                .perfops
                .ratelimit
                .origin
                .min(config.perfops.ratelimit.target),
            max_inflight_reads: config.perfops.max_inflight_reads,
            max_inflight_writes: config.perfops.max_inflight_writes,
            // Resolving `schema.origin.keyspace_table` into a `TableRef` is `cdm-service`'s job
            // (it owns identifier quoting and the origin/target fallback of `CFG-023`), so the
            // caller supplies it with `with_table`.
            table: None,
        }
    }

    /// Sets `perfops.num_parts`.
    #[must_use]
    pub const fn with_num_parts(mut self, num_parts: u64) -> Self {
        self.num_parts = num_parts;
        self
    }

    /// Sets `filter.token_coverage_percent`.
    #[must_use]
    pub const fn with_coverage_percent(mut self, coverage_percent: u8) -> Self {
        self.coverage_percent = coverage_percent;
        self
    }

    /// Sets `filter.token.min` and `filter.token.max`.
    #[must_use]
    pub const fn with_token_bounds(mut self, min: Option<i128>, max: Option<i128>) -> Self {
        self.token_min = min;
        self.token_max = max;
        self
    }

    /// Sets the planning strategy.
    #[must_use]
    pub const fn with_strategy(mut self, strategy: PlanStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Sets the adaptive subdivision threshold.
    #[must_use]
    pub const fn with_max_rows_per_range(mut self, max_rows_per_range: u64) -> Self {
        self.max_rows_per_range = max_rows_per_range;
        self
    }

    /// Sets the table whose `system.size_estimates` rows the planner may consult.
    #[must_use]
    pub fn with_table(mut self, table: TableRef) -> Self {
        self.table = Some(table);
        self
    }

    /// Sets the rate limit the duration estimate assumes.
    #[must_use]
    pub const fn with_rate_limit(mut self, rows_per_second: u32) -> Self {
        self.rate_limit_rows_per_second = rows_per_second;
        self
    }
}

/// One range of the plan, with the replicas that own it when the strategy knows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedRange {
    /// The tokens to read.
    pub range: TokenRange,
    /// The replicas holding them, or empty when the strategy did not consult the ring.
    pub replicas: Vec<String>,
}

impl PlannedRange {
    /// A range with no replica information.
    pub const fn unrouted(range: TokenRange) -> Self {
        Self {
            range,
            replicas: Vec::new(),
        }
    }
}

/// The immutable result of planning: the work list, in the order it will be scheduled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenPlan {
    partitioner: Partitioner,
    strategy: PlanStrategy,
    bounds: TokenRange,
    coverage_percent: u8,
    run_id: RunId,
    ranges: Vec<PlannedRange>,
}

impl TokenPlan {
    /// The ranges, in scheduling order (already shuffled — `TOK-006`).
    pub fn ranges(&self) -> &[PlannedRange] {
        &self.ranges
    }

    /// Just the token ranges, in scheduling order.
    pub fn token_ranges(&self) -> Vec<TokenRange> {
        self.ranges.iter().map(|planned| planned.range).collect()
    }

    /// The token ranges in ring order, which is the order `cdm plan` and the tracking table
    /// render them in.
    pub fn ring_ordered(&self) -> Vec<TokenRange> {
        let mut ordered = self.token_ranges();
        ordered.sort_unstable();
        ordered
    }

    /// How many ranges the plan holds.
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    /// Whether the plan is empty. It never is: the splitter always emits at least one range.
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// The detected origin partitioner.
    pub const fn partitioner(&self) -> Partitioner {
        self.partitioner
    }

    /// The strategy that produced this plan.
    pub const fn strategy(&self) -> PlanStrategy {
        self.strategy
    }

    /// The segment of the ring the plan covers.
    pub const fn bounds(&self) -> TokenRange {
        self.bounds
    }

    /// The coverage percentage applied, after Java's clamp (`TOK-005`).
    pub const fn coverage_percent(&self) -> u8 {
        self.coverage_percent
    }

    /// The run this plan was shuffled for (`TOK-007`).
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }
}

/// Builds token plans (`TOK-003`, `TOK-008`, `TOK-010`).
#[derive(Debug, Clone)]
pub struct Planner {
    settings: PlannerSettings,
}

impl Planner {
    /// A planner with the given settings.
    pub const fn new(settings: PlannerSettings) -> Self {
        Self { settings }
    }

    /// The settings this planner uses.
    pub const fn settings(&self) -> &PlannerSettings {
        &self.settings
    }

    /// Computes the plan for `run_id`.
    ///
    /// `topology` may be `None` for [`PlanStrategy::Fixed`], which needs nothing from the
    /// cluster; the other two strategies require it.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] for unusable bounds or an unusable `num_parts`, or when a
    /// strategy that needs cluster metadata is asked to plan without any. Propagates any error
    /// the topology reports.
    pub fn plan(
        &self,
        run_id: RunId,
        topology: Option<&dyn ClusterTopology>,
    ) -> Result<TokenPlan, CdmError> {
        let bounds = self
            .settings
            .partitioner
            .resolve_bounds(self.settings.token_min, self.settings.token_max)?;

        if self.settings.strategy.needs_topology() && topology.is_none() {
            return Err(CdmError::new(
                ErrorKind::Config,
                format!(
                    "plan strategy `{}` needs origin cluster metadata, which is not available; \
                     use `fixed` to plan without a cluster",
                    self.settings.strategy
                ),
            )
            .with_context(|ctx| ctx.with_config_key("plan.strategy")));
        }

        let mut ranges = match self.settings.strategy {
            PlanStrategy::Fixed => self.plan_fixed(bounds)?,
            PlanStrategy::RingAware => self.plan_ring_aware(bounds, topology)?,
            PlanStrategy::Adaptive => self.plan_adaptive(bounds, topology)?,
        };

        // TOK-006/TOK-007: the work list is shuffled, reproducibly for this run.
        shuffle_for_run(&mut ranges, run_id);

        Ok(TokenPlan {
            partitioner: self.settings.partitioner,
            strategy: self.settings.strategy,
            bounds,
            coverage_percent: effective_coverage(self.settings.coverage_percent),
            run_id,
            ranges,
        })
    }

    /// Builds the `cdm plan` report for a plan this planner produced (`TOK-009`).
    ///
    /// # Errors
    ///
    /// Propagates a failure to read `system.size_estimates`.
    pub fn report(
        &self,
        plan: &TokenPlan,
        topology: Option<&dyn ClusterTopology>,
    ) -> Result<PlanReport, CdmError> {
        PlanReport::build(plan, &self.settings, topology)
    }

    /// `TOK-003`: Java's splitter over the whole configured segment.
    fn plan_fixed(&self, bounds: TokenRange) -> Result<Vec<PlannedRange>, CdmError> {
        Ok(split_ring(
            bounds,
            self.settings.num_parts,
            self.settings.coverage_percent,
        )?
        .into_iter()
        .map(PlannedRange::unrouted)
        .collect())
    }

    /// `TOK-008`: one group of ranges per ring segment, so no range straddles a replica boundary.
    ///
    /// Each segment receives a share of `num_parts` proportional to its width, and never fewer
    /// than one. A segment that falls outside the configured bounds is skipped; a segment that
    /// straddles them is clipped, which is what keeps `filter.token.*` honoured.
    fn plan_ring_aware(
        &self,
        bounds: TokenRange,
        topology: Option<&dyn ClusterTopology>,
    ) -> Result<Vec<PlannedRange>, CdmError> {
        let Some(topology) = topology else {
            return self.plan_fixed(bounds);
        };
        let segments = topology.ring()?;
        if segments.is_empty() {
            return Err(CdmError::new(
                ErrorKind::Config,
                "the origin reported an empty token ring, so `ring_aware` planning is impossible",
            ));
        }

        let clipped: Vec<(TokenRange, &Vec<String>)> = segments
            .iter()
            .filter_map(|segment| {
                clip(segment.range, bounds).map(|range| (range, &segment.replicas))
            })
            .collect();
        if clipped.is_empty() {
            return Err(CdmError::new(
                ErrorKind::Config,
                format!("no ring segment overlaps the configured token bounds {bounds}"),
            ));
        }

        let total_tokens: u128 = clipped.iter().map(|(range, _)| range.token_count()).sum();
        let mut out = Vec::new();
        for (range, replicas) in clipped {
            let share = u128::from(self.settings.num_parts).saturating_mul(range.token_count())
                / total_tokens.max(1);
            let parts = u64::try_from(share).unwrap_or(u64::MAX).max(1);
            for piece in split_ring(range, parts, self.settings.coverage_percent)? {
                out.push(PlannedRange {
                    range: piece,
                    replicas: replicas.clone(),
                });
            }
        }
        Ok(out)
    }

    /// `TOK-010`: the fixed plan, with over-large ranges broken up.
    ///
    /// "Over-large" is measured against `system.size_estimates` at planning time. The observed
    /// row counts of a running job refine this further; that half of `TOK-010` belongs to the
    /// scheduler (PR #53) and is deliberately not attempted here.
    fn plan_adaptive(
        &self,
        bounds: TokenRange,
        topology: Option<&dyn ClusterTopology>,
    ) -> Result<Vec<PlannedRange>, CdmError> {
        let base = self.plan_fixed(bounds)?;
        let Some(topology) = topology else {
            return Ok(base);
        };
        if self.settings.max_rows_per_range == 0 {
            return Ok(base);
        }
        let Some(table) = self.settings.table.as_ref() else {
            return Ok(base);
        };
        let estimates = topology.size_estimates(table)?;
        if estimates.is_empty() {
            return Ok(base);
        }

        let mut out = Vec::new();
        for planned in base {
            let rows = estimated_rows_in(planned.range, &estimates);
            let factor = rows.div_ceil(u128::from(self.settings.max_rows_per_range));
            let factor = u32::try_from(factor).unwrap_or(u32::MAX).max(1);
            if factor == 1 {
                out.push(planned);
                continue;
            }
            for piece in planned.range.split(factor)? {
                out.push(PlannedRange {
                    range: piece,
                    replicas: planned.replicas.clone(),
                });
            }
        }
        Ok(out)
    }
}

/// Java's coverage clamp, mirrored here so [`TokenPlan::coverage_percent`] reports what was
/// actually applied rather than what was configured (`TOK-005`).
fn effective_coverage(configured: u8) -> u8 {
    if (1..=100).contains(&configured) {
        configured
    } else {
        100
    }
}

/// The part of `range` inside `bounds`, or `None` when they are disjoint.
fn clip(range: TokenRange, bounds: TokenRange) -> Option<TokenRange> {
    if !range.intersects(bounds) {
        return None;
    }
    TokenRange::new(range.min().max(bounds.min()), range.max().min(bounds.max())).ok()
}

/// Estimated rows inside `range`, prorated across the estimates that overlap it.
fn estimated_rows_in(range: TokenRange, estimates: &[SizeEstimate]) -> u128 {
    let mut total: u128 = 0;
    for estimate in estimates {
        if !estimate.range.intersects(range) {
            continue;
        }
        let estimate_tokens = estimate.range.token_count();
        if estimate_tokens == 0 {
            continue;
        }
        let Some(overlap) = clip(range, estimate.range).map(TokenRange::token_count) else {
            continue;
        };
        total = total.saturating_add(
            u128::from(estimate.partitions_count).saturating_mul(overlap) / estimate_tokens,
        );
    }
    total
}

/// Subdivides pending ranges for a rerun, exactly as Java does (`TRK-033`).
///
/// `track_run.rerun_multiplier > 1` breaks every pending range into that many sub-ranges at 100%
/// coverage, so that a straggler from the previous run is spread over several workers. Java runs
/// each range through `SplitPartitions.getRandomSubPartitions`, which means the *same* splitter
/// as `TOK-003` — including the `partition_size == 0 → 100_000` fallback, which is why a narrow
/// range comes back undivided rather than as `multiplier` one-token ranges — and shuffles each
/// range's pieces before appending them.
///
/// A multiplier of 0 or 1 returns the input unchanged, as in Java.
///
/// # Errors
///
/// Propagates a splitter failure; with a multiplier of at least 1 there is none.
pub fn subdivide_for_rerun(
    ranges: &[TokenRange],
    multiplier: u32,
    run_id: RunId,
) -> Result<Vec<TokenRange>, CdmError> {
    if multiplier <= 1 {
        return Ok(ranges.to_vec());
    }
    let mut out = Vec::with_capacity(
        ranges
            .len()
            .saturating_mul(usize::try_from(multiplier).unwrap_or(1)),
    );
    for range in ranges {
        let mut pieces = split_ring(*range, u64::from(multiplier), 100)?;
        shuffle_for_run(&mut pieces, run_id);
        out.append(&mut pieces);
    }
    Ok(out)
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
    use proptest::prelude::*;

    use super::*;

    /// `[a, b]`.
    fn range(min: i128, max: i128) -> TokenRange {
        TokenRange::new(min, max).unwrap()
    }

    /// A four-node Murmur3 ring, each node owning a quarter, with size estimates on one table.
    fn topology() -> InMemoryTopology {
        let quarters = TokenRange::MURMUR3_FULL.split(4).unwrap();
        let mut topology = InMemoryTopology::new(Partitioner::Murmur3);
        for (index, quarter) in quarters.iter().enumerate() {
            topology = topology.with_segment(RingSegment::new(
                *quarter,
                [format!("node-{index}"), format!("node-{}", (index + 1) % 4)],
            ));
        }
        topology
    }

    #[test]
    fn tok_003_the_default_strategy_is_javas_and_needs_no_cluster_metadata() {
        assert_eq!(PlanStrategy::default(), PlanStrategy::Fixed);
        assert!(!PlanStrategy::Fixed.needs_topology());
        assert!(PlanStrategy::RingAware.needs_topology());
        assert!(PlanStrategy::Adaptive.needs_topology());

        let settings = PlannerSettings::new(Partitioner::Murmur3).with_num_parts(16);
        let plan = Planner::new(settings)
            .plan(RunId::from_raw(7), None)
            .unwrap();

        assert_eq!(plan.len(), 16);
        assert!(!plan.is_empty());
        assert_eq!(plan.bounds(), TokenRange::MURMUR3_FULL);
        assert_eq!(plan.strategy(), PlanStrategy::Fixed);
        assert_eq!(plan.run_id(), RunId::from_raw(7));
        assert!(plan.ranges().iter().all(|r| r.replicas.is_empty()));

        // The geometry is exactly what the splitter produced, once put back in ring order.
        assert_eq!(
            plan.ring_ordered(),
            split_ring(TokenRange::MURMUR3_FULL, 16, 100).unwrap()
        );
    }

    #[test]
    fn tok_002_configured_bounds_narrow_the_plan() {
        let settings = PlannerSettings::new(Partitioner::Murmur3)
            .with_num_parts(4)
            .with_token_bounds(Some(-100), Some(99));
        let plan = Planner::new(settings)
            .plan(RunId::from_raw(1), None)
            .unwrap();

        assert_eq!(plan.bounds(), range(-100, 99));
        assert_eq!(
            plan.ring_ordered(),
            vec![
                range(-100, -51),
                range(-50, -1),
                range(0, 49),
                range(50, 99),
            ]
        );
    }

    #[test]
    fn tok_006_the_plan_is_shuffled_and_tok_007_reproducibly_so() {
        let settings = PlannerSettings::new(Partitioner::Murmur3).with_num_parts(256);
        let planner = Planner::new(settings);

        let first = planner.plan(RunId::from_raw(4_242), None).unwrap();
        let again = planner.plan(RunId::from_raw(4_242), None).unwrap();
        let other = planner.plan(RunId::from_raw(4_243), None).unwrap();

        assert_eq!(first.token_ranges(), again.token_ranges());
        assert_ne!(first.token_ranges(), other.token_ranges());
        // Shuffled, therefore not in ring order — but covering exactly the same ring.
        assert_ne!(first.token_ranges(), first.ring_ordered());
        assert_eq!(first.ring_ordered(), other.ring_ordered());
    }

    #[test]
    fn tok_005_the_plan_records_the_coverage_actually_applied() {
        let sampled = Planner::new(
            PlannerSettings::new(Partitioner::Murmur3)
                .with_num_parts(8)
                .with_coverage_percent(10),
        )
        .plan(RunId::from_raw(1), None)
        .unwrap();
        assert_eq!(sampled.coverage_percent(), 10);

        // Java clamps a nonsense value to 100, and the plan says so rather than echoing input.
        let clamped = Planner::new(
            PlannerSettings::new(Partitioner::Murmur3)
                .with_num_parts(8)
                .with_coverage_percent(0),
        )
        .plan(RunId::from_raw(1), None)
        .unwrap();
        assert_eq!(clamped.coverage_percent(), 100);
    }

    #[test]
    fn tok_008_ring_aware_ranges_never_straddle_a_replica_boundary() {
        let topology = topology();
        let settings = PlannerSettings::new(Partitioner::Murmur3)
            .with_num_parts(40)
            .with_strategy(PlanStrategy::RingAware);
        let plan = Planner::new(settings)
            .plan(RunId::from_raw(3), Some(&topology))
            .unwrap();

        assert_eq!(plan.strategy(), PlanStrategy::RingAware);
        assert_eq!(plan.len(), 40);
        // Every planned range is inside exactly one ring segment, and carries its replicas.
        for planned in plan.ranges() {
            assert_eq!(planned.replicas.len(), 2);
            let owners: Vec<_> = topology
                .ring()
                .unwrap()
                .into_iter()
                .filter(|segment| segment.range.contains_range(planned.range))
                .collect();
            assert_eq!(owners.len(), 1, "{} straddles a boundary", planned.range);
            assert_eq!(owners[0].replicas, planned.replicas);
        }
        // And the plan still covers the whole ring.
        let ordered = plan.ring_ordered();
        assert_eq!(ordered[0].min(), i128::from(i64::MIN));
        assert_eq!(ordered[ordered.len() - 1].max(), i128::from(i64::MAX));
    }

    #[test]
    fn tok_008_ring_aware_clips_segments_to_the_configured_bounds() {
        let topology = topology();
        let settings = PlannerSettings::new(Partitioner::Murmur3)
            .with_num_parts(8)
            .with_token_bounds(Some(-10), Some(10))
            .with_strategy(PlanStrategy::RingAware);
        let plan = Planner::new(settings)
            .plan(RunId::from_raw(3), Some(&topology))
            .unwrap();

        let ordered = plan.ring_ordered();
        assert_eq!(ordered[0].min(), -10);
        assert_eq!(ordered[ordered.len() - 1].max(), 10);
        assert!(ordered.iter().all(|r| range(-10, 10).contains_range(*r)));
    }

    #[test]
    fn tok_008_a_strategy_needing_metadata_says_so_instead_of_planning_blind() {
        let settings =
            PlannerSettings::new(Partitioner::Murmur3).with_strategy(PlanStrategy::RingAware);
        let err = Planner::new(settings)
            .plan(RunId::from_raw(1), None)
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert!(err.message().contains("ring_aware"));

        let empty = InMemoryTopology::new(Partitioner::Murmur3);
        let settings =
            PlannerSettings::new(Partitioner::Murmur3).with_strategy(PlanStrategy::RingAware);
        let err = Planner::new(settings)
            .plan(RunId::from_raw(1), Some(&empty))
            .unwrap_err();
        assert!(err.message().contains("empty token ring"));

        let broken = topology().failing_ring("system.peers unreadable");
        let settings =
            PlannerSettings::new(Partitioner::Murmur3).with_strategy(PlanStrategy::RingAware);
        let err = Planner::new(settings)
            .plan(RunId::from_raw(1), Some(&broken))
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Read);
    }

    #[test]
    fn tok_008_ring_aware_rejects_bounds_that_no_segment_covers() {
        let topology = InMemoryTopology::new(Partitioner::Murmur3)
            .with_segment(RingSegment::new(range(0, 100), ["node-1"]));
        let settings = PlannerSettings::new(Partitioner::Murmur3)
            .with_token_bounds(Some(1_000), Some(2_000))
            .with_strategy(PlanStrategy::RingAware);
        let err = Planner::new(settings)
            .plan(RunId::from_raw(1), Some(&topology))
            .unwrap_err();
        assert!(err.message().contains("no ring segment"));
    }

    #[test]
    fn tok_010_adaptive_subdivides_only_the_ranges_that_are_too_big() {
        let table = TableRef::new("ks", "orders");
        // The first eighth of the ring holds ten times the rows of the rest.
        let eighths = TokenRange::MURMUR3_FULL.split(8).unwrap();
        let topology = InMemoryTopology::new(Partitioner::Murmur3)
            .with_estimate(table.clone(), SizeEstimate::new(eighths[0], 1_000_000, 200))
            .with_estimate(
                table.clone(),
                SizeEstimate::new(
                    TokenRange::new(eighths[1].min(), i128::from(i64::MAX)).unwrap(),
                    70_000,
                    200,
                ),
            );

        let settings = PlannerSettings::new(Partitioner::Murmur3)
            .with_num_parts(8)
            .with_strategy(PlanStrategy::Adaptive)
            .with_max_rows_per_range(50_000)
            .with_table(table);
        let plan = Planner::new(settings)
            .plan(RunId::from_raw(5), Some(&topology))
            .unwrap();

        // The hot eighth (1_000_000 rows / 50_000) becomes 20 ranges; the other seven hold
        // 10_000 rows each and stay whole.
        assert_eq!(plan.len(), 20 + 7);
        let ordered = plan.ring_ordered();
        assert_eq!(ordered[0].min(), i128::from(i64::MIN));
        assert_eq!(ordered[ordered.len() - 1].max(), i128::from(i64::MAX));
        for pair in ordered.windows(2) {
            assert_eq!(pair[0].max() + 1, pair[1].min());
        }
    }

    #[test]
    fn tok_010_adaptive_falls_back_to_the_fixed_plan_without_estimates() {
        let bare = InMemoryTopology::new(Partitioner::Murmur3);
        let settings = PlannerSettings::new(Partitioner::Murmur3)
            .with_num_parts(12)
            .with_strategy(PlanStrategy::Adaptive)
            .with_table(TableRef::new("ks", "orders"));
        let plan = Planner::new(settings)
            .plan(RunId::from_raw(5), Some(&bare))
            .unwrap();
        assert_eq!(plan.len(), 12);
        assert_eq!(
            plan.ring_ordered(),
            split_ring(TokenRange::MURMUR3_FULL, 12, 100).unwrap()
        );
    }

    #[test]
    fn tok_008_strategies_parse_from_their_configuration_spelling() {
        assert_eq!(
            PlanStrategy::from_str("fixed").unwrap(),
            PlanStrategy::Fixed
        );
        assert_eq!(
            PlanStrategy::from_str(" RING-AWARE ").unwrap(),
            PlanStrategy::RingAware
        );
        assert_eq!(
            PlanStrategy::from_str("adaptive").unwrap(),
            PlanStrategy::Adaptive
        );
        let err = PlanStrategy::from_str("ring_awear").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        assert_eq!(err.context().config_key.as_deref(), Some("plan.strategy"));
        assert_eq!(PlanStrategy::RingAware.to_string(), "ring_aware");
    }

    #[test]
    fn tok_003_settings_are_read_from_the_configuration_model() {
        let mut config = CdmConfig::default();
        config.perfops.num_parts = 64;
        config.filter.token_coverage_percent = 25;
        config.perfops.ratelimit.origin = 5_000;
        config.perfops.ratelimit.target = 9_000;

        let settings = PlannerSettings::from_config(&config, Partitioner::Murmur3);
        assert_eq!(settings.num_parts, 64);
        assert_eq!(settings.coverage_percent, 25);
        // The slower side sets the pace.
        assert_eq!(settings.rate_limit_rows_per_second, 5_000);
        assert_eq!(settings.strategy, PlanStrategy::Fixed);
        assert_eq!(settings.token_min, None);
        assert_eq!(
            settings.max_inflight_reads,
            config.perfops.max_inflight_reads
        );

        let plan = Planner::new(settings)
            .plan(RunId::from_raw(2), None)
            .unwrap();
        assert_eq!(plan.len(), 64);
        assert_eq!(plan.coverage_percent(), 25);
    }

    #[test]
    fn tok_009_the_report_is_reachable_from_the_planner() {
        let table = TableRef::new("ks", "orders");
        let topology = InMemoryTopology::new(Partitioner::Murmur3).with_estimate(
            table.clone(),
            SizeEstimate::new(TokenRange::MURMUR3_FULL, 400_000, 128),
        );
        let planner = Planner::new(
            PlannerSettings::new(Partitioner::Murmur3)
                .with_num_parts(32)
                .with_rate_limit(20_000)
                .with_table(table),
        );
        let plan = planner.plan(RunId::from_raw(9), Some(&topology)).unwrap();
        let report = planner.report(&plan, Some(&topology)).unwrap();

        assert_eq!(report.range_count, 32);
        assert_eq!(report.estimated_rows, Some(400_000));
        assert_eq!(report.estimated_duration.unwrap().as_secs(), 20);
    }

    #[test]
    fn trk_033_a_rerun_multiplier_subdivides_each_pending_range_at_full_coverage() {
        let pending = vec![range(0, 999), range(10_000, 19_999)];
        let subdivided = subdivide_for_rerun(&pending, 4, RunId::from_raw(77)).unwrap();

        assert_eq!(subdivided.len(), 8);
        let mut ordered = subdivided.clone();
        ordered.sort_unstable();
        assert_eq!(
            ordered,
            vec![
                range(0, 249),
                range(250, 499),
                range(500, 749),
                range(750, 999),
                range(10_000, 12_499),
                range(12_500, 14_999),
                range(15_000, 17_499),
                range(17_500, 19_999),
            ]
        );
        // Deterministic for a run id, as the plan itself is.
        assert_eq!(
            subdivided,
            subdivide_for_rerun(&pending, 4, RunId::from_raw(77)).unwrap()
        );
    }

    #[test]
    fn trk_033_a_multiplier_of_one_or_zero_leaves_the_pending_ranges_alone() {
        let pending = vec![range(0, 999), range(10_000, 19_999)];
        assert_eq!(
            subdivide_for_rerun(&pending, 1, RunId::from_raw(1)).unwrap(),
            pending
        );
        assert_eq!(
            subdivide_for_rerun(&pending, 0, RunId::from_raw(1)).unwrap(),
            pending
        );
        assert!(subdivide_for_rerun(&[], 8, RunId::from_raw(1))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn trk_033_a_range_narrower_than_the_multiplier_comes_back_whole_as_in_java() {
        // `partition_size` truncates to zero and Java's 100_000 fallback swallows the whole
        // range: the multiplier cannot manufacture sub-ranges that do not exist.
        let pending = vec![range(0, 2)];
        assert_eq!(
            subdivide_for_rerun(&pending, 10, RunId::from_raw(1)).unwrap(),
            pending
        );
    }

    proptest! {
        /// `TST-010`: for any part count and any bounds, a full-coverage plan covers the
        /// requested segment exactly once, whichever order the shuffle put it in.
        #[test]
        fn tst_010_tok_003_a_plan_covers_the_configured_segment_exactly(
            min in -100_000_i128..100_000,
            width in 0_i128..10_000_000,
            num_parts in 1_u64..1_000,
            run_id in any::<i64>(),
        ) {
            let settings = PlannerSettings::new(Partitioner::Murmur3)
                .with_num_parts(num_parts)
                .with_token_bounds(Some(min), Some(min + width));
            let plan = Planner::new(settings)
                .plan(RunId::from_raw(run_id), None)
                .unwrap();

            let ordered = plan.ring_ordered();
            prop_assert_eq!(ordered[0].min(), min);
            prop_assert_eq!(ordered[ordered.len() - 1].max(), min + width);
            for pair in ordered.windows(2) {
                prop_assert_eq!(pair[0].max() + 1, pair[1].min());
            }
            let covered: u128 = ordered.iter().map(|r| r.token_count()).sum();
            prop_assert_eq!(covered, plan.bounds().token_count());
        }
    }
}
