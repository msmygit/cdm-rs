//! The filter chain — everything that decides a row is not this run's business
//! (`FEA-050`..`FEA-054`).
//!
//! # Three places a row can be excluded
//!
//! Filtering happens at three different depths, and conflating them is how a migration silently
//! skips data:
//!
//! * **In the ring.** `filter.token.min`/`.max` narrow the *planned* token ranges, so excluded
//!   partitions are never read at all (`FEA-053`, `TOK-002`).
//! * **In the origin `SELECT`.** `filter.cql_where` is appended to the range query, so the cluster
//!   does the filtering (`FEA-050`).
//! * **Per row.** The writetime window and the column-value filter examine a row cdm-rs has already
//!   read (`FEA-051`, `FEA-052`), and a rejection counts as `SKIPPED`, never as an error.
//!
//! [`FilterChain`] composes the third kind. It evaluates in declaration order and short-circuits
//! (`FEA-054`), which makes the cheap predicates worth declaring first and makes a third-party
//! filter (`PLG-003`) indistinguishable from a built-in one.

use std::sync::Arc;

use cdm_core::{
    CdmError, Diagnostic, EffectiveConfig, ErrorKind, FilterPlugin, Plugin, Record, TokenRange,
};

use crate::properties::{
    self, FILTER_COLUMN_NAME, FILTER_COLUMN_VALUE, FILTER_CQL_WHERE, FILTER_TOKEN_MAX,
    FILTER_TOKEN_MIN, FILTER_WRITETIME_MAX, FILTER_WRITETIME_MIN,
};
use crate::schema::TableFacts;
use crate::writetime::WritetimeTtlPlan;
use crate::{diagnostic, PROVIDER};

/// The user's CQL predicate, appended to the origin range select (`FEA-050`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CqlWhereFilter {
    condition: String,
}

impl CqlWhereFilter {
    /// Reads `filter.cql_where`.
    pub fn load(config: &EffectiveConfig) -> Self {
        Self {
            condition: properties::trimmed(config, FILTER_CQL_WHERE).unwrap_or_default(),
        }
    }

    /// Whether a condition is configured.
    pub fn is_enabled(&self) -> bool {
        !self.condition.is_empty()
    }

    /// The condition as configured.
    pub fn condition(&self) -> &str {
        &self.condition
    }

    /// The fragment to append to the origin select's `WHERE` clause, `AND` included (`FEA-050`).
    ///
    /// The conjunction is supplied unless the operator already wrote one. Java tests that with
    /// `toUpperCase().startsWith("AND")`, which also matches a condition that merely *starts* with
    /// those three letters — `android_id = 1` loses its conjunction and the statement fails to
    /// parse. Requiring a word boundary fixes that; see `docs/MIGRATION_FROM_JAVA.md`.
    pub fn fragment(&self) -> String {
        if self.condition.is_empty() {
            return String::new();
        }
        if starts_with_and(&self.condition) {
            format!(" {}", self.condition)
        } else {
            format!(" AND {}", self.condition)
        }
    }

    /// Appends the fragment to a `WHERE` clause.
    pub fn append_to(&self, where_clause: &str) -> String {
        format!("{where_clause}{}", self.fragment())
    }

    /// Validates the condition (`FEA-050`).
    pub fn validate(&self) -> Vec<Diagnostic> {
        // The predicate is CQL the cluster parses, so there is nothing to check here beyond it not
        // being whitespace — which `trimmed` has already collapsed to "unset".
        Vec::new()
    }
}

/// Whether a condition already opens with the `AND` keyword rather than merely with those letters.
fn starts_with_and(condition: &str) -> bool {
    let mut rest = condition.trim_start();
    if !rest
        .get(..3)
        .is_some_and(|head| head.eq_ignore_ascii_case("and"))
    {
        return false;
    }
    rest = rest.get(3..).unwrap_or_default();
    rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == '(')
}

/// The writetime window (`FEA-051`).
#[derive(Debug, Clone)]
pub struct WritetimeFilter {
    min: i64,
    max: i64,
    plan: WritetimeTtlPlan,
}

impl WritetimeFilter {
    /// Reads `filter.writetime.min`/`.max` and binds them to a resolved TTL/writetime plan.
    ///
    /// Both bounds must be positive and ordered for the filter to engage, matching Java: a
    /// half-configured window silently passing everything is safer than one that silently drops
    /// everything, and `CFG-032` rejects the half-configured case at validation time anyway.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if either bound does not parse.
    pub fn load(config: &EffectiveConfig, plan: WritetimeTtlPlan) -> Result<Self, CdmError> {
        Ok(Self {
            min: properties::integer(config, FILTER_WRITETIME_MIN)?.unwrap_or_default(),
            max: properties::integer(config, FILTER_WRITETIME_MAX)?.unwrap_or_default(),
            plan,
        })
    }

    /// Whether the window is configured well enough to engage.
    pub const fn is_enabled(&self) -> bool {
        self.min > 0 && self.max > 0 && self.max > self.min
    }

    /// The window, in microseconds.
    pub const fn window(&self) -> (i64, i64) {
        (self.min, self.max)
    }

    /// Validates the window against the resolved plan (`FEA-051`, `CFG-032`, `CFG-034`).
    pub fn validate(&self) -> Vec<Diagnostic> {
        let mut findings = Vec::new();
        if self.min < 0 || self.max < 0 {
            findings.push(
                diagnostic::config_error("a writetime filter bound must not be negative")
                    .with_rule("FEA-051"),
            );
        }
        if self.min > 0 && self.max > 0 && self.max <= self.min {
            findings.push(
                diagnostic::config_error(format!(
                    "writetime filter maximum ({}) must be greater than its minimum ({})",
                    self.max, self.min
                ))
                .with_rule("FEA-051"),
            );
        }
        if self.is_enabled() && !self.plan.has_writetime() {
            findings.push(
                diagnostic::config_error(
                    "a writetime filter is configured but no writetime column is resolvable",
                )
                .with_rule("FEA-051")
                .with_suggestion("name the columns in `schema.origin.writetime.names`"),
            );
        }
        findings
    }
}

impl Plugin for WritetimeFilter {
    fn name(&self) -> &'static str {
        "writetime-window"
    }

    fn provider(&self) -> &'static str {
        PROVIDER
    }
}

impl FilterPlugin for WritetimeFilter {
    /// Rejects rows outside the window (`FEA-051`).
    ///
    /// A row with no resolvable writetime is *kept*, as in Java. It is the conservative choice: the
    /// filter exists to narrow a re-run to recently changed data, and silently dropping rows whose
    /// writetime the origin cannot report would leave the target permanently short of them.
    fn accepts(&self, record: &Record) -> Result<bool, CdmError> {
        if !self.is_enabled() {
            return Ok(true);
        }
        let Some(writetime) = self.plan.writetime(record.origin())? else {
            return Ok(true);
        };
        Ok(writetime >= self.min && writetime <= self.max)
    }
}

/// The column-value filter (`FEA-052`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnValueFilter {
    column: String,
    value: String,
    index: Option<usize>,
}

impl ColumnValueFilter {
    /// Reads `filter.column.name`/`.value` and resolves the column's position in the projection.
    pub fn load(config: &EffectiveConfig, origin: &TableFacts) -> Self {
        let column = properties::trimmed(config, FILTER_COLUMN_NAME).unwrap_or_default();
        Self {
            index: origin.index_of(&column),
            column,
            value: properties::trimmed(config, FILTER_COLUMN_VALUE).unwrap_or_default(),
        }
    }

    /// Whether both the column and the value are configured, and the column exists.
    pub fn is_enabled(&self) -> bool {
        self.index.is_some() && !self.value.is_empty()
    }

    /// Validates the configuration (`FEA-052`).
    pub fn validate(&self, origin: &TableFacts) -> Vec<Diagnostic> {
        if self.column.is_empty() && self.value.is_empty() {
            return Vec::new();
        }
        let mut findings = Vec::new();
        if self.column.is_empty() || self.value.is_empty() {
            findings.push(
                diagnostic::config_error(
                    "the column filter needs both `filter.column.name` and `filter.column.value`",
                )
                .with_rule("FEA-052"),
            );
        }
        if !self.column.is_empty() && origin.column(&self.column).is_none() {
            findings.push(
                diagnostic::schema_error(format!(
                    "filter column `{}` is not on the origin table {}",
                    self.column,
                    origin.table()
                ))
                .with_rule("FEA-052"),
            );
        }
        findings
    }
}

impl Plugin for ColumnValueFilter {
    fn name(&self) -> &'static str {
        "column-value"
    }

    fn provider(&self) -> &'static str {
        PROVIDER
    }
}

impl FilterPlugin for ColumnValueFilter {
    /// Rejects rows whose named text column equals the configured value (`FEA-052`).
    ///
    /// The comparison trims and ignores case, as Java's does. A cell that is null, or that is not
    /// text at all, never matches — the filter is defined over a text column, and treating an
    /// undecodable cell as a match would drop rows on the strength of a type error.
    fn accepts(&self, record: &Record) -> Result<bool, CdmError> {
        let Some(index) = self.index else {
            return Ok(true);
        };
        if !self.is_enabled() {
            return Ok(true);
        }
        let cell = record.origin_cell(index)?;
        let Some(bytes) = cell.bytes() else {
            return Ok(true);
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Ok(true);
        };
        Ok(!text.trim().eq_ignore_ascii_case(&self.value))
    }
}

/// The token bounds a run is restricted to (`FEA-053`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBounds {
    min: Option<i128>,
    max: Option<i128>,
}

impl TokenBounds {
    /// Reads `filter.token.min`/`.max`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if either bound does not parse. They are 128-bit because the
    /// Random partitioner's ring is 127 bits wide (`TOK-002`).
    pub fn load(config: &EffectiveConfig) -> Result<Self, CdmError> {
        Ok(Self {
            min: properties::integer_128(config, FILTER_TOKEN_MIN)?,
            max: properties::integer_128(config, FILTER_TOKEN_MAX)?,
        })
    }

    /// Whether either bound is configured.
    pub const fn is_enabled(&self) -> bool {
        self.min.is_some() || self.max.is_some()
    }

    /// Narrows a planned range to the configured bounds, or drops it entirely (`FEA-053`).
    ///
    /// Clamping the plan rather than filtering rows is the whole point: a partition outside the
    /// bounds is never read, so the filter costs nothing at runtime.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if the bounds are inverted, which would otherwise silently plan
    /// an empty run.
    pub fn clamp(&self, range: TokenRange) -> Result<Option<TokenRange>, CdmError> {
        let min = self.min.unwrap_or(range.min()).max(range.min());
        let max = self.max.unwrap_or(range.max()).min(range.max());
        if let (Some(low), Some(high)) = (self.min, self.max) {
            if low > high {
                return Err(CdmError::new(
                    ErrorKind::Config,
                    format!(
                        "`{}` ({low}) is above `{}` ({high})",
                        FILTER_TOKEN_MIN.canonical(),
                        FILTER_TOKEN_MAX.canonical()
                    ),
                )
                .with_context(|c| c.with_config_key(FILTER_TOKEN_MIN.canonical())));
            }
        }
        if min > max {
            return Ok(None);
        }
        TokenRange::new(min, max).map(Some)
    }
}

/// An ordered, short-circuiting chain of row-level predicates (`FEA-054`).
#[derive(Clone, Default)]
pub struct FilterChain {
    filters: Vec<Arc<dyn FilterPlugin>>,
}

impl std::fmt::Debug for FilterChain {
    /// Names the filters rather than their state: a filter's state may include a configured value,
    /// and `SEC-002` keeps configured values out of debug output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilterChain")
            .field("filters", &self.names())
            .finish()
    }
}

impl FilterChain {
    /// An empty chain, which accepts everything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a filter. Declaration order is evaluation order.
    #[must_use]
    pub fn with(mut self, filter: Arc<dyn FilterPlugin>) -> Self {
        self.filters.push(filter);
        self
    }

    /// Appends a filter only if it is switched on, which keeps a disabled filter off the hot path
    /// entirely rather than paying for a predicate that always says yes.
    #[must_use]
    pub fn with_enabled(self, enabled: bool, filter: Arc<dyn FilterPlugin>) -> Self {
        if enabled {
            self.with(filter)
        } else {
            self
        }
    }

    /// The registered filters' names, in evaluation order.
    pub fn names(&self) -> Vec<&'static str> {
        self.filters.iter().map(|f| f.name()).collect()
    }

    /// How many filters are registered.
    pub fn len(&self) -> usize {
        self.filters.len()
    }

    /// Whether the chain accepts everything.
    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }

    /// Whether the record should be processed (`FEA-054`).
    ///
    /// Evaluation stops at the first rejection, so a later filter never sees a record an earlier one
    /// already excluded. A `false` result is a `SKIPPED` record, not an error.
    ///
    /// # Errors
    ///
    /// Propagates the first filter error, which the engine counts as a record-level `ERROR`.
    pub fn accepts(&self, record: &Record) -> Result<bool, CdmError> {
        for filter in &self.filters {
            if !filter.accepts(record)? {
                return Ok(false);
            }
        }
        Ok(true)
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
    use crate::schema::table_view;
    use crate::writetime::WritetimeTtl;
    use cdm_core::{PrimaryKey, RawCell, Row, TableRef};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
        pairs.iter().copied().collect()
    }

    fn origin() -> TableFacts {
        TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "src"),
                &[("id", "int"), ("status", "text"), ("v", "text")],
            ),
            &["id"],
        )
        .unwrap()
    }

    fn record(cells: Vec<RawCell>) -> Record {
        Record::new(PrimaryKey::default(), Row::new(cells))
    }

    #[test]
    fn fea_050_the_condition_is_appended_with_a_conjunction_unless_it_supplies_one() {
        let filter = CqlWhereFilter::load(&config(&[("filter.cql_where", "status = 'a'")]));
        assert!(filter.is_enabled());
        assert_eq!(filter.condition(), "status = 'a'");
        assert_eq!(filter.fragment(), " AND status = 'a'");
        assert_eq!(
            filter.append_to("TOKEN(id) >= ? AND TOKEN(id) <= ?"),
            "TOKEN(id) >= ? AND TOKEN(id) <= ? AND status = 'a'"
        );
        assert!(filter.validate().is_empty());

        let explicit = CqlWhereFilter::load(&config(&[(
            "spark.cdm.filter.cassandra.whereCondition",
            "and status = 'a'",
        )]));
        assert_eq!(explicit.fragment(), " and status = 'a'");

        let off = CqlWhereFilter::load(&EffectiveConfig::new());
        assert!(!off.is_enabled());
        assert_eq!(off.fragment(), "");
        assert_eq!(off.append_to("x"), "x");
    }

    #[test]
    fn fea_050_a_condition_that_merely_starts_with_the_letters_and_still_gets_a_conjunction() {
        // Java's `startsWith("AND")` mangles this into invalid CQL.
        let filter = CqlWhereFilter::load(&config(&[("filter.cql_where", "android_id = 1")]));
        assert_eq!(filter.fragment(), " AND android_id = 1");
        assert!(starts_with_and("AND(x = 1)"));
        assert!(starts_with_and("and"));
        assert!(!starts_with_and("andy = 1"));
    }

    fn writetime_filter(pairs: &[(&str, &str)]) -> WritetimeFilter {
        let config = config(pairs);
        let plan = WritetimeTtl::load(&config)
            .unwrap()
            .resolve(&origin())
            .unwrap();
        WritetimeFilter::load(&config, plan).unwrap()
    }

    #[test]
    fn fea_051_rows_outside_the_writetime_window_are_skipped() {
        let filter = writetime_filter(&[
            ("schema.origin.writetime.names", "status"),
            ("schema.origin.ttl.automatic", "false"),
            ("filter.writetime.min", "100"),
            ("filter.writetime.max", "200"),
        ]);
        assert!(filter.is_enabled());
        assert_eq!(filter.window(), (100, 200));
        assert!(filter.validate().is_empty());

        let inside = record(vec![
            RawCell::NULL,
            RawCell::NULL,
            RawCell::NULL,
            RawCell::new(150_i64.to_be_bytes().to_vec()),
        ]);
        assert!(filter.accepts(&inside).unwrap());

        let before = record(vec![
            RawCell::NULL,
            RawCell::NULL,
            RawCell::NULL,
            RawCell::new(99_i64.to_be_bytes().to_vec()),
        ]);
        assert!(!filter.accepts(&before).unwrap());

        let after = record(vec![
            RawCell::NULL,
            RawCell::NULL,
            RawCell::NULL,
            RawCell::new(201_i64.to_be_bytes().to_vec()),
        ]);
        assert!(!filter.accepts(&after).unwrap());
    }

    #[test]
    fn fea_051_a_row_with_no_resolvable_writetime_is_kept() {
        let filter = writetime_filter(&[
            ("schema.origin.writetime.names", "status"),
            ("schema.origin.ttl.automatic", "false"),
            ("filter.writetime.min", "100"),
            ("filter.writetime.max", "200"),
        ]);
        let unknown = record(vec![
            RawCell::NULL,
            RawCell::NULL,
            RawCell::NULL,
            RawCell::NULL,
        ]);
        assert!(filter.accepts(&unknown).unwrap());
    }

    #[test]
    fn fea_051_a_half_or_inverted_window_does_not_engage_and_is_reported() {
        let half = writetime_filter(&[
            ("schema.origin.writetime.names", "status"),
            ("schema.origin.ttl.automatic", "false"),
            ("filter.writetime.min", "100"),
        ]);
        assert!(!half.is_enabled());
        assert!(half.accepts(&record(vec![RawCell::NULL])).unwrap());

        let inverted = writetime_filter(&[
            ("schema.origin.writetime.names", "status"),
            ("schema.origin.ttl.automatic", "false"),
            ("filter.writetime.min", "200"),
            ("filter.writetime.max", "100"),
        ]);
        assert_eq!(inverted.validate().len(), 1);

        let negative = writetime_filter(&[
            ("schema.origin.writetime.names", "status"),
            ("schema.origin.ttl.automatic", "false"),
            ("filter.writetime.min", "-1"),
            ("filter.writetime.max", "-2"),
        ]);
        assert_eq!(negative.validate().len(), 1);

        let unresolvable = writetime_filter(&[
            ("schema.origin.ttl.automatic", "false"),
            ("schema.origin.writetime.automatic", "false"),
            ("filter.writetime.min", "100"),
            ("filter.writetime.max", "200"),
        ]);
        let findings = unresolvable.validate();
        assert_eq!(findings.len(), 1);
        assert!(findings[0].title.contains("no writetime column"));
        assert_eq!(Plugin::name(&unresolvable), "writetime-window");
        assert_eq!(Plugin::provider(&unresolvable), PROVIDER);
    }

    #[test]
    fn fea_052_rows_whose_column_equals_the_value_are_skipped_case_insensitively() {
        let filter = ColumnValueFilter::load(
            &config(&[
                ("filter.column.name", "status"),
                ("filter.column.value", "deleted"),
            ]),
            &origin(),
        );
        assert!(filter.is_enabled());
        assert!(filter.validate(&origin()).is_empty());

        let matching = record(vec![RawCell::NULL, RawCell::new(b" DELETED ".to_vec())]);
        assert!(!filter.accepts(&matching).unwrap());

        let other = record(vec![RawCell::NULL, RawCell::new(b"live".to_vec())]);
        assert!(filter.accepts(&other).unwrap());

        let null = record(vec![RawCell::NULL, RawCell::NULL]);
        assert!(filter.accepts(&null).unwrap());

        let binary = record(vec![RawCell::NULL, RawCell::new(vec![0xff, 0xfe])]);
        assert!(filter.accepts(&binary).unwrap());
    }

    #[test]
    fn fea_052_a_half_configured_or_unknown_filter_column_is_reported() {
        let half = ColumnValueFilter::load(&config(&[("filter.column.name", "status")]), &origin());
        assert!(!half.is_enabled());
        assert_eq!(half.validate(&origin()).len(), 1);
        assert!(half
            .accepts(&record(vec![RawCell::NULL, RawCell::NULL]))
            .unwrap());

        let unknown = ColumnValueFilter::load(
            &config(&[
                ("spark.cdm.filter.java.column.name", "nope"),
                ("spark.cdm.filter.java.column.value", "x"),
            ]),
            &origin(),
        );
        assert!(!unknown.is_enabled());
        let findings = unknown.validate(&origin());
        assert_eq!(findings.len(), 1);
        assert!(findings[0].title.contains("not on the origin table"));
        assert!(unknown.accepts(&record(vec![RawCell::NULL])).unwrap());
        assert_eq!(Plugin::name(&unknown), "column-value");

        let off = ColumnValueFilter::load(&EffectiveConfig::new(), &origin());
        assert!(off.validate(&origin()).is_empty());
    }

    #[test]
    fn fea_053_token_bounds_clamp_the_planned_ring_segment() {
        let bounds = TokenBounds::load(&config(&[
            ("filter.token.min", "-100"),
            ("filter.token.max", "100"),
        ]))
        .unwrap();
        assert!(bounds.is_enabled());

        let clamped = bounds
            .clamp(TokenRange::new(-1000, 0).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!((clamped.min(), clamped.max()), (-100, 0));

        let inside = bounds
            .clamp(TokenRange::new(-10, 10).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!((inside.min(), inside.max()), (-10, 10));

        assert!(bounds
            .clamp(TokenRange::new(1000, 2000).unwrap())
            .unwrap()
            .is_none());

        let unbounded = TokenBounds::load(&EffectiveConfig::new()).unwrap();
        assert!(!unbounded.is_enabled());
        let range = TokenRange::new(-5, 5).unwrap();
        assert_eq!(unbounded.clamp(range).unwrap(), Some(range));
    }

    #[test]
    fn fea_053_inverted_token_bounds_are_a_config_error() {
        let bounds = TokenBounds::load(&config(&[
            ("filter.token.min", "100"),
            ("filter.token.max", "-100"),
        ]))
        .unwrap();
        let error = bounds
            .clamp(TokenRange::new(-1000, 1000).unwrap())
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Config);
        assert!(TokenBounds::load(&config(&[("filter.token.min", "x")])).is_err());
    }

    #[derive(Debug)]
    struct Counting {
        verdict: bool,
        calls: Arc<AtomicUsize>,
    }

    impl Plugin for Counting {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn provider(&self) -> &'static str {
            "test"
        }
    }

    impl FilterPlugin for Counting {
        fn accepts(&self, _record: &Record) -> Result<bool, CdmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.verdict)
        }
    }

    #[test]
    fn fea_054_the_chain_evaluates_in_order_and_short_circuits() {
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let chain = FilterChain::new()
            .with(Arc::new(Counting {
                verdict: false,
                calls: Arc::clone(&first),
            }))
            .with(Arc::new(Counting {
                verdict: true,
                calls: Arc::clone(&second),
            }));

        assert!(!chain.accepts(&record(vec![RawCell::NULL])).unwrap());
        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(
            second.load(Ordering::SeqCst),
            0,
            "a rejected record must not reach the next filter"
        );
        assert_eq!(chain.len(), 2);
        assert_eq!(chain.names(), ["counting", "counting"]);
        assert!(format!("{chain:?}").contains("counting"));
    }

    #[test]
    fn fea_054_an_empty_chain_accepts_everything_and_disabled_filters_are_not_registered() {
        let chain = FilterChain::new();
        assert!(chain.is_empty());
        assert!(chain.accepts(&record(vec![RawCell::NULL])).unwrap());

        let calls = Arc::new(AtomicUsize::new(0));
        let chain = FilterChain::new()
            .with_enabled(
                false,
                Arc::new(Counting {
                    verdict: false,
                    calls: Arc::clone(&calls),
                }),
            )
            .with_enabled(
                true,
                Arc::new(Counting {
                    verdict: true,
                    calls: Arc::clone(&calls),
                }),
            );
        assert_eq!(chain.len(), 1);
        assert!(chain.accepts(&record(vec![RawCell::NULL])).unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fea_054_a_filter_error_propagates_rather_than_silently_skipping_the_row() {
        #[derive(Debug)]
        struct Failing;
        impl Plugin for Failing {
            fn name(&self) -> &'static str {
                "failing"
            }
            fn provider(&self) -> &'static str {
                "test"
            }
        }
        impl FilterPlugin for Failing {
            fn accepts(&self, _record: &Record) -> Result<bool, CdmError> {
                Err(CdmError::new(ErrorKind::Internal, "boom"))
            }
        }

        let chain = FilterChain::new().with(Arc::new(Failing));
        assert_eq!(
            chain
                .accepts(&record(vec![RawCell::NULL]))
                .unwrap_err()
                .kind(),
            ErrorKind::Internal
        );
    }
}
