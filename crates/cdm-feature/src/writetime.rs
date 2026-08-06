//! TTL and writetime — carrying the origin's cell metadata to the target (`FEA-040`..`FEA-046`).
//!
//! # Why a row has one writetime
//!
//! Cassandra records a writetime and a TTL per *cell*, but `USING TIMESTAMP`/`USING TTL` applies to
//! a whole statement. Something therefore has to collapse a row's cells into one value, and both
//! Java CDM and cdm-rs take the **maximum**: writing every cell at the newest cell's timestamp
//! preserves "this row is newer than that one" comparisons across the migration, which is what makes
//! a re-run idempotent and last-write-wins reconciliation behave.
//!
//! # What is eligible
//!
//! `WRITETIME()` and `TTL()` are rejected outright for key columns, and are only meaningful for
//! primitives, tuples and frozen values. An unfrozen collection returns a *list* of per-element
//! values, which is expensive, so it participates only when `schema.ttl_writetime.use_collections`
//! says so (`FEA-041`, `FEA-043`).
//!
//! # Java parity
//!
//! `com.datastax.cdm.feature.WritetimeTTL` is reproduced including the details that look like
//! oversights but are load-bearing: `transform.custom_writetime` overrides the computed value
//! *without* adding the increment, an explicitly named column list turns automatic mode off for that
//! dimension alone (`FEA-042`), and a TTL column set that resolves to no values yields `0` rather
//! than "absent". One divergence, documented in `docs/MIGRATION_FROM_JAVA.md`: Java decides that a
//! counter table disables the feature by looking at the *origin* only, while `CFG-036` phrases the
//! same rule in terms of the target; cdm-rs rejects the combination if either side is a counter
//! table, since neither can accept a TTL or a timestamp on write.

use std::fmt;

use cdm_core::{CdmError, Diagnostic, EffectiveConfig, ErrorKind, Row};

use crate::diagnostic;
use crate::properties::{
    self, ORIGIN_TTL_AUTOMATIC, ORIGIN_TTL_NAMES, ORIGIN_WRITETIME_AUTOMATIC,
    ORIGIN_WRITETIME_NAMES, TRANSFORM_CUSTOM_TTL, TRANSFORM_CUSTOM_WRITETIME,
    TRANSFORM_CUSTOM_WRITETIME_INCREMENT, TTL_WRITETIME_USE_COLLECTIONS,
};
use crate::schema::{FeatureSchema, TableFacts};
use crate::wire::list_elements;

/// Whether a statement carries `USING TTL` and/or `USING TIMESTAMP` (`FEA-046`).
///
/// The decision is made once per run, not per row: the statement text is prepared once, so a row
/// whose writetime turns out to be absent binds nothing rather than needing a different statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsingClause {
    /// Whether `USING TTL ?` is emitted.
    pub ttl: bool,
    /// Whether `USING TIMESTAMP ?` is emitted.
    pub timestamp: bool,
}

impl UsingClause {
    /// Whether the clause contributes anything at all.
    pub const fn is_empty(&self) -> bool {
        !self.ttl && !self.timestamp
    }
}

impl fmt::Display for UsingClause {
    /// Renders the clause exactly as Java's `TargetUpsertStatement.usingTTLTimestamp` does, leading
    /// space included, so a generated statement is byte-identical to the one operators have seen in
    /// their logs for years (`MET-005` applies the same reasoning to metric strings).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.ttl, self.timestamp) {
            (false, false) => Ok(()),
            (true, false) => f.write_str(" USING TTL ?"),
            (false, true) => f.write_str(" USING TIMESTAMP ?"),
            (true, true) => f.write_str(" USING TTL ? AND TIMESTAMP ?"),
        }
    }
}

/// The TTL/writetime feature's configuration (`FEA-040`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritetimeTtl {
    automatic_ttl: bool,
    ttl_names: Vec<String>,
    automatic_writetime: bool,
    writetime_names: Vec<String>,
    use_collections: bool,
    custom_writetime: i64,
    writetime_increment: i64,
    custom_ttl: i64,
}

impl WritetimeTtl {
    /// Reads the feature's configuration (`FEA-042`, `FEA-044`).
    ///
    /// Naming columns explicitly turns automatic mode off for *that dimension only*, which is why
    /// the two dimensions are tracked separately: a run may take its TTL from one named column and
    /// its writetime from every eligible column.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::Config`] if a numeric property does not parse.
    pub fn load(config: &EffectiveConfig) -> Result<Self, CdmError> {
        let ttl_names = properties::list(config, ORIGIN_TTL_NAMES);
        let writetime_names = properties::list(config, ORIGIN_WRITETIME_NAMES);
        Ok(Self {
            automatic_ttl: ttl_names.is_empty()
                && properties::boolean(config, ORIGIN_TTL_AUTOMATIC, true),
            ttl_names,
            automatic_writetime: writetime_names.is_empty()
                && properties::boolean(config, ORIGIN_WRITETIME_AUTOMATIC, true),
            writetime_names,
            use_collections: properties::boolean(config, TTL_WRITETIME_USE_COLLECTIONS, false),
            custom_writetime: properties::integer(config, TRANSFORM_CUSTOM_WRITETIME)?
                .unwrap_or_default(),
            writetime_increment: properties::integer(config, TRANSFORM_CUSTOM_WRITETIME_INCREMENT)?
                .unwrap_or_default(),
            custom_ttl: properties::integer(config, TRANSFORM_CUSTOM_TTL)?.unwrap_or_default(),
        })
    }

    /// Whether anything asks for a TTL or a writetime.
    pub fn is_enabled(&self) -> bool {
        self.automatic_ttl
            || self.automatic_writetime
            || !self.ttl_names.is_empty()
            || !self.writetime_names.is_empty()
            || self.custom_writetime > 0
            || self.custom_ttl > 0
    }

    /// Whether unfrozen collections participate (`FEA-041`).
    pub const fn uses_collections(&self) -> bool {
        self.use_collections
    }

    /// The configured writetime override, in microseconds; `0` means "compute it" (`FEA-044`).
    pub const fn custom_writetime(&self) -> i64 {
        self.custom_writetime
    }

    /// The configured TTL override, in seconds; `0` means "compute it" (`FEA-044`).
    pub const fn custom_ttl(&self) -> i64 {
        self.custom_ttl
    }

    /// The origin columns whose `TTL()` is selected (`FEA-042`).
    pub fn ttl_columns<'a>(&'a self, origin: &'a TableFacts) -> Vec<&'a str> {
        if self.automatic_ttl {
            origin.ttl_writetime_columns(self.use_collections)
        } else {
            self.ttl_names.iter().map(String::as_str).collect()
        }
    }

    /// The origin columns whose `WRITETIME()` is selected (`FEA-042`).
    pub fn writetime_columns<'a>(&'a self, origin: &'a TableFacts) -> Vec<&'a str> {
        if self.automatic_writetime {
            origin.ttl_writetime_columns(self.use_collections)
        } else {
            self.writetime_names.iter().map(String::as_str).collect()
        }
    }

    /// Validates the configuration against the schema (`FEA-041`, `FEA-042`, `FEA-045`).
    pub fn validate(&self, schema: &FeatureSchema) -> Vec<Diagnostic> {
        let mut findings = Vec::new();
        if schema.origin.is_counter_table() || schema.target.is_counter_table() {
            if self.is_enabled() {
                findings.push(
                    diagnostic::config_error(
                        "a counter table cannot be written with a TTL or a timestamp",
                    )
                    .with_rule("FEA-045")
                    .with_suggestion(
                        "unset the TTL/writetime properties, or set \
                         `schema.origin.ttl.automatic` and `schema.origin.writetime.automatic` to \
                         false",
                    ),
                );
            }
            return findings;
        }

        if self.writetime_increment < 0 {
            findings.push(
                diagnostic::config_error(format!(
                    "`{}` must be zero or greater",
                    TRANSFORM_CUSTOM_WRITETIME_INCREMENT.canonical()
                ))
                .with_rule("FEA-040"),
            );
        }
        if self.custom_writetime < 0 || self.custom_ttl < 0 {
            findings.push(
                diagnostic::config_warning(
                    "a negative custom writetime or TTL is out of range and is treated as unset",
                )
                .with_rule("FEA-044"),
            );
        }

        for (dimension, names) in [
            ("TTL", &self.ttl_names),
            ("writetime", &self.writetime_names),
        ] {
            for name in names {
                match schema.origin.column(name) {
                    None => findings.push(
                        diagnostic::schema_error(format!(
                            "{dimension} column `{name}` is not on the origin table {}",
                            schema.origin.table()
                        ))
                        .with_rule("FEA-042"),
                    ),
                    Some(column) if !column.can_carry_ttl_or_writetime(self.use_collections) => {
                        findings.push(
                            diagnostic::schema_error(format!(
                                "{dimension} column `{name}` ({}) cannot provide a {dimension}",
                                column.cql_type()
                            ))
                            .with_rule("FEA-041")
                            .with_suggestion(if column.is_key() {
                                "key columns have no per-cell metadata; name a regular column"
                            } else {
                                "set `schema.ttl_writetime.use_collections` to include unfrozen \
                                 collections"
                            }),
                        );
                    }
                    Some(_) => {}
                }
            }
        }
        findings
    }

    /// Resolves the projection this feature adds and the positions its values land in.
    ///
    /// The `TTL(col)` expressions are appended first and the `WRITETIME(col)` expressions after
    /// them, both following the table's own columns — the same order Java extends its column list
    /// in, so a projection built by either tool has the same shape.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::SchemaMismatch`] if a named column is not on the origin table.
    pub fn resolve(&self, origin: &TableFacts) -> Result<WritetimeTtlPlan, CdmError> {
        if origin.is_counter_table() {
            return Ok(WritetimeTtlPlan::disabled());
        }
        let base = origin.columns().len();
        let mut projection = Vec::new();
        let mut ttl = Vec::new();
        let mut writetime = Vec::new();

        for name in self.ttl_columns(origin) {
            ttl.push(SelectedColumn {
                index: base + projection.len(),
                collection: !yields_scalar(origin, name)?,
            });
            projection.push(format!("TTL({name})"));
        }
        for name in self.writetime_columns(origin) {
            writetime.push(SelectedColumn {
                index: base + projection.len(),
                collection: !yields_scalar(origin, name)?,
            });
            projection.push(format!("WRITETIME({name})"));
        }

        Ok(WritetimeTtlPlan {
            projection,
            ttl,
            writetime,
            custom_writetime: self.custom_writetime.max(0),
            writetime_increment: self.writetime_increment.max(0),
            custom_ttl: self.custom_ttl.max(0),
        })
    }
}

/// Whether the named column yields a scalar rather than a list of per-element values (`FEA-043`).
fn yields_scalar(origin: &TableFacts, name: &str) -> Result<bool, CdmError> {
    let column = origin.column(name).ok_or_else(|| {
        CdmError::new(
            ErrorKind::SchemaMismatch,
            format!(
                "TTL/writetime column `{name}` is not on the origin table {}",
                origin.table()
            ),
        )
        .with_context(|c| c.with_column(name.to_owned()))
    })?;
    Ok(!column.cql_type().is_collection() || column.cql_type().is_frozen())
}

/// One selected `TTL()`/`WRITETIME()` expression and where its value lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SelectedColumn {
    index: usize,
    collection: bool,
}

/// The resolved feature: a projection, the positions it produces and the overrides (`FEA-040`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WritetimeTtlPlan {
    projection: Vec<String>,
    ttl: Vec<SelectedColumn>,
    writetime: Vec<SelectedColumn>,
    custom_writetime: i64,
    writetime_increment: i64,
    custom_ttl: i64,
}

impl WritetimeTtlPlan {
    /// A plan that selects nothing, which is what a counter table gets (`FEA-045`).
    pub fn disabled() -> Self {
        Self::default()
    }

    /// The expressions this feature adds to the origin projection, in order.
    pub fn projection(&self) -> &[String] {
        &self.projection
    }

    /// Whether the plan resolves a writetime for every row.
    pub fn has_writetime(&self) -> bool {
        self.custom_writetime > 0 || !self.writetime.is_empty()
    }

    /// Whether the plan resolves a TTL for every row.
    pub fn has_ttl(&self) -> bool {
        self.custom_ttl > 0 || !self.ttl.is_empty()
    }

    /// What the target statement's `USING` clause must contain (`FEA-046`).
    ///
    /// When nothing is resolvable the clause is empty and the server assigns the timestamp, which is
    /// the correct behaviour rather than a fallback: binding a synthetic timestamp would silently
    /// reorder writes against anything else touching the target.
    pub fn using_clause(&self) -> UsingClause {
        UsingClause {
            ttl: self.has_ttl(),
            timestamp: self.has_writetime(),
        }
    }

    /// The row's writetime, in microseconds (`FEA-040`, `FEA-043`, `FEA-044`).
    ///
    /// The maximum over the selected columns plus `transform.custom_writetime_increment`; a
    /// configured `transform.custom_writetime` replaces the computation entirely and is *not*
    /// incremented, matching Java. `None` means no writetime is resolvable for this row, which
    /// `FEA-046` turns into an omitted `USING TIMESTAMP`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] if a selected cell is not a `bigint` or a list of them.
    pub fn writetime(&self, row: &Row) -> Result<Option<i64>, CdmError> {
        if self.custom_writetime > 0 {
            return Ok(Some(self.custom_writetime));
        }
        if self.writetime.is_empty() {
            return Ok(None);
        }
        let maximum = Self::maximum(row, &self.writetime, read_i64)?;
        Ok(maximum.map(|value| value.saturating_add(self.writetime_increment)))
    }

    /// The row's TTL, in seconds (`FEA-040`, `FEA-043`, `FEA-044`).
    ///
    /// The maximum over the selected columns, or `0` when the columns are selected but every cell is
    /// null — a row with no TTL anywhere must be written *without* one, and `0` is how Cassandra
    /// spells that. `None` means no TTL column is selected at all.
    ///
    /// # Errors
    ///
    /// Returns [`ErrorKind::TypeConversion`] if a selected cell is not an `int` or a list of them.
    pub fn ttl(&self, row: &Row) -> Result<Option<i32>, CdmError> {
        if self.custom_ttl > 0 {
            return Ok(Some(i32::try_from(self.custom_ttl).unwrap_or(i32::MAX)));
        }
        if self.ttl.is_empty() {
            return Ok(None);
        }
        let maximum = Self::maximum(row, &self.ttl, read_i32)?;
        Ok(Some(maximum.unwrap_or(0)))
    }

    /// The maximum of the selected cells, decoding a collection column's list of values.
    fn maximum<T: Ord + Copy>(
        row: &Row,
        selected: &[SelectedColumn],
        decode: fn(&[u8]) -> Result<T, CdmError>,
    ) -> Result<Option<T>, CdmError> {
        let mut maximum: Option<T> = None;
        for column in selected {
            let Some(cell) = row.get(column.index) else {
                return Err(CdmError::new(
                    ErrorKind::Internal,
                    format!(
                        "TTL/writetime projection expects {} columns; the row has {}",
                        column.index + 1,
                        row.len()
                    ),
                ));
            };
            let Some(bytes) = cell.bytes() else {
                continue;
            };
            if column.collection {
                for element in list_elements(bytes)? {
                    if let Some(element) = element.bytes() {
                        let value = decode(element)?;
                        maximum = Some(maximum.map_or(value, |current| current.max(value)));
                    }
                }
            } else {
                let value = decode(bytes)?;
                maximum = Some(maximum.map_or(value, |current| current.max(value)));
            }
        }
        Ok(maximum)
    }
}

fn read_i64(bytes: &[u8]) -> Result<i64, CdmError> {
    <[u8; 8]>::try_from(bytes)
        .map(i64::from_be_bytes)
        .map_err(|_| {
            CdmError::new(
                ErrorKind::TypeConversion,
                format!("expected an 8-byte writetime, got {} bytes", bytes.len()),
            )
        })
}

fn read_i32(bytes: &[u8]) -> Result<i32, CdmError> {
    <[u8; 4]>::try_from(bytes)
        .map(i32::from_be_bytes)
        .map_err(|_| {
            CdmError::new(
                ErrorKind::TypeConversion,
                format!("expected a 4-byte TTL, got {} bytes", bytes.len()),
            )
        })
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
    use cdm_core::{RawCell, TableRef};

    fn config(pairs: &[(&str, &str)]) -> EffectiveConfig {
        pairs.iter().copied().collect()
    }

    fn origin() -> TableFacts {
        TableFacts::from_view(
            &table_view(
                TableRef::new("ks", "src"),
                &[
                    ("id", "int"),
                    ("a", "text"),
                    ("b", "int"),
                    ("l", "list<int>"),
                ],
            ),
            &["id"],
        )
        .unwrap()
    }

    fn schema() -> FeatureSchema {
        FeatureSchema::new(origin(), origin())
    }

    /// A row of the four table columns followed by the projection this plan adds.
    fn row(table_columns: usize, appended: &[RawCell]) -> Row {
        let mut cells = vec![RawCell::NULL; table_columns];
        cells.extend_from_slice(appended);
        Row::new(cells)
    }

    fn bigint(value: i64) -> RawCell {
        RawCell::new(value.to_be_bytes().to_vec())
    }

    fn int(value: i32) -> RawCell {
        RawCell::new(value.to_be_bytes().to_vec())
    }

    fn list(values: &[i64]) -> RawCell {
        let mut out = i32::try_from(values.len()).unwrap().to_be_bytes().to_vec();
        for value in values {
            out.extend_from_slice(&8_i32.to_be_bytes());
            out.extend_from_slice(&value.to_be_bytes());
        }
        RawCell::new(out)
    }

    #[test]
    fn fea_040_a_rows_writetime_is_the_maximum_plus_the_increment() {
        let feature = WritetimeTtl::load(&config(&[
            ("schema.origin.writetime.names", "a,b"),
            ("schema.origin.ttl.names", "a,b"),
            ("transform.custom_writetime_increment", "5"),
        ]))
        .unwrap();
        let plan = feature.resolve(&origin()).unwrap();
        assert_eq!(
            plan.projection(),
            ["TTL(a)", "TTL(b)", "WRITETIME(a)", "WRITETIME(b)"]
        );

        let row = row(4, &[int(10), int(30), bigint(100), bigint(200)]);
        assert_eq!(plan.writetime(&row).unwrap(), Some(205));
        assert_eq!(plan.ttl(&row).unwrap(), Some(30));
    }

    #[test]
    fn fea_041_only_eligible_columns_may_be_named() {
        let feature =
            WritetimeTtl::load(&config(&[("schema.origin.writetime.names", "id,l")])).unwrap();
        let findings = feature.validate(&schema());
        assert_eq!(findings.len(), 2);
        assert!(findings[0].title.contains("`id`"));
        assert!(findings[0]
            .suggestion
            .as_deref()
            .unwrap()
            .contains("key columns"));
        assert!(findings[1].title.contains("`l`"));

        let with_collections = WritetimeTtl::load(&config(&[
            ("schema.origin.writetime.names", "l"),
            ("schema.ttl_writetime.use_collections", "true"),
        ]))
        .unwrap();
        assert!(with_collections.uses_collections());
        assert!(with_collections.validate(&schema()).is_empty());

        let absent = WritetimeTtl::load(&config(&[("schema.origin.ttl.names", "nope")])).unwrap();
        assert_eq!(
            absent.validate(&schema())[0].rule.as_deref(),
            Some("FEA-042")
        );
        assert_eq!(
            absent.resolve(&origin()).unwrap_err().kind(),
            ErrorKind::SchemaMismatch
        );
    }

    #[test]
    fn fea_042_naming_columns_disables_automatic_mode_for_that_dimension_only() {
        let feature = WritetimeTtl::load(&config(&[("schema.origin.ttl.names", "a")])).unwrap();
        let origin = origin();
        assert_eq!(feature.ttl_columns(&origin), ["a"]);
        assert_eq!(
            feature.writetime_columns(&origin),
            ["a", "b"],
            "writetime stays automatic and takes every eligible column"
        );

        let automatic = WritetimeTtl::load(&EffectiveConfig::new()).unwrap();
        assert!(automatic.is_enabled());
        assert_eq!(automatic.ttl_columns(&origin), ["a", "b"]);

        let off = WritetimeTtl::load(&config(&[
            ("schema.origin.ttl.automatic", "false"),
            ("schema.origin.writetime.automatic", "false"),
        ]))
        .unwrap();
        assert!(!off.is_enabled());
        assert!(off.resolve(&origin).unwrap().projection().is_empty());
    }

    #[test]
    fn fea_043_a_collection_columns_writetimes_are_a_list_whose_maximum_is_taken() {
        let feature = WritetimeTtl::load(&config(&[
            ("schema.origin.writetime.names", "l"),
            ("schema.origin.ttl.automatic", "false"),
            ("schema.ttl_writetime.use_collections", "true"),
        ]))
        .unwrap();
        let plan = feature.resolve(&origin()).unwrap();
        let collection = row(4, &[list(&[10, 40, 20])]);
        assert_eq!(plan.writetime(&collection).unwrap(), Some(40));

        // A null collection contributes nothing rather than zero.
        assert_eq!(plan.writetime(&row(4, &[RawCell::NULL])).unwrap(), None);
        assert!(plan
            .writetime(&row(4, &[RawCell::new(vec![0, 0])]))
            .is_err());
    }

    #[test]
    fn fea_044_custom_values_override_the_computation() {
        let feature = WritetimeTtl::load(&config(&[
            ("schema.origin.writetime.names", "a"),
            ("schema.origin.ttl.names", "a"),
            ("transform.custom_writetime", "999"),
            ("transform.custom_writetime_increment", "5"),
            ("transform.custom_ttl", "60"),
        ]))
        .unwrap();
        assert_eq!(feature.custom_writetime(), 999);
        assert_eq!(feature.custom_ttl(), 60);
        let plan = feature.resolve(&origin()).unwrap();
        let row = row(4, &[int(10), bigint(100)]);
        assert_eq!(
            plan.writetime(&row).unwrap(),
            Some(999),
            "the override replaces the computation and is not incremented"
        );
        assert_eq!(plan.ttl(&row).unwrap(), Some(60));

        let negative = WritetimeTtl::load(&config(&[
            ("transform.custom_writetime", "-1"),
            ("transform.custom_writetime_increment", "-1"),
        ]))
        .unwrap();
        let findings = negative.validate(&schema());
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().any(|f| !f.is_blocking()));
    }

    #[test]
    fn fea_045_a_counter_table_disables_the_feature_entirely() {
        let counters = TableFacts::from_view(
            &table_view(TableRef::new("ks", "c"), &[("id", "int"), ("n", "counter")]),
            &["id"],
        )
        .unwrap();
        let feature = WritetimeTtl::load(&config(&[("schema.origin.ttl.names", "n")])).unwrap();
        let findings = feature.validate(&FeatureSchema::new(counters.clone(), counters.clone()));
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule.as_deref(), Some("FEA-045"));

        let plan = feature.resolve(&counters).unwrap();
        assert!(plan.projection().is_empty());
        assert!(plan.using_clause().is_empty());

        let disabled = WritetimeTtl::load(&config(&[
            ("schema.origin.ttl.automatic", "false"),
            ("schema.origin.writetime.automatic", "false"),
        ]))
        .unwrap();
        assert!(disabled
            .validate(&FeatureSchema::new(counters.clone(), counters))
            .is_empty());
    }

    #[test]
    fn fea_046_the_using_clause_is_omitted_when_nothing_is_resolvable() {
        let none = WritetimeTtlPlan::disabled();
        assert_eq!(none.using_clause(), UsingClause::default());
        assert_eq!(none.using_clause().to_string(), "");
        assert_eq!(none.writetime(&Row::default()).unwrap(), None);
        assert_eq!(none.ttl(&Row::default()).unwrap(), None);
        assert!(!none.has_ttl() && !none.has_writetime());

        let both = WritetimeTtl::load(&config(&[
            ("schema.origin.ttl.names", "a"),
            ("schema.origin.writetime.names", "a"),
        ]))
        .unwrap()
        .resolve(&origin())
        .unwrap();
        assert_eq!(
            both.using_clause().to_string(),
            " USING TTL ? AND TIMESTAMP ?"
        );
        assert_eq!(
            both.writetime(&row(4, &[int(1), RawCell::NULL])).unwrap(),
            None,
            "a selected but null writetime leaves USING TIMESTAMP unbound"
        );
        assert_eq!(
            both.ttl(&row(4, &[RawCell::NULL, bigint(1)])).unwrap(),
            Some(0),
            "a selected but null TTL is zero, which is how Cassandra spells `no TTL`"
        );

        let ttl_only = UsingClause {
            ttl: true,
            timestamp: false,
        };
        assert_eq!(ttl_only.to_string(), " USING TTL ?");
        assert_eq!(
            UsingClause {
                ttl: false,
                timestamp: true
            }
            .to_string(),
            " USING TIMESTAMP ?"
        );
    }

    #[test]
    fn fea_040_a_projection_shorter_than_the_plan_is_an_error_not_a_panic() {
        let plan = WritetimeTtl::load(&config(&[("schema.origin.writetime.names", "a")]))
            .unwrap()
            .resolve(&origin())
            .unwrap();
        assert_eq!(
            plan.writetime(&Row::default()).unwrap_err().kind(),
            ErrorKind::Internal
        );
        assert!(plan.writetime(&row(4, &[RawCell::new(vec![1])])).is_err());
    }
}
