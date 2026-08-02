//! Tier 3: schema-bound validation (`CFG-020`).
//!
//! Everything here needs the live cluster, which reaches `cdm-config` only through the
//! [`SchemaProvider`] trait so that no dependency on `cdm-cql` is created
//! (`ARCHITECTURE.md` §3.2).
//!
//! A failure to *read* the schema is reported once and stops this tier — the alternative is
//! dozens of "column does not exist" diagnostics caused by a connection problem, which would
//! bury the real cause.

use cdm_core::{Diagnostic, TableRef};

use super::{
    error, notice, parse_keyspace_table, warning, ColumnDescription, SchemaProvider,
    TableDescription, ValidationOptions,
};
use crate::loader::CODE;
use crate::model::CdmConfig;

/// Runs every Tier-3 check.
pub(super) fn check(
    config: &CdmConfig,
    schema: &dyn SchemaProvider,
    _options: ValidationOptions,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    let Some(origin_ref) = config
        .schema
        .origin
        .keyspace_table
        .as_deref()
        .and_then(parse_keyspace_table)
    else {
        // Tier 1 has already said everything worth saying about this.
        return out;
    };
    // CFG-023: an unset target keyspace and table is the origin's.
    let target_ref = config
        .schema
        .target
        .keyspace_table
        .as_deref()
        .and_then(parse_keyspace_table)
        .unwrap_or_else(|| origin_ref.clone());

    let Ok(origin) = describe(
        "schema.origin.keyspace_table",
        &origin_ref,
        schema,
        &mut out,
    ) else {
        return out;
    };
    let Ok(target) = describe(
        "schema.target.keyspace_table",
        &target_ref,
        schema,
        &mut out,
    ) else {
        return out;
    };
    let (Some(origin), Some(target)) = (origin, target) else {
        return out;
    };

    columns_exist(config, &origin, &target, &mut out);
    renames(config, &origin, &target, &mut out);
    primary_keys(config, &origin, &target, &mut out);
    writetime_resolution(config, &origin, &mut out);
    counter_rules(config, &target, &mut out);
    unfrozen_lists(config, &origin, &mut out);
    out
}

/// Looks a table up, turning "no such keyspace" and "no such table" into distinct diagnostics.
///
/// `Err(())` means the schema itself could not be read, which has already been reported.
#[allow(clippy::result_unit_err)]
fn describe(
    key: &str,
    table: &TableRef,
    schema: &dyn SchemaProvider,
    out: &mut Vec<Diagnostic>,
) -> Result<Option<TableDescription>, ()> {
    match schema.table(table) {
        Ok(Some(description)) => Ok(Some(description)),
        Ok(None) => {
            let keyspace_exists = schema.keyspace_exists(table.keyspace()).unwrap_or(true);
            let diagnostic = if keyspace_exists {
                error(key, "the table does not exist", "CFG-020")
                    .with_value(table.to_string())
                    .with_suggestion("check the table name, and that it is not a materialized view")
            } else {
                error(key, "the keyspace does not exist", "CFG-020")
                    .with_value(table.keyspace().to_owned())
                    .with_suggestion("check the keyspace name and the cluster you are connected to")
            };
            out.push(diagnostic);
            Ok(None)
        }
        Err(cause) => {
            out.push(
                Diagnostic::error(CODE, "the cluster schema could not be read")
                    .with_location(key.to_owned())
                    .with_detail(cause.to_string())
                    .with_rule("CFG-020")
                    .with_suggestion(
                        "schema-bound validation needs a working connection; fix the connection \
                         and re-run",
                    ),
            );
            Err(())
        }
    }
}

/// Every property that names a column must name one that exists.
fn columns_exist(
    config: &CdmConfig,
    origin: &TableDescription,
    target: &TableDescription,
    out: &mut Vec<Diagnostic>,
) {
    let mut require = |key: &str, side: &TableDescription, name: &str, rule: &str| {
        if name.trim().is_empty() || side.column(name).is_some() {
            return;
        }
        let mut diagnostic = error(key, "the column does not exist", rule)
            .with_value(format!("{}.{name}", side.table));
        if let Some(closest) = closest_column(side, name) {
            diagnostic = diagnostic.with_suggestion(format!("did you mean `{closest}`?"));
        }
        out.push(diagnostic);
    };

    for name in &config.schema.origin.column.skip {
        require("schema.origin.column.skip", origin, name, "CFG-020");
    }
    for name in &config.schema.origin.ttl.names {
        require("schema.origin.ttl.names", origin, name, "CFG-020");
    }
    for name in &config.schema.origin.writetime.names {
        require("schema.origin.writetime.names", origin, name, "CFG-020");
    }
    for name in &config.feature.constant_columns.names {
        require("feature.constant_columns.names", target, name, "CFG-020");
    }
    if let Some(name) = &config.filter.column.name {
        require("filter.column.name", origin, name, "CFG-020");
    }
    if let Some(name) = &config.feature.extract_json.origin_column {
        require(
            "feature.extract_json.origin_column",
            origin,
            name,
            "CFG-020",
        );
    }

    explode_map(config, origin, target, out);
}

/// The explode-map columns must exist and the origin column must actually be a map.
fn explode_map(
    config: &CdmConfig,
    origin: &TableDescription,
    target: &TableDescription,
    out: &mut Vec<Diagnostic>,
) {
    let feature = &config.feature.explode_map;
    let Some(name) = feature.origin_column.as_deref().filter(|n| !n.is_empty()) else {
        return;
    };
    match origin.column(name) {
        None => out.push(
            error(
                "feature.explode_map.origin_column",
                "the column does not exist",
                "CFG-031",
            )
            .with_value(format!("{}.{name}", origin.table)),
        ),
        Some(column) if !column.cql_type().to_ascii_lowercase().contains("map<") => out.push(
            error(
                "feature.explode_map.origin_column",
                "explode map needs a map column",
                "CFG-031",
            )
            .with_value(format!("{name} is {}", column.cql_type())),
        ),
        Some(_) => {}
    }

    for (key, name) in [
        (
            "feature.explode_map.target_key_column",
            feature.target_key_column.as_deref(),
        ),
        (
            "feature.explode_map.target_value_column",
            feature.target_value_column.as_deref(),
        ),
    ] {
        let Some(name) = name.filter(|n| !n.is_empty()) else {
            continue;
        };
        if target.column(name).is_none() {
            out.push(
                error(key, "the column does not exist", "CFG-031")
                    .with_value(format!("{}.{name}", target.table)),
            );
        }
    }
}

/// `CFG-038`: each `origin:target` rename must reference columns that exist on their own side.
fn renames(
    config: &CdmConfig,
    origin: &TableDescription,
    target: &TableDescription,
    out: &mut Vec<Diagnostic>,
) {
    for pair in &config.schema.origin.column.rename {
        // Tier 2 has already reported a malformed pair.
        let Some((from, to)) = pair.split_once(':') else {
            continue;
        };
        let (from, to) = (from.trim(), to.trim());
        if from.is_empty() || to.is_empty() {
            continue;
        }
        if origin.column(from).is_none() {
            out.push(
                error(
                    "schema.origin.column.rename",
                    "the origin column of a rename does not exist",
                    "CFG-038",
                )
                .with_value(format!("{}.{from}", origin.table)),
            );
        }
        if target.column(to).is_none() {
            out.push(
                error(
                    "schema.origin.column.rename",
                    "the target column of a rename does not exist",
                    "CFG-038",
                )
                .with_value(format!("{}.{to}", target.table)),
            );
        }
    }
}

/// The origin's primary key must be reproducible on the target, allowing for renames.
fn primary_keys(
    config: &CdmConfig,
    origin: &TableDescription,
    target: &TableDescription,
    out: &mut Vec<Diagnostic>,
) {
    let renames: Vec<(String, String)> = config
        .schema
        .origin
        .column
        .rename
        .iter()
        .filter_map(|pair| pair.split_once(':'))
        .map(|(from, to)| (from.trim().to_owned(), to.trim().to_owned()))
        .collect();

    for column in origin.primary_key() {
        let mapped = renames
            .iter()
            .find(|(from, _)| from == column.name())
            .map_or_else(|| column.name().to_owned(), |(_, to)| to.clone());

        // The explode-map target columns are synthesised, not copied, so they are exempt.
        let synthesised = [
            config.feature.explode_map.target_key_column.as_deref(),
            config.feature.explode_map.target_value_column.as_deref(),
        ]
        .iter()
        .flatten()
        .any(|name| *name == mapped);

        match target.column(&mapped) {
            None if !synthesised => out.push(
                error(
                    "schema.target.keyspace_table",
                    "an origin key column has no counterpart on the target",
                    "CFG-020",
                )
                .with_value(format!("{} → {}.{mapped}", column.name(), target.table))
                .with_suggestion(
                    "add the column to the target, or map it with \
                     `schema.origin.column.rename`",
                ),
            ),
            Some(counterpart) if !counterpart.is_key() && !synthesised => out.push(
                error(
                    "schema.target.keyspace_table",
                    "an origin key column is not a key column on the target",
                    "CFG-020",
                )
                .with_value(format!("{}.{mapped}", target.table)),
            ),
            _ => {}
        }
    }
}

/// `CFG-034`: with automatic resolution, at least one column must actually be able to supply a
/// writetime.
fn writetime_resolution(config: &CdmConfig, origin: &TableDescription, out: &mut Vec<Diagnostic>) {
    let filtering = config.filter.writetime.min.is_some() || config.filter.writetime.max.is_some();
    if !filtering || !config.schema.origin.writetime.automatic {
        return;
    }
    let use_collections = config.schema.ttl_writetime.use_collections;
    if origin
        .writetime_candidates(use_collections)
        .next()
        .is_none()
    {
        let mut diagnostic = error(
            "filter.writetime.min",
            "a writetime filter needs at least one resolvable writetime column",
            "CFG-034",
        )
        .with_value(origin.table.to_string());
        if !use_collections && origin.columns.iter().any(ColumnDescription::is_collection) {
            diagnostic = diagnostic.with_suggestion(
                "every non-key column is a collection; set \
                 `schema.ttl_writetime.use_collections` to true",
            );
        }
        out.push(diagnostic);
    }
}

/// `CFG-036` and the counter half of `CFG-040`.
fn counter_rules(config: &CdmConfig, target: &TableDescription, out: &mut Vec<Diagnostic>) {
    if !target.is_counter_table() {
        return;
    }

    let ttl_writetime_requested = config.schema.origin.ttl.automatic
        || config.schema.origin.writetime.automatic
        || !config.schema.origin.ttl.names.is_empty()
        || !config.schema.origin.writetime.names.is_empty()
        || config.transform.custom_writetime != 0
        || config.transform.custom_writetime_increment != 0
        || config.transform.custom_ttl != 0;

    if ttl_writetime_requested {
        out.push(
            error(
                "schema.origin.ttl.automatic",
                "TTL and writetime are not valid for a counter table",
                "CFG-036",
            )
            .with_value(target.table.to_string())
            .with_detail("Cassandra rejects USING TTL and USING TIMESTAMP on a counter update")
            .with_suggestion(
                "set `schema.origin.ttl.automatic` and `schema.origin.writetime.automatic` to \
                 false, clear the explicit column lists, and leave the custom TTL and writetime \
                 transforms at 0",
            ),
        );
    }

    if config.perfops.batch_size > 1 {
        out.push(
            notice(
                "perfops.batch_size",
                "batch size will be coerced to 1",
                "CFG-040",
            )
            .with_value(config.perfops.batch_size.to_string())
            .with_detail(
                "counter updates are not idempotent, so they are never batched or retried \
                 (MIG-021, MIG-032)",
            ),
        );
    }
}

/// `CFG-039`: rerunning with a zero writetime increment duplicates unfrozen list entries.
fn unfrozen_lists(config: &CdmConfig, origin: &TableDescription, out: &mut Vec<Diagnostic>) {
    if config.transform.custom_writetime_increment != 0 {
        return;
    }
    let lists: Vec<&str> = origin
        .columns
        .iter()
        .filter(|column| column.is_unfrozen_list())
        .map(ColumnDescription::name)
        .collect();
    if lists.is_empty() {
        return;
    }
    out.push(
        warning(
            "transform.custom_writetime_increment",
            "rerunning this migration can duplicate list entries",
            "CFG-039",
        )
        .with_value(lists.join(","))
        .with_detail(
            "an unfrozen list is stored as one cell per element, keyed by writetime; \
             re-migrating with the same writetime appends the elements again \
             (CASSANDRA-11368)",
        )
        .with_suggestion(
            "set `transform.custom_writetime_increment` to a small positive number of \
             microseconds before a rerun",
        ),
    );
}

/// The column of `side` whose name is closest to `name`, for a "did you mean" suggestion.
fn closest_column<'a>(side: &'a TableDescription, name: &str) -> Option<&'a str> {
    const MINIMUM_SIMILARITY: f64 = 0.6;
    side.columns
        .iter()
        .map(|column| {
            (
                column.name(),
                strsim::normalized_levenshtein(name, column.name()),
            )
        })
        .filter(|(_, score)| *score >= MINIMUM_SIMILARITY)
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(name, _)| name)
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
    use cdm_core::Severity;

    use super::super::tests::{origin_table, FakeSchema};
    use super::*;

    fn valid() -> CdmConfig {
        let mut config = CdmConfig::default();
        config.schema.origin.keyspace_table = Some("ks.src".to_owned());
        config
    }

    fn target_table() -> TableDescription {
        TableDescription::new(
            TableRef::new("ks", "dst"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("bucket", "int").clustering_key(),
                ColumnDescription::new("data", "text"),
                ColumnDescription::new("const_a", "text"),
            ],
        )
    }

    fn counter_table() -> TableDescription {
        TableDescription::new(
            TableRef::new("ks", "counts"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("hits", "counter"),
            ],
        )
    }

    fn schema() -> FakeSchema {
        FakeSchema::new()
            .with(origin_table())
            .with(target_table())
            .with(counter_table())
    }

    fn rules(config: &CdmConfig, schema: &FakeSchema) -> Vec<String> {
        check(config, schema, ValidationOptions::default())
            .into_iter()
            .filter(Diagnostic::is_blocking)
            .filter_map(|d| d.rule)
            .collect()
    }

    #[test]
    fn cfg_020_a_missing_keyspace_and_a_missing_table_are_different_diagnostics() {
        let mut config = valid();
        config.schema.origin.keyspace_table = Some("ks.nope".to_owned());
        let diagnostics = check(&config, &schema(), ValidationOptions::default());
        assert!(diagnostics[0].title.contains("table does not exist"));

        config.schema.origin.keyspace_table = Some("nope.src".to_owned());
        let diagnostics = check(&config, &schema(), ValidationOptions::default());
        assert!(diagnostics[0].title.contains("keyspace does not exist"));
    }

    #[test]
    fn cfg_020_an_unreadable_schema_is_reported_once_and_stops_the_tier() {
        let mut config = valid();
        config.schema.origin.column.skip = vec!["nope".to_owned()];
        let diagnostics = check(&config, &FakeSchema::broken(), ValidationOptions::default());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert!(diagnostics[0].title.contains("could not be read"));
    }

    #[test]
    fn cfg_023_an_unset_target_table_is_resolved_to_the_origin_table() {
        let config = valid();
        // `ks.src` is both sides; every check passes without a target table being configured.
        assert!(rules(&config, &schema()).is_empty());
    }

    #[test]
    fn cfg_020_a_property_naming_a_column_must_name_one_that_exists() {
        let mut config = valid();
        config.schema.origin.column.skip = vec!["dat".to_owned()];
        let diagnostics = check(&config, &schema(), ValidationOptions::default());
        assert_eq!(diagnostics[0].rule.as_deref(), Some("CFG-020"));
        assert_eq!(
            diagnostics[0].suggestion.as_deref(),
            Some("did you mean `data`?")
        );

        config.schema.origin.column.skip = vec!["data".to_owned()];
        assert!(rules(&config, &schema()).is_empty());
    }

    #[test]
    fn cfg_038_a_rename_must_reference_columns_that_exist_on_their_own_side() {
        let mut config = valid();
        config.schema.target.keyspace_table = Some("ks.dst".to_owned());
        config.schema.origin.column.rename = vec!["nosuch:data".to_owned()];
        assert_eq!(rules(&config, &schema()), ["CFG-038"]);

        config.schema.origin.column.rename = vec!["data:nosuch".to_owned()];
        assert_eq!(rules(&config, &schema()), ["CFG-038"]);

        config.schema.origin.column.rename = vec!["data:const_a".to_owned()];
        assert!(rules(&config, &schema()).is_empty());
    }

    #[test]
    fn cfg_020_an_origin_key_column_must_be_a_key_column_on_the_target() {
        let sparse_target = TableDescription::new(
            TableRef::new("ks", "sparse"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                // `bucket` exists but is not part of the key.
                ColumnDescription::new("bucket", "int"),
            ],
        );
        let no_bucket = TableDescription::new(
            TableRef::new("ks", "nobucket"),
            vec![ColumnDescription::new("id", "int").partition_key()],
        );
        let schema = FakeSchema::new()
            .with(origin_table())
            .with(sparse_target)
            .with(no_bucket);

        let mut config = valid();
        config.schema.target.keyspace_table = Some("ks.sparse".to_owned());
        assert_eq!(rules(&config, &schema), ["CFG-020"]);

        config.schema.target.keyspace_table = Some("ks.nobucket".to_owned());
        assert_eq!(rules(&config, &schema), ["CFG-020"]);
    }

    #[test]
    fn cfg_031_explode_map_needs_a_map_column_that_exists() {
        let with_map = TableDescription::new(
            TableRef::new("ks", "src"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("bucket", "int").clustering_key(),
                ColumnDescription::new("m", "map<text, int>"),
                ColumnDescription::new("data", "text"),
            ],
        );
        let schema = FakeSchema::new().with(with_map).with(target_table());

        let mut config = valid();
        config.schema.target.keyspace_table = Some("ks.dst".to_owned());
        config.feature.explode_map.origin_column = Some("data".to_owned());
        config.feature.explode_map.target_key_column = Some("const_a".to_owned());
        config.feature.explode_map.target_value_column = Some("data".to_owned());
        // `data` is text, not a map.
        assert_eq!(rules(&config, &schema), ["CFG-031"]);

        config.feature.explode_map.origin_column = Some("m".to_owned());
        assert!(rules(&config, &schema).is_empty());

        config.feature.explode_map.target_key_column = Some("nosuch".to_owned());
        assert_eq!(rules(&config, &schema), ["CFG-031"]);
    }

    #[test]
    fn cfg_034_automatic_writetime_resolution_is_checked_against_the_real_columns() {
        let collections_only = TableDescription::new(
            TableRef::new("ks", "src"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("tags", "set<text>"),
            ],
        );
        let schema = FakeSchema::new().with(collections_only);

        let mut config = valid();
        config.filter.writetime.min = Some(1);
        config.filter.writetime.max = Some(2);
        let diagnostics = check(&config, &schema, ValidationOptions::default());
        let failure = diagnostics
            .iter()
            .find(|d| d.rule.as_deref() == Some("CFG-034"))
            .unwrap();
        assert!(failure
            .suggestion
            .as_deref()
            .unwrap()
            .contains("use_collections"));

        config.schema.ttl_writetime.use_collections = true;
        assert!(!rules(&config, &schema).contains(&"CFG-034".to_owned()));
    }

    #[test]
    fn cfg_036_ttl_and_writetime_are_rejected_for_a_counter_target() {
        let mut config = valid();
        config.schema.target.keyspace_table = Some("ks.counts".to_owned());
        // The defaults already ask for automatic TTL and writetime.
        assert!(rules(&config, &schema()).contains(&"CFG-036".to_owned()));

        config.schema.origin.ttl.automatic = false;
        config.schema.origin.writetime.automatic = false;
        assert!(!rules(&config, &schema()).contains(&"CFG-036".to_owned()));

        config.transform.custom_ttl = 60;
        assert!(rules(&config, &schema()).contains(&"CFG-036".to_owned()));
    }

    #[test]
    fn cfg_040_a_counter_target_coerces_the_batch_size_to_one() {
        let mut config = valid();
        config.schema.target.keyspace_table = Some("ks.counts".to_owned());
        config.schema.origin.ttl.automatic = false;
        config.schema.origin.writetime.automatic = false;
        config.perfops.batch_size = 5;

        let diagnostics = check(&config, &schema(), ValidationOptions::default());
        let coercion = diagnostics
            .iter()
            .find(|d| d.rule.as_deref() == Some("CFG-040"))
            .unwrap();
        assert_eq!(coercion.severity, Severity::Info);
        assert!(coercion.detail.as_deref().unwrap().contains("MIG-032"));
    }

    #[test]
    fn cfg_039_a_zero_writetime_increment_warns_about_unfrozen_lists() {
        let config = valid();
        // The origin table has an unfrozen `list<text>` and the default increment is 0.
        let diagnostics = check(&config, &schema(), ValidationOptions::default());
        let warning = diagnostics
            .iter()
            .find(|d| d.rule.as_deref() == Some("CFG-039"))
            .unwrap();
        assert_eq!(warning.severity, Severity::Warning);
        assert!(warning
            .detail
            .as_deref()
            .unwrap()
            .contains("CASSANDRA-11368"));
        assert!(!warning.is_blocking());

        let mut incremented = valid();
        incremented.transform.custom_writetime_increment = 1;
        assert!(
            !check(&incremented, &schema(), ValidationOptions::default())
                .iter()
                .any(|d| d.rule.as_deref() == Some("CFG-039"))
        );
    }

    #[test]
    fn cfg_039_a_frozen_list_does_not_warn() {
        let frozen = TableDescription::new(
            TableRef::new("ks", "src"),
            vec![
                ColumnDescription::new("id", "int").partition_key(),
                ColumnDescription::new("tags", "frozen<list<text>>"),
            ],
        );
        let schema = FakeSchema::new().with(frozen);
        assert!(!check(&valid(), &schema, ValidationOptions::default())
            .iter()
            .any(|d| d.rule.as_deref() == Some("CFG-039")));
    }
}
