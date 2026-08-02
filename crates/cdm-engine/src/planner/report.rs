//! What `cdm plan` prints (`TOK-009`) and the memory envelope it must state (`NFR-003`).
//!
//! The report is computed from a [`TokenPlan`] and, optionally, the origin's
//! [`ClusterTopology`]. It touches no data: `cdm plan` is the command an operator runs *before*
//! committing to a migration window, so everything here is derived from metadata and arithmetic.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use cdm_core::{CdmError, TokenRange};
use serde::{Deserialize, Serialize};

use super::partitioner::Partitioner;
use super::topology::ClusterTopology;
use super::{PlanStrategy, PlannerSettings, TokenPlan};

/// The baseline resident set `NFR-003` allows before per-request buffers are counted.
pub const BASELINE_RSS_BYTES: u64 = 200 * 1024 * 1024;

/// The row size assumed when `system.size_estimates` has nothing to say.
///
/// A fresh table, a table never compacted, or a cluster that denies access to `system` all
/// produce no estimate. One kilobyte is deliberately unremarkable: the envelope is a budget to
/// plan against, and it is printed with the assumption spelled out.
pub const ASSUMED_ROW_BYTES: u64 = 1024;

/// One bucket of the span histogram: how many ranges hold between `2^k` and `2^(k+1) - 1` tokens.
///
/// A migration's wall clock is set by its widest ranges, so the shape of this histogram is the
/// single most useful number in `cdm plan`: a plan whose top bucket holds three ranges will end
/// with three workers busy and the rest idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanBucket {
    /// Base-2 logarithm of the bucket's lower bound.
    pub log2_tokens: u32,
    /// Smallest token count in the bucket.
    pub min_tokens: u128,
    /// Largest token count in the bucket.
    pub max_tokens: u128,
    /// How many planned ranges fall in it.
    pub range_count: usize,
}

/// The `NFR-003` steady-state memory budget, with every term named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryEnvelope {
    /// The fixed part of the budget.
    pub baseline_bytes: u64,
    /// `perfops.max_inflight_reads`.
    pub max_inflight_reads: u32,
    /// `perfops.max_inflight_writes`.
    pub max_inflight_writes: u32,
    /// The average row size the estimate uses, in bytes.
    pub average_row_bytes: u64,
    /// Whether `average_row_bytes` came from `system.size_estimates` or from
    /// [`ASSUMED_ROW_BYTES`].
    pub average_row_bytes_measured: bool,
    /// `baseline + (reads + writes) × average_row_bytes × 2`.
    pub ceiling_bytes: u64,
}

impl MemoryEnvelope {
    /// Computes the envelope of `NFR-003`.
    ///
    /// The `× 2` is the read buffer plus the write buffer a row occupies as it crosses the
    /// pipeline. Saturating arithmetic keeps a nonsensical configuration from wrapping the
    /// budget to a small number, which would be worse than reporting an implausible one.
    pub fn compute(
        max_inflight_reads: u32,
        max_inflight_writes: u32,
        average_row_bytes: Option<u64>,
    ) -> Self {
        let measured = average_row_bytes.is_some();
        let row_bytes = average_row_bytes.unwrap_or(ASSUMED_ROW_BYTES);
        let inflight = u64::from(max_inflight_reads).saturating_add(u64::from(max_inflight_writes));
        let ceiling =
            BASELINE_RSS_BYTES.saturating_add(inflight.saturating_mul(row_bytes).saturating_mul(2));
        Self {
            baseline_bytes: BASELINE_RSS_BYTES,
            max_inflight_reads,
            max_inflight_writes,
            average_row_bytes: row_bytes,
            average_row_bytes_measured: measured,
            ceiling_bytes: ceiling,
        }
    }
}

/// Everything `cdm plan` prints, and everything `POST /v1/plan` returns (`TOK-009`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanReport {
    /// The detected origin partitioner.
    pub partitioner: Partitioner,
    /// The strategy that produced the plan.
    pub strategy: PlanStrategy,
    /// The segment of the ring the plan covers.
    pub bounds: TokenRange,
    /// How many ranges the plan contains.
    pub range_count: usize,
    /// The configured `filter.token_coverage_percent`, after Java's clamp.
    pub coverage_percent: u8,
    /// Tokens the plan actually reads, after coverage sampling.
    pub planned_tokens: u128,
    /// Tokens in the requested segment.
    pub segment_tokens: u128,
    /// The distribution of range widths.
    pub span_histogram: Vec<SpanBucket>,
    /// Estimated rows in the planned ranges, or `None` when the cluster has no estimate.
    pub estimated_rows: Option<u64>,
    /// How long the run should take at the configured rate limit, or `None` without an estimate.
    pub estimated_duration: Option<Duration>,
    /// The rate limit the duration assumes, in rows per second.
    pub rate_limit_rows_per_second: u32,
    /// The `NFR-003` memory budget.
    pub memory_envelope: MemoryEnvelope,
}

impl PlanReport {
    /// Builds the report for `plan`, consulting `topology` for size estimates when both it and
    /// [`PlannerSettings::table`] are known.
    ///
    /// # Errors
    ///
    /// Propagates a failure to read `system.size_estimates`. A *missing* estimate is not a
    /// failure: the row and duration fields are simply `None`.
    pub fn build(
        plan: &TokenPlan,
        settings: &PlannerSettings,
        topology: Option<&dyn ClusterTopology>,
    ) -> Result<Self, CdmError> {
        let ranges: Vec<TokenRange> = plan.token_ranges();
        let estimates = match (settings.table.as_ref(), topology) {
            (Some(table), Some(topology)) => topology.size_estimates(table)?,
            _ => Vec::new(),
        };

        let estimated_rows = estimate_rows(&ranges, &estimates);
        let average_row_bytes = average_row_bytes(&estimates);
        let rate = settings.rate_limit_rows_per_second;
        let estimated_duration = estimated_rows.and_then(|rows| {
            (rate > 0).then(|| Duration::from_secs(rows.div_ceil(u64::from(rate))))
        });

        Ok(Self {
            partitioner: plan.partitioner(),
            strategy: plan.strategy(),
            bounds: plan.bounds(),
            range_count: ranges.len(),
            coverage_percent: plan.coverage_percent(),
            planned_tokens: ranges.iter().map(|r| r.token_count()).sum(),
            segment_tokens: plan.bounds().token_count(),
            span_histogram: span_histogram(&ranges),
            estimated_rows,
            estimated_duration,
            rate_limit_rows_per_second: rate,
            memory_envelope: MemoryEnvelope::compute(
                settings.max_inflight_reads,
                settings.max_inflight_writes,
                average_row_bytes,
            ),
        })
    }
}

/// Buckets the ranges by the base-2 logarithm of their token count.
fn span_histogram(ranges: &[TokenRange]) -> Vec<SpanBucket> {
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for range in ranges {
        // `token_count` is at least 1, so `ilog2` is always defined.
        let bucket = range.token_count().max(1).ilog2();
        *counts.entry(bucket).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(log2_tokens, range_count)| SpanBucket {
            log2_tokens,
            min_tokens: 1_u128 << log2_tokens,
            max_tokens: (1_u128 << log2_tokens).saturating_mul(2).saturating_sub(1),
            range_count,
        })
        .collect()
}

/// Estimated rows in `ranges`, prorated over the estimate rows that overlap them.
///
/// `system.size_estimates` reports *partitions*, not rows. For the overwhelmingly common
/// single-row-per-partition table the two coincide; for a wide-partition table this is a lower
/// bound, and `cdm plan` says so. Java CDM offers no estimate at all.
fn estimate_rows(
    ranges: &[TokenRange],
    estimates: &[super::topology::SizeEstimate],
) -> Option<u64> {
    if estimates.is_empty() {
        return None;
    }
    let mut total: u128 = 0;
    for estimate in estimates {
        let estimate_tokens = estimate.range.token_count();
        if estimate_tokens == 0 {
            continue;
        }
        let overlap: u128 = ranges
            .iter()
            .filter_map(|range| overlap_tokens(*range, estimate.range))
            .sum();
        total = total.saturating_add(
            u128::from(estimate.partitions_count).saturating_mul(overlap) / estimate_tokens,
        );
    }
    Some(u64::try_from(total).unwrap_or(u64::MAX))
}

/// How many tokens two ranges share, or `None` when they are disjoint.
fn overlap_tokens(left: TokenRange, right: TokenRange) -> Option<u128> {
    if !left.intersects(right) {
        return None;
    }
    let min = left.min().max(right.min());
    let max = left.max().min(right.max());
    TokenRange::new(min, max).ok().map(TokenRange::token_count)
}

/// The partition-count-weighted mean partition size, or `None` when nothing is known.
fn average_row_bytes(estimates: &[super::topology::SizeEstimate]) -> Option<u64> {
    let mut partitions: u128 = 0;
    let mut bytes: u128 = 0;
    for estimate in estimates {
        partitions = partitions.saturating_add(u128::from(estimate.partitions_count));
        bytes = bytes.saturating_add(
            u128::from(estimate.partitions_count)
                .saturating_mul(u128::from(estimate.mean_partition_size)),
        );
    }
    if partitions == 0 {
        return None;
    }
    u64::try_from(bytes / partitions).ok()
}

impl fmt::Display for PlanReport {
    /// Renders the report the way `cdm plan` prints it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Partitioner : {}", self.partitioner)?;
        writeln!(f, "Strategy    : {}", self.strategy)?;
        writeln!(f, "Ring segment: {}", self.bounds)?;
        writeln!(f, "Ranges      : {}", self.range_count)?;
        writeln!(
            f,
            "Coverage    : {}% ({} of {} tokens)",
            self.coverage_percent, self.planned_tokens, self.segment_tokens
        )?;
        writeln!(f, "Span histogram (tokens per range):")?;
        for bucket in &self.span_histogram {
            writeln!(
                f,
                "  2^{:<3} ..= {:<42} {}",
                bucket.log2_tokens, bucket.max_tokens, bucket.range_count
            )?;
        }
        match self.estimated_rows {
            Some(rows) => writeln!(f, "Estimated rows: {rows} (from system.size_estimates)")?,
            None => writeln!(f, "Estimated rows: unknown (no system.size_estimates rows)")?,
        }
        match self.estimated_duration {
            Some(duration) => writeln!(
                f,
                "Estimated time: {}s at {} rows/s",
                duration.as_secs(),
                self.rate_limit_rows_per_second
            )?,
            None => writeln!(f, "Estimated time: unknown")?,
        }
        let envelope = self.memory_envelope;
        write!(
            f,
            "Memory ceiling: {} MB = 200 MB + ({} + {}) x {} B x 2{}",
            envelope.ceiling_bytes / (1024 * 1024),
            envelope.max_inflight_reads,
            envelope.max_inflight_writes,
            envelope.average_row_bytes,
            if envelope.average_row_bytes_measured {
                ""
            } else {
                " (assumed row size)"
            }
        )
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
    use cdm_core::{RunId, TableRef};

    use super::super::topology::{InMemoryTopology, SizeEstimate};
    use super::super::Planner;
    use super::*;

    fn settings() -> PlannerSettings {
        PlannerSettings::new(Partitioner::Murmur3)
            .with_num_parts(8)
            .with_token_bounds(Some(0), Some(799))
    }

    fn plan_of(settings: &PlannerSettings) -> TokenPlan {
        Planner::new(settings.clone())
            .plan(RunId::from_raw(11), None)
            .unwrap()
    }

    #[test]
    fn tok_009_the_report_states_the_range_count_bounds_and_span_histogram() {
        let settings = settings();
        let plan = plan_of(&settings);
        let report = PlanReport::build(&plan, &settings, None).unwrap();

        assert_eq!(report.range_count, 8);
        assert_eq!(report.bounds, TokenRange::new(0, 799).unwrap());
        assert_eq!(report.partitioner, Partitioner::Murmur3);
        assert_eq!(report.planned_tokens, 800);
        assert_eq!(report.segment_tokens, 800);
        // Eight ranges of exactly 100 tokens, so exactly one histogram bucket.
        let counted: usize = report.span_histogram.iter().map(|b| b.range_count).sum();
        assert_eq!(counted, 8);
        assert!(report
            .span_histogram
            .iter()
            .all(|bucket| bucket.min_tokens <= bucket.max_tokens));
    }

    #[test]
    fn tok_009_estimated_rows_and_duration_come_from_size_estimates_and_the_rate_limit() {
        let table = TableRef::new("ks", "orders");
        let topology = InMemoryTopology::new(Partitioner::Murmur3).with_estimate(
            table.clone(),
            SizeEstimate::new(TokenRange::new(0, 799).unwrap(), 8_000, 512),
        );
        let settings = settings().with_rate_limit(1_000).with_table(table);
        let plan = plan_of(&settings);

        let report = PlanReport::build(&plan, &settings, Some(&topology)).unwrap();
        assert_eq!(report.estimated_rows, Some(8_000));
        assert_eq!(report.estimated_duration, Some(Duration::from_secs(8)));
        assert_eq!(report.rate_limit_rows_per_second, 1_000);
        assert!(report.memory_envelope.average_row_bytes_measured);
        assert_eq!(report.memory_envelope.average_row_bytes, 512);
    }

    #[test]
    fn tok_009_sampling_scales_the_row_estimate_down() {
        let table = TableRef::new("ks", "orders");
        let topology = InMemoryTopology::new(Partitioner::Murmur3).with_estimate(
            table.clone(),
            SizeEstimate::new(TokenRange::new(0, 799).unwrap(), 8_000, 512),
        );
        let settings = settings().with_coverage_percent(50).with_table(table);
        let plan = plan_of(&settings);

        let report = PlanReport::build(&plan, &settings, Some(&topology)).unwrap();
        // Half of each range is read, so roughly half the rows are.
        let rows = report.estimated_rows.unwrap();
        assert!((3_900..=4_100).contains(&rows), "{rows}");
        assert!(report.planned_tokens < report.segment_tokens);
    }

    #[test]
    fn tok_009_an_absent_estimate_is_reported_as_unknown_rather_than_guessed() {
        let settings = settings();
        let plan = plan_of(&settings);
        let report = PlanReport::build(&plan, &settings, None).unwrap();
        assert_eq!(report.estimated_rows, None);
        assert_eq!(report.estimated_duration, None);
        assert!(!report.memory_envelope.average_row_bytes_measured);
        assert_eq!(report.memory_envelope.average_row_bytes, ASSUMED_ROW_BYTES);
        assert!(report.to_string().contains("unknown"));
    }

    #[test]
    fn nfr_003_the_memory_envelope_is_the_formula_of_the_requirement() {
        let envelope = MemoryEnvelope::compute(256, 2_000, Some(1_000));
        assert_eq!(envelope.baseline_bytes, 200 * 1024 * 1024);
        assert_eq!(
            envelope.ceiling_bytes,
            200 * 1024 * 1024 + (256 + 2_000) * 1_000 * 2
        );
        assert!(envelope.average_row_bytes_measured);

        // An absurd configuration saturates instead of wrapping to a small, reassuring number.
        let absurd = MemoryEnvelope::compute(u32::MAX, u32::MAX, Some(u64::MAX));
        assert_eq!(absurd.ceiling_bytes, u64::MAX);
    }

    #[test]
    fn nfr_003_the_rendered_report_states_the_memory_envelope() {
        let settings = settings();
        let plan = plan_of(&settings);
        let rendered = PlanReport::build(&plan, &settings, None)
            .unwrap()
            .to_string();
        assert!(rendered.contains("Memory ceiling"));
        assert!(rendered.contains("200 MB +"));
        assert!(rendered.contains("Span histogram"));
        assert!(rendered.contains("Ranges      : 8"));
    }

    #[test]
    fn tok_009_the_report_round_trips_through_json_for_the_api() {
        let settings = settings();
        let plan = plan_of(&settings);
        let report = PlanReport::build(&plan, &settings, None).unwrap();
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(serde_json::from_str::<PlanReport>(&json).unwrap(), report);
    }
}
