//! The best-practice rules engine (`UI-004`).
//!
//! Java CDM's config builder is a React app with its own copy of these rules, in
//! `cdm-config-builder/src/utils/bestPracticesRules.js`. `UI-002` forbids that duplication: the
//! rules live here, server-side, so the CLI, the HTTP API and the web UI give the same advice
//! from the same code.
//!
//! The engine is *advisory*. It never mutates a configuration; it returns
//! [`Recommendation`]s — each a property, a value and the reason — plus the
//! [`Diagnostic`]s for the findings that are warnings rather than settings. What the caller does
//! with them is the caller's business, which is what lets the UI show them as pre-filled form
//! values while the CLI shows them as a report.
//!
//! ```
//! use cdm_config::{BestPracticeInputs, BestPractices};
//!
//! let advice = BestPractices::evaluate(&BestPracticeInputs {
//!     table_size_gb: Some(500.0),
//!     ..BestPracticeInputs::default()
//! });
//! assert_eq!(advice.value("perfops.num_parts"), Some(&serde_json::json!(51200)));
//! ```

use cdm_core::Diagnostic;
use serde_json::{json, Value};

use crate::loader::CODE;
use crate::registry::PropertyRegistry;
use crate::validate::{ColumnDescription, TableDescription};

/// What the rules engine knows about the table it is advising on.
///
/// Every field is optional because the engine runs before a cluster is necessarily reachable —
/// in the config builder the operator types the numbers by hand, and pastes the DDL for the
/// shape (`UI-003`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BestPracticeInputs {
    /// The origin table's approximate size in gigabytes.
    pub table_size_gb: Option<f64>,
    /// The origin table's approximate row count.
    pub row_count: Option<u64>,
    /// The shape of the origin table.
    pub origin: Option<TableProfile>,
    /// Whether the target is a counter table, when it differs from the origin.
    pub target_is_counter: Option<bool>,
}

/// The shape of a table, reduced to what the rules engine reasons about.
///
/// Eight independent booleans would normally be a design smell, but this is a set of predicates
/// over one table rather than a set of modes: every one is answered independently by
/// [`TableProfile::from_table`], and grouping them into sub-structs would only add nesting to
/// what is, deliberately, the flat input vector of Java CDM's `bestPracticesRules.js`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TableProfile {
    /// The table holds a `blob` or a text column that may be large.
    pub has_lobs: bool,
    /// The table holds a `timestamp` column.
    pub has_timestamps: bool,
    /// The table holds a numeric column.
    pub has_numerics: bool,
    /// The table holds a collection column.
    pub has_collections: bool,
    /// The table holds a user-defined or tuple column.
    pub has_udts: bool,
    /// The table holds a counter column.
    pub has_counters: bool,
    /// The primary key is the partition key: there are no clustering columns.
    pub is_partition_key_only: bool,
    /// Every non-key column is a collection, so no row-level writetime can be read.
    pub only_collection_non_key_columns: bool,
}

impl TableProfile {
    /// Derives a profile from a schema description.
    pub fn from_table(table: &TableDescription) -> Self {
        let non_key: Vec<_> = table.columns.iter().filter(|c| !c.is_key()).collect();
        Self {
            has_lobs: table.columns.iter().any(ColumnDescription::is_lob),
            has_timestamps: table
                .columns
                .iter()
                .any(|c| matches!(c.cql_type(), "timestamp" | "date" | "time")),
            has_numerics: table.columns.iter().any(|c| {
                matches!(
                    c.cql_type(),
                    "int"
                        | "bigint"
                        | "double"
                        | "float"
                        | "decimal"
                        | "varint"
                        | "smallint"
                        | "tinyint"
                )
            }),
            has_collections: table.columns.iter().any(ColumnDescription::is_collection),
            has_udts: table.columns.iter().any(ColumnDescription::is_udt),
            has_counters: table.columns.iter().any(ColumnDescription::is_counter),
            is_partition_key_only: table.is_partition_key_only(),
            only_collection_non_key_columns: !non_key.is_empty()
                && non_key.iter().all(|c| c.is_collection()),
        }
    }
}

/// One suggested property value, with the reasoning behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct Recommendation {
    /// The canonical property name.
    pub canonical: String,
    /// The legacy `spark.cdm.*` name, for a `.properties` file.
    pub legacy: Option<String>,
    /// The suggested value.
    pub value: Value,
    /// Why the engine suggests it, in one paragraph an operator can act on.
    pub rationale: String,
}

/// What the rules engine concluded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BestPracticeReport {
    /// The suggested property values, in property order.
    pub recommendations: Vec<Recommendation>,
    /// Findings that are advice rather than a value — "consider cluster mode", and the
    /// counter-table caution.
    pub diagnostics: Vec<Diagnostic>,
}

impl BestPracticeReport {
    /// The suggested value for a property, if the engine has an opinion about it.
    pub fn value(&self, canonical: &str) -> Option<&Value> {
        self.recommendations
            .iter()
            .find(|r| r.canonical == canonical)
            .map(|r| &r.value)
    }

    /// The rationale for a property, if the engine has an opinion about it.
    pub fn rationale(&self, canonical: &str) -> Option<&str> {
        self.recommendations
            .iter()
            .find(|r| r.canonical == canonical)
            .map(|r| r.rationale.as_str())
    }

    fn recommend(&mut self, canonical: &str, value: Value, rationale: impl Into<String>) {
        let legacy = PropertyRegistry::global()
            .by_canonical(canonical)
            .and_then(|meta| meta.legacy.first().cloned());
        self.recommendations.push(Recommendation {
            canonical: canonical.to_owned(),
            legacy,
            value,
            rationale: rationale.into(),
        });
    }
}

/// The rules engine of `UI-004`.
#[derive(Debug, Clone, Copy)]
pub struct BestPractices;

impl BestPractices {
    /// Evaluates every rule against the inputs.
    ///
    /// The rules, and the thresholds in them, are ported from Java CDM's
    /// `bestPracticesRules.js` so that a migration planned in the Java config builder and one
    /// planned here come out the same.
    pub fn evaluate(inputs: &BestPracticeInputs) -> BestPracticeReport {
        let mut report = BestPracticeReport::default();
        let profile = inputs.origin.clone().unwrap_or_default();
        let size_gb = inputs.table_size_gb.unwrap_or(0.0);
        let rows = inputs.row_count.unwrap_or(0);

        num_parts(&mut report, size_gb, rows);
        let batch = batch_size(&mut report, &profile, size_gb, rows);
        fetch_size(&mut report, &profile, size_gb, rows);
        rate_limit(&mut report, &profile, size_gb, rows);
        collections(&mut report, &profile);
        counters(&mut report, inputs, &profile, batch);
        codecs(&mut report, &profile);
        scale(&mut report, size_gb, rows);

        report
    }
}

/// `table size GB → num_parts = size_gb × 1024 ÷ 10 MB`, never below 1000, never below 50 000
/// for more than 100 M rows.
fn num_parts(report: &mut BestPracticeReport, size_gb: f64, rows: u64) {
    let (mut parts, mut rationale) = if size_gb > 0.0 {
        (
            parts_for(size_gb),
            format!(
                "Calculated from the estimated table size ({size_gb} GB ÷ 10 MB per part). Aim \
                 for about 10 MB of data per part for even parallelism."
            ),
        )
    } else if rows > 0 {
        #[allow(clippy::cast_precision_loss)]
        let estimated_gb = rows as f64 / 1_000_000.0;
        (
            parts_for(estimated_gb),
            format!(
                "Estimated from the row count ({rows} rows ≈ {estimated_gb:.1} GB at roughly 1 \
                 KB per row). Adjust if the real table size differs."
            ),
        )
    } else {
        (
            5_000,
            "The default assumes a table of about 50 GB (5000 parts × 10 MB).".to_owned(),
        )
    };

    if rows > 100_000_000 {
        parts = parts.max(50_000);
        rationale.push_str(
            " Raised to at least 50,000 because the table has more than 100 million rows.",
        );
    }

    report.recommend("perfops.num_parts", json!(parts), rationale);
}

/// The Java engine's `Math.max(1000, ceil(sizeGB * 1024 / 10))`.
fn parts_for(size_gb: f64) -> u64 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let computed = (size_gb * 1024.0 / 10.0).ceil().max(0.0) as u64;
    computed.max(1_000)
}

/// A primary key with no clustering columns, or large rows, force single-row writes.
fn batch_size(
    report: &mut BestPracticeReport,
    profile: &TableProfile,
    size_gb: f64,
    rows: u64,
) -> u32 {
    let (size, rationale) = if profile.is_partition_key_only {
        (
            1,
            "Set to 1 because the primary key is the partition key: every row is its own \
             partition, so a multi-row batch would always span partitions."
                .to_owned(),
        )
    } else if profile.has_lobs {
        (
            1,
            "Set to 1 because the table has blob or large text columns, so the average row is \
             likely to exceed 20 KB."
                .to_owned(),
        )
    } else if let Some(avg_kb) = average_row_kb(size_gb, rows) {
        if avg_kb > 20.0 {
            (
                1,
                format!(
                    "Set to 1 because the estimated average row is about {avg_kb:.1} KB, which \
                     is above the 20 KB batching threshold."
                ),
            )
        } else if avg_kb < 1.0 {
            (
                20,
                format!(
                    "Raised to 20 because the estimated average row is only about {avg_kb:.2} \
                     KB; small rows amortise the write overhead better in larger batches."
                ),
            )
        } else {
            (
                5,
                "The default batch size suits rows of this size.".to_owned(),
            )
        }
    } else {
        (5, "The default batch size.".to_owned())
    };

    report.recommend("perfops.batch_size", json!(size), rationale);
    size
}

/// Large values need a smaller page, or the driver buffers a page's worth of them.
fn fetch_size(report: &mut BestPracticeReport, profile: &TableProfile, size_gb: f64, rows: u64) {
    if profile.has_lobs {
        report.recommend(
            "perfops.fetch_size",
            json!(100),
            "Reduced to 100 because the table has blob or large text columns; a full page of \
             them is a memory spike on the reader.",
        );
        return;
    }
    if let Some(avg_kb) = average_row_kb(size_gb, rows) {
        if avg_kb > 100.0 {
            report.recommend(
                "perfops.fetch_size",
                json!(200),
                format!(
                    "Reduced to 200 because the estimated average row is about {avg_kb:.0} KB."
                ),
            );
        }
    }
}

/// Rate limits scale with the size of the job and shrink for expensive rows.
fn rate_limit(report: &mut BestPracticeReport, profile: &TableProfile, size_gb: f64, rows: u64) {
    let (limit, rationale) = if profile.has_lobs {
        (
            5_000,
            "Reduced to 5,000 rows per second because blob and large text values make each \
             request slower; a higher rate invites timeouts."
                .to_owned(),
        )
    } else if rows > 500_000_000 || size_gb > 500.0 {
        (
            40_000,
            "Raised to 40,000 rows per second for a table of this size. Watch cluster load and \
             lower it if timeouts appear."
                .to_owned(),
        )
    } else {
        (
            20_000,
            "The default of 20,000 rows per second per process. Raise it once you have measured \
             the clusters' headroom."
                .to_owned(),
        )
    };

    report.recommend("perfops.ratelimit.origin", json!(limit), rationale.clone());
    report.recommend(
        "perfops.ratelimit.target",
        json!(limit),
        format!(
            "{rationale} Set equal to the origin limit; raise it if explode map multiplies the \
             number of target writes."
        ),
    );
}

/// Only-collection non-key columns, or UDTs, make row-level TTL and writetime unreadable.
fn collections(report: &mut BestPracticeReport, profile: &TableProfile) {
    if profile.only_collection_non_key_columns || profile.has_udts {
        report.recommend(
            "schema.ttl_writetime.use_collections",
            json!(true),
            "Enabled because the non-key columns are collections or user-defined types. \
             Without it, no TTL or writetime can be read from them and the target rows would be \
             written with neither.",
        );
    }
}

/// Counter tables get a caution, never an automatic setting.
fn counters(
    report: &mut BestPracticeReport,
    inputs: &BestPracticeInputs,
    profile: &TableProfile,
    batch: u32,
) {
    let is_counter = inputs.target_is_counter.unwrap_or(profile.has_counters);
    if !is_counter {
        return;
    }
    report.recommend(
        "autocorrect.missing_counter",
        json!(false),
        "Left off. Re-inserting a counter row that was deleted double-counts it — 5,323 becomes \
         10,646 — because counter updates are not idempotent. Only enable it after reasoning \
         through the counter semantics of this table.",
    );
    if batch > 1 {
        report.diagnostics.push(
            Diagnostic::warning(CODE, "counter tables are written one row at a time")
                .with_location("perfops.batch_size".to_owned())
                .with_rule("UI-004")
                .with_detail(
                    "counter updates are never batched or retried, so the batch size will be \
                     coerced to 1 (MIG-021, MIG-032)",
                ),
        );
    }
}

/// Codec advice is a hint, not a value: only the operator knows how the data is really stored.
fn codecs(report: &mut BestPracticeReport, profile: &TableProfile) {
    if profile.has_timestamps {
        report.diagnostics.push(
            Diagnostic::info(CODE, "check how timestamps are stored")
                .with_location("transform.codecs".to_owned())
                .with_rule("UI-004")
                .with_suggestion(
                    "if timestamps are held as text, enable `TIMESTAMP_STRING_MILLIS` for epoch \
                     milliseconds or `TIMESTAMP_STRING_FORMAT` with \
                     `transform.codec_timestamp_format` for a formatted string",
                ),
        );
    }
    if profile.has_numerics {
        report.diagnostics.push(
            Diagnostic::info(CODE, "check how numbers are stored")
                .with_location("transform.codecs".to_owned())
                .with_rule("UI-004")
                .with_suggestion(
                    "if a numeric column is text on one side, enable the matching codec: \
                     `INT_STRING`, `BIGINT_STRING`, `DOUBLE_STRING` or `DECIMAL_STRING`",
                ),
        );
    }
}

/// Very large tables want run tracking, and beyond a terabyte, distributed mode.
fn scale(report: &mut BestPracticeReport, size_gb: f64, rows: u64) {
    if size_gb > 1000.0 || rows > 1_000_000_000 {
        report.diagnostics.push(
            Diagnostic::info(CODE, "consider running this migration across a cluster")
                .with_location("cluster.enabled".to_owned())
                .with_rule("UI-004")
                .with_detail("the table is larger than 1 TB or has more than a billion rows")
                .with_suggestion(
                    "set `cluster.enabled` and run several cdm-rs nodes against the same \
                     tracking table (DST-001)",
                ),
        );
    }
    if size_gb > 100.0 || rows > 100_000_000 {
        report.diagnostics.push(
            Diagnostic::info(CODE, "enable run tracking for a table this size")
                .with_location("track_run.enabled".to_owned())
                .with_rule("UI-004")
                .with_suggestion(
                    "set `track_run.enabled` and `track_run.auto_rerun` so an interrupted run \
                     resumes instead of starting again",
                ),
        );
    }
}

/// The average row size in kilobytes, when both inputs are known.
fn average_row_kb(size_gb: f64, rows: u64) -> Option<f64> {
    if size_gb <= 0.0 || rows == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let rows = rows as f64;
    Some(size_gb * 1024.0 * 1024.0 / rows)
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
    use cdm_core::TableRef;

    use super::*;

    fn evaluate(inputs: &BestPracticeInputs) -> BestPracticeReport {
        BestPractices::evaluate(inputs)
    }

    #[test]
    fn ui_004_table_size_decides_the_number_of_parts() {
        // 500 GB × 1024 ÷ 10 = 51,200.
        let report = evaluate(&BestPracticeInputs {
            table_size_gb: Some(500.0),
            ..BestPracticeInputs::default()
        });
        assert_eq!(report.value("perfops.num_parts"), Some(&json!(51_200)));
        assert!(report
            .rationale("perfops.num_parts")
            .unwrap()
            .contains("10 MB"));

        // A tiny table still gets the Java engine's floor of 1000.
        let tiny = evaluate(&BestPracticeInputs {
            table_size_gb: Some(0.5),
            ..BestPracticeInputs::default()
        });
        assert_eq!(tiny.value("perfops.num_parts"), Some(&json!(1_000)));

        // With nothing to go on, the built-in default stands.
        assert_eq!(
            evaluate(&BestPracticeInputs::default()).value("perfops.num_parts"),
            Some(&json!(5_000))
        );
    }

    #[test]
    fn ui_004_more_than_a_hundred_million_rows_forces_at_least_fifty_thousand_parts() {
        let report = evaluate(&BestPracticeInputs {
            row_count: Some(150_000_000),
            ..BestPracticeInputs::default()
        });
        assert_eq!(report.value("perfops.num_parts"), Some(&json!(50_000)));

        // A table big enough on its own keeps its larger figure.
        let bigger = evaluate(&BestPracticeInputs {
            table_size_gb: Some(2000.0),
            row_count: Some(150_000_000),
            ..BestPracticeInputs::default()
        });
        assert_eq!(bigger.value("perfops.num_parts"), Some(&json!(204_800)));
    }

    #[test]
    fn ui_004_large_objects_force_a_batch_of_one_and_a_page_of_a_hundred() {
        let report = evaluate(&BestPracticeInputs {
            origin: Some(TableProfile {
                has_lobs: true,
                ..TableProfile::default()
            }),
            ..BestPracticeInputs::default()
        });
        assert_eq!(report.value("perfops.batch_size"), Some(&json!(1)));
        assert_eq!(report.value("perfops.fetch_size"), Some(&json!(100)));
        // And the rate limit comes down with them.
        assert_eq!(
            report.value("perfops.ratelimit.origin"),
            Some(&json!(5_000))
        );
    }

    #[test]
    fn ui_004_a_primary_key_equal_to_the_partition_key_forces_a_batch_of_one() {
        let report = evaluate(&BestPracticeInputs {
            origin: Some(TableProfile {
                is_partition_key_only: true,
                ..TableProfile::default()
            }),
            ..BestPracticeInputs::default()
        });
        assert_eq!(report.value("perfops.batch_size"), Some(&json!(1)));
        assert!(report
            .rationale("perfops.batch_size")
            .unwrap()
            .contains("own partition"));
        // Nothing else here asks for a smaller page.
        assert!(report.value("perfops.fetch_size").is_none());
    }

    #[test]
    fn ui_004_row_size_moves_the_batch_size_in_both_directions() {
        // 1 TB over 10 million rows is about 105 MB per row: batch of 1, page of 200.
        let heavy = evaluate(&BestPracticeInputs {
            table_size_gb: Some(1024.0),
            row_count: Some(10_000_000),
            origin: Some(TableProfile::default()),
            ..BestPracticeInputs::default()
        });
        assert_eq!(heavy.value("perfops.batch_size"), Some(&json!(1)));
        assert_eq!(heavy.value("perfops.fetch_size"), Some(&json!(200)));

        // 1 GB over 10 million rows is about 0.1 KB per row: batch of 20.
        let light = evaluate(&BestPracticeInputs {
            table_size_gb: Some(1.0),
            row_count: Some(10_000_000),
            origin: Some(TableProfile::default()),
            ..BestPracticeInputs::default()
        });
        assert_eq!(light.value("perfops.batch_size"), Some(&json!(20)));
    }

    #[test]
    fn ui_004_only_collection_non_key_columns_turn_on_use_collections() {
        let report = evaluate(&BestPracticeInputs {
            origin: Some(TableProfile {
                has_collections: true,
                only_collection_non_key_columns: true,
                ..TableProfile::default()
            }),
            ..BestPracticeInputs::default()
        });
        assert_eq!(
            report.value("schema.ttl_writetime.use_collections"),
            Some(&json!(true))
        );

        // A table with an ordinary column alongside its collection does not need it.
        let mixed = evaluate(&BestPracticeInputs {
            origin: Some(TableProfile {
                has_collections: true,
                ..TableProfile::default()
            }),
            ..BestPracticeInputs::default()
        });
        assert!(mixed
            .value("schema.ttl_writetime.use_collections")
            .is_none());
    }

    #[test]
    fn ui_004_a_counter_table_gets_a_caution_not_a_setting() {
        let report = evaluate(&BestPracticeInputs {
            origin: Some(TableProfile {
                has_counters: true,
                ..TableProfile::default()
            }),
            ..BestPracticeInputs::default()
        });
        assert_eq!(
            report.value("autocorrect.missing_counter"),
            Some(&json!(false))
        );
        assert!(report
            .rationale("autocorrect.missing_counter")
            .unwrap()
            .contains("10,646"));
        assert!(report
            .diagnostics
            .iter()
            .any(|d| d.title.contains("one row at a time")));
    }

    #[test]
    fn ui_004_more_than_a_terabyte_recommends_cluster_mode() {
        let report = evaluate(&BestPracticeInputs {
            table_size_gb: Some(1500.0),
            ..BestPracticeInputs::default()
        });
        assert!(report
            .diagnostics
            .iter()
            .any(|d| d.location.as_deref() == Some("cluster.enabled")));
        assert!(report
            .diagnostics
            .iter()
            .any(|d| d.location.as_deref() == Some("track_run.enabled")));

        // A small table is advised to do neither.
        let small = evaluate(&BestPracticeInputs {
            table_size_gb: Some(10.0),
            ..BestPracticeInputs::default()
        });
        assert!(small.diagnostics.is_empty(), "{:#?}", small.diagnostics);
    }

    #[test]
    fn ui_004_codec_hints_follow_the_column_types() {
        let report = evaluate(&BestPracticeInputs {
            origin: Some(TableProfile {
                has_timestamps: true,
                has_numerics: true,
                ..TableProfile::default()
            }),
            ..BestPracticeInputs::default()
        });
        let hints: Vec<&str> = report
            .diagnostics
            .iter()
            .filter_map(|d| d.suggestion.as_deref())
            .collect();
        assert!(hints.iter().any(|h| h.contains("TIMESTAMP_STRING_MILLIS")));
        assert!(hints.iter().any(|h| h.contains("DECIMAL_STRING")));
    }

    #[test]
    fn ui_004_recommendations_carry_the_java_property_name_for_a_properties_file() {
        let report = evaluate(&BestPracticeInputs {
            table_size_gb: Some(50.0),
            ..BestPracticeInputs::default()
        });
        let parts = report
            .recommendations
            .iter()
            .find(|r| r.canonical == "perfops.num_parts")
            .unwrap();
        assert_eq!(parts.legacy.as_deref(), Some("spark.cdm.perfops.numParts"));
    }

    #[test]
    fn ui_004_a_profile_is_derived_from_a_schema_description() {
        let table = TableDescription::new(
            TableRef::new("ks", "t"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("tags", "set<text>"),
                ColumnDescription::new("attrs", "map<text, text>"),
            ],
        );
        let profile = TableProfile::from_table(&table);
        assert!(profile.is_partition_key_only);
        assert!(profile.has_collections);
        assert!(profile.only_collection_non_key_columns);
        assert!(profile.has_numerics, "the int partition key counts");
        assert!(!profile.has_counters);
        assert!(!profile.has_lobs);

        let counters = TableDescription::new(
            TableRef::new("ks", "c"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("bucket", "int").clustering_key(),
                ColumnDescription::new("hits", "counter"),
                ColumnDescription::new("blob_col", "blob"),
                ColumnDescription::new("when", "timestamp"),
            ],
        );
        let profile = TableProfile::from_table(&counters);
        assert!(profile.has_counters);
        assert!(profile.has_lobs);
        assert!(profile.has_timestamps);
        assert!(!profile.is_partition_key_only);
        assert!(!profile.only_collection_non_key_columns);
    }
}
